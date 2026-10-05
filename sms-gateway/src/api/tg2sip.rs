use std::path::PathBuf;
use std::time::Duration;

use axum::{
    extract::Path,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("sms-gateway lives inside the repo")
        .to_path_buf()
}

async fn run_ctl(body: Value, limit: Duration) -> Response {
    let script = repo_root().join("scripts").join("tg2sip_ctl.py");
    let mut child = match Command::new("python3")
        .arg(&script)
        .current_dir(repo_root())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to start tg2sip helper: {err}")})),
            )
                .into_response()
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let payload = body.to_string();
        if let Err(err) = stdin.write_all(payload.as_bytes()).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to write tg2sip request: {err}")})),
            )
                .into_response();
        }
    }
    let finished = match timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("tg2sip helper failed: {err}")})),
            )
                .into_response()
        }
        Err(_) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({"error": "tg2sip helper timed out"})),
            )
                .into_response()
        }
    };
    let stdout = String::from_utf8_lossy(&finished.stdout);
    let parsed: Value = match serde_json::from_str(stdout.trim()) {
        Ok(value) => value,
        Err(_) => {
            let stderr = String::from_utf8_lossy(&finished.stderr);
            let detail = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": detail})),
            )
                .into_response();
        }
    };
    if finished.status.success() {
        return Json(parsed).into_response();
    }
    let code = if parsed.get("status").and_then(|value| value.as_u64()) == Some(409) {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (code, Json(parsed)).into_response()
}

async fn get_status() -> Response {
    run_ctl(json!({"cmd": "status"}), Duration::from_secs(20)).await
}

#[derive(Deserialize)]
struct ForwardBody {
    target: String,
}

async fn put_forward(Path(instance): Path<u8>, Json(body): Json<ForwardBody>) -> Response {
    if instance != 1 && instance != 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "instance must be 1 or 2"})),
        )
            .into_response();
    }
    run_ctl(
        json!({"cmd": "forward", "instance": instance, "target": body.target}),
        Duration::from_secs(90),
    )
    .await
}

#[derive(Deserialize)]
struct RouteBody {
    caller: String,
    destination: String,
}

#[derive(Deserialize)]
struct RoutesBody {
    routes: Vec<RouteBody>,
}

async fn put_routes(Path(instance): Path<u8>, Json(body): Json<RoutesBody>) -> Response {
    if instance != 1 && instance != 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "instance must be 1 or 2"})),
        )
            .into_response();
    }
    let routes: Vec<Value> = body
        .routes
        .into_iter()
        .map(|route| json!({"caller": route.caller, "destination": route.destination}))
        .collect();
    run_ctl(
        json!({"cmd": "routes", "instance": instance, "routes": routes}),
        Duration::from_secs(90),
    )
    .await
}

#[derive(Deserialize)]
struct PowerBody {
    action: String,
}

async fn post_power(Path(instance): Path<u8>, Json(body): Json<PowerBody>) -> Response {
    if instance != 1 && instance != 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "instance must be 1 or 2"})),
        )
            .into_response();
    }
    run_ctl(
        json!({"cmd": "power", "instance": instance, "action": body.action}),
        Duration::from_secs(70),
    )
    .await
}

#[derive(Deserialize)]
struct SessionBody {
    phone: Option<String>,
    code: Option<String>,
    password: Option<String>,
}

async fn post_session(Path(instance): Path<u8>, Json(body): Json<SessionBody>) -> Response {
    if instance != 1 && instance != 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "instance must be 1 or 2"})),
        )
            .into_response();
    }
    let (step, value) = if let Some(phone) = body.phone.filter(|v| !v.is_empty()) {
        ("phone", phone)
    } else if let Some(code) = body.code.filter(|v| !v.is_empty()) {
        ("code", code)
    } else if let Some(password) = body.password.filter(|v| !v.is_empty()) {
        ("password", password)
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "phone, code, or password is required"})),
        )
            .into_response();
    };
    run_ctl(
        json!({"cmd": "session", "instance": instance, "step": step, "value": value}),
        Duration::from_secs(200),
    )
    .await
}

pub fn routes() -> Router {
    Router::new()
        .route("/tg2sip", get(get_status))
        .route("/tg2sip/{instance}/forward", put(put_forward))
        .route("/tg2sip/{instance}/routes", put(put_routes))
        .route("/tg2sip/{instance}/session", post(post_session))
        .route("/tg2sip/{instance}/power", post(post_power))
}
