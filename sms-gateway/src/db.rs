// migrations: 20260529000001_calls_recording 20260529000002_calls_transcript
use anyhow::Result;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::migrate::MigrateDatabase;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use sqlx::{migrate, Sqlite, Transaction};
use sqlx::{FromRow, QueryBuilder};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use uuid::Uuid;

const MAX_BATCH_SIZE: usize = 500;

static POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Represents a single SMS message
#[derive(Debug, FromRow, Deserialize, Serialize, Default)]
pub struct Sms {
    pub id: i64,
    pub contact_id: String,
    pub timestamp: NaiveDateTime,
    pub message: String,
    pub sim_id: String,
    pub send: bool,
    pub status: SmsStatus,
    #[sqlx(default)]
    pub uploaded_to_platform: bool,
    #[sqlx(default)]
    pub platform_item_id: Option<String>,
    #[sqlx(default)]
    pub platform_uploaded_at: Option<NaiveDateTime>,
    #[sqlx(default)]
    pub platform_response: Option<String>,
}

/// SMS row with contact name resolved — used for inbox/sent list view.
#[derive(Debug, FromRow, Serialize)]
pub struct SmsRow {
    pub id: i64,
    pub contact_id: String,
    pub contact_name: String,
    pub timestamp: NaiveDateTime,
    pub message: String,
    pub sim_id: String,
    pub send: bool,
    pub status: SmsStatus,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, sqlx::Type, Default)]
#[repr(i32)]
#[serde(into = "i32", from = "i32")]
pub enum SmsStatus {
    #[default]
    Unread = 0,
    Read = 1,
    Loading = 2,
    Failed = 3,
}

impl From<i32> for SmsStatus {
    fn from(value: i32) -> Self {
        match value {
            1 => SmsStatus::Read,
            2 => SmsStatus::Loading,
            3 => SmsStatus::Failed,
            _ => SmsStatus::Unread,
        }
    }
}

impl From<SmsStatus> for i32 {
    fn from(status: SmsStatus) -> Self {
        status as i32
    }
}

