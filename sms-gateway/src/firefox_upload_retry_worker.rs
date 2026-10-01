use crate::db;
/// Background worker that retries failed 火狐狸 SMS uploads
/// This worker:
/// 1. Polls the retry queue every 10 seconds
/// 2. Retries uploads that are ready (next_retry_at <= now)
/// 3. Uses exponential backoff (5s, 10s, 20s, 40s, 80s, 160s max)
/// 4. Logs dead-letter items (exceeded max retries)
use crate::firefox_api;
use crate::firefox_upload_retry::FirefoxUploadRetryItem;
use log::{error, info, warn};
use std::time::Duration;

async fn mark_retry_sms_attempt(
    item: &FirefoxUploadRetryItem,
    uploaded_to_platform: bool,
    response: Option<String>,
) {
    let platform_item_id = if item.sms_id > 0 {
        match db::get_pool() {
            Ok(pool) => match sqlx::query_scalar::<_, Option<String>>(
                "SELECT platform_item_id FROM sms WHERE id = ?",
            )
            .bind(item.sms_id)
            .fetch_one(pool)
            .await
            {
                Ok(Some(item_id)) => item_id,
                Ok(None) => {
                    match db::FirefoxPlatformItem::find_latest_item_for_phone(&item.phone_number)
                        .await
                    {
                        Ok(Some(item_id)) => item_id,
                        Ok(None) => {
                            warn!(
                            "[Firefox Upload Retry] No platform item found for phone {} while updating SMS status",
                            item.phone_number
                        );
                            return;
                        }
                        Err(e) => {
                            error!(
                            "[Firefox Upload Retry] Failed to resolve platform item for phone {}: {}",
                            item.phone_number, e
                        );
                            return;
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "[Firefox Upload Retry] Failed to read existing platform item for SMS {}: {}",
                        item.sms_id, e
                    );
                    return;
                }
            },
            Err(e) => {
                error!(
                    "[Firefox Upload Retry] Failed to access DB pool while resolving SMS {}: {}",
                    item.sms_id, e
                );
                return;
            }
        }
    } else {
        match db::FirefoxPlatformItem::find_latest_item_for_phone(&item.phone_number).await {
            Ok(Some(item_id)) => item_id,
            Ok(None) => {
                warn!(
                "[Firefox Upload Retry] No platform item found for phone {} while updating SMS status",
                item.phone_number
            );
                return;
            }
            Err(e) => {
                error!(
                    "[Firefox Upload Retry] Failed to resolve platform item for phone {}: {}",
                    item.phone_number, e
                );
                return;
            }
        }
    };

    if item.sms_id > 0 {
        if let Err(e) = db::Sms::mark_platform_attempt(
            item.sms_id,
            &platform_item_id,
            uploaded_to_platform,
            response.as_deref(),
        )
        .await
        {
            error!(
                "[Firefox Upload Retry] Failed to update SMS {} after retry result: {}",
                item.sms_id, e
            );
        }
        return;
    }

    match db::SimCard::find_by_conditions(None, None, None, Some(&item.phone_number)).await {
        Ok(cards) => {
            if let Some(card) = cards.first() {
                if let Err(e) = db::Sms::mark_platform_attempt_by_phone_message(
                    &item.phone_number,
                    &card.id,
                    &[item.message.clone()],
                    Some(&platform_item_id),
                    uploaded_to_platform,
                    response,
                )
                .await
                {
                    error!(
                        "[Firefox Upload Retry] Failed to update SMS by phone/message after retry result: {}",
                        e
                    );
                }
            }
        }
        Err(e) => {
            error!(
                "[Firefox Upload Retry] Failed to resolve SIM card for phone {}: {}",
                item.phone_number, e
            );
        }
    }
}

/// Start the retry worker background task
pub fn start_retry_worker() {
    tokio::spawn(async move {
        retry_worker_loop().await;
    });
}

async fn retry_worker_loop() {
    let mut interval = tokio::time::interval(Duration::from_secs(10));

    loop {
        interval.tick().await;

        if let Err(e) = process_retry_queue().await {
            error!("[Firefox Upload Retry] Error processing queue: {}", e);
        }
    }
}

async fn process_retry_queue() -> anyhow::Result<()> {
    if !crate::service_control::is_running() {
        return Ok(());
    }

    let pool = db::get_pool()?;
    // Get items ready for retry (limit 10 per batch)
    let items = FirefoxUploadRetryItem::get_ready_for_retry(pool, 10).await?;

    if items.is_empty() {
        return Ok(());
    }

    info!(
        "[Firefox Upload Retry] Processing {} items from retry queue",
        items.len()
    );

    // Get API key
    let api_key = match crate::db::AppSetting::get("firefox_api_key").await {
        Ok(Some(key)) => key,
        _ => {
            warn!("[Firefox Upload Retry] API key not configured, skipping retry batch");
            return Ok(());
        }
    };

    // Build HTTP client
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            error!("[Firefox Upload Retry] Failed to build HTTP client: {}", e);
            return Ok(());
        }
    };

    // Process each item
    for item in items {
        // Resolve the item_id for this retry so we can prefix the item name if needed.
        let item_id = if item.sms_id > 0 {
            match db::get_pool() {
                Ok(pool) => match sqlx::query_scalar::<_, Option<String>>(
                    "SELECT platform_item_id FROM sms WHERE id = ?",
                )
                .bind(item.sms_id)
                .fetch_one(pool)
                .await
                {
                    Ok(Some(id)) => Some(id),
                    Ok(None) => {
                        db::FirefoxPlatformItem::find_latest_item_for_phone(&item.phone_number)
                            .await
                            .ok()
                            .flatten()
                    }
                    Err(e) => {
                        error!(
                            "[Firefox Upload Retry] Failed to read platform_item_id for SMS {}: {}",
                            item.sms_id, e
                        );
                        None
                    }
                },
                Err(e) => {
                    error!(
                        "[Firefox Upload Retry] Failed to access DB pool for SMS {}: {}",
                        item.sms_id, e
                    );
                    None
                }
            }
        } else {
            db::FirefoxPlatformItem::find_latest_item_for_phone(&item.phone_number)
                .await
                .ok()
                .flatten()
        };

        let upload_content = if let Some(ref id) = item_id {
            db::build_upload_sms_content(id, &item.message).await.unwrap_or_else(|e| {
                error!(
                    "[Firefox Upload Retry] Failed to build upload content for SMS {}: {}, using original",
                    item.sms_id, e
                );
                item.message.clone()
            })
        } else {
            item.message.clone()
        };

        match firefox_api::upload_sms(
            &client,
            &api_key,
            &item.country_id,
            &item.phone_number,
            &upload_content,
        )
        .await
        {
            Ok(resp) if resp.code == "1" => {
                // Success! Remove from queue
                info!(
                    "[Firefox Upload Retry] Upload succeeded (SMS ID: {}, retry attempt: {})",
                    item.sms_id,
                    item.retry_count + 1
                );

                if let Err(e) = FirefoxUploadRetryItem::mark_success(pool, &item.id).await {
                    error!(
                        "[Firefox Upload Retry] Failed to mark item {} as success: {}",
                        item.id, e
                    );
                }

                mark_retry_sms_attempt(&item, true, serde_json::to_string(&resp).ok()).await;
            }
            Ok(resp) => {
                // Platform rejection (code != "1")
                let error_msg = format!(
                    "Platform rejected: code={}, data={:?}",
                    resp.code, resp.data
                );

                mark_retry_sms_attempt(&item, false, Some(error_msg.clone())).await;

                if firefox_api::is_unretryable_platform_rejection(&resp) {
                    warn!(
                        "[Firefox Upload Retry] Item {} marked dead-letter immediately (non-retryable): {}",
                        item.id, error_msg
                    );
                    if let Err(e) = FirefoxUploadRetryItem::mark_dead_letter(
                        pool,
                        &item.id,
                        error_msg,
                        Some(resp.code),
                    )
                    .await
                    {
                        error!(
                            "[Firefox Upload Retry] Failed to mark item {} as dead-letter: {}",
                            item.id, e
                        );
                    }
                    continue;
                }

                if item.retry_count + 1 >= item.max_retries {
                    // Dead letter
                    warn!(
                        "[Firefox Upload Retry] Item {} exceeded max retries ({}): {}",
                        item.id, item.max_retries, error_msg
                    );
                } else {
                    // Retry
                    info!(
                        "[Firefox Upload Retry] Requeueing item {} (attempt {}/{}): {}",
                        item.id,
                        item.retry_count + 1,
                        item.max_retries,
                        error_msg
                    );
                }

                if let Err(e) = FirefoxUploadRetryItem::update_after_retry(
                    pool,
                    &item.id,
                    item.retry_count,
                    error_msg,
                    Some(resp.code),
                )
                .await
                {
                    error!(
                        "[Firefox Upload Retry] Failed to update item {}: {}",
                        item.id, e
                    );
                }
            }
            Err(e) => {
                // Network/HTTP error
                let error_msg = format!("HTTP request failed: {}", e);

                mark_retry_sms_attempt(&item, false, Some(error_msg.clone())).await;

                if item.retry_count + 1 >= item.max_retries {
                    // Dead letter
                    warn!(
                        "[Firefox Upload Retry] Item {} exceeded max retries after {} attempts: {}",
                        item.id, item.max_retries, error_msg
                    );
                } else {
                    // Retry
                    info!(
                        "[Firefox Upload Retry] Requeueing item {} after transient error (attempt {}/{})",
                        item.id, item.retry_count + 1, item.max_retries
                    );
                }

                if let Err(e) = FirefoxUploadRetryItem::update_after_retry(
                    pool,
                    &item.id,
                    item.retry_count,
                    error_msg,
                    None,
                )
                .await
                {
                    error!(
                        "[Firefox Upload Retry] Failed to update item {}: {}",
                        item.id, e
                    );
                }
            }
        }

        // Rate limiting: small delay between retries
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}
