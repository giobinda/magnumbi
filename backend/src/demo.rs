//! The NYC demo: five years (or fewer months) of New York yellow taxi trips and the weather in
//! each borough, ready to explore. `magnum-bi demo` (make demo) or Organisations › Demo data.
//!
//! 1. finds the latest month the NYC Taxi & Limousine Commission has published (TLC trip
//!    records, https://www.nyc.gov/site/tlc/about/tlc-trip-record-data.page);
//! 2. downloads each month, cleans it with DuckDB into one schema (zone and borough names,
//!    readable labels, implausible trips left out) and deletes the download; months already
//!    cleaned are skipped, so a second run only adds what's new;
//! 3. fetches hourly weather per borough from Open-Meteo (ERA5 reanalysis, CC BY 4.0);
//! 4. creates the DuckDB connections `nyc_taxi` (trips) and `meteo` (weather) and imports the
//!    `nyc_taxi` and `nyc_taxi_weather` dashboards, their period set to the latest month.
//!
//! The models `taxi` and `weather` and the blend `taxi_weather` are project files.

use std::path::Path;

use chrono::{Datelike, Months, NaiveDate};

/// Where the data comes from (tests use a stand-in).
#[derive(Debug, Clone)]
pub struct Sources {
    pub tlc: String,
    pub weather: String,
}

impl Default for Sources {
    fn default() -> Sources {
        Sources {
            tlc: "https://d37ci6vzurychx.cloudfront.net".into(),
            weather: "https://archive-api.open-meteo.com".into(),
        }
    }
}

/// How a demo setup is going (shown in the UI, printed by the CLI).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub running: bool,
    pub org: String,
    pub step: String,
    pub done: u32,
    pub total: u32,
    pub log: Vec<String>,
    pub error: Option<String>,
}

pub type Progress = std::sync::Arc<std::sync::Mutex<Status>>;

/// Set up the demo for `org` ("" = the platform workspace) with `months` months of trips.
pub async fn run(
    state: &crate::state::AppState,
    org: &str,
    months: u32,
    by: &str,
    today: NaiveDate,
    src: &Sources,
    progress: &Progress,
) -> anyhow::Result<()> {
    *progress.lock().expect("demo lock") = Status {
        running: true,
        org: org.into(),
        step: "Finding the latest published month".into(),
        ..Status::default()
    };
    let result = steps(state, org, months, by, today, src, progress).await;
    let mut st = progress.lock().expect("demo lock");
    st.running = false;
    match &result {
        Ok(()) => st.step = "Done".into(),
        Err(e) => {
            tracing::warn!(org, "demo setup failed: {e:#}");
            st.error = Some(format!("{e:#}"));
        }
    }
    result
}

fn note(progress: &Progress, step: Option<&str>, line: Option<String>) {
    let mut st = progress.lock().expect("demo lock");
    if let Some(s) = step {
        st.step = s.into();
    }
    if let Some(l) = line {
        tracing::info!("demo: {l}");
        st.log.push(l);
    }
}

/// The boroughs of the taxi zones, a point in each (the nearest ERA5 cell is used).
const BOROUGHS: [(&str, f64, f64); 6] = [
    ("Manhattan", 40.7831, -73.9712),
    ("Brooklyn", 40.6782, -73.9442),
    ("Queens", 40.7282, -73.7949),
    ("Bronx", 40.8448, -73.8648),
    ("Staten Island", 40.5795, -74.1502),
    ("EWR", 40.6895, -74.1745),
];
const HOURLY: [&str; 11] = [
    "temperature_2m",
    "apparent_temperature",
    "relative_humidity_2m",
    "precipitation",
    "rain",
    "snowfall",
    "snow_depth",
    "cloud_cover",
    "wind_speed_10m",
    "wind_gusts_10m",
    "weather_code",
];

/// Download to `path` (via a .part file); false if the source says it doesn't exist.
async fn download(http: &reqwest::Client, url: &str, path: &Path) -> anyhow::Result<bool> {
    use tokio::io::AsyncWriteExt;
    let r = http.get(url).send().await?;
    if r.status() == reqwest::StatusCode::NOT_FOUND || r.status() == reqwest::StatusCode::FORBIDDEN {
        return Ok(false);
    }
    let mut r = r.error_for_status()?;
    let part = path.with_extension("part");
    let mut f = tokio::fs::File::create(&part).await?;
    while let Some(chunk) = r.chunk().await? {
        f.write_all(&chunk).await?;
    }
    f.flush().await?;
    tokio::fs::rename(&part, path).await?;
    Ok(true)
}

