use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{AppSetting, SimCard};
use crate::firefox_api;
use crate::ModemManagerRef;

// ─── 火狐狸 platform integration handlers ─────────────────────────────────────

async fn get_firefox_api_key() -> Response {
    match AppSetting::get("firefox_api_key").await {
        Ok(value) => (StatusCode::OK, Json(json!({ "api_key": value }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to read API key: {}", e)})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SetFirefoxApiKeyRequest {
    api_key: String,
}

async fn set_firefox_api_key(Json(request): Json<SetFirefoxApiKeyRequest>) -> Response {
    let api_key = request.api_key.trim();
    if api_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "API key is required"})),
        )
            .into_response();
    }

    match AppSetting::set("firefox_api_key", Some(api_key)).await {
        Ok(()) => (StatusCode::OK, Json(json!({"message": "API key saved"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to save API key: {}", e)})),
        )
            .into_response(),
    }
}

async fn get_firefox_countries() -> Response {
    (StatusCode::OK, Json(json!(firefox_api::countries()))).into_response()
}

#[derive(Deserialize)]
struct FirefoxUploadRequest {
    sim_ids: Vec<String>,
    country_id: String,
}

async fn firefox_upload(
    State(modem_manager): State<ModemManagerRef>,
    Json(request): Json<FirefoxUploadRequest>,
) -> Response {
    if request.sim_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "No SIM cards selected"})),
        )
            .into_response();
    }

    let country_id = request.country_id.trim();
    if country_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Country code is required"})),
        )
            .into_response();
    }

    let mut invalid_network_sim_ids = Vec::new();
    for sim_id in &request.sim_ids {
        let is_allowed = match modem_manager.check_network_registration(sim_id).await {
            Ok(Some(status)) => matches!(status.status.trim(), "1" | "5"),
            // AMI has no CREG/CEREG. A missing status must not block upload.
            Ok(None) => true,
            Err(_) => false,
        };

        if !is_allowed {
            invalid_network_sim_ids.push(sim_id.clone());
        }
    }

    if !invalid_network_sim_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "Only SIM cards registered on home or roaming networks can be uploaded: {}",
                    invalid_network_sim_ids.join(", ")
                )
            })),
        )
            .into_response();
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Resolve phone numbers for the selected SIMs.
    let sim_cards = match SimCard::query_all().await {
        Ok(cards) => cards,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to query SIM cards: {}", e)})),
            )
                .into_response();
        }
    };

    let mut phone_numbers = Vec::new();
    let mut sim_ids_to_update = Vec::new();
    for sim_id in &request.sim_ids {
        if let Some(card) = sim_cards.iter().find(|c| &c.id == sim_id) {
            if let Some(phone) = card.phone_number.as_deref().filter(|p| !p.is_empty()) {
                // Normalize: keep only digits and plus sign.
                let normalized: String = phone.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
                if !normalized.is_empty() {
                    phone_numbers.push(normalized);
                    sim_ids_to_update.push(sim_id.clone());
                    continue;
                }
            }
        }
    }

    if phone_numbers.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "None of the selected SIM cards have a valid phone number"})),
        )
            .into_response();
    }

    // Persist the selected country code on each uploaded SIM.
    for sim_id in &sim_ids_to_update {
        if let Some(mut card) = sim_cards.iter().find(|c| &c.id == sim_id).cloned() {
            if let Err(e) = card.update_country_code(Some(country_id.to_string())).await {
                log::warn!("Failed to update country_code for {}: {}", sim_id, e);
            }
            modem_manager.update_sim_cache(card).await;
        }
    }

    match firefox_api::upload_phone_batch(&client, &api_key, country_id, &phone_numbers).await {
        Ok(results) => {
            let batch_ids: Vec<String> = results
                .iter()
                .map(|r| r.batch_id.clone())
                .collect();
            let api_responses: Vec<firefox_api::ApiResponse> = results
                .iter()
                .map(|r| r.response.clone())
                .collect();

            // Persist each batch upload record with its own phone numbers
            for result in &results {
                if let Err(e) = crate::db::FirefoxBatchUpload::insert(
                    &result.batch_id,
                    country_id,
                    &result.phone_numbers,
                )
                .await
                {
                    log::warn!(
                        "Failed to persist firefox batch upload record (batch_id={}): {}",
                        result.batch_id,
                        e
                    );
                }
            }

            (
                StatusCode::OK,
                Json(json!({
                    "message": "Upload completed",
                    "uploaded_count": phone_numbers.len(),
                    "batch_ids": batch_ids,
                    "results": api_responses,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Upload failed: {}", e)})),
        )
            .into_response(),
    }
}

// ─── Helper: read api key + build http client ────────────────────────────

async fn get_firefox_client() -> Result<(String, reqwest::Client), Response> {
    let api_key = AppSetting::get("firefox_api_key")
        .await
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to read API key: {}", e)}))).into_response()
        })?
        .ok_or_else(|| {
            (StatusCode::BAD_REQUEST, Json(json!({"error": "Firefox API key not configured"}))).into_response()
        })?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to build HTTP client: {}", e)}))).into_response()
        })?;

    Ok((api_key, client))
}

