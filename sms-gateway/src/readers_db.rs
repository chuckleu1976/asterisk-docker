use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::PathBuf;
use std::sync::OnceLock;

static INV_POOL: OnceLock<SqlitePool> = OnceLock::new();

fn inventory_path() -> PathBuf {
    let p = PathBuf::from("../sim_inventory.db");
    if p.exists() {
        return p;
    }
    // fallback: same directory
    PathBuf::from("./sim_inventory.db")
}

async fn inv_pool() -> Result<&'static SqlitePool> {
    if let Some(pool) = INV_POOL.get() {
        return Ok(pool);
    }
    let path = inventory_path();
    let db_url = format!("sqlite://{}", path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await?;
    INV_POOL
        .set(pool)
        .map_err(|_| anyhow::anyhow!("Failed to set sim_inventory pool"))?;
    Ok(INV_POOL.get().unwrap())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Reader {
    pub reader: i32,
    pub name: String,
    pub hostname: String,
    pub imei: String,
    pub status: String,
    pub last_seen: Option<String>,
    pub updated_at: Option<String>,
}

pub async fn query_all_readers() -> Result<Vec<Reader>> {
    let pool = inv_pool().await?;
    let rows = sqlx::query("SELECT reader, name, hostname, imei, status, last_seen, updated_at FROM readers ORDER BY reader")
        .fetch_all(pool)
        .await?;
    let readers = rows
        .iter()
        .map(|r| Reader {
            reader: r.get("reader"),
            name: r.get("name"),
            hostname: r.get("hostname"),
            imei: r.get("imei"),
            status: r.get("status"),
            last_seen: r.get("last_seen"),
            updated_at: r.get("updated_at"),
        })
        .collect();
    Ok(readers)
}

pub async fn update_reader_imei(reader: i32, imei: String) -> Result<()> {
    let pool = inv_pool().await?;
    sqlx::query("UPDATE readers SET imei = ?, updated_at = datetime('now') WHERE reader = ?")
        .bind(&imei)
        .bind(reader)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_reader_module(_reader: i32, _module: String) -> Result<()> {
    // module column does not exist in the readers table; no-op to avoid SQL error
    Ok(())
}