/// One request for the weather of [a, b] (retried: the free API can be slow), as JSON lines.
async fn weather_chunk(
    http: &reqwest::Client,
    src: &Sources,
    a: NaiveDate,
    b: NaiveDate,
    out: &Path,
) -> anyhow::Result<()> {
    let lat: Vec<String> = BOROUGHS.iter().map(|(_, la, _)| la.to_string()).collect();
    let lon: Vec<String> = BOROUGHS.iter().map(|(_, _, lo)| lo.to_string()).collect();
    let url = format!("{}/v1/archive", src.weather);
    let query = [
        ("latitude", lat.join(",")),
        ("longitude", lon.join(",")),
        ("start_date", a.to_string()),
        ("end_date", b.to_string()),
        ("hourly", HOURLY.join(",")),
        ("models", "era5_seamless".into()),
        ("cell_selection", "nearest".into()),
        ("timezone", "America/New_York".into()),
    ];
    let mut last = None;
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(20 * attempt)).await;
        }
        match http.get(&url).query(&query).send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => match r.json::<serde_json::Value>().await {
                Ok(data) => {
                    let places = data.as_array().filter(|p| p.len() == BOROUGHS.len());
                    let places = places.ok_or_else(|| anyhow::anyhow!("unexpected weather answer"))?;
                    let mut lines = String::new();
                    for ((borough, _, _), place) in BOROUGHS.iter().zip(places) {
                        let h = &place["hourly"];
                        for (i, t) in h["time"].as_array().into_iter().flatten().enumerate() {
                            let mut row = serde_json::json!({"borough": borough, "time": t});
                            for k in HOURLY {
                                row[k] = h[k][i].clone();
                            }
                            lines.push_str(&row.to_string());
                            lines.push('\n');
                        }
                    }
                    tokio::fs::write(out.with_extension("part"), lines).await?;
                    tokio::fs::rename(out.with_extension("part"), out).await?;
                    return Ok(());
                }
                Err(e) => last = Some(anyhow::Error::from(e)),
            },
            Err(e) => last = Some(e.into()),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("weather unavailable")))
}

/// Quarters (or parts of them) covering [from, to]: one weather request each.
fn quarters(from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
    let mut out = vec![];
    let mut a = from;
    while a <= to {
        let q_start = NaiveDate::from_ymd_opt(a.year(), (a.month0() / 3) * 3 + 1, 1).expect("valid");
        let q_end = q_start.checked_add_months(Months::new(3)).and_then(|d| d.pred_opt()).expect("valid");
        let b = q_end.min(to);
        out.push((a, b));
        a = b.succ_opt().expect("valid");
    }
    out
}

