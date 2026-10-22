use std::{sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    extract::{OriginalUri, Path, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use futures_util::{stream, StreamExt};

#[derive(Clone)]
pub struct Access {
    client: reqwest::Client,
    url: Option<String>,
    gateway_id: String,
}

#[derive(Clone)]
pub struct Identity {
    pub token: String,
    pub context: Value,
}

pub fn denied(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"success": false, "error": message}))).into_response()
}

impl Access {
    pub fn configured() -> anyhow::Result<Arc<Self>> {
        let url = std::env::var("MANAGEMENT_API_URL").ok();
        if let Some(value) = &url {
            let parsed = reqwest::Url::parse(value)?;
            anyhow::ensure!(matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some()
                && parsed.username().is_empty() && parsed.password().is_none() && parsed.query().is_none()
                && parsed.fragment().is_none(), "Invalid management API URL");
        }
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3)).timeout(Duration::from_secs(8)).build()?;
        Ok(Arc::new(Self { client, url: url.map(|value| value.trim_end_matches('/').to_owned()),
            gateway_id: std::env::var("GATEWAY_ID").unwrap_or_else(|_| "asterisk-lan".to_owned()) }))
    }

    async fn call(&self, method: Method, path: &str, token: Option<&str>, body: Option<Value>) -> Result<Value, Response> {
        let base = self.url.as_ref().ok_or_else(|| denied(StatusCode::SERVICE_UNAVAILABLE, "Management authentication is not configured"))?;
        let mut request = self.client.request(method, format!("{base}/rentals{path}"));
        if let Some(token) = token { request = request.bearer_auth(token); }
        if let Some(body) = body { request = request.json(&body); }
        let response = request.send().await.map_err(|_| denied(StatusCode::SERVICE_UNAVAILABLE, "Management authentication unavailable"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(denied(if status.is_client_error() { status } else { StatusCode::SERVICE_UNAVAILABLE },
                if status == StatusCode::UNAUTHORIZED { "Authentication required or session expired" } else { "Management request denied" }));
        }
        if status == StatusCode::NO_CONTENT { return Ok(Value::Null); }
        response.json().await.map_err(|_| denied(StatusCode::BAD_GATEWAY, "Invalid management response"))
    }

    async fn identify(&self, token: &str) -> Result<Identity, Response> {
        let context = self.call(Method::GET, &format!("/context?gateway_id={}", urlencoding::encode(&self.gateway_id)), Some(token), None).await?;
        if context.get("user_id").and_then(Value::as_str).is_none() || !context["rentals"].is_array() {
            return Err(denied(StatusCode::BAD_GATEWAY, "Invalid management identity"));
        }
        Ok(Identity { token: token.to_owned(), context })
    }
}

fn credential(request: &Request) -> Option<String> {
    if let Some(value) = request.headers().get(header::AUTHORIZATION) {
        return value.to_str().ok()?.strip_prefix("Bearer ").filter(|value| value.len() <= 256).map(str::to_owned);
    }
    request.headers().get(header::COOKIE)?.to_str().ok()?.split(';').find_map(|part| {
        part.trim().strip_prefix("sg_session=").filter(|value| value.len() <= 256).map(str::to_owned)
    })
}

fn same_origin(request: &Request) -> bool {
    if request.headers().get("sec-fetch-site").is_some_and(|value| value != "same-origin" && value != "none") {
        return false;
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let Some(origin) = origin.to_str().ok().and_then(|value| reqwest::Url::parse(value).ok()) else { return false; };
        let Some(host) = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()) else { return false; };
        return reqwest::Url::parse(&format!("{}://{host}", origin.scheme())).is_ok_and(|expected| expected.origin() == origin.origin());
    }
    true
}

pub fn normalize_port(port: &str) -> String {
    let port = port.trim().to_ascii_lowercase();
    if port.is_empty() {
        return port;
    }
    if port == "asterisk" || port == "com1" {
        return "asterisk1".to_owned();
    }
    if let Some(rest) = port.strip_prefix("asterisk") {
        return if rest.is_empty() { "asterisk1".to_owned() } else { format!("asterisk{rest}") };
    }
    if let Some(rest) = port.strip_prefix("com") {
        return if rest.is_empty() || rest == "1" { "asterisk1".to_owned() } else { format!("asterisk{rest}") };
    }
    port
}

