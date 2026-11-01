use crate::{
    AppState, assets, params,
    session::{SessionSigner, cookie_value},
    view,
};
use axum::{
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use chrono::Utc;
use rand::{Rng, distr::Alphanumeric};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub session_id: i64,
    pub account_id: Option<i64>,
    pub email_address: String,
    pub nickname: Option<String>,
    pub role: i32,
    pub current_sink_id: Option<i64>,
}

pub async fn current_user(
    pool: &SqlitePool,
    signer: &SessionSigner,
    headers: &HeaderMap,
) -> Result<Option<User>, sqlx::Error> {
    let id = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| cookie_value(h, "session_id"))
        .and_then(|c| signer.verify(c, Utc::now()));
    let Some(id) = id else {
        return Ok(None);
    };
    sqlx::query_as("SELECT u.id,s.id AS session_id,u.account_id,u.email_address,u.nickname,u.role,u.current_sink_id FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1")
        .bind(id).fetch_optional(pool).await
}

#[derive(Clone)]
pub struct BrowserRequest {
    pub path: String,
    pub uri: String,
    pub method: Method,
    pub headers: HeaderMap,
    pub params: Value,
    pub ip: IpAddr,
}

/// Resolve clients behind the same private/loopback proxy ranges Rails uses.
/// Public direct peers cannot substitute their identity through request headers.
pub fn client_ip(headers: &HeaderMap, peer: IpAddr) -> Result<IpAddr, StatusCode> {
    fn trusted(ip: IpAddr) -> bool {
        match ip.to_canonical() {
            IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
            IpAddr::V6(ip) => {
                ip.is_unique_local() || ip.is_loopback() || ip.is_unicast_link_local()
            }
        }
    }
    if !trusted(peer) {
        return Ok(peer);
    }
    let addresses = |name: &str| -> Vec<IpAddr> {
        headers
            .get_all(name)
            .iter()
            .filter_map(|h| h.to_str().ok())
            .flat_map(|h| h.split(|c: char| c == ',' || c.is_ascii_whitespace()))
            .filter_map(|value| value.parse().ok())
            .collect()
    };
    let forwarded = addresses("x-forwarded-for");
    let clients = addresses("client-ip");
    if !forwarded.is_empty() && clients.first().is_some_and(|ip| !forwarded.contains(ip)) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let chain: Vec<_> = forwarded
        .iter()
        .rev()
        .chain(clients.iter().rev())
        .copied()
        .collect();
    Ok(chain
        .iter()
        .copied()
        .chain([peer])
        .find(|ip| !trusted(*ip))
        .or_else(|| chain.last().copied())
        .unwrap_or(peer))
}