// ─── 5. PhoneBatchResult ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct FirefoxBatchStatusRequest {
    batch_id: String,
}

async fn firefox_batch_status(Json(request): Json<FirefoxBatchStatusRequest>) -> Response {
    let batch_id = request.batch_id.trim().to_string();
    if batch_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Batch ID is required"}))).into_response();
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    match firefox_api::query_batch_status(&client, &api_key, &batch_id).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Query failed: {}", e)}))).into_response(),
    }
}

// ─── 5b. Firefox Batch Uploads History ──────────────────────────────────

async fn firefox_batch_uploads() -> Response {
    match crate::db::FirefoxBatchUpload::query_recent(100).await {
        Ok(uploads) => (StatusCode::OK, Json(json!(uploads))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query batch uploads: {}", e)})),
        )
            .into_response(),
    }
}

// ─── 6. PhoneDeleteBatch ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct FirefoxDeleteBatchRequest {
    entries: Vec<DeleteBatchEntry>,
}

#[derive(Deserialize)]
struct DeleteBatchEntry {
    country_id: String,
    phone_num: String,
}

async fn firefox_delete_batch(Json(request): Json<FirefoxDeleteBatchRequest>) -> Response {
    if request.entries.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "No entries provided"}))).into_response();
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    let entries: Vec<(&str, &str)> = request.entries.iter().map(|e| (e.country_id.as_str(), e.phone_num.as_str())).collect();

    // Clear local country_code for deleted phone numbers
    if let Ok(cards) = SimCard::query_all().await {
        for entry in &request.entries {
            for card in &cards {
                if card.phone_number.as_deref() == Some(&entry.phone_num) {
                    let mut c = card.clone();
                    let _ = c.update_country_code(None).await;
                }
            }
        }
    }

    match firefox_api::delete_phone_batch(&client, &api_key, &entries).await {
        Ok(results) => (StatusCode::OK, Json(json!({"message": "Delete completed", "results": results}))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Delete failed: {}", e)}))).into_response(),
    }
}

// ─── 6b. PhoneDeleteBatchById ────────────────────────────────────────────

#[derive(Deserialize)]
struct FirefoxDeleteBatchByIdRequest {
    batch_id: String,
}