impl Identity {
    pub fn is_admin(&self) -> bool {
        self.context.get("is_admin") == Some(&Value::Bool(true))
    }

    fn rentals(&self) -> impl Iterator<Item = &Value> {
        self.context.get("rentals").and_then(Value::as_array).into_iter().flatten()
    }

    pub fn has_permission(&self, permission: &str) -> bool {
        self.rentals().any(|rental| {
            rental.get("permissions").and_then(Value::as_array).into_iter().flatten().any(|value| value == permission)
        })
    }

    pub fn allows_port(&self, port: &str) -> bool {
        if self.is_admin() {
            return true;
        }
        let wanted = normalize_port(port);
        self.rentals().any(|rental| rental.get("com_port").and_then(Value::as_str).is_some_and(|value| normalize_port(value) == wanted))
    }
}

fn sim_dashboard_path(path: &str) -> bool {
    path == "/api/sims/info"
        || path == "/api/sims/stats"
        || path == "/api/sim-cards"
        || (path.starts_with("/api/sims/") && (path.ends_with("/info") || path.ends_with("/refresh")))
}

fn customer_route(path: &str, identity: &Identity) -> bool {
    if path == "/api/check" || path.starts_with("/api/auth/") || path == "/api/sim-mappings" || path.starts_with("/api/esim/") {
        return true;
    }
    let can_view_lines = identity.has_permission("esim:read") || identity.has_permission("sms:read") || identity.has_permission("sms:send");
    if can_view_lines && sim_dashboard_path(path) {
        return true;
    }
    let messaging = identity.has_permission("sms:read") || identity.has_permission("sms:send");
    messaging && (
        path.starts_with("/api/sims")
            || path.starts_with("/api/sim-cards")
            || path == "/api/sms"
            || path.starts_with("/api/sms/")
            || path.starts_with("/api/calls")
            || path == "/api/conversation"
            || path.starts_with("/api/conversations")
            || path == "/api/contacts"
            || path.starts_with("/api/contacts/")
            || path.starts_with("/api/firefox")
            || path == "/api/settings/firefox-api-key"
            || path.starts_with("/api/phone-numbers")
            || path.starts_with("/api/mms")
            || path.starts_with("/api/tg2sip")
    )
}

pub async fn guard(State(access): State<Arc<Access>>, mut request: Request, next: Next) -> Response {
    if !same_origin(&request) { return denied(StatusCode::FORBIDDEN, "Cross-site requests are not allowed"); }
    let path = request.extensions().get::<OriginalUri>().map(|uri| uri.0.path()).unwrap_or_else(|| request.uri().path());
    if path == "/api/auth/login" && request.method() == Method::POST { return next.run(request).await; }
    let Some(token) = credential(&request) else { return denied(StatusCode::UNAUTHORIZED, "Authentication required"); };
    let identity = match access.identify(&token).await { Ok(identity) => identity, Err(response) => return response };
    let path = request.extensions().get::<OriginalUri>().map(|uri| uri.0.path()).unwrap_or_else(|| request.uri().path());
    let path = path.to_owned();
    if !identity.is_admin() && !customer_route(&path, &identity) {
        return denied(StatusCode::FORBIDDEN, "This route requires administrator access");
    }
    request.extensions_mut().insert(identity);
    let mut response = next.run(request).await;
    if response.headers().get(header::CONTENT_TYPE).is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream")) {
        let body = std::mem::replace(response.body_mut(), Body::empty());
        let stream = stream::unfold((body.into_data_stream(), access, token, path), |(mut data, access, token, path)| async move {
            let chunk = data.next().await?;
            let identity = access.identify(&token).await.ok()?;
            if !identity.is_admin() && !customer_route(&path, &identity) { return None; }
            Some((chunk, (data, access, token, path)))
        });
        *response.body_mut() = Body::from_stream(stream);
    }
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
}

