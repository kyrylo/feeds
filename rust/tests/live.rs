mod common;
use base64::Engine as _;
use common::{Client, Harness};
use feeds::{
    AppState,
    feed::Filters,
    live::{Grant, Relay},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest},
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
struct Server {
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
    _relay: Relay,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn new(state: &AppState) -> Self {
        let relay = state
            .live
            .start(&state.config.database_url, state.pool.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = feeds::router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            address,
            task,
            _relay: relay,
        }
    }
    async fn connect(
        &self,
        client: &Client,
        origin: Option<&str>,
    ) -> Result<Socket, tungstenite::Error> {
        let mut request = format!("ws://{}/cable", self.address)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "origin",
            origin
                .map(str::to_owned)
                .unwrap_or_else(|| format!("http://{}", self.address))
                .parse()
                .unwrap(),
        );
        request.headers_mut().insert(
            "cookie",
            client
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ")
                .parse()
                .unwrap(),
        );
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "actioncable-v1-json".parse().unwrap(),
        );
        let (socket, response) = tokio_tungstenite::connect_async(request).await?;
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            "actioncable-v1-json"
        );
        Ok(socket)
    }
}

async fn until(socket: &mut Socket, predicate: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message = socket
                .next()
                .await
                .expect("socket closed")
                .expect("socket error");
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if predicate(&value) {
                    return value;
                }
            }
        }
    })
    .await
    .expect("expected cable frame did not arrive")
}
fn identifier(token: &str) -> String {
    json!({"channel":"Turbo::StreamsChannel","signed_stream_name":token}).to_string()
}
async fn subscribe(socket: &mut Socket, token: &str, accepted: bool) -> String {
    let id = identifier(token);
    socket
        .send(Message::Text(
            json!({"command":"subscribe","identifier":id})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    until(socket, |v| {
        v["identifier"] == id
            && v["type"]
                == if accepted {
                    "confirm_subscription"
                } else {
                    "reject_subscription"
                }
    })
    .await;
    id
}
async fn html(socket: &mut Socket, id: &str, needle: &str) -> String {
    until(socket, |v| {
        v["identifier"] == id && v["message"].as_str().is_some_and(|s| s.contains(needle))
    })
    .await["message"]
        .as_str()
        .unwrap()
        .into()
}
async fn silent(socket: &mut Socket) {
    assert!(
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let value = socket.next().await.unwrap().unwrap();
                if let Message::Text(text) = value {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    assert_ne!(value["type"], "disconnect");
                    if value.get("identifier").is_some() {
                        return value;
                    }
                }
            }
        })
        .await
        .is_err(),
        "unexpected subscription message"
    );
}
fn feed_token(h: &Harness, sink: i64, filters: Value) -> String {
    Grant::Feed {
        user: h.user,
        sink,
        filters: Filters::from_params(&filters),
        zone: "Asia/Manila".into(),
        month: None,
        after: 0,
        seen: vec![],
    }
    .token(&h.state)
}
fn token_from_page(h: &Harness, page: &str, kind: &str) -> String {
    regex::Regex::new(r#"signed-stream-name="([^"]+)""#)
        .unwrap()
        .captures_iter(page)
        .map(|c| c[1].replace("&#x2f;", "/"))
        .find(|token| {
            h.state
                .tokens
                .verify_stream(token)
                .is_some_and(|s| s.contains(&format!("\"kind\":\"{kind}\"")))
        })
        .unwrap()
}

#[test]
fn stream_signatures_match_the_original_rails_verifier_in_both_directions() {
    let signer = feeds::tokens::RailsTokens::new(
        b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    );
    let reference: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/live-contracts.json")).unwrap();
    for case in reference {
        let token = case["token"].as_str().unwrap();
        let name = case["name"].as_str().unwrap();
        assert_eq!(signer.verify_stream(token).as_deref(), Some(name));
        assert_eq!(signer.issue_stream(name), token);
        assert!(signer.verify_stream(&(token.to_owned() + "0")).is_none());
    }
}

#[tokio::test]
async fn cable_authenticates_origins_sessions_signatures_and_source_memberships() {
    let h = Harness::legacy_reference().await;
    let outside = Harness::new().await;
    let sink = h.sink("Allowed").await;
    let hidden = h.sink("Hidden").await;
    let foreign = outside.sink("Foreign").await;
    h.grant(h.user, sink).await;
    let server = Server::new(&h.state).await;
    let mut client = h.signed_in().await;
    for (c, origin, expected) in [
        (&client, Some("https://evil.example"), 403),
        (&h.client(), None, 401),
    ] {
        match server.connect(c, origin).await.unwrap_err() {
            tungstenite::Error::Http(response) => assert_eq!(response.status().as_u16(), expected),
            error => panic!("unexpected {error}"),
        }
    }
    let mut socket = server.connect(&client, None).await.unwrap();
    until(&mut socket, |v| v["type"] == "welcome").await;
    for token in [
        feed_token(&h, hidden, json!({})),
        feed_token(&h, foreign, json!({})),
        Grant::Sidebar { user: outside.user }.token(&h.state),
        "tampered".into(),
    ] {
        subscribe(&mut socket, &token, false).await;
    }
    let page = client.get(&format!("/sinks/{sink}")).await.html;
    let token = token_from_page(&h, &page, "feed");
    let id = subscribe(&mut socket, &token, true).await;
    html(&mut socket, &id, "event_type_filter_list_desktop").await;
    let event = h
        .event(
            sink,
            json!({"event":"payment_failed","text":"<script>live content</script>"}),
        )
        .await;
    let payload = html(&mut socket, &id, &format!("feed_item_event_{event}")).await;
    assert!(payload.contains("action=\"append\" target=\"events_feed\""));
    assert!(payload.contains("&lt;script&gt;live content"));
    assert!(!payload.contains("<script>live content"));
    assert!(payload.contains("Payment failed"));
    assert!(payload.contains("event_type_filter_list_mobile"));
    socket
        .send(Message::Text(
            json!({"command":"unsubscribe","identifier":id})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    h.event(sink, json!({"text":"unsubscribed"})).await;
    silent(&mut socket).await;
    socket.close(None).await.unwrap();
    drop(server);
    outside.finish().await;
    h.finish().await;
}

#[tokio::test]
async fn live_filters_catch_up_page_races_and_keep_source_boundaries_across_instances() {
    let h = Harness::legacy_reference().await;
    let sink = h.sink("Filtered").await;
    let other = h.sink("Other").await;
    h.grant(h.user, sink).await;
    h.grant(h.user, other).await;
    // A separate app state/relay simulates another server instance. Ingestion
    // below uses only PostgreSQL, never a direct in-process broadcast shortcut.
    let state = AppState::new(h.pool.clone(), h.state.config.clone());
    let server = Server::new(&state).await;
    let raw = json!({"event":"payment_failed","text":"needle","properties":{"amount":"20"},"occurred_at":"2026-10-16T16:00:00Z"});
    let seen = h.event(sink, raw.clone()).await;
    let mut client = h.signed_in().await;
    client
        .cookies
        .insert("telesink_time_zone".into(), "Asia%2FManila".into());
    let page=client.get(&format!("/sinks/{sink}?q=needle&event_type=payment_failed&date=2026-10-17&property_key=amount&property_op=gt&property_value=10")).await.html;
    let token = token_from_page(&h, &page, "feed");
    let between = h.event(sink, raw.clone()).await;
    let mut socket = server.connect(&client, None).await.unwrap();
    let id = subscribe(&mut socket, &token, true).await;
    let catchup = html(&mut socket, &id, &format!("feed_item_event_{between}")).await;
    assert!(!catchup.contains(&format!("feed_item_event_{seen}")));
    assert!(!catchup.contains("event_type_filter_list"));
    for (source, event) in [
        (other, raw.clone()),
        (
            sink,
            json!({"event":"payment_failed","text":"different","properties":{"amount":20},"occurred_at":"2026-10-16T16:00:00Z"}),
        ),
        (
            sink,
            json!({"event":"payment_failed","text":"needle","properties":{"amount":5},"occurred_at":"2026-10-16T16:00:00Z"}),
        ),
        (
            sink,
            json!({"event":"payment_failed","text":"needle","properties":{"amount":20},"occurred_at":"2026-10-16T15:59:59Z"}),
        ),
    ] {
        h.event(source, event).await;
    }
    silent(&mut socket).await;
    let matching = h.event(sink, raw.clone()).await;
    html(&mut socket, &id, &format!("feed_item_event_{matching}")).await;
    socket.close(None).await.unwrap();
    // A larger reconnect gap refreshes the snapshot instead of silently leaving
    // a hole between older-page cursors and the live tail.
    for _ in 0..61 {
        h.event(sink, raw.clone()).await;
    }
    let mut reconnect = server.connect(&client, None).await.unwrap();
    let id = subscribe(&mut reconnect, &token, true).await;
    html(&mut reconnect, &id, "action=\"refresh\"").await;
    reconnect.close(None).await.unwrap();
    drop(server);
    h.finish().await;
}

#[tokio::test]
async fn unread_updates_and_revocations_reach_existing_connections() {
    let h = Harness::legacy_reference().await;
    let sink = h.sink("Unread").await;
    h.grant(h.user, sink).await;
    let server = Server::new(&h.state).await;
    let mut client = h.signed_in().await;
    client.get("/settings/nickname/edit").await;
    let mut socket = server.connect(&client, None).await.unwrap();
    let sidebar = subscribe(
        &mut socket,
        &Grant::Sidebar { user: h.user }.token(&h.state),
        true,
    )
    .await;
    html(&mut socket, &sidebar, "list_item_sink_").await;
    h.event(sink, json!({"text":"unread"})).await;
    let unread = html(&mut socket, &sidebar, "sidebar-list__unread").await;
    assert!(unread.contains("sidebar-list__unread\">1</span>"));
    client.get(&format!("/sinks/{sink}")).await;
    let cleared = html(&mut socket, &sidebar, "sidebar-list__row-current").await;
    assert!(!cleared.contains("sidebar-list__unread"));
    let feed = subscribe(&mut socket, &feed_token(&h, sink, json!({})), true).await;
    html(&mut socket, &feed, "event_type_filter_list_desktop").await;
    sqlx::query("DELETE FROM sink_memberships WHERE sink_id=$1 AND user_id=$2")
        .bind(sink)
        .bind(h.user)
        .execute(&h.pool)
        .await
        .unwrap();
    h.event(sink, json!({"text":"must not be delivered"})).await;
    let revoked = until(&mut socket, |v| {
        assert!(
            !v["message"]
                .as_str()
                .is_some_and(|s| s.contains("must not be delivered"))
        );
        v["type"] == "disconnect"
    })
    .await;
    assert_eq!(revoked["reconnect"], false);
    let mut socket = server.connect(&client, None).await.unwrap();
    until(&mut socket, |v| v["type"] == "welcome").await;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(h.user)
        .execute(&h.pool)
        .await
        .unwrap();
    socket.send(Message::Text(json!({"command":"subscribe","identifier":identifier(&Grant::Sidebar{user:h.user}.token(&h.state))}).to_string().into())).await.unwrap();
    until(&mut socket, |v| v["type"] == "disconnect").await;
    drop(socket);
    drop(server);
    h.finish().await;
}

#[tokio::test]
async fn legacy_streams_refresh_and_bounded_change_log_recovers_without_a_worker() {
    let h = Harness::new().await;
    let sink = h.sink("Recovery").await;
    h.grant(h.user, sink).await;
    let server = Server::new(&h.state).await;
    let client = h.signed_in().await;
    let mut socket = server.connect(&client, None).await.unwrap();
    let gid = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("gid://telesink/Sink/{sink}"));
    let legacy = h
        .state
        .tokens
        .issue_stream(&format!("{gid}:events:all:live"));
    let id = subscribe(&mut socket, &legacy, true).await;
    html(&mut socket, &id, "action=\"refresh\"").await;
    socket
        .send(Message::Text(
            json!({"command":"unsubscribe","identifier":id})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let id = subscribe(&mut socket, &feed_token(&h, sink, json!({})), true).await;
    html(&mut socket, &id, "event_type_filter_list_desktop").await;
    // A single committed burst exceeds retention before a reader can observe it.
    // The relay must refresh rather than silently miss an unbounded backlog.
    sqlx::query("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<520) INSERT INTO telesink_changes(payload) SELECT 'u:0' FROM n")
        .execute(&h.pool).await.unwrap();
    html(&mut socket, &id, "action=\"refresh\"").await;
    let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM telesink_changes")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(retained <= 512);
    let event = h.event(sink, json!({"text":"after change log gap"})).await;
    html(&mut socket, &id, &format!("feed_item_event_{event}")).await;
    socket.close(None).await.unwrap();
    drop(server);
    h.finish().await;
}
