use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;

use crate::db::{BarcodeScan, SimCard};
use crate::ModemManagerRef;

#[derive(Debug, Clone, Default, Serialize)]
struct PhoneNumberTask {
    running: bool,
    task_type: String,
    total: usize,
    done: usize,
    current: String,
    errors: Vec<String>,
    results: Vec<PhoneResult>,
}

#[derive(Debug, Clone, Serialize)]
struct PhoneResult {
    com_port: String,
    sim_id: String,
    phone_number: Option<String>,
    status: String,
    message: String,
}

type TaskHandle = Arc<RwLock<PhoneNumberTask>>;

fn new_task_handle() -> TaskHandle {
    Arc::new(RwLock::new(PhoneNumberTask::default()))
}

static PHONE_NUMBER_TASK: OnceLock<TaskHandle> = OnceLock::new();

fn get_phone_number_task() -> &'static TaskHandle {
    PHONE_NUMBER_TASK.get_or_init(new_task_handle)
}

#[derive(Clone)]
struct BarcodeState {
    output_file: Option<String>,
    launcher_path: Option<String>,
}

const NOT_AVAILABLE: &str = "This action needs a serial modem and is not available over VoWiFi";

async fn not_available() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": NOT_AVAILABLE})),
    )
        .into_response()
}

async fn import_phone_numbers(mm: ModemManagerRef, task: TaskHandle, entries: Vec<(String, String)>) {
    {
        let mut t = task.write().await;
        t.running = true;
        t.task_type = "import".to_string();
        t.total = entries.len();
        t.done = 0;
        t.current.clear();
        t.errors.clear();
        t.results.clear();
    }

    for (idx, (iccid, msisdn)) in entries.iter().enumerate() {
        let result = match SimCard::find_by_conditions(Some(iccid), None, None, None).await {
            Ok(cards) => match cards.into_iter().next() {
                Some(mut card) => match card.update_phone_number(Some(msisdn.clone())).await {
                    Ok(()) => {
                        mm.update_sim_cache(card).await;
                        PhoneResult {
                            com_port: String::new(),
                            sim_id: iccid.clone(),
                            phone_number: Some(msisdn.clone()),
                            status: "Success".into(),
                            message: "导入成功".into(),
                        }
                    }
                    Err(e) => PhoneResult {
                        com_port: String::new(),
                        sim_id: iccid.clone(),
                        phone_number: None,
                        status: "Failed".into(),
                        message: format!("写入数据库失败: {e}"),
                    },
                },
                None => PhoneResult {
                    com_port: String::new(),
                    sim_id: iccid.clone(),
                    phone_number: None,
                    status: "Failed".into(),
                    message: "未找到匹配的 SIM".into(),
                },
            },
            Err(e) => PhoneResult {
                com_port: String::new(),
                sim_id: iccid.clone(),
                phone_number: None,
                status: "Failed".into(),
                message: format!("查询 SIM 失败: {e}"),
            },
        };
        let mut t = task.write().await;
        if result.status == "Failed" {
            t.errors.push(format!("{iccid}: {}", result.message));
        }
        t.results.push(result);
        t.done = idx + 1;
    }

    let mut t = task.write().await;
    t.running = false;
    t.current = "导入完成".into();
}

#[derive(Serialize)]
struct BarcodeScanEntry {
    iccid: String,
    msisdn: String,
}

#[derive(Debug, Deserialize)]
struct BarcodeSubmitRequest {
    iccid: String,
    msisdn: String,
}

#[derive(Deserialize)]
struct PhoneNumberImportEntry {
    iccid: String,
    msisdn: String,
}

#[derive(Deserialize)]
struct PhoneNumberImportRequest {
    entries: Vec<PhoneNumberImportEntry>,
}

#[derive(Serialize)]
struct BarcodeInvalidLine {
    line_no: usize,
    raw: String,
    reason: String,
}

#[derive(Serialize)]
struct BarcodeScanResponse {
    source_file: String,
    total_lines: usize,
    valid_count: usize,
    invalid_count: usize,
    entries: Vec<BarcodeScanEntry>,
    invalid_lines: Vec<BarcodeInvalidLine>,
}

fn resolve_barcode_output_file(configured: Option<&str>) -> PathBuf {
    if let Some(path) = configured {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let candidates = [
        PathBuf::from("../bar_code/dist/号码.txt"),
        PathBuf::from("../bar_code/号码.txt"),
        PathBuf::from("./号码.txt"),
    ];

    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("../bar_code/dist/号码.txt"))
}

fn resolve_barcode_launcher_file(configured: Option<&str>) -> PathBuf {
    if let Some(path) = configured {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let candidates = [
        PathBuf::from("../bar_code/dist/扫码枪录入程序.exe"),
        PathBuf::from("../bar_code/扫码枪录入程序.exe"),
        PathBuf::from("./扫码枪录入程序.exe"),
    ];

    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("../bar_code/dist/扫码枪录入程序.exe"))
}