async fn steps(
    state: &crate::state::AppState,
    org: &str,
    months_wanted: u32,
    by: &str,
    today: NaiveDate,
    src: &Sources,
    progress: &Progress,
) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(900))
        .build()?;
    let store = state.orgs.store(org).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let dir = crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, org).join("nyc_taxi");
    let (raw, trips, weather) = (dir.join("raw"), dir.join("trips"), dir.join("weather"));
    for d in [&raw, &trips, &weather] {
        std::fs::create_dir_all(d)?;
    }

    // 1. the months
    let mut latest = None;
    for m in recent_months(today) {
        let r = http.head(format!("{}/trip-data/yellow_tripdata_{m}.parquet", src.tlc)).send().await?;
        if r.status().is_success() {
            latest = Some(m);
            break;
        }
    }
    let latest = latest.ok_or_else(|| anyhow::anyhow!("no recent taxi data is published (or it can't be reached)"))?;
    let wanted = months(&latest, months_wanted.clamp(1, 120));
    note(progress, Some("Downloading taxi trips"), Some(format!("{} months, up to {latest}", wanted.len())));
    progress.lock().expect("demo lock").total = wanted.len() as u32;

    // 2. the trips, month by month
    let zones = raw.join("taxi_zone_lookup.csv");
    if !zones.exists() && !download(&http, &format!("{}/misc/taxi_zone_lookup.csv", src.tlc), &zones).await? {
        anyhow::bail!("the taxi zone lookup isn't available");
    }
    for (i, m) in wanted.iter().enumerate() {
        let out = trips.join(format!("{m}.parquet"));
        if out.exists() {
            note(progress, None, Some(format!("{m}: already downloaded")));
        } else {
            let file = raw.join(format!("yellow_tripdata_{m}.parquet"));
            let url = format!("{}/trip-data/yellow_tripdata_{m}.parquet", src.tlc);
            if file.exists() || download(&http, &url, &file).await? {
                let (f, z, o, month) = (file.clone(), zones.clone(), out.clone(), m.clone());
                let n = tokio::task::spawn_blocking(move || clean_month(&f, &z, &o, &month)).await??;
                std::fs::remove_file(&file).ok(); // the cleaned month is what's kept
                note(progress, None, Some(format!("{m}: {n} trips")));
            } else {
                note(progress, None, Some(format!("{m}: not published")));
            }
        }
        progress.lock().expect("demo lock").done = i as u32 + 1;
    }

    // 3. the weather
    note(progress, Some("Fetching the weather"), None);
    let (first, _) = month_days(&wanted[0]).expect("valid month");
    let (_, last) = month_days(&latest).expect("valid month");
    for (a, b) in quarters(first, last) {
        let f = raw.join(format!("weather_{a}_{b}.jsonl"));
        if !f.exists() {
            weather_chunk(&http, src, a, b, &f).await?;
        }
    }
    let (glob, out) = (raw.join("weather_*.jsonl"), weather.join("hourly.parquet"));
    let hours = tokio::task::spawn_blocking(move || weather_parquet(&glob, &out)).await??;
    note(progress, None, Some(format!("weather: {hours} borough-hours")));

    // 4. connections and dashboards
    note(progress, Some("Creating connections and dashboards"), None);
    let connections = [
        (
            "nyc_taxi",
            serde_json::json!({"type": "duckdb", "tables": {"trips": "nyc_taxi/trips/*.parquet"}, "statement_timeout_ms": 120000}),
        ),
        ("meteo", serde_json::json!({"type": "duckdb", "tables": {"hourly": "nyc_taxi/weather/hourly.parquet"}})),
    ];
    for (name, config) in connections {
        let config = serde_json::from_value(config)?;
        state
            .connectors
            .save(&store, org, name, config, None, Some(by))
            .await
            .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    }
    let project = state.project.current();
    for id in ["nyc_taxi", "nyc_taxi_weather"] {
        let path = state.project.dashboards_dir().join(format!("{id}.yaml"));
        let text = with_month(&std::fs::read_to_string(&path)?, &latest)?;
        store.write_dashboard(org, id, &text, Some(by)).await?;
    }
    note(progress, None, Some(format!("dashboards nyc_taxi and nyc_taxi_weather, showing {latest}")));
    if !org.is_empty() {
        let (access, _) = state.access.get(state, &project).await;
        let models = access.platform.organisations.get(org).map(|o| o.models.clone()).unwrap_or_default();
        let missing: Vec<&str> =
            ["taxi", "weather"].into_iter().filter(|m| !models.iter().any(|x| x == "*" || x == m)).collect();
        if !missing.is_empty() {
            note(
                progress,
                None,
                Some(format!(
                    "note: {org} can't use the models {} yet: add them on Organisations",
                    missing.join(" and ")
                )),
            );
        }
    }
    Ok(())
}

/// The months before `today`, newest first: where the latest published one is (the TLC
/// publishes with a delay of about two months).
pub fn recent_months(today: NaiveDate) -> Vec<String> {
    (1..=6)
        .filter_map(|k| today.checked_sub_months(Months::new(k)))
        .map(|d| format!("{:04}-{:02}", d.year(), d.month()))
        .collect()
}

/// `n` months ending with `latest` ("2026-07"), oldest first.
pub fn months(latest: &str, n: u32) -> Vec<String> {
    let Some((first, _)) = month_days(latest) else { return vec![] };
    (0..n)
        .rev()
        .filter_map(|k| first.checked_sub_months(Months::new(k)))
        .map(|d| format!("{:04}-{:02}", d.year(), d.month()))
        .collect()
}

