//! Bounded Action Cable transport. A small transactional SQLite change log
//! carries committed changes between overlapping Kamal processes.
use crate::{
    AppState,
    browser::{self, User},
    feed::{self, Filters},
    feed_view, sinks, view,
};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{NaiveDate, Utc};
use futures_util::SinkExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, broadcast};

const REFRESH: &str = "<turbo-stream action=\"refresh\"></turbo-stream>";

#[derive(Clone, Debug)]
enum Change {
    Event { sink: i64, id: i64 },
    User(i64),
    Refresh,
}

pub struct Hub {
    sender: broadcast::Sender<Change>,
    ready: AtomicBool,
    slots: Arc<Semaphore>,
}
impl Default for Hub {
    fn default() -> Self {
        Self {
            sender: broadcast::channel(512).0,
            ready: AtomicBool::new(false),
            slots: Arc::new(Semaphore::new(512)),
        }
    }
}
pub struct Relay {
    task: tokio::task::JoinHandle<()>,
    hub: Arc<Hub>,
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.hub.ready.store(false, Ordering::Relaxed);
        self.task.abort();
    }
}

impl Hub {
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    pub async fn start(
        self: &Arc<Self>,
        _url: &str,
        pool: SqlitePool,
    ) -> Result<Relay, sqlx::Error> {
        let mut cursor: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(id),0) FROM telesink_changes")
                .fetch_one(&pool)
                .await?;
        self.ready.store(true, Ordering::Relaxed);
        let hub = self.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(200));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let rows: Result<Vec<(i64, String)>, _> = sqlx::query_as(
                    "SELECT id,payload FROM telesink_changes WHERE id>$1 ORDER BY id LIMIT 512",
                )
                .bind(cursor)
                .fetch_all(&pool)
                .await;
                match rows {
                    Ok(rows) => {
                        if !hub.ready.swap(true, Ordering::Relaxed)
                            || rows.first().is_some_and(|(id, _)| *id > cursor + 1)
                        {
                            let _ = hub.sender.send(Change::Refresh);
                        }
                        for (sequence, payload) in rows {
                            cursor = sequence;
                            let change = if let Some(id) =
                                payload.strip_prefix("u:").and_then(|s| s.parse().ok())
                            {
                                Some(Change::User(id))
                            } else if let Ok(id) = payload.parse::<i64>() {
                                match sqlx::query_scalar::<_, i64>(
                                    "SELECT sink_id FROM events WHERE id=$1",
                                )
                                .bind(id)
                                .fetch_optional(&pool)
                                .await
                                {
                                    Ok(Some(sink)) => Some(Change::Event { sink, id }),
                                    Ok(None) => None,
                                    Err(_) => Some(Change::Refresh),
                                }
                            } else {
                                None
                            };
                            if let Some(change) = change {
                                let _ = hub.sender.send(change);
                            }
                        }
                    }
                    Err(_) => {
                        hub.ready.store(false, Ordering::Relaxed);
                        let _ = hub.sender.send(Change::Refresh);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        Ok(Relay {
            task,
            hub: self.clone(),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Grant {
    Feed {
        user: i64,
        sink: i64,
        filters: Filters,
        zone: String,
        month: Option<NaiveDate>,
        after: i64,
        seen: Vec<i64>,
    },
    Sidebar {
        user: i64,
    },
    // Existing Rails pages reconnect once and refresh into native subscriptions.
    Legacy {
        user: i64,
        sink: Option<i64>,
    },
}
impl Grant {
    fn user(&self) -> i64 {
        match self {
            Self::Feed { user, .. } | Self::Sidebar { user } | Self::Legacy { user, .. } => *user,
        }
    }
    fn sink(&self) -> Option<i64> {
        match self {
            Self::Feed { sink, .. }
            | Self::Legacy {
                sink: Some(sink), ..
            } => Some(*sink),
            _ => None,
        }
    }
    async fn authorized(&self, state: &AppState, user: &User) -> bool {
        if self.user() != user.id {
            return false;
        }
        if let Some(sink) = self.sink() {
            sinks::authorized(state, user.id, sink).await.is_ok()
        } else {
            true
        }
    }
    pub fn token(&self, state: &AppState) -> String {
        state.tokens.issue_stream(&format!(
            "telesink/v1:{}",
            serde_json::to_string(self).unwrap()
        ))
    }
}

fn grant(state: &AppState, user: i64, token: &str) -> Option<Grant> {
    let name = state.tokens.verify_stream(token)?;
    if let Some(payload) = name.strip_prefix("telesink/v1:") {
        return serde_json::from_str(payload).ok();
    }
    if name == format!("user_{user}_sinks") {
        return Some(Grant::Legacy { user, sink: None });
    }
    let (gid, scope) = name.split_once(':')?;
    if scope != "event_type_counts" && !scope.starts_with("events:") {
        return None;
    }
    let gid = URL_SAFE_NO_PAD
        .decode(gid)
        .or_else(|_| STANDARD.decode(gid))
        .ok()?;
    let gid = std::str::from_utf8(&gid).ok()?;
    let sink = gid
        .strip_prefix("gid://telesink/Sink/")?
        .parse::<i64>()
        .ok()?;
    Some(Grant::Legacy {
        user,
        sink: Some(sink),
    })
}

pub async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|h| h.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;
    if origin != format!("http://{host}") && origin != format!("https://{host}") {
        return Err(StatusCode::FORBIDDEN);
    }
    browser::current_user(&state.pool, &state.signer, &headers)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !state.live.ready.load(Ordering::Relaxed) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let slot = state
        .live
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(ws
        .protocols(["actioncable-v1-json"])
        .read_buffer_size(8192)
        .write_buffer_size(8192)
        .max_write_buffer_size(131072)
        .max_message_size(16384)
        .max_frame_size(16384)
        .on_upgrade(move |socket| async move {
            let _slot = slot;
            connection(state, headers, socket).await;
        }))
}

async fn send(socket: &mut WebSocket, value: Value) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            socket.send(Message::Text(value.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    )
}
async fn message(socket: &mut WebSocket, identifier: &str, html: &str) -> bool {
    send(socket, json!({"identifier":identifier,"message":html})).await
}

async fn connection(state: AppState, headers: HeaderMap, mut socket: WebSocket) {
    let mut changes = state.live.sender.subscribe();
    if !send(&mut socket, json!({"type":"welcome"})).await {
        return;
    }
    let mut subscriptions = HashMap::<String, (Grant, std::collections::VecDeque<i64>)>::new();
    let mut ping = tokio::time::interval(Duration::from_secs(3));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            incoming=socket.recv()=>{
                let text=match incoming {
                    Some(Ok(Message::Text(text)))=>text,
                    Some(Ok(Message::Ping(_)|Message::Pong(_)))=>continue,
                    _=>break,
                };
                let Ok(command)=serde_json::from_str::<Value>(&text) else {break;};
                let Some(identifier)=command.get("identifier").and_then(Value::as_str).filter(|s|s.len()<=12288) else{continue;};
                match command.get("command").and_then(Value::as_str) {
                    Some("unsubscribe")=>{subscriptions.remove(identifier);}
                    Some("subscribe")=>{
                        let user=match browser::current_user(&state.pool,&state.signer,&headers).await {
                            Ok(Some(current))=>current,
                            result=>{let _=send(&mut socket,json!({"type":"disconnect","reason":"unauthorized","reconnect":result.is_err()})).await;break;}
                        };
                        let parsed=serde_json::from_str::<Value>(identifier).ok();
                        let subscription=parsed.as_ref().filter(|p|p["channel"]=="Turbo::StreamsChannel")
                            .and_then(|p|p.get("signed_stream_name")).and_then(Value::as_str).and_then(|t|grant(&state,user.id,t));
                        let authorized=if let Some(subscription)=&subscription {subscription.authorized(&state,&user).await}else{false};
                        if !authorized || (subscriptions.len()>=8 && !subscriptions.contains_key(identifier)) {
                            if !send(&mut socket,json!({"identifier":identifier,"type":"reject_subscription"})).await{break;}
                            continue;
                        }
                        let subscription=subscription.unwrap();
                        if !send(&mut socket,json!({"identifier":identifier,"type":"confirm_subscription"})).await {break;}
                        let (html,seen)=initial(&state,&user,&subscription).await.unwrap_or_else(|_|(REFRESH.into(),vec![]));
                        if !html.trim().is_empty() && !message(&mut socket,identifier,&html).await{break;}
                        subscriptions.insert(identifier.into(),(subscription,seen.into()));
                    }
                    _=>{}
                }
            }
            _=ping.tick()=>{
                let user=match browser::current_user(&state.pool,&state.signer,&headers).await {
                    Ok(Some(current))=>current,
                    result=>{let _=send(&mut socket,json!({"type":"disconnect","reason":"unauthorized","reconnect":result.is_err()})).await;break;}
                };
                let mut revoked=false;
                for (subscription,_) in subscriptions.values() {if !subscription.authorized(&state,&user).await{revoked=true;break;}}
                if revoked {let _=send(&mut socket,json!({"type":"disconnect","reason":"unauthorized","reconnect":false})).await;break;}
                if !send(&mut socket,json!({"type":"ping","message":Utc::now().timestamp()})).await{break;}
            }
            change=changes.recv()=>{
                let change=match change {Ok(change)=>change,Err(broadcast::error::RecvError::Lagged(_))=>Change::Refresh,Err(_)=>break};
                let user=match browser::current_user(&state.pool,&state.signer,&headers).await {
                    Ok(Some(current))=>current,
                    result=>{let _=send(&mut socket,json!({"type":"disconnect","reason":"unauthorized","reconnect":result.is_err()})).await;break;}
                };
                let mut failed=false;
                for (identifier,(subscription,seen)) in &mut subscriptions {
                    if !subscription.authorized(&state,&user).await {
                        let _=send(&mut socket,json!({"type":"disconnect","reason":"unauthorized","reconnect":false})).await;failed=true;break;
                    }
                    if let (Grant::Feed{..},Change::Event{id,..})=(&subscription,&change) {
                        if seen.contains(id) {continue;}
                        seen.push_back(*id);
                        if seen.len()>512 {seen.pop_front();}
                    }
                    match render_change(&state,&user,subscription,&change).await {
                        Ok(html)=>{if !html.trim().is_empty() && !message(&mut socket,identifier,&html).await{failed=true;break;}}
                        Err(_)=>{failed=true;break;}
                    }
                }
                if failed {break;}
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}

async fn sidebar(state: &AppState, user: &User) -> Result<String, StatusCode> {
    view::fragment("sidebar-stream.html",json!({"navigation":sinks::navigation(state,user).await?,"current_user":user,"can_administer":matches!(user.role,1|2)})).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)
}
async fn initial(
    state: &AppState,
    user: &User,
    subscription: &Grant,
) -> Result<(String, Vec<i64>), StatusCode> {
    match subscription {
        Grant::Sidebar { .. } => Ok((sidebar(state, user).await?, vec![])),
        Grant::Legacy { .. } => Ok((REFRESH.into(), vec![])),
        Grant::Feed {
            sink,
            filters,
            zone,
            month,
            after,
            seen,
            ..
        } => {
            let mut events = feed::after(
                &state.pool,
                user.id,
                *sink,
                *after,
                filters,
                feed::time_zone(Some(zone)),
            )
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            if events.len() > 60 {
                return Ok((REFRESH.into(), vec![]));
            }
            // Include newly visible recent rows whose transaction obtained an ID
            // before a later transaction committed. Never re-append page rows:
            // Turbo removes duplicate target children before inserting them.
            let recent = feed::batch(
                &state.pool,
                user.id,
                Some(*sink),
                filters,
                feed::time_zone(Some(zone)),
                None,
                60,
            )
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            for event in recent {
                if !events.iter().any(|e| e.id == event.id) {
                    events.push(event);
                }
            }
            events.retain(|e| !seen.contains(&e.id));
            events.sort_by_key(|e| (e.occurred_at, e.id));
            let mut delivered = seen.clone();
            delivered.extend(events.iter().map(|e| e.id));
            Ok((
                render_feed(state, user, *sink, filters, zone, *month, &events).await?,
                delivered,
            ))
        }
    }
}
async fn render_change(
    state: &AppState,
    user: &User,
    subscription: &Grant,
    change: &Change,
) -> Result<String, StatusCode> {
    match (subscription, change) {
        (_, Change::Refresh) => Ok(REFRESH.into()),
        (Grant::Sidebar { .. }, Change::User(id)) if *id == user.id => sidebar(state, user).await,
        (Grant::Sidebar { .. }, Change::Event { sink, .. })
            if sinks::authorized(state, user.id, *sink).await.is_ok() =>
        {
            sidebar(state, user).await
        }
        (
            Grant::Feed {
                sink,
                filters,
                zone,
                month,
                ..
            },
            Change::Event { sink: source, id },
        ) if sink == source => {
            let event = feed::matching_event(
                &state.pool,
                user.id,
                *sink,
                *id,
                filters,
                feed::time_zone(Some(zone)),
            )
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            render_feed(
                state,
                user,
                *sink,
                filters,
                zone,
                *month,
                &event.into_iter().collect::<Vec<_>>(),
            )
            .await
        }
        _ => Ok(String::new()),
    }
}

async fn render_feed(
    state: &AppState,
    user: &User,
    sink: i64,
    filters: &Filters,
    zone: &str,
    month: Option<NaiveDate>,
    events: &[feed::Event],
) -> Result<String, StatusCode> {
    let mut context = json!({"events":events.iter().map(|e|feed_view::row(e,filters,month)).collect::<Vec<_>>(),"new_event":true});
    if month.is_none()
        && filters.event_date.is_none()
        && filters.search_query.is_none()
        && filters.property_key.is_none()
    {
        context["type_filters"] = sinks::type_filters(
            state,
            user.id,
            sink,
            filters,
            feed::time_zone(Some(zone)),
            month,
        )
        .await?;
    }
    view::fragment("live-feed.html", context).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn notify_user(pool: &SqlitePool, user: i64) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO telesink_changes(payload) VALUES ($1)")
        .bind(format!("u:{user}"))
        .execute(pool)
        .await?;
    Ok(())
}