fn normalize_msisdn(raw: &str) -> String {
    let digits: String = raw.trim().chars().filter(|c| c.is_ascii_digit()).collect();
    digits.trim_start_matches('0').to_string()
}

fn normalize_iccid(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn validate_barcode_pair(iccid: &str, msisdn: &str) -> Result<(), String> {
    if !iccid.starts_with("8944") || iccid.len() != 20 {
        return Err("Invalid ICCID".to_string());
    }

    if msisdn.is_empty() {
        return Err("Invalid MSISDN".to_string());
    }

    Ok(())
}

async fn phone_numbers_barcode_submit(Json(payload): Json<BarcodeSubmitRequest>) -> Response {
    let iccid = normalize_iccid(&payload.iccid);
    let msisdn = normalize_msisdn(&payload.msisdn);

    if let Err(err) = validate_barcode_pair(&iccid, &msisdn) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": err}))).into_response();
    }

    match BarcodeScan::upsert(&iccid, &msisdn).await {
        Ok(()) => (StatusCode::OK, Json(json!({"status": "saved"}))).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to save barcode scan: {}", e)})),
        )
            .into_response(),
    }
}

async fn phone_numbers_barcode_scan(State(state): State<BarcodeState>) -> Response {
    let output_path = resolve_barcode_output_file(state.output_file.as_deref());
    let source_file = output_path.to_string_lossy().to_string();

    if !output_path.exists() {
        let response = BarcodeScanResponse {
            source_file,
            total_lines: 0,
            valid_count: 0,
            invalid_count: 0,
            entries: Vec::new(),
            invalid_lines: Vec::new(),
        };
        return (StatusCode::OK, Json(response)).into_response();
    }

    let bytes = match tokio::fs::read(&output_path).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to read barcode output file: {}", e)})),
            )
                .into_response();
        }
    };

    let content = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).to_string(),
    };

    let mut entries = Vec::new();
    let mut invalid_lines = Vec::new();
    let mut seen = HashSet::<(String, String)>::new();
    let mut total_lines = 0usize;

    for (idx, line) in content.lines().enumerate() {
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }
        total_lines += 1;

        let mut parts = raw.splitn(2, ',').map(|s| s.trim());
        let iccid_raw = parts.next().unwrap_or_default();
        let msisdn_raw = parts.next().unwrap_or_default();

        if iccid_raw.is_empty() || msisdn_raw.is_empty() {
            invalid_lines.push(BarcodeInvalidLine {
                line_no: idx + 1,
                raw: raw.to_string(),
                reason: "Expected format ICCID,MSISDN".to_string(),
            });
            continue;
        }

        let iccid = normalize_iccid(iccid_raw);
        let msisdn = normalize_msisdn(msisdn_raw);

        if let Err(reason) = validate_barcode_pair(&iccid, &msisdn) {
            invalid_lines.push(BarcodeInvalidLine {
                line_no: idx + 1,
                raw: raw.to_string(),
                reason,
            });
            continue;
        }

        if seen.insert((iccid.clone(), msisdn.clone())) {
            entries.push(BarcodeScanEntry { iccid, msisdn });
        }
    }

    let response = BarcodeScanResponse {
        source_file,
        total_lines,
        valid_count: entries.len(),
        invalid_count: invalid_lines.len(),
        entries,
        invalid_lines,
    };

    (StatusCode::OK, Json(response)).into_response()
}

async fn phone_numbers_barcode_scans_list() -> Response {
    match BarcodeScan::list_unimported().await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to load barcode scans: {}", e)})),
        )
            .into_response(),
    }
}

async fn phone_numbers_barcode_scans_clear() -> Response {
    match BarcodeScan::delete_all().await {
        Ok(()) => (StatusCode::OK, Json(json!({"status": "cleared"}))).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to clear barcode scans: {}", e)})),
        )
            .into_response(),
    }
}

async fn phone_numbers_barcode_scans_import(State(mm): State<ModemManagerRef>) -> Response {
    let rows = match BarcodeScan::list_unimported().await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to load barcode scans: {}", e)})),
            )
                .into_response()
        }
    };

    let entries: Vec<(String, String)> = rows
        .iter()
        .map(|row| (row.iccid.clone(), normalize_msisdn(&row.msisdn)))
        .collect();
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();

    if entries.is_empty() {
        return (StatusCode::OK, Json(json!({"status": "empty"}))).into_response();
    }

    match BarcodeScan::mark_imported(&ids).await {
        Ok(()) => {
            let task = get_phone_number_task().clone();
            tokio::spawn(import_phone_numbers(mm, task, entries));
            (StatusCode::ACCEPTED, Json(json!({"status": "started", "count": ids.len()}))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to mark barcode scans imported: {}", e)})),
        )
            .into_response(),
    }
}