/// First and last day of a month ("2026-07" -> 2026-07-01, 2026-07-31).
pub fn month_days(month: &str) -> Option<(NaiveDate, NaiveDate)> {
    let first = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").ok()?;
    let last = first.checked_add_months(Months::new(1))?.pred_opt()?;
    Some((first, last))
}

fn quoted(p: &Path) -> anyhow::Result<String> {
    crate::compiler::quote(&p.to_string_lossy()).map_err(|e| anyhow::anyhow!(e.0))
}

/// One month of raw TLC trips -> the demo's schema (blocking: DuckDB).
pub fn clean_month(raw: &Path, zones: &Path, out: &Path, month: &str) -> anyhow::Result<u64> {
    let (first, _) = month_days(month).ok_or_else(|| anyhow::anyhow!("invalid month {month:?}"))?;
    let (src, zones, part) = (quoted(raw)?, quoted(zones)?, quoted(&out.with_extension("part"))?);
    let c = duckdb::Connection::open_in_memory()?;
    // columns the TLC added over the years: 0 where a month doesn't have them
    let mut cols = std::collections::BTreeSet::new();
    let mut stmt = c.prepare(&format!("DESCRIBE SELECT * FROM read_parquet({src})"))?;
    for name in stmt.query_map([], |r| r.get::<_, String>(0))? {
        cols.insert(name?.to_lowercase());
    }
    let opt = |c: &str| if cols.contains(c) { format!("CAST(t.{c} AS DOUBLE)") } else { "0.0".into() };
    c.execute_batch(&format!(
        "CREATE TABLE zones AS SELECT LocationID AS id, Borough AS borough, Zone AS zone FROM read_csv_auto({zones});
         COPY (
           SELECT
             t.tpep_pickup_datetime AS pickup_at,
             t.tpep_dropoff_datetime AS dropoff_at,
             CAST(hour(t.tpep_pickup_datetime) AS INTEGER) AS pickup_hour,
             dayname(t.tpep_pickup_datetime) AS pickup_weekday,
             CASE CAST(t.VendorID AS INTEGER) WHEN 1 THEN 'Creative Mobile Technologies' WHEN 2 THEN 'Curb Mobility'
               WHEN 6 THEN 'Myle Technologies' WHEN 7 THEN 'Helix' ELSE 'Other' END AS vendor,
             coalesce(pu.borough, 'Unknown') AS pickup_borough,
             coalesce(pu.zone, 'Unknown') AS pickup_zone,
             coalesce(dz.borough, 'Unknown') AS dropoff_borough,
             coalesce(dz.zone, 'Unknown') AS dropoff_zone,
             CASE CAST(t.payment_type AS INTEGER) WHEN 0 THEN 'Flex fare' WHEN 1 THEN 'Credit card' WHEN 2 THEN 'Cash'
               WHEN 3 THEN 'No charge' WHEN 4 THEN 'Dispute' WHEN 6 THEN 'Voided' ELSE 'Unknown' END AS payment,
             CASE CAST(t.RatecodeID AS INTEGER) WHEN 1 THEN 'Standard' WHEN 2 THEN 'JFK' WHEN 3 THEN 'Newark'
               WHEN 4 THEN 'Nassau or Westchester' WHEN 5 THEN 'Negotiated' WHEN 6 THEN 'Group ride' ELSE 'Unknown' END AS rate,
             CAST(t.passenger_count AS INTEGER) AS passengers,
             CAST(t.trip_distance AS DOUBLE) AS distance_mi,
             round(date_diff('second', t.tpep_pickup_datetime, t.tpep_dropoff_datetime) / 60.0, 2) AS duration_min,
             CAST(t.fare_amount AS DOUBLE) AS fare,
             CAST(t.tip_amount AS DOUBLE) AS tip,
             CAST(t.tolls_amount AS DOUBLE) AS tolls,
             {} AS congestion_surcharge,
             {} AS airport_fee,
             {} AS cbd_congestion_fee,
             CAST(t.total_amount AS DOUBLE) AS total
           FROM read_parquet({src}) t
           LEFT JOIN zones pu ON pu.id = t.PULocationID
           LEFT JOIN zones dz ON dz.id = t.DOLocationID
           WHERE t.tpep_pickup_datetime >= DATE '{first}'
             AND t.tpep_pickup_datetime < DATE '{first}' + INTERVAL 1 MONTH
             AND t.tpep_dropoff_datetime > t.tpep_pickup_datetime
             AND t.tpep_dropoff_datetime < t.tpep_pickup_datetime + INTERVAL 6 HOUR
             AND t.total_amount BETWEEN 0 AND 1000
             AND t.trip_distance BETWEEN 0 AND 200
           ORDER BY pickup_at
         ) TO {part} (FORMAT parquet, COMPRESSION zstd)",
        opt("congestion_surcharge"),
        opt("airport_fee"),
        opt("cbd_congestion_fee"),
    ))?;
    std::fs::rename(out.with_extension("part"), out)?;
    Ok(c.query_row(&format!("SELECT count(*) FROM read_parquet({})", quoted(out)?), [], |r| r.get::<_, i64>(0))? as u64)
}