#[derive(Debug, Clone)]
pub struct ModemSMS {
    pub contact: String,
    pub timestamp: NaiveDateTime,
    pub message: String,
    pub send: bool,
    pub sim_id: String,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct Contact {
    pub id: String,
    pub name: String,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct SMSPreview {
    pub message: String,
    pub timestamp: NaiveDateTime,
    pub status: SmsStatus,
    pub sim_id: String,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct SimCard {
    pub id: String, // ICCID
    pub imsi: Option<String>,
    pub phone_number: Option<String>, // Phone number from SIM
    pub alias: Option<String>,        // User-defined alias
    #[sqlx(default)]
    pub country_code: Option<String>,
    // Note: port_path removed - SIM to port mapping is runtime only
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct Conversation {
    #[sqlx(flatten)]
    pub contact: Contact,
    #[sqlx(flatten)]
    pub sms_preview: SMSPreview,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct SimSmsStat {
    pub sim_id: String,
    pub recv: i64,
    pub sent: i64,
    pub phone_number: Option<String>,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Clone)]
pub struct Call {
    pub id: String,
    pub sim_id: String,
    pub phone: Option<String>,
    pub direction: String, // "inbound" | "outbound"
    pub status: String,    // "ringing" | "active" | "ended" | "missed"
    pub started_at: NaiveDateTime,
    pub ended_at: Option<NaiveDateTime>,
}

impl Call {
    #[allow(dead_code)]
    pub async fn insert(sim_id: &str, phone: Option<&str>, direction: &str) -> Result<String> {
        let pool = get_pool()?;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO calls (id, sim_id, phone, direction, status) VALUES (?, ?, ?, ?, 'ringing')"#,
        )
        .bind(&id)
        .bind(sim_id)
        .bind(phone)
        .bind(direction)
        .execute(pool)
        .await?;
        Ok(id)
    }

    /// Insert a call with a caller-supplied id (used by AMI consumer to use
    /// Asterisk's UNIQUEID as the primary key so subsequent UserEvents can
    /// look the row up directly). Idempotent: duplicate ids are ignored.
    pub async fn insert_with_id(
        id: &str,
        sim_id: &str,
        phone: Option<&str>,
        direction: &str,
    ) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            r#"INSERT OR IGNORE INTO calls (id, sim_id, phone, direction, status) VALUES (?, ?, ?, ?, 'ringing')"#,
        )
        .bind(id)
        .bind(sim_id)
        .bind(phone)
        .bind(direction)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn update_status(id: &str, status: &str) -> Result<()> {
        let pool = get_pool()?;
        // Use CURRENT_TIMESTAMP (UTC) to match started_at which is also stored in UTC
        if status == "ended" || status == "missed" {
            sqlx::query(
                r#"UPDATE calls SET status = ?, ended_at = CURRENT_TIMESTAMP WHERE id = ?"#,
            )
            .bind(status)
            .bind(id)
            .execute(pool)
            .await?;
        } else {
            sqlx::query(
                r#"UPDATE calls SET status = ? WHERE id = ?"#,
            )
            .bind(status)
            .bind(id)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    /// Update the phone number on an existing call record (e.g. when +CLIP: arrives after RING).
    #[allow(dead_code)]
    pub async fn update_phone(id: &str, phone: &str) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(r#"UPDATE calls SET phone = ? WHERE id = ?"#)
            .bind(phone)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn query_by_sim(sim_id: &str, limit: i64) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let calls = sqlx::query_as(
            r#"SELECT id, sim_id, phone, direction, status, started_at, ended_at
               FROM calls WHERE sim_id = ? ORDER BY started_at DESC LIMIT ?"#,
        )
        .bind(sim_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(calls)
    }

    pub async fn query_all(limit: i64, offset: i64) -> Result<(Vec<Self>, i64)> {
        let pool = get_pool()?;
        let calls = sqlx::query_as(
            r#"SELECT id, sim_id, phone, direction, status, started_at, ended_at
               FROM calls ORDER BY started_at DESC LIMIT ? OFFSET ?"#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM calls")
            .fetch_one(pool)
            .await?;
        Ok((calls, total))
    }

    /// Find the most recent ringing/active inbound call for a SIM.
    #[allow(dead_code)]
    pub async fn find_active_inbound(sim_id: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let call = sqlx::query_as(
            r#"SELECT id, sim_id, phone, direction, status, started_at, ended_at
               FROM calls WHERE sim_id = ? AND direction = 'inbound'
               AND status IN ('ringing', 'active')
               ORDER BY started_at DESC LIMIT 1"#,
        )
        .bind(sim_id)
        .fetch_optional(pool)
        .await?;
        Ok(call)
    }

    /// Save a raw recording blob for a call.
    pub async fn save_recording(id: &str, data: &[u8]) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(r#"UPDATE calls SET recording = ? WHERE id = ?"#)
            .bind(data)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Fetch the raw recording blob for a call. Returns `None` if no recording exists.
    pub async fn get_recording(id: &str) -> Result<Option<Vec<u8>>> {
        let pool = get_pool()?;
        let row: Option<(Vec<u8>,)> = sqlx::query_as(
            r#"SELECT recording FROM calls WHERE id = ? AND recording IS NOT NULL"#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(|(data,)| data))
    }

    /// Save the Whisper transcript for a call.
    pub async fn save_transcript(id: &str, text: &str) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(r#"UPDATE calls SET transcript = ? WHERE id = ?"#)
            .bind(text)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Fetch the transcript for a call. Returns `None` if not yet transcribed.
    pub async fn get_transcript(id: &str) -> Result<Option<String>> {
        let pool = get_pool()?;
        let row: Option<(String,)> = sqlx::query_as(
            r#"SELECT transcript FROM calls WHERE id = ? AND transcript IS NOT NULL"#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(|(text,)| text))
    }
}

impl Sms {
    pub(crate) fn contact_name_expr_for_direction(send: bool) -> &'static str {
        if send {
            "COALESCE(c.name, s.contact_id)"
        } else {
            "s.contact_id"
        }
    }

    pub(crate) fn extract_phone_like_token(text: &str) -> Option<String> {
        let mut best = String::new();
        let mut current = String::new();

        for ch in text.chars() {
            if ch.is_ascii_digit() || (ch == '+' && current.is_empty()) {
                current.push(ch);
            } else {
                if current.len() >= 5 {
                    // Keep the longest token found in the message.
                    if current.len() > best.len() {
                        best = current.clone();
                    }
                }
                current.clear();
            }
        }

        if current.len() >= 5 && current.len() > best.len() {
            best = current;
        }

        if best.is_empty() {
            None
        } else {
            Some(best)
        }
    }

    async fn infer_sender_from_recent_sent(
        pool: &SqlitePool,
        inbox_sim_id: &str,
        message: &str,
        _timestamp: NaiveDateTime,
    ) -> Result<Option<String>> {
        let matched = sqlx::query_scalar::<_, String>(
            r#"
            SELECT
                CASE
                    WHEN sc.phone_number IS NULL OR TRIM(sc.phone_number) = '' THEN ''
                    WHEN sc.phone_number LIKE '+%' THEN sc.phone_number
                    ELSE '+' || sc.phone_number
                END AS inferred_sender
            FROM sms s
            JOIN sim_cards recv ON recv.id = ?
            LEFT JOIN sim_cards sc ON sc.id = s.sim_id
            WHERE s.send = 1
              AND TRIM(s.message) = TRIM(?)
              AND REPLACE(s.contact_id, '+', '') = REPLACE(recv.phone_number, '+', '')
              AND sc.phone_number IS NOT NULL
              AND TRIM(sc.phone_number) != ''
            ORDER BY s.id DESC
            LIMIT 1
            "#,
        )
        .bind(inbox_sim_id)
        .bind(message)
        .fetch_optional(pool)
        .await?;

        Ok(matched)
    }

    pub async fn count() -> Result<i64> {
        let pool = get_pool()?;
        let count = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM sms
            "#,
        )
        .fetch_one(pool)
        .await?;
        Ok(count)
    }

    #[allow(dead_code)]
    pub async fn count_by_contact_id(contact_id: &str) -> Result<i64> {
        let pool = get_pool()?;
        let count = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM sms WHERE contact_id = ?
            "#,
        )
        .bind(contact_id)
        .fetch_one(pool)
        .await?;
        Ok(count)
    }

    pub async fn count_by_sim_id() -> Result<Vec<SimSmsStat>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, SimSmsStat>(
            r#"
            SELECT
                s.sim_id,
                SUM(CASE WHEN s.send = 0 THEN 1 ELSE 0 END) AS recv,
                SUM(CASE WHEN s.send = 1 THEN 1 ELSE 0 END) AS sent,
                sc.phone_number
            FROM sms s
            LEFT JOIN sim_cards sc ON s.sim_id = sc.id
            GROUP BY s.sim_id
            "#,
        )
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Retrieves paginated SMS records
    pub async fn paginate(page: u32, per_page: u32) -> Result<(Vec<Self>, i64)> {
        if page == 0 {
            return Err(anyhow::anyhow!("Page number must be greater than 0"));
        }
        let offset = (page - 1) * per_page;
        let pool = get_pool()?;

        let sms_list = sqlx::query_as(
            r#"
            SELECT id, contact_id, timestamp, message, sim_id, send, status
            FROM sms 
            ORDER BY timestamp DESC
            LIMIT ? OFFSET ?
            "#,
        )
        .bind(per_page as i32)
        .bind(offset as i32)
        .fetch_all(pool)
        .await?;

        let total = Sms::count().await?;

        Ok((sms_list, total))
    }
    pub async fn paginate_by_contact_id(
        contact_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<Self>, i64)> {
        if page == 0 {
            return Err(anyhow::anyhow!("Page number must be greater than 0"));
        }
        let offset = (page - 1) * per_page;
        let pool = get_pool()?;
        let normalized_target: String = contact_id.chars().filter(|c| c.is_ascii_digit()).collect();

        let mut tx = pool.begin().await?;

        let mut sms_list: Vec<Self> = sqlx::query_as(
            r#"
            SELECT id, contact_id, timestamp, message, sim_id, send, status
            FROM sms 
            WHERE contact_id = ?
            ORDER BY timestamp DESC, id DESC
            "#,
        )
        .bind(contact_id)
        .fetch_all(&mut *tx)
        .await?;

        if !normalized_target.is_empty() {
            let mut inferred_rows: Vec<Self> = sqlx::query_as(
                r#"
                SELECT i.id, i.contact_id, i.timestamp, i.message, i.sim_id, i.send, i.status
                FROM sms i
                WHERE i.send = 0
                  AND (i.contact_id IS NULL OR TRIM(i.contact_id) = '')
                  AND EXISTS (
                      SELECT 1
                      FROM sms s
                      JOIN sim_cards recv ON recv.id = i.sim_id
                      JOIN sim_cards sc_send ON sc_send.id = s.sim_id
                      WHERE s.send = 1
                        AND TRIM(s.message) = TRIM(i.message)
                        AND REPLACE(s.contact_id, '+', '') = REPLACE(recv.phone_number, '+', '')
                        AND REPLACE(sc_send.phone_number, '+', '') = ?
                  )
                ORDER BY i.timestamp DESC, i.id DESC
                "#,
            )
            .bind(&normalized_target)
            .fetch_all(&mut *tx)
            .await?;

            sms_list.append(&mut inferred_rows);
        }

        sms_list.sort_by(|a, b| {
            b.timestamp
                .cmp(&a.timestamp)
                .then_with(|| b.id.cmp(&a.id))
        });
        sms_list.dedup_by(|a, b| a.id == b.id);

        let total = sms_list.len() as i64;
        let paged_list: Vec<Self> = sms_list
            .into_iter()
            .skip(offset as usize)
            .take(per_page as usize)
            .collect();

        if page == 1 {
            sqlx::query(
                r#"
                UPDATE sms
                SET status = ?
                WHERE contact_id = ? AND status = ?
                "#,
            )
            .bind(SmsStatus::Read as i32)
            .bind(contact_id)
            .bind(SmsStatus::Unread as i32)
            .execute(&mut *tx)
            .await?;

            if !normalized_target.is_empty() {
                sqlx::query(
                    r#"
                    UPDATE sms
                    SET status = ?
                    WHERE send = 0
                      AND (contact_id IS NULL OR TRIM(contact_id) = '')
                      AND status = ?
                      AND EXISTS (
                          SELECT 1
                          FROM sms s
                          JOIN sim_cards recv ON recv.id = sms.sim_id
                          JOIN sim_cards sc_send ON sc_send.id = s.sim_id
                          WHERE s.send = 1
                            AND TRIM(s.message) = TRIM(sms.message)
                            AND REPLACE(s.contact_id, '+', '') = REPLACE(recv.phone_number, '+', '')
                            AND REPLACE(sc_send.phone_number, '+', '') = ?
                      )
                    "#,
                )
                .bind(SmsStatus::Read as i32)
                .bind(SmsStatus::Unread as i32)
                .bind(&normalized_target)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        Ok((paged_list, total))
    }

    /// Paginated inbox (send=false) or sent (send=true) view, with contact name resolved.
    pub async fn paginate_by_direction(send: bool, page: u32, per_page: u32) -> Result<(Vec<SmsRow>, i64)> {
        if page == 0 {
            return Err(anyhow::anyhow!("Page number must be greater than 0"));
        }
        let offset = (page - 1) * per_page;
        let pool = get_pool()?;

        // Inbox should show the raw sender id; sent keeps resolved contact name.
        let contact_name_expr = Self::contact_name_expr_for_direction(send);

        let query = format!(
            r#"
            SELECT s.id, s.contact_id,
                   {} AS contact_name,
                   s.timestamp, s.message, s.sim_id, s.send, s.status
            FROM sms s
            LEFT JOIN contacts c ON s.contact_id = c.id
            WHERE s.send = ?
            ORDER BY s.timestamp DESC, s.id DESC
            LIMIT ? OFFSET ?
            "#,
            contact_name_expr
        );

        let mut rows = sqlx::query_as::<_, SmsRow>(&query)
        .bind(send)
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(pool)
        .await?;

        // Some legacy inbound rows may have an empty sender id; recover a
        // phone-like token from message text so the inbox does not render blank.
        if !send {
            for row in &mut rows {
                if row.contact_name.trim().is_empty() {
                    let mut inferred_contact_id: Option<String> = None;

                    if let Some(inferred) =
                        Self::infer_sender_from_recent_sent(
                            pool,
                            &row.sim_id,
                            &row.message,
                            row.timestamp,
                        )
                            .await?
                    {
                        row.contact_name = inferred.clone();
                        row.contact_id = inferred.clone();
                        inferred_contact_id = Some(inferred);
                    } else if let Some(inferred) = Self::extract_phone_like_token(&row.message) {
                        row.contact_name = inferred.clone();
                        row.contact_id = inferred.clone();
                        inferred_contact_id = Some(inferred);
                    }

                    if let Some(contact_id) = inferred_contact_id {
                        sqlx::query(
                            r#"
                            UPDATE sms
                            SET contact_id = ?
                            WHERE id = ?
                              AND (contact_id IS NULL OR TRIM(contact_id) = '')
                            "#,
                        )
                        .bind(contact_id)
                        .bind(row.id)
                        .execute(pool)
                        .await?;
                    }
                }
            }
        }

        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sms WHERE send = ?")
            .bind(send)
            .fetch_one(pool)
            .await?;

        Ok((rows, total))
    }

    #[allow(dead_code)]
    pub async fn insert(&self) -> Result<i64> {
        let pool = get_pool()?;
        let sms_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO sms (contact_id, timestamp, message, sim_id, send, status)
            VALUES (?, ?, ?, ?, ?, ?) RETURNING id
            "#,
        )
        .bind(&self.contact_id)
        .bind(self.timestamp)
        .bind(&self.message)
        .bind(&self.sim_id)
        .bind(self.send)
        .bind(self.status as i32)
        .fetch_one(pool)
        .await?;

        Ok(sms_id)
    }
    pub async fn query_unread_by_contact_id(contact_id: &str) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let normalized_target: String = contact_id.chars().filter(|c| c.is_ascii_digit()).collect();
        let mut tx = pool.begin().await?;
        let mut sms_list: Vec<Self> = sqlx::query_as(
            r#"
            SELECT id, contact_id, timestamp, message, sim_id, send, status
            FROM sms 
            WHERE contact_id = ? AND status = ?
            ORDER BY timestamp DESC
            "#,
        )
        .bind(contact_id)
        .bind(SmsStatus::Unread as i32)
        .fetch_all(&mut *tx)
        .await?;

        if !normalized_target.is_empty() {
            let mut inferred_unread: Vec<Self> = sqlx::query_as(
                r#"
                SELECT i.id, i.contact_id, i.timestamp, i.message, i.sim_id, i.send, i.status
                FROM sms i
                WHERE i.send = 0
                  AND (i.contact_id IS NULL OR TRIM(i.contact_id) = '')
                  AND i.status = ?
                  AND EXISTS (
                      SELECT 1
                      FROM sms s
                      JOIN sim_cards recv ON recv.id = i.sim_id
                      JOIN sim_cards sc_send ON sc_send.id = s.sim_id
                      WHERE s.send = 1
                        AND TRIM(s.message) = TRIM(i.message)
                        AND REPLACE(s.contact_id, '+', '') = REPLACE(recv.phone_number, '+', '')
                        AND REPLACE(sc_send.phone_number, '+', '') = ?
                  )
                ORDER BY i.timestamp DESC, i.id DESC
                "#,
            )
            .bind(SmsStatus::Unread as i32)
            .bind(&normalized_target)
            .fetch_all(&mut *tx)
            .await?;
            sms_list.append(&mut inferred_unread);
        }

        sms_list.sort_by(|a, b| {
            b.timestamp
                .cmp(&a.timestamp)
                .then_with(|| b.id.cmp(&a.id))
        });
        sms_list.dedup_by(|a, b| a.id == b.id);

        sqlx::query(
            r#"
                UPDATE sms
                SET status = ?
                WHERE contact_id = ? AND status = ?
                "#,
        )
        .bind(SmsStatus::Read as i32)
        .bind(contact_id)
        .bind(SmsStatus::Unread as i32)
        .execute(&mut *tx)
        .await?;

        if !normalized_target.is_empty() {
            sqlx::query(
                r#"
                    UPDATE sms
                    SET status = ?
                    WHERE send = 0
                      AND (contact_id IS NULL OR TRIM(contact_id) = '')
                      AND status = ?
                      AND EXISTS (
                          SELECT 1
                          FROM sms s
                          JOIN sim_cards recv ON recv.id = sms.sim_id
                          JOIN sim_cards sc_send ON sc_send.id = s.sim_id
                          WHERE s.send = 1
                            AND TRIM(s.message) = TRIM(sms.message)
                            AND REPLACE(s.contact_id, '+', '') = REPLACE(recv.phone_number, '+', '')
                            AND REPLACE(sc_send.phone_number, '+', '') = ?
                      )
                "#,
            )
            .bind(SmsStatus::Read as i32)
            .bind(SmsStatus::Unread as i32)
            .bind(&normalized_target)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(sms_list)
    }

    pub async fn _update_status(&self, status: SmsStatus) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            r#"
            UPDATE sms
            SET status = ?
            WHERE id = ?
            "#,
        )
        .bind(status as i32)
        .bind(self.id)
        .execute(pool)
        .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn update_status_by_id(id: i64, status: SmsStatus) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            r#"
            UPDATE sms
            SET status = ?
            WHERE id = ?
            "#,
        )
        .bind(status as i32)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn find_latest_incoming_by_sim_id(sim_id: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let sms = sqlx::query_as(
            r#"
            SELECT id, contact_id, timestamp, message, sim_id, send, status, 
                   uploaded_to_platform, platform_item_id, platform_uploaded_at, platform_response
            FROM sms
            WHERE sim_id = ? AND send = 0
            ORDER BY timestamp DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(sim_id)
        .fetch_optional(pool)
        .await?;
        Ok(sms)
    }

    /// Find the latest incoming SMS row id by SIM and message body.
    pub async fn find_latest_incoming_id_by_sim_message(
        sim_id: &str,
        message: &str,
    ) -> Result<Option<i64>> {
        let pool = get_pool()?;
        let sms_id = sqlx::query_scalar(
            r#"
            SELECT id
            FROM sms
            WHERE sim_id = ? AND message = ? AND send = 0
            ORDER BY timestamp DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(sim_id)
        .bind(message)
        .fetch_optional(pool)
        .await?;
        Ok(sms_id)
    }

    fn resolve_platform_item_id(
        explicit_item_id: Option<&str>,
        resolved_item_id: Option<String>,
        phone_num: &str,
    ) -> Result<String> {
        if let Some(item_id) = explicit_item_id {
            return Ok(item_id.to_string());
        }

        match resolved_item_id {
            Some(item_id) => Ok(item_id),
            None => Err(anyhow::anyhow!(
                "No platform item found for phone_num: {}",
                phone_num
            )),
        }
    }

    /// Mark SMS records with the latest platform upload attempt result.
    pub async fn mark_platform_attempt_by_phone_message(
        phone_num: &str,
        sim_id: &str,
        messages: &[String],
        explicit_item_id: Option<&str>,
        uploaded_to_platform: bool,
        platform_response: Option<String>,
    ) -> Result<()> {
        let pool = get_pool()?;
        let resolved_item_id = if explicit_item_id.is_some() {
            None
        } else {
            FirefoxPlatformItem::find_latest_item_for_phone(phone_num).await?
        };
        let platform_item_id =
            Self::resolve_platform_item_id(explicit_item_id, resolved_item_id, phone_num)?;

        for message in messages {
            sqlx::query(
                r#"
                UPDATE sms
                SET uploaded_to_platform = ?,
                    platform_item_id = ?,
                    platform_uploaded_at = datetime('now'),
                    platform_response = ?
                WHERE id = (
                    SELECT id
                    FROM sms
                    WHERE sim_id = ?
                      AND message = ?
                      AND send = 0
                      AND (platform_item_id IS NULL OR platform_item_id = ?)
                    ORDER BY timestamp DESC, id DESC
                    LIMIT 1
                )
                "#,
            )
            .bind(uploaded_to_platform)
            .bind(&platform_item_id)
            .bind(&platform_response)
            .bind(sim_id)
            .bind(message)
            .bind(&platform_item_id)
            .execute(pool)
            .await?;
        }

        Ok(())
    }

    /// Mark SMS records as successfully uploaded to platform.
    pub async fn mark_uploaded_by_phone_message(
        phone_num: &str,
        sim_id: &str,
        messages: &[String],
        platform_response: Option<String>,
    ) -> Result<()> {
        Self::mark_platform_attempt_by_phone_message(
            phone_num,
            sim_id,
            messages,
            None,
            true,
            platform_response,
        )
        .await
    }

    /// Find SMS records that were recently inserted for a SIM card
    pub async fn find_recent_received_sms(sim_id: &str, limit: i32) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let sms_records = sqlx::query_as(
            r#"
            SELECT id, contact_id, timestamp, message, sim_id, send, status, 
                   uploaded_to_platform, platform_item_id, platform_uploaded_at, platform_response
            FROM sms
            WHERE sim_id = ? AND send = 0 AND uploaded_to_platform = 0
            ORDER BY timestamp DESC
            LIMIT ?
            "#,
        )
        .bind(sim_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        Ok(sms_records)
    }

    /// Normalize failed SMS attempt rows to the currently active platform item for a phone/SIM.
    ///
    /// This is used to repair historical rows that were previously recorded with stale item IDs.
    pub async fn normalize_failed_item_mapping_for_phone(
        sim_id: &str,
        phone_num: &str,
        active_item_id: &str,
    ) -> Result<u64> {
        let pool = get_pool()?;

        // Look up the item name for the active item. Only remap SMS whose content
        // contains the item name, to avoid assigning a Yahoo message to Instagram, etc.
        let item_name: Option<String> =
            sqlx::query_scalar("SELECT item_name FROM firefox_item_names WHERE item_id = ?")
                .bind(active_item_id)
                .fetch_optional(pool)
                .await?;

        let Some(item_name) = item_name else {
            return Ok(0);
        };

        let result = sqlx::query(
            r#"
            UPDATE sms
            SET platform_item_id = ?
            WHERE sim_id = ?
              AND send = 0
              AND uploaded_to_platform = 0
              AND platform_item_id IS NOT NULL
              AND platform_item_id != ?
              AND timestamp > datetime('now', '-2 days')
              AND LOWER(message) LIKE '%' || LOWER(?) || '%'
            "#,
        )
        .bind(active_item_id)
        .bind(sim_id)
        .bind(active_item_id)
        .bind(item_name)
        .execute(pool)
        .await?;

        if result.rows_affected() > 0 {
            log::info!(
                "[数据修复] 已修正失败短信平台映射: sim_id={}, phone={}, active_item_id={}, affected={}",
                sim_id,
                phone_num,
                active_item_id,
                result.rows_affected()
            );
        }

        Ok(result.rows_affected())
    }
}

impl Contact {
    pub async fn query_all() -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let contacts = sqlx::query_as("SELECT id, name FROM contacts")
            .fetch_all(pool)
            .await?;

        Ok(contacts)
    }
    pub async fn query_by_id(id: &str) -> Result<Self> {
        let pool = get_pool()?;
        let contact = sqlx::query_as("SELECT id, name FROM contacts WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;

        Ok(contact)
    }
    /// Look up a contact id by name (the canonical phone number we store).
    pub async fn find_id_by_name(name: &str) -> Result<Option<String>> {
        let pool = get_pool()?;
        let id = sqlx::query_scalar::<_, String>("SELECT id FROM contacts WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
        Ok(id)
    }
    pub async fn insert(&self) -> Result<()> {
        let pool = get_pool()?;

        sqlx::query(
            r#"
            INSERT INTO contacts (id, name) VALUES (?, ?)
            "#,
        )
        .bind(&self.id)
        .bind(&self.name)
        .execute(pool)
        .await?;

        Ok(())
    }
    pub async fn find_or_create(&mut self) -> Result<()> {
        let pool = get_pool()?;

        let existing_id = sqlx::query_scalar::<_, Option<String>>(
            r#"
            SELECT id FROM contacts WHERE name = ?
            "#,
        )
        .bind(&self.name)
        .fetch_one(pool)
        .await;

        if let Ok(Some(id)) = existing_id {
            self.id = id;
            return Ok(());
        }

        // Always use the contact name (phone number) as the id so the SMS
        // destination is guaranteed to be a routable number, not a UUID.
        sqlx::query(
            r#"
            INSERT INTO contacts (id, name) VALUES (?, ?)
            "#,
        )
        .bind(&self.name)
        .bind(&self.name)
        .execute(pool)
        .await?;
        self.id = self.name.clone();

        Ok(())
    }

    pub async fn delete_contacts_without_messages() -> Result<u64> {
        let pool = get_pool()?;

        let affected_rows = sqlx::query(
            r#"
            DELETE FROM contacts 
            WHERE id NOT IN (SELECT DISTINCT contact_id FROM sms)
            "#,
        )
        .execute(pool)
        .await?;

        Ok(affected_rows.rows_affected())
    }
    pub async fn delete_by_id(id: &str) -> Result<bool> {
        let pool = get_pool()?;

        sqlx::query(
            r#"
            DELETE FROM sms WHERE contact_id = ?
            "#,
        )
        .bind(id)
        .execute(pool)
        .await?;

        let result = sqlx::query(
            r#"
            DELETE FROM contacts WHERE id = ?
            "#,
        )
        .bind(id)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl SimCard {
    /// 1. 根据条件查询
    pub async fn find_by_conditions(
        id: Option<&str>,
        imsi: Option<&str>,
        alias: Option<&str>,
        phone_number: Option<&str>,
    ) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let mut query = String::from(
            "SELECT id, imsi, phone_number, alias, country_code, created_at, updated_at FROM sim_cards WHERE 1=1",
        );
        let mut binds = Vec::new();

        if let Some(id) = id {
            query.push_str(" AND id = ?");
            binds.push(id);
        }
        if let Some(imsi) = imsi {
            query.push_str(" AND imsi = ?");
            binds.push(imsi);
        }
        if let Some(alias) = alias {
            query.push_str(" AND alias = ?");
            binds.push(alias);
        }
        if let Some(phone_number) = phone_number {
            query.push_str(" AND phone_number = ?");
            binds.push(phone_number);
        }

        let mut query_builder = sqlx::query_as::<_, SimCard>(&query);
        for bind in binds {
            query_builder = query_builder.bind(bind);
        }

        Ok(query_builder.fetch_all(pool).await?)
    }

    /// Persist the Firefox country code selected for this SIM.
    pub async fn update_country_code(&mut self, country_code: Option<String>) -> Result<()> {
        let pool = get_pool()?;
        self.country_code = country_code.clone();
        sqlx::query(
            "UPDATE sim_cards SET country_code = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&country_code)
        .bind(&self.id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// 2. 更新手机号
    pub async fn update_phone_number(&mut self, phone_number: Option<String>) -> Result<()> {
        let pool = get_pool()?;
        self.phone_number = phone_number.clone();
        sqlx::query(
            "UPDATE sim_cards SET phone_number = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&phone_number)
        .bind(&self.id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// 3. 更新别名
    pub async fn update_alias(&mut self, alias: Option<String>) -> Result<()> {
        let pool = get_pool()?;
        self.alias = alias.clone();
        sqlx::query("UPDATE sim_cards SET alias = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(&alias)
            .bind(&self.id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// 4. 删除
    pub async fn delete(&self) -> Result<bool> {
        let pool = get_pool()?;
        let result = sqlx::query("DELETE FROM sim_cards WHERE id = ?")
            .bind(&self.id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// 5. 新增 - 批量插入
    pub async fn bulk_insert(sim_cards: &[Self]) -> Result<()> {
        if sim_cards.is_empty() {
            return Ok(());
        }

        let pool = get_pool()?;
        let mut transaction = pool.begin().await?;

        for chunk in sim_cards.chunks(MAX_BATCH_SIZE) {
            let mut query_builder = QueryBuilder::new(
                "INSERT INTO sim_cards (id, imsi, phone_number, alias, created_at, updated_at) ",
            );
            query_builder.push_values(chunk, |mut b, sim_card| {
                b.push_bind(&sim_card.id)
                    .push_bind(&sim_card.imsi)
                    .push_bind(&sim_card.phone_number)
                    .push_bind(&sim_card.alias)
                    .push_bind(sim_card.created_at)
                    .push_bind(sim_card.updated_at);
            });
            query_builder.build().execute(&mut *transaction).await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    /// 5. 新增 - 单条插入（调用批量插入）
    pub async fn insert(&self) -> Result<()> {
        Self::bulk_insert(std::slice::from_ref(self)).await
    }

    /// 6. 查询全部
    pub async fn query_all() -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let sim_cards = sqlx::query_as::<_, SimCard>(
            "SELECT id, imsi, phone_number, alias, country_code, created_at, updated_at FROM sim_cards ORDER BY created_at"
        )
        .fetch_all(pool)
        .await?;
        Ok(sim_cards)
    }

    /// 工具方法：获取有效别名（非数据库操作）
    pub fn get_effective_alias(&self) -> String {
        self.alias
            .as_ref()
            .filter(|a| !a.trim().is_empty())
            .cloned()
            .or_else(|| {
                self.phone_number
                    .as_ref()
                    .filter(|p| !p.trim().is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| format!("SIM-{}", &self.id[self.id.len().saturating_sub(4)..]))
    }

    /// 兼容性方法：根据ID批量查询（返回HashMap）
    pub async fn get_by_ids(ids: &[&str]) -> Result<HashMap<String, Self>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let pool = get_pool()?;
        let mut query_builder = QueryBuilder::new(
            "SELECT id, imsi, phone_number, alias, country_code, created_at, updated_at FROM sim_cards WHERE id IN ("
        );
        let mut separated = query_builder.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");

        let sim_cards: Vec<SimCard> = query_builder.build_query_as().fetch_all(pool).await?;
        Ok(sim_cards.into_iter().map(|s| (s.id.clone(), s)).collect())
    }

    /// 兼容性方法：查找或创建（带手机号）
    pub async fn find_or_create_with_phone(
        id: &str,
        imsi: Option<String>,
        phone_number: Option<String>,
    ) -> Result<Self> {
        let existing = Self::find_by_conditions(Some(id), None, None, None).await?;

        if let Some(mut sim_card) = existing.into_iter().next() {
            // Update IMSI if changed. Never touch phone_number for existing records —
            // it is set only on first SIM creation (below) or via explicit API calls.
            // This prevents modem NV memory from overwriting a user-cleared value.
            let imsi_changed = sim_card.imsi != imsi;
            if imsi_changed {
                sim_card.imsi = imsi.clone();
                let pool = get_pool()?;
                sqlx::query(
                    "UPDATE sim_cards SET imsi = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
                )
                .bind(&sim_card.imsi)
                .bind(id)
                .execute(pool)
                .await?;
            }
            return Ok(sim_card);
        }

        // Only create new SIM card with baseband phone_number on first initialization
        let now = chrono::Utc::now().naive_utc();
        let sim_card = SimCard {
            id: id.to_string(),
            imsi,
            phone_number,
            alias: None,
            country_code: None,
            created_at: now,
            updated_at: now,
        };
        sim_card.insert().await?;
        Ok(sim_card)
    }
}

impl Conversation {
    pub async fn query_all() -> Result<Vec<Self>> {
        let pool = get_pool()?;

        let conversations = sqlx::query_as(
              "SELECT id, name, timestamp, message, status, sim_id FROM v_contacts_with_sim ORDER BY timestamp DESC"
        )
        .fetch_all(pool)
        .await?;

        Ok(conversations)
    }

    pub async fn _query_unread() -> Result<Vec<Self>> {
        let pool = get_pool()?;

        let conversations = sqlx::query_as(
              "SELECT id, name, timestamp, message, status, sim_id FROM v_contacts_with_sim where status = ? ORDER BY timestamp DESC"
        )
        .bind(SmsStatus::Unread as i32)
        .fetch_all(pool)
        .await?;

        Ok(conversations)
    }
    pub async fn query_by_contact_ids(contact_ids: &[String]) -> Result<Vec<Self>> {
        let pool = get_pool()?;

        if contact_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut query_builder = QueryBuilder::new(
            "SELECT id, name, timestamp, message, status, sim_id FROM v_contacts_with_sim WHERE id IN (",
        );

        let mut separated = query_builder.separated(", ");
        for id in contact_ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(") ORDER BY timestamp DESC");

        let conversations = query_builder.build_query_as().fetch_all(pool).await?;

        Ok(conversations)
    }

    pub async fn _mark_as_read(&self) -> Result<()> {
        let pool = get_pool()?;

        sqlx::query(
            r#"
            UPDATE sms 
            SET status = ? 
            WHERE contact_id = ? 
            AND timestamp = (
                SELECT timestamp 
                FROM sms 
                WHERE contact_id = ? 
                ORDER BY timestamp DESC 
                LIMIT 1
            )"#,
        )
        .bind(SmsStatus::Read as i32)
        .bind(&self.contact.id)
        .bind(&self.contact.id)
        .execute(pool)
        .await?;

        Ok(())
    }
}

impl ModemSMS {
    pub async fn get_contact_id<'a>(
        &self,
        transaction: &'a mut Transaction<'_, Sqlite>,
    ) -> Result<String> {
        let contact_id = sqlx::query_scalar::<_, String>(
            r#"
            SELECT id FROM contacts WHERE name = ?
            "#,
        )
        .bind(&self.contact)
        .fetch_optional(&mut **transaction)
        .await?;

        if let Some(contact_id) = contact_id {
            Ok(contact_id)
        } else if self.contact.is_empty() {
            // Empty sender — carrier noise (RP-ACK, etc.). Don't create a
            // spurious contact with empty id/name.
            Ok(String::new())
        } else {
            // Use the phone number as both id and name so the SMS destination
            // is the actual E.164 number, not a UUID.
            sqlx::query(
                r#"
                INSERT INTO contacts (id, name) VALUES (?, ?)
                "#,
            )
            .bind(&self.contact)
            .bind(&self.contact)
            .execute(&mut **transaction)
            .await?;

            Ok(self.contact.clone())
        }
    }

    pub async fn insert(&self) -> Result<i64> {
        let pool = get_pool()?;

        let mut transaction = pool.begin().await?;
        let sms_id = self.insert_transaction(&mut transaction).await?;
        transaction.commit().await?;
        Ok(sms_id)
    }

    pub async fn insert_transaction(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<i64> {
        let contact_id = self.get_contact_id(transaction).await?;

        //When send is true, status defaults to Read
        let sms_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO sms (contact_id, timestamp, message, sim_id, send, status)
            VALUES (?, ?, ?, ?, ?, ?) RETURNING id
            "#,
        )
        .bind(contact_id)
        .bind(self.timestamp)
        .bind(&self.message)
        .bind(&self.sim_id)
        .bind(self.send)
        .bind(if self.send {
            SmsStatus::Read as i32
        } else {
            SmsStatus::Unread as i32
        })
        .fetch_one(&mut **transaction)
        .await?;

        Ok(sms_id)
    }

    /// Check if an SMS with the same non-empty body and sim_id was inserted in
    /// the last 5 seconds. Used to deduplicate native MessageReceived vs
    /// dialplan UserEvent SmsReceived which both fire for the same SIP MESSAGE.
    pub async fn exists_recent(body: &str, sim_id: &str) -> Result<bool> {
        let pool = get_pool()?;
        let exists = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*) FROM sms
            WHERE message = ? AND sim_id = ? AND send = 0
              AND timestamp >= datetime('now', '-5 seconds')
            "#,
        )
        .bind(body)
        .bind(sim_id)
        .fetch_one(pool)
        .await?;
        Ok(exists > 0)
    }

    pub async fn bulk_insert(records: &[Self]) -> Result<Vec<String>> {
        let pool = get_pool()?;

        let mut transaction = pool.begin().await?;

        let mut contact_names = HashSet::new();
        for record in records {
            contact_names.insert(record.contact.clone());
        }

        let contact_names = contact_names.iter().cloned().collect::<Vec<String>>();

        // 查询已存在的联系人
        let mut query_builder = QueryBuilder::new("SELECT id, name FROM contacts WHERE name IN (");

        let mut separated = query_builder.separated(", ");
        for contact in contact_names.iter() {
            separated.push_bind(contact);
        }
        separated.push_unseparated(") ");

        let rows = query_builder.build().fetch_all(&mut *transaction).await?;

        let mut contact_map: HashMap<String, String> = rows
            .into_iter()
            .map(|row| {
                let id: String = row.try_get(0).unwrap();
                let name: String = row.try_get(1).unwrap();

                (name, id)
            })
            .collect();

        // 插入新联系人
        for contact_name in contact_names.iter() {
            if !contact_map.contains_key(contact_name) {
                let uuid = Uuid::new_v4().to_string();

                sqlx::query(
                    r#"
                    INSERT INTO contacts (id, name) VALUES (?, ?)
                    "#,
                )
                .bind(&uuid)
                .bind(contact_name)
                .execute(&mut *transaction)
                .await?;

                contact_map.insert(contact_name.clone(), uuid);
            }
        }

        for chunk in records.chunks(MAX_BATCH_SIZE) {
            let mut query_builder = QueryBuilder::new(
                "INSERT OR IGNORE INTO sms (contact_id, timestamp, message, sim_id, send, status) ",
            );

            query_builder.push_values(chunk, |mut b, sms| {
                b.push_bind(contact_map.get(&sms.contact))
                    .push_bind(sms.timestamp)
                    .push_bind(&sms.message)
                    .push_bind(&sms.sim_id)
                    .push_bind(sms.send)
                    .push_bind(if sms.send {
                        SmsStatus::Read as i32
                    } else {
                        SmsStatus::Unread as i32
                    });
            });

            query_builder.build().execute(&mut *transaction).await?;
        }

        transaction.commit().await?;

        Ok(contact_map.into_values().collect())
    }
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct FirefoxBatchUpload {
    pub id: i64,
    pub batch_id: String,
    pub country_id: String,
    pub phone_numbers: String,
    pub created_at: Option<chrono::NaiveDateTime>,
}

impl FirefoxBatchUpload {
    pub async fn insert(batch_id: &str, country_id: &str, phone_numbers: &[String]) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            "INSERT INTO firefox_batch_uploads (batch_id, country_id, phone_numbers) VALUES (?, ?, ?)",
        )
        .bind(batch_id)
        .bind(country_id)
        .bind(phone_numbers.join(","))
        .execute(pool)
        .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn query_by_country(country_id: &str) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let uploads = sqlx::query_as(
            "SELECT id, batch_id, country_id, phone_numbers, created_at FROM firefox_batch_uploads WHERE country_id = ? ORDER BY created_at DESC",
        )
        .bind(country_id)
        .fetch_all(pool)
        .await?;
        Ok(uploads)
    }

    pub async fn query_recent(limit: i64) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let uploads = sqlx::query_as(
            "SELECT id, batch_id, country_id, phone_numbers, created_at FROM firefox_batch_uploads ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(uploads)
    }

    pub async fn query_by_batch_id(batch_id: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let upload = sqlx::query_as(
            "SELECT id, batch_id, country_id, phone_numbers, created_at FROM firefox_batch_uploads WHERE batch_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(batch_id)
        .fetch_optional(pool)
        .await?;
        Ok(upload)
    }

    #[allow(dead_code)]
    pub async fn exists() -> Result<bool> {
        let pool = get_pool()?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM firefox_batch_uploads")
            .fetch_one(pool)
            .await?;
        Ok(count > 0)
    }

    pub async fn delete_by_id(id: i64) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query("DELETE FROM firefox_batch_uploads WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn delete_all() -> Result<()> {
        let pool = get_pool()?;
        sqlx::query("DELETE FROM firefox_batch_uploads")
            .execute(pool)
            .await?;
        Ok(())
    }
}

impl Sms {
    /// Mark an SMS with the latest platform upload attempt result.
    pub async fn mark_platform_attempt(
        id: i64,
        item_id: &str,
        uploaded_to_platform: bool,
        response: Option<&str>,
    ) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            "UPDATE sms SET uploaded_to_platform = ?, platform_item_id = ?, platform_uploaded_at = datetime('now'), platform_response = ? WHERE id = ?",
        )
        .bind(uploaded_to_platform)
        .bind(item_id)
        .bind(response)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Mark an SMS as uploaded to the platform.
    pub async fn mark_uploaded(id: i64, item_id: &str, response: Option<&str>) -> Result<()> {
        Self::mark_platform_attempt(id, item_id, true, response).await
    }

    /// Query SMS attempts for a given platform item.
    pub async fn query_by_platform_item(item_id: &str) -> Result<Vec<Self>> {
        Self::query_by_platform_item_and_sim(item_id, None).await
    }

    /// Query SMS attempts for a given platform item and optional SIM context.
    pub async fn query_by_platform_item_and_sim(
        item_id: &str,
        sim_id: Option<&str>,
    ) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let sms_list = if let Some(sim_id) = sim_id {
            sqlx::query_as(
                "SELECT id, contact_id, timestamp, message, sim_id, send, status, \
                 uploaded_to_platform, platform_item_id, platform_uploaded_at, platform_response \
                 FROM sms WHERE platform_item_id = ? AND sim_id = ? ORDER BY platform_uploaded_at DESC, id DESC",
            )
            .bind(item_id)
            .bind(sim_id)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as(
                "SELECT id, contact_id, timestamp, message, sim_id, send, status, \
                 uploaded_to_platform, platform_item_id, platform_uploaded_at, platform_response \
                 FROM sms WHERE platform_item_id = ? ORDER BY platform_uploaded_at DESC, id DESC",
            )
            .bind(item_id)
            .fetch_all(pool)
            .await?
        };
        Ok(sms_list)
    }

    /// Find an incoming SMS by SIM ID and message content (latest match).
    pub async fn find_by_sim_and_message(sim_id: &str, message: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let sms = sqlx::query_as(
            "SELECT id, contact_id, timestamp, message, sim_id, send, status, \
             uploaded_to_platform, platform_item_id, platform_uploaded_at, platform_response \
             FROM sms WHERE sim_id = ? AND message = ? AND send = 0 \
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(sim_id)
        .bind(message)
        .fetch_optional(pool)
        .await?;
        Ok(sms)
    }
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct FirefoxPlatformItem {
    pub id: i64,
    pub item_id: String,
    pub item_name: Option<String>,
    pub country_id: String,
    pub phone_num: String,
    pub iccid: Option<String>,
    pub sim_id: Option<String>,
    pub status: String,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
}

impl FirefoxPlatformItem {
    #[allow(dead_code)]
    pub async fn upsert(item_id: &str, country_id: &str, phone_num: &str) -> Result<()> {
        let pool = get_pool()?;
        // sim_cards.id is the ICCID
        let sim_info: Option<(String,)> =
            sqlx::query_as("SELECT id FROM sim_cards WHERE phone_number = ? LIMIT 1")
                .bind(phone_num)
                .fetch_optional(pool)
                .await?;

        let (sim_id, iccid) = match sim_info {
            Some((id,)) => (Some(id.clone()), Some(id)),
            None => (None, None),
        };

        sqlx::query(
            "INSERT INTO firefox_platform_items (item_id, country_id, phone_num, iccid, sim_id) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(item_id, country_id, phone_num) DO UPDATE SET \
             iccid = excluded.iccid, \
             sim_id = excluded.sim_id, \
             updated_at = datetime('now')",
        )
        .bind(item_id)
        .bind(country_id)
        .bind(phone_num)
        .bind(iccid)
        .bind(sim_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn query_all() -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            "SELECT p.id, p.item_id, n.item_name AS item_name, p.country_id, p.phone_num, p.iccid, p.sim_id, p.status, p.created_at, p.updated_at \
             FROM firefox_platform_items p
             LEFT JOIN firefox_item_names n ON p.item_id = n.item_id
             ORDER BY p.updated_at DESC",
        )
        .fetch_all(pool)
        .await?;
        Ok(items)
    }

    pub async fn query_by_item_id(item_id: &str) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            "SELECT id, p.item_id, n.item_name AS item_name, p.country_id, p.phone_num, p.iccid, p.sim_id, p.status, p.created_at, p.updated_at \
             FROM firefox_platform_items p \
             LEFT JOIN firefox_item_names n ON p.item_id = n.item_id \
             WHERE p.item_id = ? ORDER BY p.phone_num",
        )
        .bind(item_id)
        .fetch_all(pool)
        .await?;
        Ok(items)
    }

    /// Find the latest item_id for a given phone number.
    #[allow(dead_code)]
    pub async fn find_latest_item_for_phone(phone_num: &str) -> Result<Option<String>> {
        let pool = get_pool()?;
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT item_id FROM firefox_platform_items \
             WHERE phone_num = ? \
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(phone_num)
        .fetch_optional(pool)
        .await?;
        Ok(result.map(|r| r.0))
    }

    pub async fn find_latest_country_for_phone(phone_num: &str) -> Result<Option<String>> {
        let pool = get_pool()?;
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT country_id FROM firefox_platform_items \
             WHERE phone_num = ? AND TRIM(country_id) <> '' \
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(phone_num)
        .fetch_optional(pool)
        .await?;
        Ok(result.map(|r| r.0))
    }

    pub async fn query_statistics() -> Result<Vec<PlatformItemStat>> {
        let pool = get_pool()?;
        let stats = sqlx::query_as::<_, PlatformItemStat>(
            "SELECT \
                s.platform_item_id AS item_id, \
                MAX(n.item_name) AS item_name, \
               COALESCE(MAX(i.country_id), MAX(sc.country_code), '') AS country_id, \
               COALESCE(MAX(i.phone_num), MAX(sc.phone_number), '') AS phone_num, \
                s.sim_id AS iccid, \
                COUNT(*) AS total_sms, \
                SUM(CASE WHEN s.uploaded_to_platform THEN 1 ELSE 0 END) AS uploaded_sms, \
                SUM(CASE WHEN s.uploaded_to_platform THEN 0 ELSE 1 END) AS failed_sms \
             FROM sms s \
             LEFT JOIN firefox_platform_items i \
                 ON s.platform_item_id = i.item_id AND s.sim_id = i.iccid \
             LEFT JOIN sim_cards sc \
                 ON s.sim_id = sc.id \
             LEFT JOIN firefox_item_names n \
                 ON s.platform_item_id = n.item_id \
             WHERE s.platform_item_id IS NOT NULL \
             GROUP BY s.platform_item_id, s.sim_id \
             ORDER BY total_sms DESC",
        )
        .fetch_all(pool)
        .await?;
        Ok(stats)
    }
}

/// Build the SMS content to upload to the platform.
/// If the raw message does not already contain the item name keyword,
/// prefix it with `[Item_Name] ` so the platform accepts it.
pub async fn build_upload_sms_content(item_id: &str, raw_content: &str) -> Result<String> {
    let pool = get_pool()?;
    let item_name: Option<String> =
        sqlx::query_scalar("SELECT item_name FROM firefox_item_names WHERE item_id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;

    let Some(item_name) = item_name else {
        return Ok(raw_content.to_string());
    };

    let raw_lower = raw_content.to_ascii_lowercase();
    let name_lower = item_name.to_ascii_lowercase();
    if raw_lower.contains(&name_lower) {
        return Ok(raw_content.to_string());
    }

    let prefixed = format!("[{}] {}", item_name, raw_content);
    log::info!(
        "[Upload SMS] Prefixed item name to SMS content: item_id={}, item_name={}, original={:?}, prefixed={:?}",
        item_id, item_name, raw_content, prefixed
    );
    Ok(prefixed)
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct BarcodeScan {
    pub id: i64,
    pub iccid: String,
    pub msisdn: String,
    pub imported: bool,
    pub created_at: Option<NaiveDateTime>,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct BarcodeScanRow {
    pub id: i64,
    pub iccid: String,
    pub msisdn: String,
    pub imported: bool,
    pub created_at: Option<NaiveDateTime>,
    pub phone_number: Option<String>,
}

impl BarcodeScan {
    pub async fn upsert(iccid: &str, msisdn: &str) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            "INSERT INTO barcode_scans (iccid, msisdn) \
             VALUES (?, ?) \
             ON CONFLICT(iccid) DO UPDATE SET \
             msisdn = excluded.msisdn, \
             imported = 0, \
               created_at = datetime('now', 'localtime')",
        )
        .bind(iccid)
        .bind(msisdn)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_unimported() -> Result<Vec<BarcodeScanRow>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, BarcodeScanRow>(
            "SELECT bs.id, bs.iccid, bs.msisdn, bs.imported, bs.created_at, sc.phone_number \
             FROM barcode_scans bs \
             LEFT JOIN sim_cards sc ON bs.iccid = sc.id \
             WHERE bs.imported = 0 \
             ORDER BY bs.created_at DESC",
        )
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn mark_imported(ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let pool = get_pool()?;
        let mut query_builder =
            QueryBuilder::new("UPDATE barcode_scans SET imported = 1 WHERE id IN (");
        let mut separated = query_builder.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");
        query_builder.build().execute(pool).await?;
        Ok(())
    }

    pub async fn delete_all() -> Result<()> {
        let pool = get_pool()?;
        sqlx::query("DELETE FROM barcode_scans")
            .execute(pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct PlatformRejectionReasonStat {
    pub reason: String,
    pub count: i64,
}

impl Sms {
    pub async fn query_platform_rejection_reason_summary(
        limit: i64,
    ) -> Result<Vec<PlatformRejectionReasonStat>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, PlatformRejectionReasonStat>(
            "SELECT \
                CASE \
                    WHEN s.platform_response LIKE '%丢弃纯数字%' THEN '丢弃纯数字' \
                    WHEN s.platform_response LIKE '%未匹配到关键字%' THEN '未匹配到关键字' \
                    WHEN s.platform_response LIKE '%code=0%' THEN 'Platform rejected (other)' \
                    WHEN s.platform_response IS NULL OR TRIM(s.platform_response) = '' THEN 'Unknown failure' \
                    ELSE 'Upload/Network error' \
                END AS reason, \
                COUNT(*) AS count \
             FROM sms s \
             WHERE s.send = 0 \
               AND s.platform_item_id IS NOT NULL \
               AND s.uploaded_to_platform = 0 \
             GROUP BY reason \
             ORDER BY count DESC \
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct PlatformItemStat {
    pub item_id: String,
    pub item_name: Option<String>,
    pub country_id: String,
    pub phone_num: String,
    pub iccid: String,
    pub total_sms: i64,
    pub uploaded_sms: i64,
    pub failed_sms: i64,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct FirefoxMoneyStat {
    pub sim_id: String,
    pub phone_number: Option<String>,
    pub received_sms_count: i64,
    pub successful_uploaded_sms_count: i64,
    pub failed_sms_count: i64,
    pub money_earning: f64,
    pub earning_item_names: Option<String>,
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct MoneyItemOption {
    pub item_id: String,
    pub item_name: String,
    pub seller_item_price: f64,
}

/// One platform ("大厅") price row for an item, scoped to a country.
#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct PlatformItemPrice {
    pub country_id: Option<String>,
    pub country_title: Option<String>,
    pub item_uprice: f64,
}

/// One received SMS row for the Money page's per-SIM detail panel.
#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct MoneySmsDetailRow {
    pub id: i64,
    pub timestamp: NaiveDateTime,
    pub phone_number: String,
    pub message: String,
    pub success: bool,
    pub platform_response: Option<String>,
    pub money: f64,
}

impl FirefoxMoneyStat {
    pub async fn query_all_by_sim() -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, FirefoxMoneyStat>(
            r#"
            SELECT
                s.sim_id AS sim_id,
                MAX(sc.phone_number) AS phone_number,
                SUM(CASE WHEN s.send = 0 THEN 1 ELSE 0 END) AS received_sms_count,
                SUM(CASE WHEN s.send = 0 AND s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1 THEN 1 ELSE 0 END) AS successful_uploaded_sms_count,
                SUM(CASE WHEN s.send = 0 AND s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 0 THEN 1 ELSE 0 END) AS failed_sms_count,
                COALESCE(
                    SUM(
                        CASE
                            WHEN s.send = 0 AND s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1
                                THEN COALESCE(fin.seller_item_price, 0.0)
                            ELSE 0.0
                        END
                    ),
                    0.0
                ) AS money_earning,
                GROUP_CONCAT(
                    DISTINCT CASE
                        WHEN s.send = 0 AND s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1
                            THEN COALESCE(fin.item_name, s.platform_item_id)
                    END
                ) AS earning_item_names
            FROM sms s
            LEFT JOIN sim_cards sc
                ON sc.id = s.sim_id
            LEFT JOIN firefox_item_names fin
                ON fin.item_id = s.platform_item_id
            GROUP BY s.sim_id
            "#,
        )
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn query_money_item_options(
        keyword: Option<&str>,
        limit: i64,
    ) -> Result<Vec<MoneyItemOption>> {
        let pool = get_pool()?;
        let keyword = keyword
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(|k| format!("%{}%", k));

        let rows = sqlx::query_as::<_, MoneyItemOption>(
            r#"
            WITH ids AS (
                SELECT DISTINCT item_id FROM firefox_platform_items
                UNION
                SELECT DISTINCT item_id FROM firefox_item_prices
                UNION
                SELECT DISTINCT item_id FROM firefox_item_names
                UNION
                SELECT DISTINCT platform_item_id AS item_id
                FROM sms
                WHERE platform_item_id IS NOT NULL AND TRIM(platform_item_id) <> ''
            )
            SELECT
                i.item_id AS item_id,
                COALESCE(fin.item_name, i.item_id) AS item_name,
                COALESCE(fin.seller_item_price, 0.0) AS seller_item_price
            FROM ids i
            LEFT JOIN firefox_item_names fin ON fin.item_id = i.item_id
            WHERE (
                ?1 IS NULL
                OR i.item_id LIKE ?1
                OR COALESCE(fin.item_name, '') LIKE ?1
            )
            ORDER BY i.item_id
            LIMIT ?2
            "#,
        )
        .bind(keyword)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        Ok(rows)
    }

    /// Returns the platform ("大厅") reference price for an item, broken down by country.
    pub async fn query_platform_item_prices(item_id: &str) -> Result<Vec<PlatformItemPrice>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, PlatformItemPrice>(
            "SELECT country_id, country_title, item_uprice FROM firefox_item_prices WHERE item_id = ? ORDER BY country_id",
        )
        .bind(item_id)
        .fetch_all(pool)
        .await?;

        Ok(rows)
    }

    /// Returns recent received SMS for a SIM, with per-message upload status and earning, for the Money page detail panel.
    pub async fn query_sms_detail_by_sim(sim_id: &str, limit: i64) -> Result<Vec<MoneySmsDetailRow>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, MoneySmsDetailRow>(
            r#"
            SELECT
                s.id AS id,
                s.timestamp AS timestamp,
                COALESCE(sc.phone_number, '') AS phone_number,
                s.message AS message,
                (s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1) AS success,
                s.platform_response AS platform_response,
                CASE
                    WHEN s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1
                        THEN COALESCE(fin.seller_item_price, 0.0)
                    ELSE 0.0
                END AS money
            FROM sms s
            LEFT JOIN sim_cards sc ON sc.id = s.sim_id
            LEFT JOIN firefox_item_names fin ON fin.item_id = s.platform_item_id
            WHERE s.sim_id = ? AND s.send = 0
            ORDER BY s.timestamp DESC, s.id DESC
            LIMIT ?
            "#,
        )
        .bind(sim_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        Ok(rows)
    }

    /// Returns recent SMS attempts for a platform item (across all SIMs), for the Money page detail panel.
    pub async fn query_sms_detail_by_item(item_id: &str, limit: i64) -> Result<Vec<MoneySmsDetailRow>> {
        let pool = get_pool()?;
        let rows = sqlx::query_as::<_, MoneySmsDetailRow>(
            r#"
            SELECT
                s.id AS id,
                s.timestamp AS timestamp,
                COALESCE(sc.phone_number, '') AS phone_number,
                s.message AS message,
                (s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1) AS success,
                s.platform_response AS platform_response,
                CASE
                    WHEN s.platform_item_id IS NOT NULL AND s.uploaded_to_platform = 1
                        THEN COALESCE(fin.seller_item_price, 0.0)
                    ELSE 0.0
                END AS money
            FROM sms s
            LEFT JOIN sim_cards sc ON sc.id = s.sim_id
            LEFT JOIN firefox_item_names fin ON fin.item_id = s.platform_item_id
            WHERE s.platform_item_id = ? AND s.send = 0
            ORDER BY s.timestamp DESC, s.id DESC
            LIMIT ?
            "#,
        )
        .bind(item_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        Ok(rows)
    }

    /// Updates the seller's own price for an existing item, or inserts a new item row if none exists yet.
    pub async fn update_money_item_price(item_id: &str, seller_item_price: f64) -> Result<()> {
        let pool = get_pool()?;
        let updated = sqlx::query(
            "UPDATE firefox_item_names SET seller_item_price = ? WHERE item_id = ?",
        )
        .bind(seller_item_price)
        .bind(item_id)
        .execute(pool)
        .await?;

        if updated.rows_affected() == 0 {
            sqlx::query(
                "INSERT INTO firefox_item_names (item_id, item_name, seller_item_price) VALUES (?, ?, ?)",
            )
            .bind(item_id)
            .bind(item_id)
            .bind(seller_item_price)
            .execute(pool)
            .await?;
        }

        Ok(())
    }

    pub async fn query_successful_uploaded_count_for_item(item_id: &str) -> Result<i64> {
        let pool = get_pool()?;
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM sms
            WHERE send = 0
              AND platform_item_id = ?
              AND uploaded_to_platform = 1
            "#,
        )
        .bind(item_id)
        .fetch_one(pool)
        .await?;

        Ok(count)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FirefoxItemPriceUpsert {
    pub item_id: String,
    pub country_id: Option<String>,
    pub item_name: String,
    pub item_uprice: f64,
    pub country_title: Option<String>,
}

pub async fn upsert_firefox_item_prices(items: &[FirefoxItemPriceUpsert]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let pool = get_pool()?;
    let mut tx = pool.begin().await?;

    for item in items {
        sqlx::query(
            r#"
            INSERT INTO firefox_item_prices (item_id, country_id, item_name, item_uprice, country_title, updated_at)
            VALUES (?, ?, ?, ?, ?, datetime('now'))
            ON CONFLICT(item_id, country_id)
            DO UPDATE SET
                item_name = excluded.item_name,
                item_uprice = excluded.item_uprice,
                country_title = excluded.country_title,
                updated_at = datetime('now')
            "#,
        )
        .bind(&item.item_id)
        .bind(item.country_id.as_deref())
        .bind(&item.item_name)
        .bind(item.item_uprice)
        .bind(item.country_title.as_deref())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO firefox_item_names (item_id, item_name)
            VALUES (?, ?)
            ON CONFLICT(item_id)
            DO UPDATE SET item_name = excluded.item_name
            "#,
        )
        .bind(&item.item_id)
        .bind(&item.item_name)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

#[derive(Debug, FromRow, Deserialize, Serialize, Default, Clone)]
pub struct AppSetting {
    pub key: String,
    pub value: Option<String>,
}

impl AppSetting {
    pub async fn get(key: &str) -> Result<Option<String>> {
        let pool = get_pool()?;
        let value: Option<(String,)> =
            sqlx::query_as("SELECT value FROM app_settings WHERE key = ?")
                .bind(key)
                .fetch_optional(pool)
                .await?;
        Ok(value.map(|v| v.0))
    }

    pub async fn set(key: &str, value: Option<&str>) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            r#"
            INSERT INTO app_settings (key, value) VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
        Ok(())
    }
}


/// Represents a queued/sent MMS message
#[derive(Debug, FromRow, Deserialize, Serialize, Clone)]
pub struct MmsMessage {
    pub id: String,
    pub sim_id: String,
    pub to_number: String,
    pub subject: Option<String>,
    /// queued | sending | sent | failed | timeout
    pub status: String,
    pub quectel_err_code: Option<i32>,
    pub http_response_code: Option<i32>,
    pub error_message: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl MmsMessage {
    /// Enqueue a new MMS send job with status "queued". Returns the new job id.
    pub async fn insert_queued(
        sim_id: &str,
        to_number: &str,
        subject: Option<&str>,
    ) -> Result<String> {
        let pool = get_pool()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().naive_utc();
        sqlx::query(
            r#"INSERT INTO mms_messages (id, sim_id, to_number, subject, status, created_at, updated_at)
               VALUES (?, ?, ?, ?, 'queued', ?, ?)"#,
        )
        .bind(&id)
        .bind(sim_id)
        .bind(to_number)
        .bind(subject)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(id)
    }

    /// Fetch up to `limit` jobs that are still queued, oldest first.
    pub async fn get_queued(limit: i64) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            r#"SELECT id, sim_id, to_number, subject, status, quectel_err_code, http_response_code,
                      error_message, created_at, updated_at
               FROM mms_messages WHERE status = 'queued' ORDER BY created_at ASC LIMIT ?"#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(items)
    }

    pub async fn mark_sending(id: &str) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query("UPDATE mms_messages SET status = 'sending', updated_at = ? WHERE id = ?")
            .bind(chrono::Utc::now().naive_utc())
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Record the final outcome of a send attempt.
    pub async fn mark_result(
        id: &str,
        status: &str,
        quectel_err_code: Option<i32>,
        http_response_code: Option<i32>,
        error_message: Option<&str>,
    ) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            r#"UPDATE mms_messages
               SET status = ?, quectel_err_code = ?, http_response_code = ?, error_message = ?, updated_at = ?
               WHERE id = ?"#,
        )
        .bind(status)
        .bind(quectel_err_code)
        .bind(http_response_code)
        .bind(error_message)
        .bind(chrono::Utc::now().naive_utc())
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(id: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let item = sqlx::query_as(
            r#"SELECT id, sim_id, to_number, subject, status, quectel_err_code, http_response_code,
                      error_message, created_at, updated_at
               FROM mms_messages WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(item)
    }

    pub async fn query_paginated(limit: i64, offset: i64) -> Result<(Vec<Self>, i64)> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            r#"SELECT id, sim_id, to_number, subject, status, quectel_err_code, http_response_code,
                      error_message, created_at, updated_at
               FROM mms_messages ORDER BY created_at DESC LIMIT ? OFFSET ?"#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mms_messages")
            .fetch_one(pool)
            .await?;
        Ok((items, total))
    }
}

/// Attachment metadata (no blob data) — safe to serialize directly in API responses.
#[derive(Debug, FromRow, Deserialize, Serialize, Clone)]
pub struct MmsAttachmentMeta {
    pub id: String,
    pub mms_id: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub size_bytes: i64,
}

/// Namespace for MMS attachment blob storage/retrieval.
pub struct MmsAttachment;

impl MmsAttachment {
    pub async fn insert(
        mms_id: &str,
        filename: &str,
        content_type: Option<&str>,
        data: &[u8],
    ) -> Result<String> {
        let pool = get_pool()?;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO mms_attachments (id, mms_id, filename, content_type, size_bytes, data)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(mms_id)
        .bind(filename)
        .bind(content_type)
        .bind(data.len() as i64)
        .bind(data)
        .execute(pool)
        .await?;
        Ok(id)
    }

    pub async fn list_meta(mms_id: &str) -> Result<Vec<MmsAttachmentMeta>> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            r#"SELECT id, mms_id, filename, content_type, size_bytes
               FROM mms_attachments WHERE mms_id = ?"#,
        )
        .bind(mms_id)
        .fetch_all(pool)
        .await?;
        Ok(items)
    }

    /// Fetch attachment (filename, raw bytes) pairs for a job, in insertion order.
    pub async fn fetch_all_with_data(mms_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let pool = get_pool()?;
        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
            r#"SELECT filename, data FROM mms_attachments WHERE mms_id = ? ORDER BY rowid ASC"#,
        )
        .bind(mms_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Per-SIM MMS profile (APN/MMSC/proxy differ by carrier).
#[derive(Debug, FromRow, Deserialize, Serialize, Clone, Default)]
pub struct MmsProfile {
    pub sim_id: String,
    pub mms_apn: Option<String>,
    pub mms_mmsc: Option<String>,
    pub mms_proxy_host: Option<String>,
    pub mms_proxy_port: Option<i32>,
    pub mms_send_mode: Option<String>,
    #[serde(skip_serializing)]
    pub mms_send_mode_source: Option<String>,
}

impl MmsProfile {
    pub async fn get(sim_id: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<i32>, Option<String>, Option<String>)> = sqlx::query_as(
            r#"SELECT id, mms_apn, mms_mmsc, mms_proxy_host, mms_proxy_port, mms_send_mode, mms_send_mode_source FROM sim_cards WHERE id = ?"#,
        )
        .bind(sim_id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(
            |(sim_id, mms_apn, mms_mmsc, mms_proxy_host, mms_proxy_port, mms_send_mode, mms_send_mode_source)| Self {
                sim_id,
                mms_apn,
                mms_mmsc,
                mms_proxy_host,
                mms_proxy_port,
                mms_send_mode,
                mms_send_mode_source,
            },
        ))
    }

    pub async fn set(
        sim_id: &str,
        mms_apn: Option<&str>,
        mms_mmsc: Option<&str>,
        mms_proxy_host: Option<&str>,
        mms_proxy_port: Option<i32>,
        mms_send_mode: Option<&str>,
        _ignored_apn: Option<&str>,
    ) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
             r#"UPDATE sim_cards SET mms_apn = ?, mms_mmsc = ?, mms_proxy_host = ?, mms_proxy_port = ?,
             mms_send_mode = COALESCE(?, mms_send_mode),
             mms_send_mode_source = CASE WHEN ? IS NULL THEN mms_send_mode_source ELSE 'user' END
               WHERE id = ?"#,
        )
        .bind(mms_apn)
        .bind(mms_mmsc)
        .bind(mms_proxy_host)
        .bind(mms_proxy_port)
        .bind(mms_send_mode)
        .bind(mms_send_mode)
        .bind(sim_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// A detected MMS WAP-push notification (phase 1: detect + store) whose content
/// may since have been fetched and decoded (phase 2, see `mms_retrieve.rs`).
#[derive(Debug, FromRow, Deserialize, Serialize, Clone)]
pub struct MmsInboxNotification {
    pub id: String,
    pub sim_id: String,
    pub sender: String,
    pub transaction_id: String,
    pub content_location: Option<String>,
    pub message_size: Option<i64>,
    pub message_class: Option<String>,
    pub expiry_at: Option<NaiveDateTime>,
    /// notified | fetching | fetched | failed | expired
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub next_retry_at: Option<NaiveDateTime>,
    /// Populated once the content has been fetched and decoded (status = "fetched").
    pub subject: Option<String>,
    pub from_address: Option<String>,
    pub fetched_at: Option<NaiveDateTime>,
    pub report_recipient: Option<String>,
    pub report_status: Option<i32>,
    #[serde(skip_serializing)]
    // Kept for re-processing if the decoder in mms_wap.rs/mms_retrieve.rs is
    // fixed later; not useful to API consumers.
    #[allow(dead_code)]
    pub notification_raw: Vec<u8>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl MmsInboxNotification {
    /// Insert a newly-seen notification, or refresh an existing one if the carrier
    /// re-sent the same (sim_id, transaction_id) — common when the network retries
    /// delivery before we've fetched the content.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_or_update(
        sim_id: &str,
        transaction_id: &str,
        sender: &str,
        content_location: Option<&str>,
        message_size: Option<i64>,
        expiry_at: Option<NaiveDateTime>,
        message_class: Option<&str>,
        notification_raw: &[u8],
        received_at: NaiveDateTime,
    ) -> Result<()> {
        let pool = get_pool()?;
        let now = chrono::Utc::now().naive_utc();

        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM mms_inbox WHERE sim_id = ? AND transaction_id = ?")
                .bind(sim_id)
                .bind(transaction_id)
                .fetch_optional(pool)
                .await?;

        if let Some(id) = existing {
            sqlx::query(
                r#"UPDATE mms_inbox
                   SET sender = ?, content_location = ?, message_size = ?, expiry_at = ?,
                       message_class = ?, notification_raw = ?, updated_at = ?
                   WHERE id = ?"#,
            )
            .bind(sender)
            .bind(content_location)
            .bind(message_size)
            .bind(expiry_at)
            .bind(message_class)
            .bind(notification_raw)
            .bind(now)
            .bind(&id)
            .execute(pool)
            .await?;
        } else {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                r#"INSERT INTO mms_inbox
                   (id, sim_id, sender, transaction_id, content_location, message_size,
                    message_class, expiry_at, status, retry_count, notification_raw, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'notified', 0, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(sim_id)
            .bind(sender)
            .bind(transaction_id)
            .bind(content_location)
            .bind(message_size)
            .bind(message_class)
            .bind(expiry_at)
            .bind(notification_raw)
            .bind(received_at)
            .bind(now)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub async fn insert_delivery_report(
        sim_id: &str,
        message_id: &str,
        sender: &str,
        recipient: Option<&str>,
        delivery_status: Option<u8>,
        received_at: NaiveDateTime,
        notification_raw: &[u8],
    ) -> Result<()> {
        let pool = get_pool()?;
        let now = chrono::Utc::now().naive_utc();
        let message_class = delivery_status.map(|status| format!("delivery-status-0x{status:02X}"));
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM mms_inbox WHERE sim_id = ? AND transaction_id = ?",
        )
        .bind(sim_id)
        .bind(message_id)
        .fetch_optional(pool)
        .await?;

        if let Some(id) = existing {
            sqlx::query(
                r#"UPDATE mms_inbox
                   SET sender = ?, report_recipient = ?, report_status = ?, message_class = ?,
                       status = 'delivery_report',
                       notification_raw = ?, updated_at = ?
                   WHERE id = ?"#,
            )
            .bind(sender)
            .bind(recipient)
            .bind(delivery_status.map(i32::from))
            .bind(message_class.as_deref())
            .bind(notification_raw)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        } else {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                r#"INSERT INTO mms_inbox
                   (id, sim_id, sender, transaction_id, message_class, status, retry_count,
                    report_recipient, report_status, notification_raw, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, 'delivery_report', 0, ?, ?, ?, ?, ?)"#,
            )
            .bind(id)
            .bind(sim_id)
            .bind(sender)
            .bind(message_id)
            .bind(message_class.as_deref())
            .bind(recipient)
            .bind(delivery_status.map(i32::from))
            .bind(notification_raw)
            .bind(received_at)
            .bind(now)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub async fn query_paginated(limit: i64, offset: i64) -> Result<(Vec<Self>, i64)> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            r#"SELECT id, sim_id, sender, transaction_id, content_location, message_size,
                      message_class, expiry_at, status, error_message, retry_count, next_retry_at,
                      subject, from_address, fetched_at, report_recipient, report_status,
                      notification_raw, created_at, updated_at
               FROM mms_inbox ORDER BY created_at DESC LIMIT ? OFFSET ?"#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mms_inbox")
            .fetch_one(pool)
            .await?;
        Ok((items, total))
    }

    pub async fn find_by_id(id: &str) -> Result<Option<Self>> {
        let pool = get_pool()?;
        let item = sqlx::query_as(
            r#"SELECT id, sim_id, sender, transaction_id, content_location, message_size,
                      message_class, expiry_at, status, error_message, retry_count, next_retry_at,
                      subject, from_address, fetched_at, report_recipient, report_status,
                      notification_raw, created_at, updated_at
               FROM mms_inbox WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(item)
    }

    /// Fetch up to `limit` notifications eligible for a content-fetch attempt:
    /// newly notified, or previously failed but due for retry, and not yet past
    /// their expiry (the MMSC will have discarded expired content anyway).
    pub async fn get_fetchable(limit: i64) -> Result<Vec<Self>> {
        let pool = get_pool()?;
        let now = chrono::Utc::now().naive_utc();
        let items = sqlx::query_as(
            r#"SELECT id, sim_id, sender, transaction_id, content_location, message_size,
                      message_class, expiry_at, status, error_message, retry_count, next_retry_at,
                      subject, from_address, fetched_at, report_recipient, report_status,
                      notification_raw, created_at, updated_at
               FROM mms_inbox
               WHERE status IN ('notified', 'failed')
                 AND content_location IS NOT NULL
                 AND (next_retry_at IS NULL OR next_retry_at <= ?)
                 AND (expiry_at IS NULL OR expiry_at > ?)
               ORDER BY created_at ASC
               LIMIT ?"#,
        )
        .bind(now)
        .bind(now)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(items)
    }

    /// Mark any still-fetchable notifications that are now past their expiry as
    /// "expired" so the worker stops retrying content the MMSC has discarded.
    pub async fn expire_overdue() -> Result<u64> {
        let pool = get_pool()?;
        let now = chrono::Utc::now().naive_utc();
        let result = sqlx::query(
            r#"UPDATE mms_inbox SET status = 'expired', updated_at = ?
               WHERE status IN ('notified', 'failed') AND expiry_at IS NOT NULL AND expiry_at <= ?"#,
        )
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn mark_fetching(id: &str) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query("UPDATE mms_inbox SET status = 'fetching', updated_at = ? WHERE id = ?")
            .bind(chrono::Utc::now().naive_utc())
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn mark_fetched(
        id: &str,
        subject: Option<&str>,
        from_address: Option<&str>,
    ) -> Result<()> {
        let pool = get_pool()?;
        let now = chrono::Utc::now().naive_utc();
        sqlx::query(
            r#"UPDATE mms_inbox
               SET status = 'fetched', subject = ?, from_address = ?, fetched_at = ?,
                   error_message = NULL, next_retry_at = NULL, updated_at = ?
               WHERE id = ?"#,
        )
        .bind(subject)
        .bind(from_address)
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Record a failed fetch attempt. `next_retry_at = None` means retries are
    /// exhausted -- the row stays in "failed" but will no longer be picked up by
    /// `get_fetchable()`.
    pub async fn mark_fetch_failed(
        id: &str,
        error_message: &str,
        next_retry_at: Option<NaiveDateTime>,
    ) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query(
            r#"UPDATE mms_inbox
               SET status = 'failed', error_message = ?, retry_count = retry_count + 1,
                   next_retry_at = ?, updated_at = ?
               WHERE id = ?"#,
        )
        .bind(error_message)
        .bind(next_retry_at)
        .bind(chrono::Utc::now().naive_utc())
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// One decoded part of a fetched MMS (SMIL, text, image, ...). Metadata-only
/// variant (no blob) is used for list views; `fetch_data` retrieves the bytes.
#[derive(Debug, FromRow, Deserialize, Serialize, Clone)]
pub struct MmsInboxPartMeta {
    pub id: String,
    pub inbox_id: String,
    pub content_type: Option<String>,
    pub filename: Option<String>,
    pub size_bytes: i64,
}

/// Namespace for fetched MMS part storage/retrieval.
pub struct MmsInboxPart;

impl MmsInboxPart {
    pub async fn insert(
        inbox_id: &str,
        content_type: &str,
        filename: Option<&str>,
        data: &[u8],
    ) -> Result<String> {
        let pool = get_pool()?;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO mms_inbox_parts (id, inbox_id, content_type, filename, size_bytes, data, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(inbox_id)
        .bind(content_type)
        .bind(filename)
        .bind(data.len() as i64)
        .bind(data)
        .bind(chrono::Utc::now().naive_utc())
        .execute(pool)
        .await?;
        Ok(id)
    }

    /// Replace any previously stored parts for this notification (used when a
    /// retry succeeds after a partial failure, so stale parts don't linger).
    pub async fn delete_all_for_inbox(inbox_id: &str) -> Result<()> {
        let pool = get_pool()?;
        sqlx::query("DELETE FROM mms_inbox_parts WHERE inbox_id = ?")
            .bind(inbox_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn list_meta(inbox_id: &str) -> Result<Vec<MmsInboxPartMeta>> {
        let pool = get_pool()?;
        let items = sqlx::query_as(
            r#"SELECT id, inbox_id, content_type, filename, size_bytes
               FROM mms_inbox_parts WHERE inbox_id = ? ORDER BY rowid ASC"#,
        )
        .bind(inbox_id)
        .fetch_all(pool)
        .await?;
        Ok(items)
    }

    /// Fetch a single part's raw bytes + content type, for a download/view endpoint.
    pub async fn fetch_data(
        part_id: &str,
    ) -> Result<Option<(Option<String>, Option<String>, Vec<u8>)>> {
        let pool = get_pool()?;
        let row: Option<(Option<String>, Option<String>, Vec<u8>)> = sqlx::query_as(
            r#"SELECT content_type, filename, data FROM mms_inbox_parts WHERE id = ?"#,
        )
        .bind(part_id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }
}

/// Initializes SQLite database
/// Append a row to the eSIM operation audit log. Best-effort: callers log but
/// do not fail their operation if this write fails.

/// Initializes SQLite database
pub async fn db_init() -> Result<()> {
    let mut db_dir = PathBuf::from("/var/lib/sms-gateway");
    if std::fs::create_dir_all(&db_dir).is_err() {
        db_dir = PathBuf::from("./data");
        std::fs::create_dir_all(&db_dir)?;
    }

    let db_path = format!("sqlite://{}/data.db", db_dir.display());

    if !sqlx::Sqlite::database_exists(&db_path).await? {
        sqlx::Sqlite::create_database(&db_path).await?;
    };

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_path)
        .await?;

    migrate!("./migrations").run(&pool).await?;

    POOL.set(pool)
        .map_err(|_| anyhow::anyhow!("Failed to initialize database connection pool"))?;

    tokio::spawn(async {
        match Contact::delete_contacts_without_messages().await {
            Ok(count) => {
                log::info!("{} contacts without messages have been cleaned up", count);
            }
            Err(e) => {
                log::error!("Failed to clean up contacts without messages: {}", e);
            }
        }
    });

    Ok(())
}

/// Retrieves the database connection pool
pub fn get_pool() -> Result<&'static SqlitePool> {
    POOL.get()
        .ok_or(anyhow::anyhow!("Database not initialized"))
}