async fn phone_numbers_barcode_launch(State(state): State<BarcodeState>) -> Response {
    let launcher_path = resolve_barcode_launcher_file(state.launcher_path.as_deref());
    let launcher_str = launcher_path.to_string_lossy().to_string();

    if !launcher_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Barcode scanner executable not found: {}", launcher_str)})),
        )
            .into_response();
    }

    match std::process::Command::new(&launcher_path).spawn() {
        Ok(child) => (
            StatusCode::OK,
            Json(json!({"message": "Barcode scanner launched", "pid": child.id(), "path": launcher_str})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to launch barcode scanner: {}", e)})),
        )
            .into_response(),
    }
}

async fn phone_numbers_barcode_run(State(state): State<BarcodeState>) -> Response {
    let launcher_path = resolve_barcode_launcher_file(state.launcher_path.as_deref());
    let launcher_str = launcher_path.to_string_lossy().to_string();

    if !launcher_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Barcode scanner executable not found: {}", launcher_str)})),
        )
            .into_response();
    }

    let mut child = match std::process::Command::new(&launcher_path).spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to launch barcode scanner: {}", e)})),
            )
                .into_response();
        }
    };

    let pid = child.id();
    let wait_result = tokio::task::spawn_blocking(move || child.wait()).await;
    match wait_result {
        Ok(Ok(_status)) => {
            let scan_resp = phone_numbers_barcode_scan(State(state)).await;
            let mut response = scan_resp.into_response();
            response
                .headers_mut()
                .insert("x-barcode-scanner-pid", header::HeaderValue::from_str(&pid.to_string()).unwrap_or(header::HeaderValue::from_static("0")));
            response
        }
        Ok(Err(e)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Barcode scanner process wait failed: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Barcode scanner task join failed: {}", e)})),
        )
            .into_response(),
    }
}

async fn phone_numbers_import(
    State(mm): State<ModemManagerRef>,
    Json(payload): Json<PhoneNumberImportRequest>,
) -> Response {
    let entries: Vec<(String, String)> = payload
        .entries
        .into_iter()
        .map(|e| (e.iccid, normalize_msisdn(&e.msisdn)))
        .collect();

    let task = get_phone_number_task().clone();
    tokio::spawn(import_phone_numbers(mm, task, entries));

    (StatusCode::ACCEPTED, Json(json!({"status": "started"}))).into_response()
}

async fn phone_numbers_call_exchange() -> Response {
    not_available().await
}

async fn phone_numbers_sms_exchange() -> Response {
    not_available().await
}

async fn phone_numbers_ussd() -> Response {
    not_available().await
}

async fn phone_numbers_status() -> Response {
    let task = get_phone_number_task().read().await;
    let task_clone = PhoneNumberTask {
        running: task.running,
        task_type: task.task_type.clone(),
        total: task.total,
        done: task.done,
        current: task.current.clone(),
        errors: task.errors.clone(),
        results: task.results.clone(),
    };
    Json(task_clone).into_response()
}


pub fn routes(modem_manager: ModemManagerRef) -> Router {
    let barcode_state = BarcodeState {
        output_file: None,
        launcher_path: None,
    };
    Router::new()
        .route(
            "/phone-numbers/import",
            post(phone_numbers_import).with_state(modem_manager.clone()),
        )
        .route(
            "/phone-numbers/barcode-scan",
            post(phone_numbers_barcode_submit).with_state(barcode_state.clone()),
        )
        .route(
            "/phone-numbers/barcode-scan",
            get(phone_numbers_barcode_scan).with_state(barcode_state.clone()),
        )
        .route("/phone-numbers/barcode-scans", get(phone_numbers_barcode_scans_list))
        .route("/phone-numbers/barcode-scans", delete(phone_numbers_barcode_scans_clear))
        .route(
            "/phone-numbers/barcode-scans/import",
            post(phone_numbers_barcode_scans_import).with_state(modem_manager.clone()),
        )
        .route(
            "/phone-numbers/barcode-scan/launch",
            post(phone_numbers_barcode_launch).with_state(barcode_state.clone()),
        )
        .route(
            "/phone-numbers/barcode-scan/run",
            post(phone_numbers_barcode_run).with_state(barcode_state),
        )
        .route("/phone-numbers/call-exchange", post(phone_numbers_call_exchange))
        .route("/phone-numbers/sms-exchange", post(phone_numbers_sms_exchange))
        .route("/phone-numbers/ussd", post(phone_numbers_ussd))
        .route("/phone-numbers/status", get(phone_numbers_status))
        .route("/sims/{sim_id}/phone", post(sim_phone_unavailable))
}

async fn sim_phone_unavailable(Path(_sim_id): Path<String>) -> Response {
    not_available().await
}
