use chrono::{Duration as ChronoDuration, NaiveDateTime, Utc};

use crate::db;
use crate::firefox_api;
use crate::firefox_upload_retry;
use crate::ModemManagerRef;

pub async fn firefox_item_catalog_sync_worker() {
    let check_interval = tokio::time::Duration::from_secs(6 * 60 * 60);

    loop {
        if !crate::service_control::is_running() {
            tokio::time::sleep(check_interval).await;
            continue;
        }

        if let Err(e) = sync_firefox_item_catalog_if_due().await {
            log::warn!("[火狐狸价目] 定时同步失败: {}", e);
        }

        tokio::time::sleep(check_interval).await;
    }
}

async fn sync_firefox_item_catalog_if_due() -> anyhow::Result<()> {
    let now = Utc::now().naive_utc();
    let last_sync = db::AppSetting::get("firefox_item_catalog_last_sync_at")
        .await?
        .and_then(|v| NaiveDateTime::parse_from_str(v.trim(), "%Y-%m-%d %H:%M:%S").ok());

    if let Some(last) = last_sync {
        if now - last < ChronoDuration::days(3) {
            return Ok(());
        }
    }

    let token = db::AppSetting::get("firefox_user_token").await.ok().flatten();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let items = firefox_api::get_user_item_prices(&client, token.as_deref(), None).await?;
    if items.is_empty() {
        log::warn!("[火狐狸价目] 同步返回空列表，保留本地数据");
        return Ok(());
    }

    let upserts: Vec<db::FirefoxItemPriceUpsert> = items
        .into_iter()
        .map(|item| {
            let price = item.item_uprice.trim().parse::<f64>().unwrap_or(0.0);
            db::FirefoxItemPriceUpsert {
                item_id: item.item_id.trim().to_string(),
                country_id: item.country_id.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
                item_name: item.item_name.trim().to_string(),
                item_uprice: if price.is_finite() { price.max(0.0) } else { 0.0 },
                country_title: item.country_title.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
            }
        })
        .filter(|item| !item.item_id.is_empty() && !item.item_name.is_empty())
        .collect();

    if upserts.is_empty() {
        log::warn!("[火狐狸价目] 同步后没有有效项目，跳过更新");
        return Ok(());
    }

    db::upsert_firefox_item_prices(&upserts).await?;

    let timestamp = now.format("%Y-%m-%d %H:%M:%S").to_string();
    db::AppSetting::set("firefox_item_catalog_last_sync_at", Some(&timestamp)).await?;

    log::info!("[火狐狸价目] 同步完成: {} 项", upserts.len());
    Ok(())
}