async fn firefox_delete_batch_by_id(Json(request): Json<FirefoxDeleteBatchByIdRequest>) -> Response {
    let batch_id = request.batch_id.trim().to_string();
    if batch_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Batch ID is required"}))).into_response();
    }

    let upload = match crate::db::FirefoxBatchUpload::query_by_batch_id(&batch_id).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "Batch not found"}))).into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to query batch: {}", e)}))).into_response();
        }
    };

    let phone_numbers: Vec<String> = upload
        .phone_numbers
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if phone_numbers.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "No phone numbers in batch"}))).into_response();
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    let entries: Vec<(&str, &str)> = phone_numbers
        .iter()
        .map(|p| (upload.country_id.as_str(), p.as_str()))
        .collect();

    // Clear local country_code for deleted phone numbers
    if let Ok(cards) = SimCard::query_all().await {
        for phone_num in &phone_numbers {
            for card in &cards {
                if card.phone_number.as_deref() == Some(phone_num) {
                    let mut c = card.clone();
                    let _ = c.update_country_code(None).await;
                }
            }
        }
    }

    match firefox_api::delete_phone_batch(&client, &api_key, &entries).await {
        Ok(results) => {
            // Optionally delete the local batch record after successful deletion
            let _ = crate::db::FirefoxBatchUpload::delete_by_id(upload.id).await;
            (StatusCode::OK, Json(json!({"message": "Delete by batch completed", "phone_numbers": phone_numbers, "results": results}))).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Delete failed: {}", e)}))).into_response(),
    }
}

// ─── 7. PhoneDeleteCountry ───────────────────────────────────────────────

#[derive(Deserialize)]
struct FirefoxDeleteCountryRequest {
    country_id: String,
}

async fn firefox_delete_country(Json(request): Json<FirefoxDeleteCountryRequest>) -> Response {
    let country_id = request.country_id.trim().to_string();
    if country_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Country ID is required"}))).into_response();
    }

    // Clear local country_code for all SIMs in this country
    if let Ok(cards) = SimCard::query_all().await {
        for card in &cards {
            if card.country_code.as_deref() == Some(&country_id) {
                let mut c = card.clone();
                let _ = c.update_country_code(None).await;
            }
        }
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    match firefox_api::delete_phone_country(&client, &api_key, &country_id).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Delete failed: {}", e)}))).into_response(),
    }
}

// ─── 8. PhoneDeleteAll ───────────────────────────────────────────────────

async fn firefox_delete_all() -> Response {
    // Clear local country_code for all SIMs
    if let Ok(cards) = SimCard::query_all().await {
        for card in &cards {
            if card.country_code.is_some() {
                let mut c = card.clone();
                let _ = c.update_country_code(None).await;
            }
        }
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    match firefox_api::delete_phone_all(&client, &api_key).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Delete failed: {}", e)}))).into_response(),
    }
}

// ─── 9. GetWaitPhoneList ─────────────────────────────────────────────────

async fn firefox_wait_list() -> Response {
    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    match firefox_api::get_wait_phone_list(&client, &api_key).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Query failed: {}", e)}))).into_response(),
    }
}

// ─── 10. GetResultPhoneList ──────────────────────────────────────────────

#[derive(Deserialize)]
struct FirefoxResultListRequest {
    country_id: String,
    phone_num: String,
    item_id: String,
}

async fn firefox_result_list(Query(request): Query<FirefoxResultListRequest>) -> Response {
    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    match firefox_api::get_result_phone_list(&client, &api_key, &request.country_id, &request.phone_num, &request.item_id).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Query failed: {}", e)}))).into_response(),
    }
}

// ─── 11. UploadSms ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FirefoxUploadSmsRequest {
    country_id: String,
    phone_num: String,
    sms_content: String,
}

async fn firefox_upload_sms(Json(request): Json<FirefoxUploadSmsRequest>) -> Response {
    let country_id = request.country_id.trim().to_string();
    let phone_num = request.phone_num.trim().to_string();
    let sms_content = request.sms_content.trim().to_string();

    if country_id.is_empty() || phone_num.is_empty() || sms_content.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "country_id, phone_num, and sms_content are required"}))).into_response();
    }

    let (api_key, client) = match get_firefox_client().await {
        Ok(v) => v,
        Err(e) => return e,
    };

    match firefox_api::upload_sms(&client, &api_key, &country_id, &phone_num, &sms_content).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Upload SMS failed: {}", e)}))).into_response(),
    }
}

// ─── 12. Platform Items & Statistics ─────────────────────────────────────

