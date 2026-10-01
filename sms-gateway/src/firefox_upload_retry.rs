use anyhow::Result;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

/// Represents a failed 火狐狸 SMS upload queued for retry
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FirefoxUploadRetryItem {
    pub id: String,
    pub sms_id: i64,
    pub phone_number: String,
    pub country_id: String,
    pub message: String,
    pub retry_count: i32,
    pub max_retries: i32,
    pub next_retry_at: chrono::NaiveDateTime,
    pub last_error: Option<String>,
    pub last_response_code: Option<String>,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

impl FirefoxUploadRetryItem {
    /// Create a new retry item with exponential backoff
    pub fn new(
        sms_id: i64,
        phone_number: String,
        country_id: String,
        message: String,
        error: String,
        response_code: Option<String>,
    ) -> Self {
        let now = Utc::now().naive_utc();
        // Initial retry after 5 seconds
        let next_retry_at = now + Duration::seconds(5);

        Self {
            id: Uuid::new_v4().to_string(),
            sms_id,
            phone_number,
            country_id,
            message,
            retry_count: 0,
            max_retries: 5,
            next_retry_at,
            last_error: Some(error),
            last_response_code: response_code,
            created_at: now,
            updated_at: now,
        }
    }

    /// Calculate next retry time with exponential backoff
    /// Base: 5s, 10s, 20s, 40s, 80s, 160s (cap at ~3 minutes)
    fn calculate_next_retry_delay(retry_count: i32) -> i64 {
        let base_delay = 5i64; // 5 seconds
        let delay_secs = base_delay * 2_i64.pow(retry_count as u32);
        delay_secs.min(180) // Cap at 3 minutes
    }

    /// Insert a new retry item
    pub async fn insert(pool: &SqlitePool, item: &Self) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO firefox_upload_retry_queue
            (id, sms_id, phone_number, country_id, message, retry_count, max_retries,
             next_retry_at, last_error, last_response_code, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&item.id)
        .bind(item.sms_id)
        .bind(&item.phone_number)
        .bind(&item.country_id)
        .bind(&item.message)
        .bind(item.retry_count)
        .bind(item.max_retries)
        .bind(item.next_retry_at)
        .bind(&item.last_error)
        .bind(&item.last_response_code)
        .bind(item.created_at)
        .bind(item.updated_at)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Get all items ready for retry (next_retry_at <= now, retry_count < max_retries)
    pub async fn get_ready_for_retry(pool: &SqlitePool, limit: i32) -> Result<Vec<Self>> {
        let now = Utc::now().naive_utc();
        let items = sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM firefox_upload_retry_queue
            WHERE next_retry_at <= ? AND retry_count < max_retries
            ORDER BY next_retry_at ASC
            LIMIT ?
            "#,
        )
        .bind(now)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        Ok(items)
    }

    /// Update retry metadata after a failed attempt
    pub async fn update_after_retry(
        pool: &SqlitePool,
        id: &str,
        retry_count: i32,
        error: String,
        response_code: Option<String>,
    ) -> Result<()> {
        let next_delay = Self::calculate_next_retry_delay(retry_count);
        let next_retry_at = Utc::now().naive_utc() + Duration::seconds(next_delay);
        let now = Utc::now().naive_utc();

        sqlx::query(
            r#"
            UPDATE firefox_upload_retry_queue
            SET retry_count = ?, next_retry_at = ?, last_error = ?, 
                last_response_code = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(retry_count + 1)
        .bind(next_retry_at)
        .bind(&error)
        .bind(&response_code)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Mark item as successfully uploaded (remove from queue)
    pub async fn mark_success(pool: &SqlitePool, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM firefox_upload_retry_queue WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(())
    }

    /// Mark item as dead-letter immediately (no more retries).
    pub async fn mark_dead_letter(
        pool: &SqlitePool,
        id: &str,
        error: String,
        response_code: Option<String>,
    ) -> Result<()> {
        let now = Utc::now().naive_utc();

        sqlx::query(
            r#"
            UPDATE firefox_upload_retry_queue
            SET retry_count = max_retries,
                next_retry_at = ?,
                last_error = ?,
                last_response_code = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(&error)
        .bind(&response_code)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Get dead-letter items (exceeded max retries)
    pub async fn get_dead_letter_items(pool: &SqlitePool, limit: i32) -> Result<Vec<Self>> {
        let items = sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM firefox_upload_retry_queue
            WHERE retry_count >= max_retries
            ORDER BY created_at ASC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;

        Ok(items)
    }

    /// Get queue statistics
    pub async fn get_stats(pool: &SqlitePool) -> Result<QueueStats> {
        let now = Utc::now().naive_utc();

        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM firefox_upload_retry_queue")
            .fetch_one(pool)
            .await?;

        let ready: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM firefox_upload_retry_queue
            WHERE next_retry_at <= ? AND retry_count < max_retries
            "#,
        )
        .bind(now)
        .fetch_one(pool)
        .await?;

        let dead_letter: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM firefox_upload_retry_queue WHERE retry_count >= max_retries",
        )
        .fetch_one(pool)
        .await?;

        Ok(QueueStats {
            total_items: total as i32,
            ready_for_retry: ready as i32,
            dead_letter_items: dead_letter as i32,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct QueueStats {
    pub total_items: i32,
    pub ready_for_retry: i32,
    pub dead_letter_items: i32,
}