pub async fn firefox_poll_worker(modem_manager: ModemManagerRef) {
    use std::collections::{HashMap, HashSet};
    let poll_interval = tokio::time::Duration::from_secs(3);
    let mut processed_tasks: HashSet<String> = HashSet::new();
    let mut no_sms_backoff_until: HashMap<String, tokio::time::Instant> = HashMap::new();
    let mut no_sms_miss_count: HashMap<String, u8> = HashMap::new();
    let mut heartbeat_count: u32 = 0;
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::error!("[火狐狸轮询] 创建 HTTP 客户端失败: {}", e);
            return;
        }
    };

    loop {
        tokio::time::sleep(poll_interval).await;
        heartbeat_count += 1;

        if !crate::service_control::is_running() {
            continue;
        }

        let api_key = match db::AppSetting::get("firefox_api_key").await {
            Ok(Some(key)) => key,
            _ => continue,
        };

        let wait_list_resp = {
            let mut last_error: Option<anyhow::Error> = None;
            let mut response = None;
            for attempt in 1..=3 {
                match firefox_api::get_wait_phone_list(&client, &api_key).await {
                    Ok(resp) => {
                        response = Some(resp);
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e);
                        if attempt < 3 {
                            tokio::time::sleep(tokio::time::Duration::from_millis(700)).await;
                        }
                    }
                }
            }
            match response {
                Some(resp) => resp,
                None => {
                    log::warn!(
                        "[火狐狸轮询] 获取等待列表失败（已重试3次）: {}",
                        last_error
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    );
                    continue;
                }
            }
        };

        {
            let resp = wait_list_resp;
                let count = resp.data.as_ref().and_then(|d| d.as_array()).map(|a| a.len()).unwrap_or(0);
                if count > 0 {
                    if let Some(items) = resp.data.as_ref().and_then(|d| d.as_array()) {
                        for item in items {
                            let phone_num = item.get("Phone_Num").and_then(|v| v.as_str()).unwrap_or("?");
                            let country_id = item.get("Country_ID").and_then(|v| v.as_str()).unwrap_or("?");
                            let item_id = item.get("Item_ID").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|v| v.to_string()).unwrap_or_else(|| "?".to_string());

                            if phone_num == "?" || item_id == "?" || country_id == "?" {
                                continue;
                            }

                            if let Err(e) = db::FirefoxPlatformItem::upsert(&item_id, country_id, phone_num).await {
                                log::error!(
                                    "[火狐狸轮询] 刷新平台项目映射失败 - 号码: {}, Item_ID: {}, 错误: {}",
                                    phone_num,
                                    item_id,
                                    e
                                );
                            }

                            let task_key = format!("{}|{}|{}", country_id, phone_num, item_id);
                            let now = tokio::time::Instant::now();

                            if let Some(until) = no_sms_backoff_until.get(&task_key) {
                                if *until > now {
                                    continue;
                                }
                                no_sms_backoff_until.remove(&task_key);
                            }

                            // Skip if we already processed this item
                            if processed_tasks.contains(&task_key) {
                                // Still check result list for release status
                                if let Ok(result_resp) = firefox_api::get_result_phone_list(&client, &api_key, country_id, phone_num, &item_id).await {
                                    if let Some(data) = result_resp.data {
                                        if let Some(arr) = data.as_array() {
                                            for result_item in arr {
                                                let is_ret = result_item.get("Phone_IsRet").and_then(|v| v.as_str()).unwrap_or("false");
                                                let remark = result_item.get("Phone_Remark").and_then(|v| v.as_str()).unwrap_or("");
                                                if is_ret == "True" || is_ret == "true" || !remark.is_empty() {
                                                    processed_tasks.remove(&task_key);
                                                    no_sms_backoff_until.remove(&task_key);
                                                    no_sms_miss_count.remove(&task_key);
                                                    log::info!("[火狐狸轮询] 任务已释放 - 号码: {}, Item_ID: {}, 备注: {}", phone_num, item_id, remark);
                                                }
                                            }
                                        }
                                    }
                                }
                                continue;
                            }

                            log::info!("[火狐狸轮询] 等待短信 - 号码: {}, Country: {}, Item_ID: {}", phone_num, country_id, item_id);

                            // Find the SIM that owns this phone number
                            let sim_id = modem_manager.find_sim_id_by_phone_number(phone_num).await;
                            let sim_id = match sim_id {
                                Some(id) => id,
                                None => {
                                    log::warn!("[火狐狸轮询] 未找到号码 {} 对应的 SIM 卡", phone_num);
                                    processed_tasks.insert(task_key.clone());
                                    continue;
                                }
                            };

                            if let Err(e) = db::Sms::normalize_failed_item_mapping_for_phone(&sim_id, phone_num, &item_id).await {
                                log::error!(
                                    "[火狐狸轮询] 修正失败短信平台映射失败 - 号码: {}, SIM: {}, Item_ID: {}, 错误: {}",
                                    phone_num,
                                    sim_id,
                                    item_id,
                                    e
                                );
                            }

                            log::info!("[火狐狸轮询] 找到 SIM: {}, 强制读取短信", sim_id);

                            // Read SMS from modem, sync to DB, and get the latest incoming message
                            let mut mark_task_processed = true;
                            match crate::db::Sms::find_recent_received_sms(&sim_id, 1).await.map(|mut rows| rows.into_iter().next()) {
                                Ok(Some(sms)) => {
                                    let upload_content = db::build_upload_sms_content(&item_id, &sms.message)
                                        .await
                                        .unwrap_or_else(|e| {
                                            log::error!(
                                                "[火狐狸轮询] 构建上传短信内容失败 (SMS ID: {}, Item_ID: {}): {}，使用原始短信内容",
                                                sms.id,
                                                item_id,
                                                e
                                            );
                                            sms.message.clone()
                                        });
                                    log::info!("[火狐狸轮询] 获取到短信内容 ({}): {}, 上传中 - 号码: {}", sms.contact_id, sms.message, phone_num);
                                    match firefox_api::upload_sms(&client, &api_key, country_id, phone_num, &upload_content).await {
                                        Ok(upload_resp) if upload_resp.code == "1" => {
                                            let response_json = serde_json::to_string(&upload_resp).ok();
                                            if let Err(e) = db::Sms::mark_platform_attempt(
                                                sms.id,
                                                &item_id,
                                                true,
                                                response_json.as_deref(),
                                            )
                                            .await
                                            {
                                                log::error!(
                                                    "[火狐狸轮询] 标记短信上传成功失败 (SMS ID: {}, Item_ID: {}): {}",
                                                    sms.id,
                                                    item_id,
                                                    e
                                                );
                                            }
                                            log::info!("[火狐狸轮询] 短信上传成功 - 号码: {}, Item_ID: {}, 响应: {:?}", phone_num, item_id, upload_resp)
                                        }
                                        Ok(upload_resp) => {
                                            let response_json = serde_json::to_string(&upload_resp).ok();
                                            if let Err(e) = db::Sms::mark_platform_attempt(
                                                sms.id,
                                                &item_id,
                                                false,
                                                response_json.as_deref(),
                                            )
                                            .await
                                            {
                                                log::error!(
                                                    "[火狐狸轮询] 标记短信上传失败失败 (SMS ID: {}, Item_ID: {}): {}",
                                                    sms.id,
                                                    item_id,
                                                    e
                                                );
                                            }

                                            if firefox_api::is_unretryable_platform_rejection(&upload_resp) {
                                                log::warn!(
                                                    "[火狐狸轮询] 短信上传失败（不可重试），不进入重试队列 - 号码: {}, Item_ID: {}, code: {}, data: {:?}",
                                                    phone_num,
                                                    item_id,
                                                    upload_resp.code,
                                                    upload_resp.data
                                                );
                                            } else {
                                                // Enqueue retry only for retryable failures.
                                                let retry_item = firefox_upload_retry::FirefoxUploadRetryItem::new(
                                                    sms.id,
                                                    phone_num.to_string(),
                                                    country_id.to_string(),
                                                    sms.message.clone(),
                                                    format!("Upload failed with code: {}", upload_resp.code),
                                                    Some(upload_resp.code.clone()),
                                                );

                                                match firefox_upload_retry::FirefoxUploadRetryItem::insert(
                                                    &db::get_pool().unwrap_or_else(|_| panic!("No DB pool")),
                                                    &retry_item
                                                ).await {
                                                    Ok(_) => log::info!("[火狐狸轮询] 短信上传失败，已加入重试队列 - 号码: {}, Item_ID: {}, 重试ID: {}, code: {}", phone_num, item_id, retry_item.id, upload_resp.code),
                                                    Err(e) => log::error!("[火狐狸轮询] 将失败的短信加入重试队列失败: {}", e),
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            let error_message = format!("Upload failed: {}", e);
                                            if let Err(mark_err) = db::Sms::mark_platform_attempt(
                                                sms.id,
                                                &item_id,
                                                false,
                                                Some(&error_message),
                                            )
                                            .await
                                            {
                                                log::error!(
                                                    "[火狐狸轮询] 标记短信上传错误失败 (SMS ID: {}, Item_ID: {}): {}",
                                                    sms.id,
                                                    item_id,
                                                    mark_err
                                                );
                                            }

                                            // Network error - also queue for retry
                                            let retry_item = firefox_upload_retry::FirefoxUploadRetryItem::new(
                                                sms.id,
                                                phone_num.to_string(),
                                                country_id.to_string(),
                                                sms.message.clone(),
                                                format!("Upload failed: {}", e),
                                                None,
                                            );
                                            
                                            match firefox_upload_retry::FirefoxUploadRetryItem::insert(
                                                &db::get_pool().unwrap_or_else(|_| panic!("No DB pool")),
                                                &retry_item
                                            ).await {
                                                Ok(_) => log::info!("[火狐狸轮询] 短信上传失败，已加入重试队列 - 号码: {}, Item_ID: {}, 重试ID: {}, 错误: {}", phone_num, item_id, retry_item.id, e),
                                                Err(e) => log::error!("[火狐狸轮询] 将失败的短信加入重试队列失败: {}", e),
                                            }
                                        },
                                    }
                                }
                                Ok(None) => {
                                    mark_task_processed = false;
                                    log::warn!("[火狐狸轮询] 读取 SMS 后未找到短信 (SIM: {}, 号码: {})", sim_id, phone_num);
                                    // Check the result list in case platform already has content
                                    if let Ok(result_resp) = firefox_api::get_result_phone_list(&client, &api_key, country_id, phone_num, &item_id).await {
                                        if let Some(data) = result_resp.data {
                                            if let Some(arr) = data.as_array() {
                                                for result_item in arr {
                                                    if let Some(content) = result_item.get("Phone_SmsContent").and_then(|v| v.as_str()) {
                                                        if !content.is_empty() {
                                                            let upload_content = db::build_upload_sms_content(&item_id, content)
                                                                .await
                                                                .unwrap_or_else(|e| {
                                                                    log::error!(
                                                                        "[火狐狸轮询] 构建平台列表上传短信内容失败 (SIM: {}, Item_ID: {}): {}，使用原始短信内容",
                                                                        sim_id,
                                                                        item_id,
                                                                        e
                                                                    );
                                                                    content.to_string()
                                                                });
                                                            log::info!("[火狐狸轮询] 从平台获取到短信内容, 上传中 - 号码: {}, Item_ID: {}", phone_num, item_id);
                                                            match firefox_api::upload_sms(&client, &api_key, country_id, phone_num, &upload_content).await {
                                                                Ok(upload_resp) if upload_resp.code == "1" => {
                                                                    let response_json = serde_json::to_string(&upload_resp).ok();
                                                                    if let Err(e) = db::Sms::mark_platform_attempt_by_phone_message(
                                                                        phone_num,
                                                                        &sim_id,
                                                                        &[content.to_string()],
                                                                        Some(&item_id),
                                                                        true,
                                                                        response_json,
                                                                    )
                                                                    .await
                                                                    {
                                                                        log::error!(
                                                                            "[火狐狸轮询] 标记平台列表短信上传成功失败 (SIM: {}, Item_ID: {}): {}",
                                                                            sim_id,
                                                                            item_id,
                                                                            e
                                                                        );
                                                                    }
                                                                    log::info!("[火狐狸轮询] 短信上传成功 - 号码: {}, Item_ID: {}, 响应: {:?}", phone_num, item_id, upload_resp)
                                                                }
                                                                Ok(upload_resp) => {
                                                                    let response_json = serde_json::to_string(&upload_resp).ok();
                                                                    if let Err(e) = db::Sms::mark_platform_attempt_by_phone_message(
                                                                        phone_num,
                                                                        &sim_id,
                                                                        &[content.to_string()],
                                                                        Some(&item_id),
                                                                        false,
                                                                        response_json,
                                                                    )
                                                                    .await
                                                                    {
                                                                        log::error!(
                                                                            "[火狐狸轮询] 标记平台列表短信上传失败失败 (SIM: {}, Item_ID: {}): {}",
                                                                            sim_id,
                                                                            item_id,
                                                                            e
                                                                        );
                                                                    }

                                                                    if firefox_api::is_unretryable_platform_rejection(&upload_resp) {
                                                                        log::warn!(
                                                                            "[火狐狸轮询] 平台列表短信上传失败（不可重试），不进入重试队列 - 号码: {}, Item_ID: {}, code: {}, data: {:?}",
                                                                            phone_num,
                                                                            item_id,
                                                                            upload_resp.code,
                                                                            upload_resp.data
                                                                        );
                                                                    } else {
                                                                        let sms_id = match db::Sms::find_latest_incoming_id_by_sim_message(
                                                                            &sim_id,
                                                                            content,
                                                                        )
                                                                        .await
                                                                        {
                                                                            Ok(Some(id)) => id,
                                                                            Ok(None) => {
                                                                                log::error!(
                                                                                    "[火狐狸轮询] 平台列表短信重试入队失败：未找到SMS记录 (SIM: {}, Item_ID: {}, phone: {})",
                                                                                    sim_id,
                                                                                    item_id,
                                                                                    phone_num
                                                                                );
                                                                                continue;
                                                                            }
                                                                            Err(e) => {
                                                                                log::error!(
                                                                                    "[火狐狸轮询] 平台列表短信重试入队失败：查询SMS记录错误 (SIM: {}, Item_ID: {}): {}",
                                                                                    sim_id,
                                                                                    item_id,
                                                                                    e
                                                                                );
                                                                                continue;
                                                                            }
                                                                        };

                                                                        let retry_item = firefox_upload_retry::FirefoxUploadRetryItem::new(
                                                                            sms_id,
                                                                            phone_num.to_string(),
                                                                            country_id.to_string(),
                                                                            content.to_string(),
                                                                            format!("Upload failed with code: {}", upload_resp.code),
                                                                            Some(upload_resp.code.clone()),
                                                                        );

                                                                        match firefox_upload_retry::FirefoxUploadRetryItem::insert(
                                                                            &db::get_pool().unwrap_or_else(|_| panic!("No DB pool")),
                                                                            &retry_item
                                                                        ).await {
                                                                            Ok(_) => log::info!("[火狐狸轮询] 短信上传失败（来自平台列表），已加入重试队列 - 号码: {}, Item_ID: {}, 重试ID: {}", phone_num, item_id, retry_item.id),
                                                                            Err(e) => log::error!("[火狐狸轮询] 将失败的短信加入重试队列失败: {}", e),
                                                                        }
                                                                    }
                                                                },
                                                                Err(e) => {
                                                                    let error_message = format!("Upload failed: {}", e);
                                                                    if let Err(mark_err) = db::Sms::mark_platform_attempt_by_phone_message(
                                                                        phone_num,
                                                                        &sim_id,
                                                                        &[content.to_string()],
                                                                        Some(&item_id),
                                                                        false,
                                                                        Some(error_message.clone()),
                                                                    )
                                                                    .await
                                                                    {
                                                                        log::error!(
                                                                            "[火狐狸轮询] 标记平台列表短信上传错误失败 (SIM: {}, Item_ID: {}): {}",
                                                                            sim_id,
                                                                            item_id,
                                                                            mark_err
                                                                        );
                                                                    }

                                                                    // Network error - also queue for retry
                                                                    let sms_id = match db::Sms::find_latest_incoming_id_by_sim_message(
                                                                        &sim_id,
                                                                        content,
                                                                    )
                                                                    .await
                                                                    {
                                                                        Ok(Some(id)) => id,
                                                                        Ok(None) => {
                                                                            log::error!(
                                                                                "[火狐狸轮询] 平台列表短信重试入队失败：未找到SMS记录 (SIM: {}, Item_ID: {}, phone: {})",
                                                                                sim_id,
                                                                                item_id,
                                                                                phone_num
                                                                            );
                                                                            continue;
                                                                        }
                                                                        Err(e) => {
                                                                            log::error!(
                                                                                "[火狐狸轮询] 平台列表短信重试入队失败：查询SMS记录错误 (SIM: {}, Item_ID: {}): {}",
                                                                                sim_id,
                                                                                item_id,
                                                                                e
                                                                            );
                                                                            continue;
                                                                        }
                                                                    };

                                                                    let retry_item = firefox_upload_retry::FirefoxUploadRetryItem::new(
                                                                        sms_id,
                                                                        phone_num.to_string(),
                                                                        country_id.to_string(),
                                                                        content.to_string(),
                                                                        format!("Upload failed: {}", e),
                                                                        None,
                                                                    );
                                                                     
                                                                    match firefox_upload_retry::FirefoxUploadRetryItem::insert(
                                                                        &db::get_pool().unwrap_or_else(|_| panic!("No DB pool")),
                                                                        &retry_item
                                                                    ).await {
                                                                        Ok(_) => log::info!("[火狐狸轮询] 短信上传失败（来自平台列表），已加入重试队列 - 号码: {}, Item_ID: {}, 重试ID: {}", phone_num, item_id, retry_item.id),
                                                                        Err(e) => log::error!("[火狐狸轮询] 将失败的短信加入重试队列失败: {}", e),
                                                                    }
                                                                },
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    mark_task_processed = false;
                                    log::warn!("[火狐狸轮询] 读取/查询短信失败 (SIM: {}): {}", sim_id, e)
                                },
                            }

                            if mark_task_processed {
                                no_sms_backoff_until.remove(&task_key);
                                no_sms_miss_count.remove(&task_key);
                                processed_tasks.insert(task_key);
                            } else {
                                let miss = no_sms_miss_count
                                    .entry(task_key.clone())
                                    .and_modify(|v| *v = (*v + 1).min(6))
                                    .or_insert(1);
                                let shift = (*miss as u32).saturating_sub(1);
                                let delay_secs = (3u64.saturating_mul(1u64 << shift)).min(60);
                                no_sms_backoff_until.insert(
                                    task_key.clone(),
                                    tokio::time::Instant::now()
                                        + tokio::time::Duration::from_secs(delay_secs),
                                );
                                log::info!(
                                    "[火狐狸轮询] 暂未读到短信，进入退避: 号码={}, Item_ID={}, miss_count={}, delay={}s",
                                    phone_num,
                                    item_id,
                                    miss,
                                    delay_secs
                                );
                            }
                        }
                    }
                }
            }

        if heartbeat_count % 10 == 0 {
            log::info!("[火狐狸轮询] 运行中 (心跳), 已轮询 {} 次", heartbeat_count);
        }
    }
}