async fn firefox_platform_items() -> Response {
    match crate::db::FirefoxPlatformItem::query_all().await {
        Ok(items) => (StatusCode::OK, Json(json!(items))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query platform items: {}", e)})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct FirefoxPlatformItemDetailQuery {
    sim_id: Option<String>,
}

async fn firefox_platform_item_detail(
    Path(item_id): Path<String>,
    Query(query): Query<FirefoxPlatformItemDetailQuery>,
) -> Response {
    let item_id = item_id.trim().to_string();
    if item_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Item ID is required"}))).into_response();
    }

    match crate::db::FirefoxPlatformItem::query_by_item_id(&item_id).await {
        Ok(items) => match crate::db::Sms::query_by_platform_item_and_sim(&item_id, query.sim_id.as_deref()).await {
            Ok(sms_list) => {
                let items = if let Some(sim_id) = query.sim_id.as_deref() {
                    items.into_iter()
                        .filter(|item| item.iccid.as_deref() == Some(sim_id) || item.sim_id.as_deref() == Some(sim_id))
                        .collect::<Vec<_>>()
                } else {
                    items
                };

                if items.is_empty() && sms_list.is_empty() {
                    return (StatusCode::NOT_FOUND, Json(json!({"error": "Item not found"}))).into_response();
                }

                (StatusCode::OK, Json(json!({
                    "item_id": item_id,
                    "sim_id": query.sim_id,
                    "items": items,
                    "sms_list": sms_list,
                }))).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to query SMS: {}", e)}))).into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to query item: {}", e)}))).into_response(),
    }
}

async fn firefox_platform_statistics() -> Response {
    match crate::db::FirefoxPlatformItem::query_statistics().await {
        Ok(stats) => (StatusCode::OK, Json(json!(stats))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query statistics: {}", e)})),
        )
            .into_response(),
    }
}

#[derive(Debug, Serialize)]
struct FirefoxMoneyStatsRow {
    com_port: String,
    phone_number: String,
    sim_id: Option<String>,
    imsi: Option<String>,
    country_code: Option<String>,
    platform_connected: bool,
    waiting_sms_count: i64,
    received_sms_count: i64,
    successful_uploaded_sms_count: i64,
    failed_sms_count: i64,
    money_earning: f64,
    earning_item_names: String,
}

