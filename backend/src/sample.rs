//! Synthetic store sales for the `sales` model and the demo dashboard (`magnum-bi seed-sample`).
//! Generated here rather than in SQL so it's the same on every database: a fixed seed, dates
//! relative to today, a little growth and a weekly rhythm so the charts have something to show.

use anyhow::Result;
use chrono::{Duration, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::db::DbPool;

pub const TABLE: &str = "sample_sales";
const REGIONS: [&str; 3] = ["EMEA", "AMER", "APAC"];
const CATEGORIES: [(&str, f64); 4] =
    [("Apparel", 60.0), ("Footwear", 95.0), ("Accessories", 35.0), ("Outerwear", 160.0)];
const DAYS: i64 = 540;

pub async fn seed(pool: &DbPool, rows: usize) -> Result<usize> {
    let client = pool.get().await?;
    client.batch_execute(&format!("DROP TABLE IF EXISTS {TABLE}")).await?;
    client
        .batch_execute(&format!(
            "CREATE TABLE {TABLE} (
               id BIGINT PRIMARY KEY,
               sold_at TIMESTAMPTZ NOT NULL,
               region TEXT NOT NULL,
               store TEXT NOT NULL,
               category TEXT NOT NULL,
               amount NUMERIC(10, 2) NOT NULL,
               qty INTEGER NOT NULL
             )"
        ))
        .await?;
    let mut rng = StdRng::seed_from_u64(42);
    let now = Utc::now();
    let mut batch: Vec<String> = Vec::with_capacity(1000);
    for id in 1..=rows {
        // later days are a bit busier (growth); weekends busier still
        let age_days = (DAYS as f64 * (1.0 - rng.r#gen::<f64>().powf(0.85))) as i64;
        let sold_at = now - Duration::days(age_days) - Duration::seconds(rng.gen_range(0..86_400));
        let weekend = matches!(sold_at.format("%u").to_string().as_str(), "6" | "7");
        if !weekend && rng.r#gen::<f64>() < 0.12 {
            continue; // thin out weekdays a little
        }
        let store_no: usize = rng.gen_range(1..=24);
        // regions own fixed stores, so the region -> store drill path is meaningful
        let region = REGIONS[store_no % 3];
        let (category, base) = CATEGORIES[rng.gen_range(0..CATEGORIES.len())];
        let qty: i32 = 1 + (rng.r#gen::<f64>().powi(2) * 4.0) as i32;
        let amount = (base * (0.4 + rng.r#gen::<f64>() * 1.2) * f64::from(qty) * 100.0).round() / 100.0;
        batch.push(format!(
            "({id}, '{}', '{region}', 'Store {store_no:02}', '{category}', {amount:.2}, {qty})",
            sold_at.format("%Y-%m-%d %H:%M:%S+00")
        ));
        if batch.len() == 1000 {
            insert(&client, &batch).await?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        insert(&client, &batch).await?;
    }
    let n: i64 = client.query_one(&format!("SELECT COUNT(*) FROM {TABLE}"), &[]).await?.get(0);
    Ok(n as usize)
}

async fn insert(client: &deadpool_postgres::Client, values: &[String]) -> Result<()> {
    client
        .batch_execute(&format!(
            "INSERT INTO {TABLE} (id, sold_at, region, store, category, amount, qty) VALUES {}",
            values.join(", ")
        ))
        .await?;
    Ok(())
}

/// A `scramdb` connection to the database the sample was loaded into (no TLS: local development).
pub fn warehouse(database_url: &str) -> Result<crate::connections::ConnectionConfig> {
    let u = url::Url::parse(database_url)?;
    let config = serde_json::json!({
        "type": "scramdb",
        "host": u.host_str().unwrap_or("127.0.0.1"),
        "port": u.port().unwrap_or(5432),
        "database": u.path().trim_start_matches('/'),
        "user": u.username(),
        "tls": false,
    });
    Ok(serde_json::from_value(config)?)
}
