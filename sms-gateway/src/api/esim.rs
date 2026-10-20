use std::{sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    extract::{OriginalUri, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use serde_json::json;

struct RemoteEsim {
    client: reqwest::Client,
    url: Option<reqwest::Url>,
    token: String,
}

impl RemoteEsim {
    async fn check_rental_worker(&self, credential: Option<&str>) -> Result<(), Response> {
        let Some(credential) = credential else { return Ok(()); };
        let Some(mut url) = self.url.clone() else {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE, "Remote eSIM worker is not configured"));
        };
        url.set_path("/api/esim/access");
        let reply = self.client.get(url).bearer_auth(&self.token).header("X-Gateway-Credential", credential).send().await;
        match reply {
            Ok(reply) if reply.status().is_success() => {
                let data = reply.json::<serde_json::Value>().await.unwrap_or_default();
                if data["rental_enforced"] == true { return Ok(()); }
            }
            _ => {}
        }
        Err(failure(StatusCode::SERVICE_UNAVAILABLE, "Worker rental authentication is unavailable; no card operation was sent"))
    }
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"success": false, "error": message}))).into_response()
}

fn browser_request_allowed(request: &Request) -> bool {
    if request.headers().get("sec-fetch-site").is_some_and(|value| value == "cross-site") {
        return false;
    }
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return true;
    };
    let Some(host) = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Ok(origin) = origin.to_str().unwrap_or_default().parse::<reqwest::Url>() else {
        return false;
    };
    matches!(origin.scheme(), "http" | "https") &&
        reqwest::Url::parse(&format!("{}://{}", origin.scheme(), host))
            .is_ok_and(|expected| expected.origin() == origin.origin())
}

async fn mappings(State(remote): State<Arc<RemoteEsim>>, request: Request) -> Response {
    let credential = request.extensions().get::<super::access::Identity>().map(|identity| identity.token.clone());
    if let Err(response) = remote.check_rental_worker(credential.as_deref()).await { return response; }
    let Some(base) = &remote.url else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "Remote SIM mapping feed is not configured");
    };
    let mut url = base.clone();
    url.set_path("/api/esim/targets");
    let result = async {
        let mut upstream = remote.client.get(url).bearer_auth(&remote.token);
        if let Some(credential) = credential {
            upstream = upstream.header("X-Gateway-Credential", credential);
        }
        let reply = upstream.send().await?.error_for_status()?;
        reply.json::<serde_json::Value>().await
    }.await;
    match result {
        Ok(payload) => {
            let Some(targets) = payload.get("data").and_then(|value| value.as_array()) else {
                return failure(StatusCode::BAD_GATEWAY, "Invalid remote SIM mapping response");
            };
            let labels: Vec<_> = targets.iter().map(|target| {
                let mut label = serde_json::Map::new();
                for field in ["com_port", "slot_label", "mapping_label", "state"] {
                    if let Some(value) = target.get(field) {
                        label.insert(field.to_owned(), value.clone());
                    }
                }
                label
            }).collect();
            let mut response = Json(json!({"success": true, "data": labels})).into_response();
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => failure(StatusCode::BAD_GATEWAY, "Remote SIM mapping feed unavailable"),
    }
}