#[derive(Debug, Deserialize)]
struct MoneyItemsQuery {
    keyword: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MoneyItemEarningQuery {
    item_id: String,
}

#[derive(Debug, Deserialize)]
struct MoneyItemPriceUpdate {
    item_id: String,
    item_uprice: f64,
}

fn normalize_phone_for_match(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .trim_start_matches('0')
        .to_string()
}

fn phone_match_loose(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.ends_with(b) || b.ends_with(a)
}

fn com_port_sort_key(port: &str) -> u32 {
    port.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse::<u32>()
        .unwrap_or(u32::MAX)
}

async fn firefox_money_stats(State(modem_manager): State<ModemManagerRef>) -> Response {
    let aggregate_rows = match crate::db::FirefoxMoneyStat::query_all_by_sim().await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to query money stats: {}", e)})),
            )
                .into_response();
        }
    };

    let mut aggregate_by_sim: HashMap<String, crate::db::FirefoxMoneyStat> = HashMap::new();
    for row in aggregate_rows {
        aggregate_by_sim.insert(row.sim_id.clone(), row);
    }

    let mut wait_count_by_phone: HashMap<String, i64> = HashMap::new();
    let mut wait_count_by_sim: HashMap<String, i64> = HashMap::new();
    if let Ok(Some(api_key)) = AppSetting::get("firefox_api_key").await {
        if !api_key.trim().is_empty() {
            if let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(10)).build() {
                if let Ok(wait_resp) = firefox_api::get_wait_phone_list(&client, &api_key).await {
                    if let Some(items) = wait_resp.data.and_then(|d| d.as_array().cloned()) {
                        for item in items {
                            let phone = item
                                .get("Phone_Num")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .trim();
                            if phone.is_empty() {
                                continue;
                            }
                            let key = normalize_phone_for_match(phone);
                            if key.is_empty() {
                                continue;
                            }
                            *wait_count_by_phone.entry(key).or_insert(0) += 1;

                            if let Some(sim_id) = modem_manager.find_sim_id_by_phone_number(phone).await {
                                *wait_count_by_sim.entry(sim_id).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    let sim_cards = match SimCard::query_all().await {
        Ok(cards) => cards,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to query SIM cards: {}", e)})),
            )
                .into_response();
        }
    };

    let sim_card_by_id: HashMap<String, SimCard> = sim_cards
        .into_iter()
        .map(|card| (card.id.clone(), card))
        .collect();

    let mut port_rows: Vec<(String, Option<String>, String)> = Vec::new();
    let sim_ids = modem_manager.get_sim_ids().await;
    let mut active_ports: HashSet<String> = HashSet::new();

    for sim_id in sim_ids {
        if let Some(modem) = modem_manager.get_modem(&sim_id).await {
            let phone_number = sim_card_by_id
                .get(&sim_id)
                .and_then(|c| c.phone_number.clone())
                .unwrap_or_default();
            active_ports.insert(modem.com_port.clone());
            port_rows.push((modem.com_port.clone(), Some(sim_id.clone()), phone_number));
        }
    }

    port_rows.sort_by_key(|row| com_port_sort_key(&row.0));

    let mut result: Vec<FirefoxMoneyStatsRow> = Vec::new();
    for (com_port, sim_id, phone_number) in port_rows {
        let agg = sim_id
            .as_ref()
            .and_then(|id| aggregate_by_sim.get(id));
        let sim_card = sim_id.as_ref().and_then(|id| sim_card_by_id.get(id));
        let live = match sim_id.as_ref() {
            Some(id) => match modem_manager.get_transport(id).await {
                Some(transport) => transport.sim_info().await.ok(),
                None => None,
            },
            None => None,
        };
        let phone_number = if phone_number.trim().is_empty() {
            live.as_ref()
                .and_then(|info| info.msisdn.clone())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_default()
        } else {
            phone_number
        };
        let imsi = sim_card
            .and_then(|c| c.imsi.clone())
            .filter(|value| !value.trim().is_empty())
            .or_else(|| live.as_ref().and_then(|info| info.imsi.clone()))
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                let mcc = live.as_ref().and_then(|info| info.mcc.clone())?;
                let mnc = live.as_ref().and_then(|info| info.mnc.clone())?;
                if mcc.trim().is_empty() {
                    None
                } else {
                    Some(format!("{mcc}{mnc}"))
                }
            });
        let country_code = sim_card
            .and_then(|c| c.country_code.clone())
            .filter(|code| !code.trim().is_empty());
        let platform_connected = country_code.is_some();

        let normalized_phone = normalize_phone_for_match(&phone_number);
        let waiting_by_sim = sim_id
            .as_ref()
            .and_then(|id| wait_count_by_sim.get(id))
            .copied()
            .unwrap_or(0);

        let waiting_by_phone = if normalized_phone.is_empty() {
            0
        } else {
            wait_count_by_phone
                .iter()
                .filter(|(k, _)| phone_match_loose(k, &normalized_phone))
                .map(|(_, v)| *v)
                .sum::<i64>()
        };

        let waiting_sms_count = waiting_by_sim.max(waiting_by_phone);

        let earning_item_names = agg
            .and_then(|a| a.earning_item_names.clone())
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" | ");

        result.push(FirefoxMoneyStatsRow {
            com_port,
            phone_number,
            sim_id,
            imsi,
            country_code,
            platform_connected,
            waiting_sms_count,
            received_sms_count: agg.map(|a| a.received_sms_count).unwrap_or(0),
            successful_uploaded_sms_count: agg
                .map(|a| a.successful_uploaded_sms_count)
                .unwrap_or(0),
            failed_sms_count: agg.map(|a| a.failed_sms_count).unwrap_or(0),
            money_earning: agg.map(|a| a.money_earning).unwrap_or(0.0),
            earning_item_names,
        });
    }

    (StatusCode::OK, Json(json!(result))).into_response()
}

