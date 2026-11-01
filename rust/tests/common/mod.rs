#![allow(dead_code)]
pub mod smtp;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
};
use chrono::Utc;
use feeds::{AppState, config::Config};
use sqlx::SqlitePool;
use std::{collections::BTreeMap, net::SocketAddr, sync::Arc};
use tower::ServiceExt;

/// Every test executable owns a fresh file. Harnesses share that file so
/// cross-account forgery tests still exercise a real shared authorization boundary.
pub async fn database() -> (SqlitePool, String) {
    static URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    let url = URL
        .get_or_init(|| async {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tmp/sqlite-migration/tests");
            std::fs::create_dir_all(&root).unwrap();
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = root.join(format!("test-{}-{nonce}.sqlite3", std::process::id()));
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            format!("sqlite://{}", path.display())
        })
        .await
        .clone();
    let pool = feeds::schema::connect(&url, 3, false).await.unwrap();
    feeds::schema::prepare(&pool, "test").await.unwrap();
    (pool, url)
}

pub struct Harness {
    pub pool: SqlitePool,
    pub state: AppState,
    pub app: Router,
    pub account: i64,
    pub user: i64,
    pub email: String,
    pub digest: String,
    pub join_code: String,
}

impl Harness {
    /// Exercise retained Rails rendering/query contracts directly. This router
    /// exists only in tests; production routes are covered by activity.rs and
    /// redirect legacy links instead of exposing these management screens.
    pub async fn legacy_reference() -> Self {
        use axum::{extract::State, response::IntoResponse, routing::any};
        async fn reference(
            State(state): State<AppState>,
            request: axum::extract::Request,
        ) -> axum::response::Response {
            let result = async {
                let request = feeds::browser::BrowserRequest::read(request).await?;
                if request.method != axum::http::Method::GET && !request.csrf_valid(&state.signer) {
                    return Err(StatusCode::UNPROCESSABLE_ENTITY);
                }
                let user =
                    feeds::browser::current_user(&state.pool, &state.signer, &request.headers)
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                        .ok_or(StatusCode::UNAUTHORIZED)?;
                if let Some(result) = feeds::folders::route(&state, &request, &user).await {
                    return result;
                }
                feeds::sinks::route(&state, &request, &user)
                    .await
                    .unwrap_or(Err(StatusCode::NOT_FOUND))
            }
            .await;
            result.unwrap_or_else(IntoResponse::into_response)
        }
        let mut h = Self::new().await;
        h.app = Router::new()
            .route("/", any(reference))
            .route("/sinks", any(reference))
            .route("/sinks/{*path}", any(reference))
            .route("/folders", any(reference))
            .route("/folders/{*path}", any(reference))
            .route("/events/{id}", any(reference))
            .with_state(h.state.clone())
            .fallback_service(h.app);
        h
    }
    pub async fn new() -> Self {
        let (pool, url) = database().await;
        Self::from_database(pool, url).await
    }

    pub async fn isolated() -> Self {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tmp/sqlite-migration/tests");
        std::fs::create_dir_all(&root).unwrap();
        let url = format!(
            "sqlite://{}",
            root.join(format!(
                "isolated-{}-{}.sqlite3",
                std::process::id(),
                rand::random::<u64>()
            ))
            .display()
        );
        let pool = feeds::schema::connect(&url, 3, true).await.unwrap();
        feeds::schema::prepare(&pool, "test").await.unwrap();
        Self::from_database(pool, url).await
    }