async fn proxy(State(remote): State<Arc<RemoteEsim>>, OriginalUri(uri): OriginalUri, request: Request) -> Response {
    if !browser_request_allowed(&request) {
        return failure(StatusCode::FORBIDDEN, "Cross-site eSIM requests are not allowed");
    }
    let Some(base) = &remote.url else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "Remote eSIM worker is not configured");
    };
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::POST {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "Unsupported eSIM method");
    }
    let mut url = base.clone();
    url.set_path(uri.path());
    url.set_query(uri.query());
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    let credential = request.extensions().get::<super::access::Identity>().map(|identity| identity.token.clone());
    if let Err(response) = remote.check_rental_worker(credential.as_deref()).await { return response; }
    let method = request.method().clone();
    let body = match to_bytes(request.into_body(), 8 * 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => return failure(StatusCode::PAYLOAD_TOO_LARGE, "eSIM request exceeds 8 MiB"),
    };
    let mut upstream = remote.client.request(method, url).bearer_auth(&remote.token).body(body);
    if let Some(credential) = credential {
        upstream = upstream.header("X-Gateway-Credential", credential);
    }
    if let Some(value) = content_type {
        upstream = upstream.header(header::CONTENT_TYPE, value);
    }
    match upstream.send().await {
        Ok(reply) => {
            let status = reply.status();
            let content_type = reply.headers().get(header::CONTENT_TYPE).cloned();
            let mut response = Response::new(Body::from_stream(reply.bytes_stream()));
            *response.status_mut() = status;
            if let Some(value) = content_type {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => {
            log::warn!("Remote eSIM transport failed: {:?}", error.without_url());
            failure(StatusCode::BAD_GATEWAY, "Remote eSIM worker unavailable; inspect operation status before retrying")
        }
    }
}

pub fn routes(_username: &str, _password: &str) -> anyhow::Result<Router> {
    let url = std::env::var("REMOTE_ESIM_WORKER_URL").ok().map(|value| reqwest::Url::parse(&value)).transpose()?;
    let token = std::env::var("REMOTE_ESIM_WORKER_TOKEN").unwrap_or_default();
    if let Some(url) = &url {
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
        anyhow::ensure!(url.scheme() == "https" || (url.scheme() == "http" && loopback), "Remote eSIM worker requires HTTPS or loopback HTTP");
        anyhow::ensure!(url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none(), "Worker URL must not contain credentials, query, or fragment");
        anyhow::ensure!(token.len() >= 32, "REMOTE_ESIM_WORKER_TOKEN must contain at least 32 characters");
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let remote = Arc::new(RemoteEsim { client, url, token });
    Ok(Router::new().route("/sim-mappings", get(mappings)).route("/esim/{*path}", any(proxy)).with_state(remote))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_requests_reject_foreign_origins() {
        let same = Request::builder().header(header::HOST, "192.168.31.25:8080")
            .header(header::ORIGIN, "http://192.168.31.25:8080").body(Body::empty()).unwrap();
        let foreign = Request::builder().header(header::HOST, "192.168.31.25:8080")
            .header(header::ORIGIN, "http://example.invalid").body(Body::empty()).unwrap();
        let cross_site = Request::builder().header("sec-fetch-site", "cross-site").body(Body::empty()).unwrap();
        assert!(browser_request_allowed(&same));
        assert!(!browser_request_allowed(&foreign));
        assert!(!browser_request_allowed(&cross_site));
    }

    #[tokio::test]
    async fn authenticated_proxy_supports_the_http10_worker() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let worker_url = format!("http://{}", listener.local_addr().unwrap());
        let worker = tokio::spawn(async move {
            for path in ["/api/esim/access", "/api/esim/ports"] {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0; 1];
                    connection.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                assert!(String::from_utf8(request).unwrap().starts_with(&format!("GET {path} ")));
                let body = if path.ends_with("access") { r#"{"rental_enforced":true}"# }
                    else { r#"{"success":true,"data":[]}"# };
                connection.write_all(format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                connection.shutdown().await.unwrap();
            }
        });
        let remote = Arc::new(RemoteEsim { client: reqwest::Client::new(), url: Some(worker_url.parse().unwrap()), token: "worker-test-token".into() });
        let app = Router::new().nest("/api", Router::new().route("/esim/{*path}", any(proxy)).with_state(remote))
            .layer(axum::Extension(super::super::access::Identity { token: "synthetic-credential".into(), context: json!({}) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/esim/ports", listener.local_addr().unwrap());
        let gateway = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let response = reqwest::Client::new().get(url).send().await.unwrap();
        worker.abort();
        gateway.abort();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<serde_json::Value>().await.unwrap()["data"], json!([]));
    }

    #[tokio::test]
    async fn nested_proxy_preserves_path_and_replaces_browser_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let worker_url = format!("http://{}", listener.local_addr().unwrap());
        let worker = tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(any(|request: Request| async move {
                if request.uri().path() == "/api/esim/targets" && request.uri().query().is_none() {
                    assert_eq!(request.headers()[header::AUTHORIZATION], "Bearer worker-test-token");
                    return Json(json!({"success": true, "data": [{"com_port": "COM1", "slot_label": "Bank 1 / Slot 0",
                        "mapping_label": "B1:0 -> C0:0", "state": "ACTIVE", "reader": "private-reader", "expected_eid": "private-eid"}]}));
                }
                Json(json!({"path": request.uri().to_string(),
                    "authorization": request.headers()[header::AUTHORIZATION].to_str().unwrap()}))
            }))).await.unwrap();
        });
        let remote = Arc::new(RemoteEsim {
            client: reqwest::Client::new(), url: Some(worker_url.parse().unwrap()),
            token: "worker-test-token".into(),
        });
        let app = Router::new().nest("/api", Router::new().route("/sim-mappings", get(mappings)).route("/esim/{*path}", any(proxy)).with_state(remote));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/esim/targets?test=1", listener.local_addr().unwrap());
        let gateway = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let client = reqwest::Client::new();
        let anonymous = client.get(&url).send().await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::OK);
        assert!(anonymous.headers().get(header::WWW_AUTHENTICATE).is_none());
        assert_eq!(client.post(&url).header(header::ORIGIN, "http://example.invalid")
            .send().await.unwrap().status(), StatusCode::FORBIDDEN);
        let labels: serde_json::Value = client.get(url.replace("/esim/targets?test=1", "/sim-mappings"))
            .send().await.unwrap().json().await.unwrap();
        assert_eq!(labels["data"][0]["mapping_label"], "B1:0 -> C0:0");
        assert!(labels["data"][0].get("reader").is_none());
        assert!(labels["data"][0].get("expected_eid").is_none());
        let result: serde_json::Value = client.get(url).header(header::AUTHORIZATION, "Basic browser-test-token")
            .send().await.unwrap().json().await.unwrap();
        worker.abort();
        gateway.abort();
        assert_eq!(result["path"], "/api/esim/targets?test=1");
        assert_eq!(result["authorization"], "Bearer worker-test-token");
    }
}