async fn firefox_platform_rejection_reasons() -> Response {
    match crate::db::Sms::query_platform_rejection_reason_summary(8).await {
        Ok(stats) => (StatusCode::OK, Json(json!(stats))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query rejection reason summary: {}", e)})),
        )
            .into_response(),
    }
}

async fn firefox_money_items(Query(query): Query<MoneyItemsQuery>) -> Response {
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    match crate::db::FirefoxMoneyStat::query_money_item_options(query.keyword.as_deref(), limit)
        .await
    {
        Ok(items) => (StatusCode::OK, Json(json!(items))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query money item options: {}", e)})),
        )
            .into_response(),
    }
}

async fn firefox_money_item_earning(Query(query): Query<MoneyItemEarningQuery>) -> Response {
    let item_id = query.item_id.trim();
    if item_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "item_id is required"})),
        )
            .into_response();
    }

    match crate::db::FirefoxMoneyStat::query_successful_uploaded_count_for_item(item_id).await {
        Ok(success_count) => (
            StatusCode::OK,
            Json(json!({"item_id": item_id, "success_count": success_count})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query selected item earning stats: {}", e)})),
        )
            .into_response(),
    }
}

async fn firefox_money_item_platform_prices(Query(query): Query<MoneyItemEarningQuery>) -> Response {
    let item_id = query.item_id.trim();
    if item_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "item_id is required"})),
        )
            .into_response();
    }

    match crate::db::FirefoxMoneyStat::query_platform_item_prices(item_id).await {
        Ok(prices) => (StatusCode::OK, Json(json!(prices))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query platform item prices: {}", e)})),
        )
            .into_response(),
    }
}

async fn firefox_update_money_item_price(Json(payload): Json<MoneyItemPriceUpdate>) -> Response {
    let item_id = payload.item_id.trim();
    if item_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "item_id is required"})),
        )
            .into_response();
    }
    if !payload.item_uprice.is_finite() || payload.item_uprice < 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "item_uprice must be a non-negative number"})),
        )
            .into_response();
    }

    match crate::db::FirefoxMoneyStat::update_money_item_price(item_id, payload.item_uprice).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"item_id": item_id, "item_uprice": payload.item_uprice})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to save item price: {}", e)})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct MoneySmsDetailQuery {
    sim_id: Option<String>,
    item_id: Option<String>,
}

async fn firefox_money_sms_detail(Query(query): Query<MoneySmsDetailQuery>) -> Response {
    let sim_id = query.sim_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let item_id = query.item_id.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let result = if let Some(item_id) = item_id {
        crate::db::FirefoxMoneyStat::query_sms_detail_by_item(item_id, 200).await
    } else if let Some(sim_id) = sim_id {
        crate::db::FirefoxMoneyStat::query_sms_detail_by_sim(sim_id, 200).await
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "sim_id or item_id is required"})),
        )
            .into_response();
    };

    match result {
        Ok(rows) => (StatusCode::OK, Json(json!(rows))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to query SMS detail: {}", e)})),
        )
            .into_response(),
    }
}


// ─── Firefox Upload Retry Queue Handlers ─────────────────────────────────