impl BrowserRequest {
    pub fn base_url(&self) -> String {
        let host = self
            .headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost");
        let scheme = if self
            .headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            == Some("https")
        {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{host}")
    }
    pub async fn read(request: Request) -> Result<Self, StatusCode> {
        let (parts, body) = request.into_parts();
        let mut parameters = params::parse_form(parts.uri.query().unwrap_or("").as_bytes())
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let bytes = to_bytes(body, 2 * 1024 * 1024)
            .await
            .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
        if !bytes.is_empty() {
            let content_type = parts
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let submitted = if content_type.starts_with("application/json") {
                serde_json::from_slice::<Value>(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?
            } else {
                params::parse_form(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?
            };
            if let Some(fields) = submitted.as_object() {
                parameters
                    .as_object_mut()
                    .unwrap()
                    .extend(fields.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        let mut method = parts.method;
        if method == Method::POST
            && let Some(override_method) = parameters.get("_method").and_then(Value::as_str)
        {
            method = match override_method.to_ascii_uppercase().as_str() {
                "PATCH" => Method::PATCH,
                "PUT" => Method::PUT,
                "DELETE" => Method::DELETE,
                _ => method,
            };
        }
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        let ip = client_ip(&parts.headers, peer)?;
        Ok(Self {
            path: parts.uri.path().to_owned(),
            uri: parts.uri.to_string(),
            headers: parts.headers,
            params: parameters,
            method,
            ip,
        })
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| cookie_value(h, name))
    }

    pub fn csrf_valid(&self, signer: &SessionSigner) -> bool {
        if let Some(origin) = self
            .headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            let host = self
                .headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if origin != format!("http://{host}") && origin != format!("https://{host}") {
                return false;
            }
        }
        let Some(cookie) = self.cookie("telesink_csrf") else {
            return false;
        };
        let form_token = self
            .params
            .get("authenticity_token")
            .and_then(Value::as_str);
        let header_token = self
            .headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok());
        (form_token == Some(cookie) || header_token == Some(cookie))
            && signer.verify_message("csrf", cookie, Utc::now()).is_some()
    }
}

pub fn redirect(path: &str, status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header(header::LOCATION, path)
        .body(Body::empty())
        .unwrap()
}

pub fn set_cookie(response: &mut Response, name: &str, value: &str, max_age: u64) {
    let cookie = format!("{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax");
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

pub fn flash_redirect(state: &AppState, path: &str, message: &str, kind: &str) -> Response {
    let mut response = redirect(path, StatusCode::FOUND);
    let cookie = state.signer.sign_message(
        "flash",
        &json!({kind:message}),
        Utc::now() + chrono::Duration::minutes(5),
    );
    set_cookie(&mut response, "telesink_flash", &cookie, 300);
    response
}

pub fn page(
    state: &AppState,
    request: &BrowserRequest,
    name: &str,
    mut context: Value,
    status: StatusCode,
) -> Result<Response, StatusCode> {
    let csrf = request
        .cookie("telesink_csrf")
        .filter(|c| state.signer.verify_message("csrf", c, Utc::now()).is_some())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let nonce: String = rand::rng()
                .sample_iter(Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();
            state.signer.sign_message(
                "csrf",
                &json!(nonce),
                Utc::now() + chrono::Duration::days(1),
            )
        });
    context["csrf"] = json!(csrf);
    context["demo"] = json!(state.config.demo);
    if let Some(flash) = request
        .cookie("telesink_flash")
        .and_then(|cookie| state.signer.verify_message("flash", cookie, Utc::now()))
        && let Some(fields) = flash.as_object()
    {
        context
            .as_object_mut()
            .unwrap()
            .extend(fields.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let html = view::render(name, context).map_err(|error| {
        eprintln!("template {name}: {error}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut response = (status, Html(html)).into_response();
    set_cookie(&mut response, "telesink_csrf", &csrf, 86400);
    if request.cookie("telesink_flash").is_some() {
        set_cookie(&mut response, "telesink_flash", "", 0);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    Ok(response)
}

pub(crate) fn limited(ip: IpAddr, action: &'static str) -> bool {
    type Buckets = HashMap<(IpAddr, &'static str), Vec<Instant>>;
    static BUCKETS: OnceLock<Mutex<Buckets>> = OnceLock::new();
    let mut buckets = BUCKETS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    let window = Duration::from_secs(180);
    buckets.retain(|_, hits| {
        hits.retain(|at| at.elapsed() < window);
        !hits.is_empty()
    });
    if buckets.len() >= 10_000 && !buckets.contains_key(&(ip, action)) {
        return true;
    }
    let hits = buckets.entry((ip, action)).or_default();
    if hits.len() >= 10 {
        return true;
    }
    hits.push(Instant::now());
    false
}

async fn login(state: &AppState, request: &BrowserRequest) -> Result<Response, StatusCode> {
    if limited(request.ip, "login") {
        return Ok(flash_redirect(
            state,
            "/session/new",
            "try again later.",
            "alert",
        ));
    }
    let email = params::scalar(request.params.get("email_address"))
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let password = params::scalar(request.params.get("password")).unwrap_or_default();
    // A single owner needs only a password. Retain explicit email login for
    // imported installations, without choosing an arbitrary user when ambiguous.
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id,password_digest FROM users WHERE email_address=$1 OR ($1='' AND (SELECT COUNT(*) FROM users)=1) LIMIT 1")
            .bind(email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let authenticated = if let Some((id, digest)) = row {
        tokio::task::spawn_blocking(move || {
            bcrypt::verify(password, digest.as_str())
                .unwrap_or(false)
                .then_some(id)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        tokio::task::spawn_blocking(move || {
            let _ = bcrypt::hash(password, 12);
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        None
    };
    let Some(user_id) = authenticated else {
        return Ok(flash_redirect(
            state,
            "/session/new",
            "username or password incorrect.",
            "alert",
        ));
    };
    let destination = request
        .cookie("telesink_return_to")
        .and_then(|c| state.signer.verify_message("return_to", c, Utc::now()))
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|s| s.starts_with('/') && !s.starts_with("//") && !s.contains('\\'))
        .filter(|s| !crate::activity::background_path(s))
        .unwrap_or_else(|| "/".into());
    start_session(state, request, user_id, &destination).await
}

pub async fn start_session(
    state: &AppState,
    request: &BrowserRequest,
    user_id: i64,
    destination: &str,
) -> Result<Response, StatusCode> {
    let user_agent = request
        .headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let id:i64=sqlx::query_scalar("INSERT INTO sessions (user_id,user_agent,ip_address,created_at,updated_at) VALUES ($1,$2,$3,now(),now()) RETURNING id")
        .bind(user_id).bind(user_agent).bind(request.ip.to_string()).fetch_one(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    let cookie = state
        .signer
        .issue(id, Utc::now())
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = redirect(destination, StatusCode::FOUND);
    set_cookie(&mut response, "session_id", &cookie, 631152000);
    set_cookie(&mut response, "telesink_return_to", "", 0);
    Ok(response)
}

pub(crate) fn valid_email(email: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(||Regex::new(r"\A[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z").unwrap()).is_match(email)
}

pub(crate) fn join_code() -> String {
    let random: String = rand::rng()
        .sample_iter(Alphanumeric)
        .take(12)
        .map(char::from)
        .collect();
    format!("{}-{}-{}", &random[..4], &random[4..8], &random[8..])
}

async fn register(state: &AppState, request: &BrowserRequest) -> Result<Response, StatusCode> {
    if !setup_available(state).await? {
        return Ok(flash_redirect(
            state,
            "/session/new",
            "setup is complete. sign in with your account.",
            "alert",
        ));
    }
    let fields = request
        .params
        .get("user")
        .filter(|v| v.is_object())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let email = params::scalar(fields.get("email_address"))
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let password = params::scalar(fields.get("password")).unwrap_or_default();
    let confirmation = params::scalar(fields.get("password_confirmation"));
    if !valid_email(&email)
        || password.is_empty()
        || password.len() > 72
        || confirmation.as_ref().is_some_and(|c| *c != password)
    {
        return Ok(flash_redirect(
            state,
            "/registration/new",
            "registration failed. please try again.",
            "alert",
        ));
    }
    let digest = tokio::task::spawn_blocking(move || bcrypt::hash(password, 12))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Serialize first-owner creation across requests and overlapping releases.
    let configured: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts)")
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if configured {
        return Ok(flash_redirect(
            state,
            "/session/new",
            "setup is complete. sign in with your account.",
            "alert",
        ));
    }
    let account_id:i64=sqlx::query_scalar("INSERT INTO accounts (join_code,created_at,updated_at) VALUES ($1,now(),now()) RETURNING id")
        .bind(join_code()).fetch_one(&mut *tx).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    let nickname = email.split('@').next().unwrap_or_default();
    let result=sqlx::query_scalar::<_,i64>("INSERT INTO users (account_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES ($1,$2,$3,$4,2,now(),now()) RETURNING id")
        .bind(account_id).bind(&email).bind(nickname).bind(digest).fetch_one(&mut *tx).await;
    let user_id = match result {
        Ok(id) => id,
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Ok(flash_redirect(
                state,
                "/registration/new",
                "registration failed. please try a different email address.",
                "alert",
            ));
        }
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    start_session(state, request, user_id, "/").await
}

async fn setup_available(state: &AppState) -> Result<bool, StatusCode> {
    sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM accounts)")
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

pub async fn handle(State(state): State<AppState>, mut request: Request) -> Response {
    let head = request.method() == Method::HEAD;
    if head {
        *request.method_mut() = Method::GET;
    }
    let response = handle_get_or_mutation(state, request).await;
    if head {
        let (parts, _) = response.into_parts();
        Response::from_parts(parts, Body::empty())
    } else {
        response
    }
}

async fn handle_get_or_mutation(state: AppState, request: Request) -> Response {
    if request.method() == Method::GET
        && (request.uri().path().starts_with("/assets/")
            || assets::ASSETS
                .iter()
                .any(|(path, _, _)| *path == request.uri().path()))
    {
        return assets::serve(request.uri().clone()).await;
    }
    match dispatch(&state, request).await {
        Ok(response) => response,
        Err(status) => status.into_response(),
    }
}

async fn dispatch(state: &AppState, request: Request) -> Result<Response, StatusCode> {
    let request = BrowserRequest::read(request).await?;
    if request.method != Method::GET
        && request.method != Method::HEAD
        && !request.csrf_valid(&state.signer)
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let user = current_user(&state.pool, &state.signer, &request.headers)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if state.config.demo {
        if !crate::demo::allowed(&request) {
            return Ok(redirect(
                &crate::demo::return_path(&request),
                StatusCode::FOUND,
            ));
        }
        if user.is_none() {
            return crate::demo::start_session(state, &request).await;
        }
    }
    if let Some(response) = crate::auth::route(state, &request).await {
        return response;
    }
    if let Some(user) = user.as_ref()
        && !state.config.demo
        && let Some(response) = crate::activity::route(state, &request, user).await
    {
        return response;
    }
    if let Some(user) = user.as_ref()
        && let Some(response) = crate::settings::route(state, &request, user).await
    {
        return response;
    }
    if let Some(user) = user.as_ref()
        && let Some(response) = crate::folders::route(state, &request, user).await
    {
        return response;
    }
    if let Some(user) = user.as_ref()
        && let Some(response) = crate::sinks::route(state, &request, user).await
    {
        return response;
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/new") if user.is_some() => Ok(redirect("/sinks/new", StatusCode::FOUND)),
        ("GET", "/session/new") => page(
            state,
            &request,
            "auth.html",
            json!({"title":"sign in","mode":"login","prefix":"","action":"/session","submit":"sign in","email_address":request.params.get("email_address"),"setup_available":setup_available(state).await?,"single_owner":sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM users").fetch_one(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)? == 1}),
            StatusCode::OK,
        ),
        ("POST", "/session") => login(state, &request).await,
        ("GET", "/registration/new") if user.is_none() && setup_available(state).await? => page(
            state,
            &request,
            "auth.html",
            json!({"title":"set up Feeds","mode":"register","prefix":"user_","action":"/registration","submit":"create account","email_address":request.params.get("email_address")}),
            StatusCode::OK,
        ),
        ("POST", "/registration") if user.is_none() => register(state, &request).await,
        (_, "/registration/new" | "/registration") if user.is_some() => {
            Ok(redirect("/", StatusCode::FOUND))
        }
        ("GET", "/registration/new") => Ok(redirect("/session/new", StatusCode::FOUND)),
        ("DELETE", "/session") if user.is_some() => {
            sqlx::query("DELETE FROM sessions WHERE id=$1")
                .bind(user.unwrap().session_id)
                .execute(&state.pool)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let mut response = redirect("/session/new", StatusCode::SEE_OTHER);
            set_cookie(&mut response, "session_id", "", 0);
            Ok(response)
        }
        _ if user.is_none() => {
            // Background requests must not replace the page to return to after login.
            if crate::activity::background_path(&request.path)
                || request
                    .headers
                    .get(header::ACCEPT)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("application/json"))
            {
                return Err(StatusCode::UNAUTHORIZED);
            }
            let mut response = redirect("/session/new", StatusCode::FOUND);
            let cookie = state.signer.sign_message(
                "return_to",
                &json!(request.uri),
                Utc::now() + chrono::Duration::hours(1),
            );
            set_cookie(&mut response, "telesink_return_to", &cookie, 3600);
            Ok(response)
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}
