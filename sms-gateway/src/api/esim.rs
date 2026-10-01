use axum::{
    extract::Path,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::StatusCode;
use serde_json::json;

const UNAVAILABLE: &str = "eSIM modem actions are not available over VoWiFi";

async fn unavailable() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": UNAVAILABLE})),
    )
        .into_response()
}

async fn unavailable_port(Path(_com): Path<String>) -> Response {
    unavailable().await
}

pub fn routes() -> Router {
    Router::new()
        .route("/esim/ports", get(unavailable))
        .route("/esim/{com}/session/enter", post(unavailable_port))
        .route("/esim/{com}/session/exit", post(unavailable_port))
        .route("/esim/{com}/reset", post(unavailable_port))
        .route("/esim/{com}/chip", get(unavailable_port))
        .route("/esim/{com}/profiles", get(unavailable_port))
        .route("/esim/{com}/profiles/download", post(unavailable_port))
        .route("/esim/{com}/profiles/enable", post(unavailable_port))
        .route("/esim/{com}/profiles/disable", post(unavailable_port))
        .route("/esim/{com}/profiles/delete", post(unavailable_port))
        .route("/esim/{com}/profiles/nickname", post(unavailable_port))
        .route("/esim/{com}/notifications", get(unavailable_port))
        .route("/esim/{com}/notifications/process", post(unavailable_port))
        .route("/esim/{com}/notifications/remove", post(unavailable_port))
        .route("/esim/{com}/provision", post(unavailable_port))
        .route("/esim/sources", get(unavailable))
        .route("/esim/sources/upload", post(unavailable))
        .route("/esim/batch", post(unavailable))
        .route("/esim/batch", get(unavailable))
        .route("/esim/batch/cancel", post(unavailable))
        .route("/esim/batch/events", get(unavailable))
}