    async fn from_database(pool: SqlitePool, url: String) -> Self {
        let account: i64 = sqlx::query_scalar(
            "INSERT INTO accounts (created_at,updated_at) VALUES (now(),now()) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let join_code = format!("test-join-{account}");
        sqlx::query("UPDATE accounts SET join_code=$1 WHERE id=$2")
            .bind(&join_code)
            .bind(account)
            .execute(&pool)
            .await
            .unwrap();
        let email = format!("browser-{account}@example.test");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/rails-contracts.json")).unwrap();
        let digest = fixture["session"]["password_digest"]
            .as_str()
            .unwrap()
            .to_owned();
        let user:i64=sqlx::query_scalar("INSERT INTO users (account_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES ($1,$2,'Tester',$3,2,now(),now()) RETURNING id")
            .bind(account).bind(&email).bind(&digest).fetch_one(&pool).await.unwrap();
        let config = Config {
            database_url: url,
            secret_key_base: "0".repeat(64),
            bind: "127.0.0.1:0".parse().unwrap(),
            public_url: None,
            database_connections: 3,
            mailer: None,
            demo: false,
        };
        let state = AppState::new(pool.clone(), Arc::new(config));
        let app = feeds::router(state.clone());
        Self {
            pool,
            state,
            app,
            account,
            user,
            email,
            digest,
            join_code,
        }
    }

    pub fn client(&self) -> Client {
        let port = (self.user as u16).max(1);
        Client {
            app: self.app.clone(),
            cookies: BTreeMap::new(),
            peer: SocketAddr::from(([127, 1, (port >> 8) as u8, port as u8], 12345)),
        }
    }

    pub async fn signed_in(&self) -> Client {
        self.signed_in_as(self.user).await
    }

    pub async fn signed_in_as(&self, user: i64) -> Client {
        let id:i64=sqlx::query_scalar("INSERT INTO sessions (user_id,created_at,updated_at) VALUES ($1,now(),now()) RETURNING id")
            .bind(user).fetch_one(&self.pool).await.unwrap();
        let mut client = self.client();
        client.cookies.insert(
            "session_id".into(),
            self.state.signer.issue(id, Utc::now()).unwrap(),
        );
        client
    }

    pub async fn member(&self, name: &str, role: i32) -> i64 {
        sqlx::query_scalar("INSERT INTO users (account_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,now(),now()) RETURNING id")
            .bind(self.account).bind(format!("{name}-{}@example.test",self.account)).bind(name).bind(&self.digest).bind(role)
            .fetch_one(&self.pool).await.unwrap()
    }

    pub async fn sink(&self, name: &str) -> i64 {
        sqlx::query_scalar("INSERT INTO sinks (account_id,name,token,created_at,updated_at) VALUES ($1,$2,$3,now(),now()) RETURNING id")
            .bind(self.account).bind(name).bind(format!("{}-{name}",self.account)).fetch_one(&self.pool).await.unwrap()
    }

    pub async fn grant(&self, user: i64, sink: i64) {
        sqlx::query("INSERT INTO sink_memberships (user_id,sink_id,created_at,updated_at) VALUES ($1,$2,now(),now()) ON CONFLICT DO NOTHING")
            .bind(user).bind(sink).execute(&self.pool).await.unwrap();
    }

    pub async fn event(&self, sink: i64, raw: serde_json::Value) -> i64 {
        let event = feeds::events::EventInput::from_params(&raw, Utc::now().naive_utc()).unwrap();
        feeds::events::persist(&self.pool, sink, &event)
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn finish(self) {
        delete_account(&self.pool, self.account).await;
        self.pool.close().await;
    }
}

pub async fn delete_account(pool: &SqlitePool, id: i64) {
    for query in [
        "DELETE FROM events WHERE sink_id IN (SELECT id FROM sinks WHERE account_id=$1)",
        "DELETE FROM saved_views WHERE user_id IN (SELECT id FROM users WHERE account_id=$1) OR sink_id IN (SELECT id FROM sinks WHERE account_id=$1)",
        "DELETE FROM columns WHERE sink_id IN (SELECT id FROM sinks WHERE account_id=$1)",
        "DELETE FROM sink_memberships WHERE user_id IN (SELECT id FROM users WHERE account_id=$1) OR sink_id IN (SELECT id FROM sinks WHERE account_id=$1)",
        "DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE account_id=$1)",
        "DELETE FROM sinks WHERE account_id=$1",
        "DELETE FROM folders WHERE account_id=$1",
        "DELETE FROM users WHERE account_id=$1",
        "DELETE FROM accounts WHERE id=$1",
    ] {
        sqlx::query(query).bind(id).execute(pool).await.unwrap();
    }
}

pub struct Client {
    pub app: Router,
    pub cookies: BTreeMap<String, String>,
    pub peer: SocketAddr,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub html: String,
}
impl Reply {
    pub fn location(&self) -> &str {
        self.headers[header::LOCATION].to_str().unwrap()
    }
}

impl Client {
    pub async fn get(&mut self, path: &str) -> Reply {
        self.send("GET", path, String::new(), &[]).await
    }
    pub async fn form(&mut self, method: &str, path: &str, fields: &[(&str, &str)]) -> Reply {
        let mut fields = fields.to_vec();
        let csrf = self
            .cookies
            .get("telesink_csrf")
            .cloned()
            .unwrap_or_default();
        fields.push(("authenticity_token", &csrf));
        self.send(
            method,
            path,
            serde_urlencoded::to_string(fields).unwrap(),
            &[],
        )
        .await
    }
    pub async fn send(
        &mut self,
        method: &str,
        path: &str,
        body: String,
        headers: &[(&str, &str)],
    ) -> Reply {
        let cookie = self
            .cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost:3001")
            .header("cookie", cookie)
            .header("user-agent", "Rust compatibility test")
            .header(
                "content-type",
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                    .map(|(_, value)| *value)
                    .unwrap_or("application/x-www-form-urlencoded"),
            );
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("content-type") {
                continue;
            }
            request = request.header(*name, *value);
        }
        let mut request = request.body(Body::from(body)).unwrap();
        request.extensions_mut().insert(ConnectInfo(self.peer));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        for value in headers.get_all(header::SET_COOKIE) {
            let cookie = value.to_str().unwrap();
            let (name, value) = cookie.split(';').next().unwrap().split_once('=').unwrap();
            if cookie.contains("Max-Age=0;") {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.into(), value.into());
            }
        }
        let html = String::from_utf8(
            to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        Reply {
            status,
            headers,
            html,
        }
    }
}