/// WMO weather codes: (description, condition).
const CODES: [(i32, &str, &str); 28] = [
    (0, "Clear sky", "Clear"),
    (1, "Mainly clear", "Clear"),
    (2, "Partly cloudy", "Cloudy"),
    (3, "Overcast", "Cloudy"),
    (45, "Fog", "Fog"),
    (48, "Depositing rime fog", "Fog"),
    (51, "Light drizzle", "Drizzle"),
    (53, "Moderate drizzle", "Drizzle"),
    (55, "Dense drizzle", "Drizzle"),
    (56, "Light freezing drizzle", "Drizzle"),
    (57, "Dense freezing drizzle", "Drizzle"),
    (61, "Slight rain", "Rain"),
    (63, "Moderate rain", "Rain"),
    (65, "Heavy rain", "Rain"),
    (66, "Light freezing rain", "Rain"),
    (67, "Heavy freezing rain", "Rain"),
    (71, "Slight snowfall", "Snow"),
    (73, "Moderate snowfall", "Snow"),
    (75, "Heavy snowfall", "Snow"),
    (77, "Snow grains", "Snow"),
    (80, "Slight rain showers", "Rain"),
    (81, "Moderate rain showers", "Rain"),
    (82, "Violent rain showers", "Rain"),
    (85, "Slight snow showers", "Snow"),
    (86, "Heavy snow showers", "Snow"),
    (95, "Thunderstorm", "Thunderstorm"),
    (96, "Thunderstorm with slight hail", "Thunderstorm"),
    (99, "Thunderstorm with heavy hail", "Thunderstorm"),
];

/// Weather answers (JSON lines: borough, time, hourly values) -> weather/hourly.parquet.
pub fn weather_parquet(jsonl_glob: &Path, out: &Path) -> anyhow::Result<u64> {
    let c = duckdb::Connection::open_in_memory()?;
    let codes: Vec<String> = CODES.iter().map(|(c, w, g)| format!("({c}, '{w}', '{g}')")).collect();
    let (src, part) = (quoted(jsonl_glob)?, quoted(&out.with_extension("part"))?);
    c.execute_batch(&format!(
        "CREATE TABLE codes (code INTEGER, weather VARCHAR, condition VARCHAR);
         INSERT INTO codes VALUES {};
         COPY (
           SELECT
             w.borough,
             CAST(w.time AS TIMESTAMP) AS observed_at,
             CAST(CAST(w.time AS TIMESTAMP) AS DATE) AS observed_on,
             CAST(w.temperature_2m AS DOUBLE) AS temperature_c,
             CAST(w.apparent_temperature AS DOUBLE) AS feels_like_c,
             CAST(w.relative_humidity_2m AS INTEGER) AS humidity_pct,
             CAST(w.precipitation AS DOUBLE) AS precipitation_mm,
             CAST(w.rain AS DOUBLE) AS rain_mm,
             CAST(w.snowfall AS DOUBLE) AS snowfall_cm,
             CAST(w.snow_depth AS DOUBLE) AS snow_depth_m,
             CAST(w.cloud_cover AS INTEGER) AS cloud_cover_pct,
             CAST(w.wind_speed_10m AS DOUBLE) AS wind_speed_kmh,
             CAST(w.wind_gusts_10m AS DOUBLE) AS wind_gusts_kmh,
             CAST(w.weather_code AS INTEGER) AS weather_code,
             coalesce(c.weather, 'Unknown') AS weather,
             coalesce(c.condition, 'Unknown') AS condition
           FROM read_json({src}, format = 'newline_delimited') w
           LEFT JOIN codes c ON c.code = w.weather_code
           WHERE w.temperature_2m IS NOT NULL
           QUALIFY row_number() OVER (PARTITION BY w.borough, w.time) = 1
           ORDER BY observed_at, borough
         ) TO {part} (FORMAT parquet, COMPRESSION zstd)",
        codes.join(", ")
    ))?;
    std::fs::rename(out.with_extension("part"), out)?;
    Ok(c.query_row(&format!("SELECT count(*) FROM read_parquet({})", quoted(out)?), [], |r| r.get::<_, i64>(0))? as u64)
}

