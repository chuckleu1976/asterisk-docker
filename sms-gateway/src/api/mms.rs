use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use reqwest::header;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
struct MmsProfileState {
    default_send_mode: String,
}

// ── MMS routes ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MmsAttachmentPayload {
    filename: String,
    #[serde(default)]
    content_type: Option<String>,
    /// Standard base64-encoded file bytes (no data: URL prefix).
    base64: String,
}

#[derive(Deserialize)]
pub struct CreateMmsPayload {
    sim_id: String,
    to: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    attachments: Vec<MmsAttachmentPayload>,
}

#[derive(Deserialize, Debug)]
pub struct MmsQuery {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_per_page")]
    per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    20
}

#[derive(Serialize)]
pub struct PaginatedMmsResponse {
    data: Vec<crate::db::MmsMessage>,
    total: i64,
    page: u32,
    per_page: u32,
}

/// MMS send needs a Quectel AT modem. VoWiFi has no such path.
async fn create_mms() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "MMS send is not available over VoWiFi"})),
    )
        .into_response()
}

async fn get_mms_paginated(Query(query): Query<MmsQuery>) -> Response {
    match crate::db::MmsMessage::query_paginated(query.per_page as i64, ((query.page - 1) * query.per_page) as i64)
        .await
    {
        Ok((data, total)) => Json(PaginatedMmsResponse {
            data,
            total,
            page: query.page,
            per_page: query.per_page,
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list MMS: {}", e)).into_response(),
    }
}

async fn get_mms_detail(Path(id): Path<String>) -> Response {
    let job = match crate::db::MmsMessage::find_by_id(&id).await {
        Ok(Some(job)) => job,
        Ok(None) => return (StatusCode::NOT_FOUND, "MMS job not found".to_string()).into_response(),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load MMS: {}", e)).into_response()
        }
    };
    let attachments = crate::db::MmsAttachment::list_meta(&id).await.unwrap_or_default();
    Json(json!({ "job": job, "attachments": attachments })).into_response()
}

async fn get_mms_profile(
    State(state): State<MmsProfileState>,
    Path(sim_id): Path<String>,
) -> Response {
    match crate::db::MmsProfile::get(&sim_id).await {
        Ok(Some(mut profile)) => {
            profile.mms_send_mode = Some(
                crate::config::resolve_mms_send_mode(
                    profile.mms_send_mode.as_deref(),
                    profile.mms_send_mode_source.as_deref(),
                    &state.default_send_mode,
                )
                .to_string(),
            );
            Json(profile).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "SIM card not found".to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load MMS profile: {}", e))
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetMmsProfilePayload {
    apn: Option<String>,
    mmsc: Option<String>,
    proxy_host: Option<String>,
    proxy_port: Option<i32>,
    mms_send_mode: Option<String>,
    ftp_apn: Option<String>,
}

async fn set_mms_profile(
    State(_state): State<MmsProfileState>,
    Path(sim_id): Path<String>,
    Json(payload): Json<SetMmsProfilePayload>,
) -> Response {
    let mms_send_mode = match payload.mms_send_mode.as_deref() {
        Some(value) => match crate::config::normalize_mms_send_mode(Some(value)) {
            Some(mode) => Some(mode),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "Invalid MMS send mode"})),
                )
                    .into_response();
            }
        },
        None => None,
    };

    match crate::db::MmsProfile::set(
        &sim_id,
        payload.apn.as_deref(),
        payload.mmsc.as_deref(),
        payload.proxy_host.as_deref(),
        payload.proxy_port,
        mms_send_mode,
        payload.ftp_apn.as_deref(),
    )
    .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to update MMS profile: {}", e))
            .into_response(),
    }
}

#[derive(Serialize)]
pub struct PaginatedMmsInboxResponse {
    data: Vec<crate::db::MmsInboxNotification>,
    total: i64,
    page: u32,
    per_page: u32,
}

/// Phase 1: lists detected MMS WAP-push notifications (status will read
/// "notified" until the fetch/decode phase is implemented).
async fn get_mms_inbox_paginated(Query(query): Query<MmsQuery>) -> Response {
    match crate::db::MmsInboxNotification::query_paginated(
        query.per_page as i64,
        ((query.page - 1) * query.per_page) as i64,
    )
    .await
    {
        Ok((data, total)) => Json(PaginatedMmsInboxResponse {
            data,
            total,
            page: query.page,
            per_page: query.per_page,
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list MMS inbox: {}", e))
            .into_response(),
    }
}

async fn get_mms_inbox_detail(Path(id): Path<String>) -> Response {
    match crate::db::MmsInboxNotification::find_by_id(&id).await {
        Ok(Some(item)) => {
            let parts = crate::db::MmsInboxPart::list_meta(&id).await.unwrap_or_default();
            Json(json!({ "notification": item, "parts": parts })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "MMS notification not found".to_string()).into_response(),
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load MMS notification: {}", e))
                .into_response()
        }
    }
}

/// Download/view a single decoded MMS part's raw bytes (image, SMIL, text, ...).
async fn get_mms_inbox_part(Path((_id, part_id)): Path<(String, String)>) -> Response {
    match crate::db::MmsInboxPart::fetch_data(&part_id).await {
        Ok(Some((content_type, filename, data))) => {
            let content_type = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
            let filename = filename.unwrap_or_else(|| part_id.clone());
            // `.leak()` returns `&'static mut str`; explicitly type as `&'static str`
            // (a valid mut->immut reborrow) so the array elements match the `&str`
            // header-value type axum expects -- otherwise both being `&mut str`
            // leaves the array typed `[(HeaderName, &mut str); 2]`, which has no
            // `IntoResponse` impl.
            let content_type: &'static str = content_type.leak();
            let content_disposition: &'static str =
                format!("inline; filename=\"{}\"", filename).leak();
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CONTENT_DISPOSITION, content_disposition),
                ],
                data,
            )
                .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "MMS part not found".to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load MMS part: {}", e)).into_response(),
    }
}



pub fn routes() -> Router {
    let profile_state = MmsProfileState {
        default_send_mode: crate::config::MMS_SEND_MODE_DIRECT.to_string(),
    };
    Router::new()
        .route("/mms", post(create_mms))
        .route("/mms", get(get_mms_paginated))
        .route("/mms/{id}", get(get_mms_detail))
        .route(
            "/sim-cards/{sim_id}/mms-profile",
            get(get_mms_profile).with_state(profile_state.clone()),
        )
        .route(
            "/sim-cards/{sim_id}/mms-profile",
            put(set_mms_profile).with_state(profile_state),
        )
        .route("/mms/inbox", get(get_mms_inbox_paginated))
        .route("/mms/inbox/{id}", get(get_mms_inbox_detail))
        .route("/mms/inbox/{id}/parts/{part_id}", get(get_mms_inbox_part))
}