async fn login(State(access): State<Arc<Access>>, Json(body): Json<Login>) -> Response {
    if body.username.is_empty() || body.username.len() > 64 || body.password.is_empty() || body.password.len() > 1024 {
        return denied(StatusCode::BAD_REQUEST, "Username and password required");
    }
    let result = match access.call(Method::POST, "/sessions", None, Some(json!({"username": body.username, "password": body.password}))).await {
        Ok(value) => value, Err(response) => return response,
    };
    let Some(token) = result["token"].as_str().filter(|value| value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))) else {
        return denied(StatusCode::BAD_GATEWAY, "Invalid management session");
    };
    let identity = match access.identify(token).await { Ok(identity) => identity, Err(response) => return response };
    let mut response = Json(identity.context).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&format!("sg_session={token}; Path=/api; HttpOnly; SameSite=Strict; Max-Age=43200")) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn me(request: Request) -> Response {
    match request.extensions().get::<Identity>() {
        Some(identity) => Json(identity.context.clone()).into_response(),
        None => denied(StatusCode::UNAUTHORIZED, "Authentication required"),
    }
}

async fn logout(State(access): State<Arc<Access>>, request: Request) -> Response {
    let Some(identity) = request.extensions().get::<Identity>() else { return denied(StatusCode::UNAUTHORIZED, "Authentication required"); };
    let mut response = match access.call(Method::DELETE, "/credentials/current", Some(&identity.token), None).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(), Err(response) => response,
    };
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_static("sg_session=; Path=/api; HttpOnly; SameSite=Strict; Max-Age=0"));
    response
}

async fn tokens(State(access): State<Arc<Access>>, request: Request) -> Response {
    let Some(identity) = request.extensions().get::<Identity>().cloned() else { return denied(StatusCode::UNAUTHORIZED, "Authentication required"); };
    let method = request.method().clone();
    let path = request.extensions().get::<OriginalUri>().map(|uri| uri.0.path()).unwrap_or_else(|| request.uri().path());
    let path = path.strip_prefix("/api/auth/tokens").unwrap_or("");
    let path = format!("/credentials{path}");
    let body = if method == Method::POST {
        let bytes = match to_bytes(request.into_body(), 4096).await { Ok(bytes) => bytes, Err(_) => return denied(StatusCode::PAYLOAD_TOO_LARGE, "Request too large") };
        match serde_json::from_slice(&bytes) { Ok(body) => Some(body), Err(_) => return denied(StatusCode::BAD_REQUEST, "Invalid request") }
    } else { None };
    match access.call(method, &path, Some(&identity.token), body).await {
        Ok(Value::Null) => StatusCode::NO_CONTENT.into_response(),
        Ok(value) => Json(value).into_response(), Err(response) => response,
    }
}

async fn configured_msisdn(State(access): State<Arc<Access>>, Path(id): Path<String>, request: Request) -> Response {
    let Some(identity) = request.extensions().get::<Identity>().cloned() else {
        return denied(StatusCode::UNAUTHORIZED, "Authentication required");
    };
    let Some(rental) = identity.context["rentals"].as_array().and_then(|rows|
        rows.iter().find(|row| row["id"].as_str() == Some(id.as_str()))).cloned() else {
        return denied(StatusCode::NOT_FOUND, "Active rental not found");
    };
    if request.method() == Method::GET { return Json(rental).into_response(); }
    if !rental["permissions"].as_array().is_some_and(|permissions| permissions.iter().any(|value| value == "esim:msisdn")) {
        return denied(StatusCode::FORBIDDEN, "MSISDN permission required");
    }
    let bytes = match to_bytes(request.into_body(), 4096).await {
        Ok(bytes) => bytes, Err(_) => return denied(StatusCode::PAYLOAD_TOO_LARGE, "Request too large"),
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body, Err(_) => return denied(StatusCode::BAD_REQUEST, "Invalid request"),
    };
    match access.call(Method::POST, &format!("/{}/configured-msisdn", urlencoding::encode(&id)), Some(&identity.token), Some(body)).await {
        Ok(value) => Json(value).into_response(), Err(response) => response,
    }
}