/// A dashboard's YAML with its period set to `month` (keeping its comparison).
pub fn with_month(yaml: &str, month: &str) -> anyhow::Result<String> {
    let mut d = crate::project::parse_dashboard(yaml).map_err(anyhow::Error::msg)?;
    let (first, last) = month_days(month).ok_or_else(|| anyhow::anyhow!("invalid month {month:?}"))?;
    if let Some(p) = &mut d.period {
        p.current = crate::schema::PeriodRange { from: Some(first), to: Some(last), ..Default::default() };
    }
    Ok(crate::project::dump_dashboard(&d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("magnum-demo-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn months_and_days() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        assert_eq!(recent_months(today)[..3], ["2026-09", "2026-08", "2026-07"], "newest first");
        assert_eq!(recent_months(today).len(), 6);
        assert_eq!(months("2026-02", 3), ["2025-12", "2026-01", "2026-02"]);
        assert_eq!(months("2026-07", 1), ["2026-07"]);
        let (a, b) = month_days("2024-02").unwrap();
        assert_eq!((a.to_string(), b.to_string()), ("2024-02-01".into(), "2024-02-29".into()));
        assert!(month_days("nope").is_none());
    }

    /// A raw month like the TLC's: this one before the congestion fee existed, with a trip from
    /// another month and one with a negative total (both left out).
    pub fn raw_month(path: &Path, month: &str, with_cbd_fee: bool) {
        let fee = if with_cbd_fee { ", 0.75 AS cbd_congestion_fee" } else { "" };
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                   SELECT 2 AS VendorID, CAST('{month}-01 08:00:00' AS TIMESTAMP) + INTERVAL (i) HOUR AS tpep_pickup_datetime,
                          CAST('{month}-01 08:20:00' AS TIMESTAMP) + INTERVAL (i) HOUR AS tpep_dropoff_datetime,
                          1.0 AS passenger_count, 2.5 AS trip_distance, 1.0 AS RatecodeID, 'N' AS store_and_fwd_flag,
                          CASE WHEN i % 2 = 0 THEN 161 ELSE 1 END AS PULocationID, 236 AS DOLocationID, 1 AS payment_type,
                          15.0 AS fare_amount, 3.0 AS tip_amount, 0.0 AS tolls_amount,
                          CASE WHEN i = 3 THEN -20.0 ELSE 22.5 END AS total_amount, 2.5 AS congestion_surcharge,
                          0.0 AS Airport_fee{fee}
                   FROM range(5) t(i)
                   UNION ALL
                   SELECT 2, TIMESTAMP '2001-01-01 00:00:00', TIMESTAMP '2001-01-01 00:10:00', 1.0, 1.0, 1.0, 'N', 161, 236, 1,
                          5.0, 0.0, 0.0, 7.0, 0.0, 0.0{}
                 ) TO '{}' (FORMAT parquet)",
                if with_cbd_fee { ", 0.0" } else { "" },
                path.display()
            ))
            .unwrap();
    }

    pub fn zones(path: &Path) {
        std::fs::write(
            path,
            "\"LocationID\",\"Borough\",\"Zone\",\"service_zone\"\n1,\"EWR\",\"Newark Airport\",\"EWR\"\n\
             161,\"Manhattan\",\"Midtown Center\",\"Yellow Zone\"\n236,\"Manhattan\",\"Upper East Side North\",\"Yellow Zone\"\n",
        )
        .unwrap();
    }

    #[test]
    fn cleaning_one_month() {
        let d = tmp();
        zones(&d.join("zones.csv"));
        for (month, fee) in [("2024-12", false), ("2026-07", true)] {
            raw_month(&d.join(format!("raw-{month}.parquet")), month, fee);
            let out = d.join(format!("{month}.parquet"));
            let n = clean_month(&d.join(format!("raw-{month}.parquet")), &d.join("zones.csv"), &out, month).unwrap();
            assert_eq!(n, 4, "{month}: the other month's trip and the negative total are left out");
            let c = Connection::open_in_memory().unwrap();
            let (borough, zone, payment, fee, minutes): (String, String, String, f64, f64) = c
                .query_row(
                    &format!(
                        "SELECT pickup_borough, pickup_zone, payment, cbd_congestion_fee, duration_min
                         FROM read_parquet('{}') ORDER BY pickup_at LIMIT 1",
                        out.display()
                    ),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .unwrap();
            assert_eq!(
                (borough.as_str(), zone.as_str(), payment.as_str()),
                ("Manhattan", "Midtown Center", "Credit card")
            );
            assert_eq!(fee, if fee > 0.0 { 0.75 } else { 0.0 });
            assert_eq!(minutes, 20.0);
        }
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn weather_to_parquet() {
        let d = tmp();
        let line = |b: &str, t: &str, code: i64| {
            serde_json::json!({"borough": b, "time": t, "temperature_2m": 21.5, "apparent_temperature": 22.0,
                "relative_humidity_2m": 60, "precipitation": 0.4, "rain": 0.4, "snowfall": 0.0, "snow_depth": 0.0,
                "cloud_cover": 80, "wind_speed_10m": 12.0, "wind_gusts_10m": 30.0, "weather_code": code})
            .to_string()
        };
        let rows = [
            line("Manhattan", "2026-07-01T00:00", 61),
            line("EWR", "2026-07-01T00:00", 3),
            line("Manhattan", "2026-07-01T00:00", 61),
        ];
        std::fs::write(d.join("weather_a.jsonl"), rows.join("\n") + "\n").unwrap();
        let out = d.join("hourly.parquet");
        assert_eq!(weather_parquet(&d.join("weather_*.jsonl"), &out).unwrap(), 2, "one row per borough and hour");
        let c = Connection::open_in_memory().unwrap();
        let (cond, weather): (String, String) = c
            .query_row(
                &format!(
                    "SELECT condition, weather FROM read_parquet('{}') WHERE borough = 'Manhattan'",
                    out.display()
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((cond.as_str(), weather.as_str()), ("Rain", "Slight rain"));
        std::fs::remove_dir_all(d).ok();
    }

    /// The TLC and Open-Meteo, as a test stand-in: two published months, the zone lookup, and
    /// weather for whatever dates are asked.
    async fn sources(dir: &Path) -> Sources {
        use axum::extract::Query;
        use axum::routing::get;
        use std::collections::HashMap;
        zones(&dir.join("zones.csv"));
        for m in ["2026-06", "2026-07"] {
            raw_month(&dir.join(format!("yellow_tripdata_{m}.parquet")), m, true);
        }
        let files = dir.to_path_buf();
        let app = axum::Router::new()
            .route(
                "/trip-data/{file}",
                get(move |axum::extract::Path(file): axum::extract::Path<String>| {
                    let files = files.clone();
                    async move {
                        match std::fs::read(files.join(&file)) {
                            Ok(b) => (axum::http::StatusCode::OK, b),
                            Err(_) => (axum::http::StatusCode::NOT_FOUND, vec![]),
                        }
                    }
                }),
            )
            .route(
                "/misc/taxi_zone_lookup.csv",
                get({
                    let z = std::fs::read(dir.join("zones.csv")).unwrap();
                    move || async move { z }
                }),
            )
            .route(
                "/v1/archive",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    let (a, b) = (
                        NaiveDate::parse_from_str(&q["start_date"], "%Y-%m-%d").unwrap(),
                        NaiveDate::parse_from_str(&q["end_date"], "%Y-%m-%d").unwrap(),
                    );
                    let hours: Vec<String> = a
                        .iter_days()
                        .take_while(|d| *d <= b)
                        .flat_map(|d| (0..24).map(move |h| format!("{d}T{h:02}:00")))
                        .collect();
                    let n = hours.len();
                    let place = |code: i64| {
                        serde_json::json!({"hourly": {
                        "time": hours, "temperature_2m": vec![20.0; n], "apparent_temperature": vec![21.0; n],
                        "relative_humidity_2m": vec![60; n], "precipitation": vec![0.0; n], "rain": vec![0.0; n],
                        "snowfall": vec![0.0; n], "snow_depth": vec![0.0; n], "cloud_cover": vec![10; n],
                        "wind_speed_10m": vec![5.0; n], "wind_gusts_10m": vec![9.0; n], "weather_code": vec![code; n]}})
                    };
                    axum::Json(serde_json::Value::Array((0..6).map(|i| place(if i == 0 { 61 } else { 0 })).collect()))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Sources { tlc: base.clone(), weather: base }
    }

    #[tokio::test]
    async fn the_whole_demo() {
        use crate::api::tests::{OWNER, app};
        use serde_json::{Value, json};
        let Some((app, state, cookie)) = app().await else { return };
        // a throwaway organisation (never the platform workspace's real demo), its rows in the
        // test database under its own id
        let org = format!("demo{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        state.orgs.pin(&org, state.store.clone());
        let access =
            json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "Demo test", "models": ["*"]}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let call = |method: &'static str, path: &'static str, body: Option<Value>| {
            let (app, cookie, org) = (app.clone(), cookie.clone(), org.clone());
            async move {
                let req = axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("cookie", cookie)
                    .header("x-magnum-org", org)
                    .header("content-type", "application/json");
                let body = body.map_or(axum::body::Body::empty(), |b| axum::body::Body::from(b.to_string()));
                let resp = tower::ServiceExt::oneshot(app, req.body(body).unwrap()).await.unwrap();
                let status = resp.status();
                let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap_or_default())
            }
        };
        let dir = tmp();
        let src = sources(&dir).await;
        let today = NaiveDate::from_ymd_opt(2026, 9, 20).unwrap();
        let progress = Progress::default();
        run(&state, &org, 3, OWNER, today, &src, &progress).await.unwrap();
        let st = progress.lock().unwrap().clone();
        assert!(!st.running && st.error.is_none(), "{st:?}");
        assert!(st.log.iter().any(|l| l.contains("2026-07")), "{:?}", st.log);
        assert!(
            st.log.iter().any(|l| l.contains("2026-05") && l.contains("not published")),
            "only what exists: {:?}",
            st.log
        );

        let names: Vec<String> = state.store.connections(&org).await.unwrap().into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"nyc_taxi".to_string()) && names.contains(&"meteo".to_string()), "{names:?}");
        let (_, d) = call("GET", "/api/dashboards/nyc_taxi_weather", None).await;
        assert_eq!(d["dashboard"]["period"]["current"]["from"], json!("2026-07-01"), "{d}");

        // taxi trips x weather (two connections), joined on ScramDB
        let q = json!({"model": "taxi_weather", "dimensions": ["weather.condition"], "measures": ["taxi.trips"],
                       "sort": ["weather.condition"], "filters": [{"field": "weather.condition", "op": "not_null"}]});
        let (s, r) = call("POST", "/api/query", Some(q)).await;
        assert_eq!(s, axum::http::StatusCode::OK, "{r}");
        // two published months, each: 3 Manhattan trips (rain there), 1 Newark trip (clear)
        assert_eq!(r["rows"], json!([["Clear", 2], ["Rain", 6]]), "{r}");

        // again: nothing to download, the same result
        run(&state, &org, 3, OWNER, today, &src, &progress).await.unwrap();
        assert!(
            progress.lock().unwrap().log.iter().any(|l| l.contains("already")),
            "{:?}",
            progress.lock().unwrap().log
        );

        // the test organisation's rows and files, gone
        let c = state.store.pool.get().await.unwrap();
        for table in ["connections", "dashboards", "dashboard_versions"] {
            c.execute(&format!("DELETE FROM {table} WHERE org = $1"), &[&org]).await.unwrap();
        }
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, &org)).ok();
    }

    #[test]
    fn dashboards_show_the_latest_month() {
        let yaml = "title: T\nperiod:\n  model: taxi\n  field: pickup_at\n  current: {from: 2026-07-01, to: 2026-07-31}\n  compare: previous_year\ntiles: []\n";
        let out = with_month(yaml, "2027-02").unwrap();
        let d = crate::project::parse_dashboard(&out).unwrap();
        let p = d.period.unwrap();
        assert_eq!(p.current.from.unwrap().to_string(), "2027-02-01");
        assert_eq!(p.current.to.unwrap().to_string(), "2027-02-28");
        assert!(p.compare.is_some(), "the comparison stays");
    }
}