async fn firefox_upload_retry_stats() -> Response {
    match crate::firefox_upload_retry::FirefoxUploadRetryItem::get_stats(&crate::db::get_pool().unwrap_or_else(|_| panic!("No DB pool"))).await {
        Ok(stats) => (StatusCode::OK, Json(json!(stats))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to get queue statistics: {}", e)}))).into_response(),
    }
}

async fn firefox_upload_retry_queue() -> Response {
    match crate::firefox_upload_retry::FirefoxUploadRetryItem::get_ready_for_retry(&crate::db::get_pool().unwrap_or_else(|_| panic!("No DB pool")), 100).await {
        Ok(items) => (StatusCode::OK, Json(json!({"items": items, "count": items.len()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to get queue items: {}", e)}))).into_response(),
    }
}

async fn firefox_upload_retry_dead_letter() -> Response {
    match crate::firefox_upload_retry::FirefoxUploadRetryItem::get_dead_letter_items(&crate::db::get_pool().unwrap_or_else(|_| panic!("No DB pool")), 100).await {
        Ok(items) => (StatusCode::OK, Json(json!({"items": items, "count": items.len()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to get dead-letter items: {}", e)}))).into_response(),
    }
}

async fn firefox_upload_retry_manual(Path(id): Path<String>) -> Response {
    let pool = match crate::db::get_pool() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Database error: {}", e)}))).into_response(),
    };
    
    // Update the item to retry immediately
    let now = chrono::Utc::now().naive_utc();
    match sqlx::query(
        "UPDATE firefox_upload_retry_queue SET next_retry_at = ? WHERE id = ?"
    )
    .bind(now)
    .bind(&id)
    .execute(pool)
    .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({"message": "Item scheduled for immediate retry"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to update item: {}", e)}))).into_response(),
    }
}

async fn firefox_upload_retry_delete(Path(id): Path<String>) -> Response {
    let pool = match crate::db::get_pool() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Database error: {}", e)}))).into_response(),
    };
    
    match sqlx::query("DELETE FROM firefox_upload_retry_queue WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({"message": "Item deleted from retry queue"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to delete item: {}", e)}))).into_response(),
    }
}


pub fn routes(modem_manager: crate::ModemManagerRef) -> Router {
    Router::new()
        .route("/settings/firefox-api-key", get(get_firefox_api_key))
        .route("/settings/firefox-api-key", put(set_firefox_api_key))
        .route("/firefox/countries", get(get_firefox_countries))
        .route(
            "/firefox/upload",
            post(firefox_upload).with_state(modem_manager.clone()),
        )
        .route("/firefox/batch-status", post(firefox_batch_status))
        .route("/firefox/batch-uploads", get(firefox_batch_uploads))
        .route("/firefox/delete-batch", post(firefox_delete_batch))
        .route("/firefox/delete-batch-by-id", post(firefox_delete_batch_by_id))
        .route("/firefox/delete-country", post(firefox_delete_country))
        .route("/firefox/delete-all", post(firefox_delete_all))
        .route("/firefox/wait-list", get(firefox_wait_list))
        .route("/firefox/result-list", get(firefox_result_list))
        .route("/firefox/upload-sms", post(firefox_upload_sms))
        .route("/firefox/platform-items", get(firefox_platform_items))
        .route("/firefox/platform-items/{item_id}", get(firefox_platform_item_detail))
        .route("/firefox/platform-statistics", get(firefox_platform_statistics))
        .route(
            "/firefox/money-stats",
            get(firefox_money_stats).with_state(modem_manager),
        )
        .route("/firefox/money-items", get(firefox_money_items))
        .route("/firefox/money-item-earning", get(firefox_money_item_earning))
        .route("/firefox/money-item-platform-prices", get(firefox_money_item_platform_prices))
        .route("/firefox/money-item-price", post(firefox_update_money_item_price))
        .route("/firefox/money-sms-detail", get(firefox_money_sms_detail))
        .route("/firefox/platform-rejection-reasons", get(firefox_platform_rejection_reasons))
        .route("/firefox/upload-retry/stats", get(firefox_upload_retry_stats))
        .route("/firefox/upload-retry/queue", get(firefox_upload_retry_queue))
        .route("/firefox/upload-retry/dead-letter", get(firefox_upload_retry_dead_letter))
        .route("/firefox/upload-retry/{id}/retry", post(firefox_upload_retry_manual))
        .route("/firefox/upload-retry/{id}", delete(firefox_upload_retry_delete))
}