pub fn routes(access: Arc<Access>) -> Router {
    Router::new().route("/auth/login", post(login)).route("/auth/me", get(me))
        .route("/auth/logout", post(logout)).route("/auth/tokens", get(tokens).post(tokens))
        .route("/auth/tokens/{id}", delete(tokens))
        .route("/auth/rentals/{id}/configured-msisdn", get(configured_msisdn).post(configured_msisdn)).with_state(access)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{middleware, routing::any};

    #[tokio::test]
    async fn configured_msisdn_uses_only_owned_scoped_rentals_without_a_card_worker() {
        let authority = Router::new().fallback(any(|request: Request| async move {
            assert_eq!(request.headers()[header::AUTHORIZATION], "Bearer sgs_test");
            assert_eq!(request.uri().path(), "/rentals/owned/configured-msisdn");
            let body: Value = serde_json::from_slice(&to_bytes(request.into_body(), 4096).await.unwrap()).unwrap();
            assert_eq!(body["phone_number"], "+00123456789");
            Json(json!({"operator_msisdn":"+00123456789", "sim_msisdn":null}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let management = format!("http://{}", listener.local_addr().unwrap());
        let authority_task = tokio::spawn(async move { axum::serve(listener, authority).await.unwrap(); });
        let access = Arc::new(Access {client:reqwest::Client::new(), url:Some(management), gateway_id:"test".into()});
        let app = routes(access).layer(axum::Extension(Identity {token:"sgs_test".into(), context:json!({
            "rentals":[{"id":"owned", "permissions":["esim:msisdn"]}, {"id":"readonly", "permissions":["esim:read"]}]
        })}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let client = reqwest::Client::new();
        let body = json!({"phone_number":"+00123456789", "presence_generation":1, "identity_generation":2});
        assert_eq!(client.post(format!("{base}/auth/rentals/other/configured-msisdn")).json(&body).send().await.unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(client.post(format!("{base}/auth/rentals/readonly/configured-msisdn")).json(&body).send().await.unwrap().status(), StatusCode::FORBIDDEN);
        let saved = client.post(format!("{base}/auth/rentals/owned/configured-msisdn")).json(&body).send().await.unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(saved.json::<Value>().await.unwrap()["operator_msisdn"], "+00123456789");
        task.abort(); authority_task.abort();
    }

    #[tokio::test]
    async fn administrator_stream_stops_after_revocation_or_role_downgrade() {
        use std::sync::atomic::{AtomicU8, Ordering};

        for revoked in [true, false] {
            let mode = Arc::new(AtomicU8::new(0));
            let authority_mode = mode.clone();
            let authority = Router::new().fallback(any(move || {
                let mode = authority_mode.clone();
                async move {
                    if mode.load(Ordering::SeqCst) == 1 { return StatusCode::UNAUTHORIZED.into_response(); }
                    Json(json!({"user_id":"admin-test", "is_admin":mode.load(Ordering::SeqCst) == 0, "rentals":[]})).into_response()
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let management = format!("http://{}", listener.local_addr().unwrap());
            let authority_task = tokio::spawn(async move { axum::serve(listener, authority).await.unwrap(); });
            let access = Arc::new(Access { client:reqwest::Client::new(), url:Some(management), gateway_id:"test".into() });
            let (sender, receiver) = tokio::sync::mpsc::channel::<String>(2);
            let receiver = Arc::new(tokio::sync::Mutex::new(Some(receiver)));
            let api = Router::new().route("/calls/events", get(move || {
                let receiver = receiver.clone();
                async move {
                    let receiver = receiver.lock().await.take().unwrap();
                    let chunks = stream::unfold(receiver, |mut receiver| async move {
                        receiver.recv().await.map(|chunk| (Ok::<_, std::io::Error>(chunk), receiver))
                    });
                    ([(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(chunks))
                }
            })).layer(middleware::from_fn_with_state(access, guard));
            let app = Router::new().nest_service("/api", api);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/api/calls/events", listener.local_addr().unwrap());
            let gateway_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
            sender.send("data: first\n\n".into()).await.unwrap();
            let response = reqwest::Client::new().get(url).bearer_auth("synthetic-session").send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut chunks = response.bytes_stream();
            assert_eq!(chunks.next().await.unwrap().unwrap().as_ref(), b"data: first\n\n");
            mode.store(if revoked { 1 } else { 2 }, Ordering::SeqCst);
            sender.send("data: must-not-leak\n\n".into()).await.unwrap();
            assert!(tokio::time::timeout(Duration::from_secs(3), chunks.next()).await.unwrap().is_none());
            gateway_task.abort();
            authority_task.abort();
        }
    }

    #[tokio::test]
    async fn http_sessions_reject_anonymous_legacy_and_cross_site_requests() {
        let authority = Router::new().fallback(any(|request: Request| async move {
            if request.uri().path() == "/rentals/sessions" {
                return Json(json!({"token":"sgs_test"})).into_response();
            }
            if request.headers().get(header::AUTHORIZATION).is_none_or(|value| value != "Bearer sgs_test") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(json!({"user_id":"customer-a", "is_admin":false, "rentals":[]})).into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let management = format!("http://{}", listener.local_addr().unwrap());
        let authority_task = tokio::spawn(async move { axum::serve(listener, authority).await.unwrap(); });
        let access = Arc::new(Access {client:reqwest::Client::new(),url:Some(management),gateway_id:"test".into()});
        let api = Router::new().merge(routes(access.clone())).fallback(any(|| async {StatusCode::OK}))
            .layer(middleware::from_fn_with_state(access, guard));
        let app = Router::new().nest_service("/api", api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gateway_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let client = reqwest::Client::new();
        let login = client.post(format!("{base}/api/auth/login")).json(&json!({"username":"test","password":"test-only"})).send().await.unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && !cookie.contains("Secure"));
        assert!(login.json::<Value>().await.unwrap().get("token").is_none());
        assert_eq!(client.get(format!("{base}/api/esim/ports")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(client.get(format!("{base}/api/esim/ports")).header(header::AUTHORIZATION,"Basic old-password").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(client.get(format!("{base}/api/esim/ports")).header(header::COOKIE,"sg_session=sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sms")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        assert_eq!(client.get(format!("{base}/api/esim/ports")).bearer_auth("sgs_test").header("sec-fetch-site","cross-site").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        gateway_task.abort(); authority_task.abort();
    }

    #[tokio::test]
    async fn rental_customer_with_sms_permission_can_use_messaging_routes() {
        let authority = Router::new().fallback(any(|request: Request| async move {
            if request.headers().get(header::AUTHORIZATION).is_none_or(|value| value != "Bearer sgs_test") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(json!({
                "user_id":"customer-a",
                "is_admin":false,
                "rentals":[{"id":"r1","com_port":"asterisk","permissions":["sms:read","sms:send"]}]
            })).into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let management = format!("http://{}", listener.local_addr().unwrap());
        let authority_task = tokio::spawn(async move { axum::serve(listener, authority).await.unwrap(); });
        let access = Arc::new(Access {client:reqwest::Client::new(),url:Some(management),gateway_id:"test".into()});
        let api = Router::new().merge(routes(access.clone())).fallback(any(|| async {StatusCode::OK}))
            .layer(middleware::from_fn_with_state(access, guard));
        let app = Router::new().nest_service("/api", api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gateway_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let client = reqwest::Client::new();
        assert_eq!(client.get(format!("{base}/api/sms")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sims/info")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sim-cards")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sims/stats")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.post(format!("{base}/api/calls/make")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/firefox/countries")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/settings/firefox-api-key")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/phone-numbers/status")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/mms")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/tg2sip")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/readers")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        gateway_task.abort(); authority_task.abort();
    }

    #[tokio::test]
    async fn rental_customer_with_esim_read_can_open_sim_dashboard() {
        let authority = Router::new().fallback(any(|request: Request| async move {
            if request.headers().get(header::AUTHORIZATION).is_none_or(|value| value != "Bearer sgs_test") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(json!({
                "user_id":"customer-a",
                "is_admin":false,
                "rentals":[{"id":"r1","com_port":"asterisk1","permissions":["esim:read","esim:msisdn"]}]
            })).into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let management = format!("http://{}", listener.local_addr().unwrap());
        let authority_task = tokio::spawn(async move { axum::serve(listener, authority).await.unwrap(); });
        let access = Arc::new(Access {client:reqwest::Client::new(),url:Some(management),gateway_id:"test".into()});
        let api = Router::new().merge(routes(access.clone())).fallback(any(|| async {StatusCode::OK}))
            .layer(middleware::from_fn_with_state(access, guard));
        let app = Router::new().nest_service("/api", api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gateway_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let client = reqwest::Client::new();
        assert_eq!(client.get(format!("{base}/api/sims/info")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sim-cards")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sims/stats")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{base}/api/sms")).bearer_auth("sgs_test").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        gateway_task.abort(); authority_task.abort();
    }
}
