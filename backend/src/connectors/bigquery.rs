//! BigQuery, through its REST API, with the organisation's own service account.
//!
//! The service-account key is the connection's sealed secret. Access tokens come from Google's
//! token endpoint (a signed JWT exchanged for a token, cached until shortly before expiry).
//! Every query is dry-run first and refused above the connection's `max_bytes_billed`, which is
//! also sent to BigQuery as a hard cap. Unqualified table names resolve in the connection's
//! dataset. Results are read page by page (at most 50,000 rows, the query limit).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::DateTime;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{Connector, ConnectorError, QueryResult};
use crate::connections::BigQuery;
use crate::materialize::ColType;

const API: &str = "https://bigquery.googleapis.com/bigquery/v2";
/// Tokens only ever come from Google (never from a URL inside the uploaded key file).
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/bigquery";
const MAX_ROWS: usize = 50_000;

#[derive(Clone, Deserialize)]
pub struct ServiceAccount {
    #[serde(rename = "type")]
    kind: String,
    client_email: String,
    private_key: String,
}

impl ServiceAccount {
    pub fn parse(s: &str) -> Result<ServiceAccount, String> {
        let sa: ServiceAccount =
            serde_json::from_str(s).map_err(|_| "paste the service account's JSON key file".to_string())?;
        if sa.kind != "service_account" {
            return Err("that isn't a service-account key (\"type\": \"service_account\")".into());
        }
        EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .map_err(|_| "the key's private_key isn't readable".to_string())?;
        Ok(sa)
    }
}

pub struct BigQueryConnector {
    cfg: BigQuery,
    sa: ServiceAccount,
    api: String,
    token_url: String,
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

fn unavailable(msg: impl Into<String>) -> ConnectorError {
    ConnectorError::Unavailable(msg.into())
}

impl BigQueryConnector {
    pub fn new(cfg: &BigQuery, secret: Option<String>) -> anyhow::Result<BigQueryConnector> {
        let secret =
            secret.ok_or_else(|| anyhow::anyhow!("this BigQuery connection has no service-account key yet"))?;
        let sa = ServiceAccount::parse(&secret).map_err(anyhow::Error::msg)?;
        Ok(BigQueryConnector::with_api(cfg, sa, API, GOOGLE_TOKEN))
    }

    fn with_api(cfg: &BigQuery, sa: ServiceAccount, api: &str, token_url: &str) -> BigQueryConnector {
        let http =
            reqwest::Client::builder().timeout(Duration::from_secs(cfg.timeout_s + 30)).build().expect("http client");
        let api = api.trim_end_matches('/').into();
        BigQueryConnector { cfg: cfg.clone(), sa, api, token_url: token_url.into(), http, token: Mutex::new(None) }
    }

    async fn access_token(&self) -> Result<String, ConnectorError> {
        let mut cached = self.token.lock().await;
        if let Some((t, _)) = cached.as_ref().filter(|(_, until)| Instant::now() < *until) {
            return Ok(t.clone());
        }
        let now = chrono::Utc::now().timestamp();
        let claims =
            json!({"iss": self.sa.client_email, "scope": SCOPE, "aud": self.token_url, "iat": now, "exp": now + 3600});
        let key = EncodingKey::from_rsa_pem(self.sa.private_key.as_bytes())
            .map_err(|e| unavailable(format!("service-account key: {e}")))?;
        let assertion = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(|e| unavailable(format!("signing: {e}")))?;
        #[derive(Deserialize)]
        struct Token {
            access_token: String,
            #[serde(default = "hour")]
            expires_in: u64,
        }
        fn hour() -> u64 {
            3600
        }
        let r = self
            .http
            .post(&self.token_url)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", &assertion)])
            .send()
            .await
            .map_err(|e| unavailable(format!("can't reach Google ({e})")))?;
        if !r.status().is_success() {
            let body: Value = r.json().await.unwrap_or(Value::Null);
            let why = body["error_description"].as_str().or(body["error"].as_str()).unwrap_or("refused");
            return Err(unavailable(format!("Google refused the service account: {why}")));
        }
        let t: Token = r.json().await.map_err(|e| unavailable(format!("Google's token answer: {e}")))?;
        let until = Instant::now() + Duration::from_secs(t.expires_in.saturating_sub(120));
        *cached = Some((t.access_token.clone(), until));
        Ok(t.access_token)
    }

    fn billing(&self) -> &str {
        self.cfg.billing_project.as_deref().unwrap_or(&self.cfg.project)
    }

    fn request(&self, sql: &str, dry_run: bool) -> Value {
        let mut body = json!({
            "query": sql,
            "useLegacySql": false,
            "dryRun": dry_run,
            "maximumBytesBilled": self.cfg.max_bytes_billed.to_string(),
            "timeoutMs": self.cfg.timeout_s * 1000,
            "maxResults": 10_000,
            "formatOptions": {"useInt64Timestamp": true},
            "labels": {"app": "magnum-bi"},
        });
        if let Some(d) = &self.cfg.dataset {
            body["defaultDataset"] = json!({"projectId": self.cfg.project, "datasetId": d});
        }
        if let Some(l) = &self.cfg.location {
            body["location"] = json!(l);
        }
        body
    }

    async fn call(&self, req: reqwest::RequestBuilder) -> Result<Value, ConnectorError> {
        let token = self.access_token().await?;
        let r = req.bearer_auth(token).send().await.map_err(|e| unavailable(format!("can't reach BigQuery ({e})")))?;
        let status = r.status();
        let body: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("request refused").to_string();
            return Err(if status.as_u16() == 401 || status.as_u16() == 403 {
                unavailable(format!("BigQuery: {msg}"))
            } else {
                ConnectorError::Query(format!("BigQuery: {msg}"))
            });
        }
        Ok(body)
    }

    async fn dry_run(&self, sql: &str) -> Result<u64, ConnectorError> {
        let url = format!("{}/projects/{}/queries", self.api, self.billing());
        let r = self.call(self.http.post(&url).json(&self.request(sql, true))).await?;
        Ok(r["totalBytesProcessed"].as_str().and_then(|b| b.parse().ok()).unwrap_or(0))
    }
}

#[async_trait]
impl Connector for BigQueryConnector {
    async fn run(&self, sql: &str, _columns: &[String]) -> Result<QueryResult, ConnectorError> {
        let started = Instant::now();
        let bytes = self.dry_run(sql).await?;
        if bytes > self.cfg.max_bytes_billed {
            return Err(ConnectorError::Query(format!(
                "this query would scan {}, over this connection's limit of {}",
                gib(bytes),
                gib(self.cfg.max_bytes_billed)
            )));
        }
        let url = format!("{}/projects/{}/queries", self.api, self.billing());
        let mut page = self.call(self.http.post(&url).json(&self.request(sql, false))).await?;
        let deadline = Instant::now() + Duration::from_secs(self.cfg.timeout_s);
        let job = page["jobReference"]["jobId"].as_str().unwrap_or_default().to_string();
        let location = page["jobReference"]["location"].as_str().map(String::from).or(self.cfg.location.clone());
        let results = |token: Option<&str>| {
            let mut q: Vec<(&str, String)> = vec![
                ("timeoutMs", "10000".into()),
                ("maxResults", "10000".into()),
                ("formatOptions.useInt64Timestamp", "true".into()),
            ];
            if let Some(l) = &location {
                q.push(("location", l.clone()));
            }
            if let Some(t) = token {
                q.push(("pageToken", t.into()));
            }
            self.http.get(format!("{}/projects/{}/queries/{job}", self.api, self.billing())).query(&q)
        };
        while page["jobComplete"] != json!(true) {
            if Instant::now() > deadline {
                return Err(ConnectorError::Query(format!("the query took longer than {} s", self.cfg.timeout_s)));
            }
            page = self.call(results(None)).await?;
        }
        let fields: Vec<Value> = page["schema"]["fields"].as_array().cloned().unwrap_or_default();
        let types: Vec<ColType> = fields.iter().map(col_type).collect();
        let mut rows: Vec<Vec<Value>> = vec![];
        loop {
            for r in page["rows"].as_array().map(Vec::as_slice).unwrap_or_default() {
                let cells = r["f"].as_array().map(Vec::as_slice).unwrap_or_default();
                rows.push(fields.iter().zip(cells).map(|(f, c)| cell(f, &c["v"])).collect());
            }
            let next = page["pageToken"].as_str().map(String::from);
            match next {
                Some(t) if rows.len() < MAX_ROWS => page = self.call(results(Some(&t))).await?,
                _ => break,
            }
        }
        rows.truncate(MAX_ROWS);
        Ok(QueryResult { rows, types, duration_ms: started.elapsed().as_millis() as u64 })
    }

    async fn tables(&self) -> Result<Vec<String>, ConnectorError> {
        let dataset = self.dataset(None)?;
        let r =
            self.run(&format!("SELECT table_name FROM `{dataset}`.INFORMATION_SCHEMA.TABLES ORDER BY 1"), &[]).await?;
        Ok(r.rows.iter().filter_map(|row| row.first()?.as_str().map(String::from)).collect())
    }

    async fn columns(&self, table: &str) -> Result<Vec<(String, ColType)>, ConnectorError> {
        let parts = crate::org_models::table_parts(table).filter(|p| p.len() <= 2).ok_or_else(|| {
            ConnectorError::Query(format!("invalid table name {table:?}: use table or dataset.table"))
        })?;
        let (dataset, name) = match parts.as_slice() {
            [d, t] => (self.dataset(Some(d))?, *t),
            _ => (self.dataset(None)?, parts[0]),
        };
        // letters, digits and underscores only (table_parts): safe inside the quotes
        let sql = format!(
            "SELECT column_name, data_type FROM `{dataset}`.INFORMATION_SCHEMA.COLUMNS
             WHERE table_name = '{name}' ORDER BY ordinal_position"
        );
        let r = self.run(&sql, &[]).await?;
        Ok(r.rows
            .iter()
            .filter_map(|row| Some((row.first()?.as_str()?.to_string(), type_named(row.get(1)?.as_str()?))))
            .collect())
    }

    async fn test(&self) -> Result<String, ConnectorError> {
        self.dry_run("SELECT 1").await?;
        if let Some(d) = &self.cfg.dataset {
            let url = format!("{}/projects/{}/datasets/{d}", self.api, self.cfg.project);
            self.call(self.http.get(&url))
                .await
                .map_err(|e| ConnectorError::Query(format!("dataset {}.{d}: {e}", self.cfg.project)))?;
        }
        Ok(format!("BigQuery as {} (billing project {})", self.sa.client_email, self.billing()))
    }
}

fn gib(b: u64) -> String {
    format!("{:.2} GiB", b as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn col_type(f: &Value) -> ColType {
    if f["mode"] == "REPEATED" {
        return ColType::Json;
    }
    match f["type"].as_str().unwrap_or_default() {
        "BOOLEAN" | "BOOL" => ColType::Bool,
        "INTEGER" | "INT64" => ColType::Int,
        "FLOAT" | "FLOAT64" => ColType::Float,
        "NUMERIC" | "BIGNUMERIC" | "DECIMAL" | "BIGDECIMAL" => ColType::Numeric,
        "DATE" => ColType::Date,
        "DATETIME" => ColType::Timestamp,
        "TIMESTAMP" => ColType::Timestamptz,
        "JSON" | "RECORD" | "STRUCT" => ColType::Json,
        _ => ColType::Text,
    }
}

/// BigQuery's JSON rows carry every value as a string (TIMESTAMP: microseconds since the epoch).
fn cell(field: &Value, v: &Value) -> Value {
    let Some(s) = v.as_str() else {
        return if v.is_null() { Value::Null } else { v.clone() }; // records, arrays
    };
    match col_type(field) {
        ColType::Bool => json!(s == "true"),
        ColType::Int => s.parse::<i64>().map_or_else(|_| json!(s), |i| json!(i)),
        ColType::Float => {
            s.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(Value::Null, Value::Number)
        }
        ColType::Numeric => s.parse::<rust_decimal::Decimal>().map_or(json!(s), super::postgres::decimal),
        ColType::Timestamptz => s
            .parse::<i64>()
            .ok()
            .and_then(DateTime::from_timestamp_micros)
            .map_or(Value::Null, |t| json!(t.to_rfc3339())),
        ColType::Json => serde_json::from_str(s).unwrap_or(json!(s)),
        _ => json!(s), // STRING, DATE (yyyy-mm-dd), DATETIME (yyyy-mm-ddThh:mm:ss[.f]), TIME
    }
}

impl BigQueryConnector {
    /// `project.dataset` for INFORMATION_SCHEMA: the one named, else the connection's.
    fn dataset(&self, named: Option<&str>) -> Result<String, ConnectorError> {
        let d = named.or(self.cfg.dataset.as_deref()).ok_or_else(|| {
            ConnectorError::Query("set the connection's dataset (or name the table as dataset.table)".into())
        })?;
        Ok(format!("{}.{d}", self.cfg.project))
    }
}

/// The neutral type of a BigQuery type name (INFORMATION_SCHEMA.COLUMNS.data_type).
fn type_named(t: &str) -> ColType {
    match t.to_uppercase().as_str() {
        "BOOL" | "BOOLEAN" => ColType::Bool,
        "INT64" | "INTEGER" => ColType::Int,
        "FLOAT64" | "FLOAT" => ColType::Float,
        "DATE" => ColType::Date,
        "TIMESTAMP" => ColType::Timestamptz,
        "DATETIME" => ColType::Timestamp,
        t if t.starts_with("NUMERIC") || t.starts_with("BIGNUMERIC") => ColType::Numeric,
        _ => ColType::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::{Form, Json, Router};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    const KEY: &str = include_str!("../../tests/fixtures/test-sa-key.pem");

    fn sa(token_uri: &str) -> String {
        json!({"type": "service_account", "client_email": "magnum@acme-analytics.iam.gserviceaccount.com",
               "private_key": KEY, "token_uri": token_uri, "project_id": "acme-analytics"})
        .to_string()
    }

    #[test]
    fn service_account_keys_are_checked() {
        assert!(ServiceAccount::parse(&sa("https://oauth2.googleapis.com/token")).is_ok());
        assert!(ServiceAccount::parse("{}").is_err());
        // the key file's token_uri is never used: tokens only ever come from Google
        let cfg: BigQuery = serde_json::from_value(json!({"project": "acme-analytics"})).unwrap();
        let c = BigQueryConnector::new(&cfg, Some(sa("http://127.0.0.1:8011/steal"))).unwrap();
        assert_eq!(c.token_url, GOOGLE_TOKEN);
        let bad = json!({"type": "authorized_user", "client_email": "x", "private_key": KEY}).to_string();
        assert!(ServiceAccount::parse(&bad).is_err());
    }

    #[test]
    fn cells() {
        let f = |t: &str| json!({"name": "x", "type": t});
        assert_eq!(cell(&f("INTEGER"), &json!("42")), json!(42));
        assert_eq!(cell(&f("NUMERIC"), &json!("12.50")), json!(12.5));
        assert_eq!(cell(&f("BOOLEAN"), &json!("false")), json!(false));
        assert_eq!(cell(&f("TIMESTAMP"), &json!("1788220800000000")), json!("2026-09-01T00:00:00+00:00"));
        assert_eq!(cell(&f("DATE"), &json!("2026-09-01")), json!("2026-09-01"));
        assert_eq!(cell(&f("STRING"), &Value::Null), Value::Null);
    }

    /// A stand-in for Google: the token endpoint and the BigQuery jobs.query API.
    async fn mock(scan_bytes: u64) -> (String, Arc<StdMutex<Vec<Value>>>) {
        let seen: Arc<StdMutex<Vec<Value>>> = Arc::default();
        let (s1, s2) = (seen.clone(), seen.clone());
        let app = Router::new()
            .route(
                "/token",
                post(|Form(f): Form<HashMap<String, String>>| async move {
                    let assertion = f.get("assertion").cloned().unwrap_or_default();
                    assert_eq!(f.get("grant_type").map(String::as_str), Some("urn:ietf:params:oauth:grant-type:jwt-bearer"));
                    assert_eq!(assertion.split('.').count(), 3, "a signed JWT");
                    Json(json!({"access_token": "ya29.test", "expires_in": 3600}))
                }),
            )
            .route(
                "/projects/{p}/queries",
                post(move |Json(body): Json<Value>| {
                    let seen = s1.clone();
                    async move {
                        seen.lock().unwrap().push(body.clone());
                        if body["dryRun"] == json!(true) {
                            return Json(json!({"totalBytesProcessed": scan_bytes.to_string(), "jobComplete": true}));
                        }
                        Json(json!({"jobComplete": false, "jobReference": {"jobId": "job_1", "location": "EU"}}))
                    }
                }),
            )
            .route(
                "/projects/{p}/queries/{job}",
                get(move |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| {
                    let seen = s2.clone();
                    async move {
                        seen.lock().unwrap().push(json!({"poll": q}));
                        let schema = json!({"fields": [{"name": "region", "type": "STRING"}, {"name": "day", "type": "TIMESTAMP"},
                                                       {"name": "revenue", "type": "NUMERIC"}]});
                        match q.get("pageToken").map(String::as_str) {
                            None => Json(json!({"jobComplete": true, "schema": schema, "pageToken": "p2",
                                "rows": [{"f": [{"v": "EMEA"}, {"v": "1788220800000000"}, {"v": "10.5"}]}]})),
                            Some(_) => Json(json!({"jobComplete": true, "schema": schema,
                                "rows": [{"f": [{"v": "AMER"}, {"v": null}, {"v": "3"}]}]})),
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, seen)
    }

    fn connector(url: &str, max: u64) -> BigQueryConnector {
        let cfg: BigQuery =
            serde_json::from_value(json!({"project": "acme-analytics", "dataset": "sales", "location": "EU",
                                                          "max_bytes_billed": max}))
            .unwrap();
        let sa = ServiceAccount::parse(&sa(GOOGLE_TOKEN)).unwrap();
        BigQueryConnector::with_api(&cfg, sa, url, &format!("{url}/token"))
    }

    #[tokio::test]
    async fn dry_run_then_poll_then_pages() {
        let (url, seen) = mock(1_000).await;
        let r = connector(&url, 1_000_000).run("SELECT 1", &[]).await.unwrap();
        assert_eq!(r.types, [ColType::Text, ColType::Timestamptz, ColType::Numeric]);
        assert_eq!(
            r.rows,
            vec![
                vec![json!("EMEA"), json!("2026-09-01T00:00:00+00:00"), json!(10.5)],
                vec![json!("AMER"), Value::Null, json!(3)],
            ]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0]["dryRun"], json!(true));
        assert_eq!(seen[1]["dryRun"], json!(false));
        assert_eq!(seen[1]["defaultDataset"], json!({"projectId": "acme-analytics", "datasetId": "sales"}));
        assert_eq!(seen[1]["maximumBytesBilled"], json!("1000000"));
        assert_eq!(seen[2]["poll"]["location"], json!("EU"));
    }

    #[tokio::test]
    async fn lists_tables_and_columns_from_the_datasets_information_schema() {
        let (url, seen) = mock(1_000).await;
        let c = connector(&url, 1_000_000);
        c.tables().await.unwrap();
        c.columns("orders").await.unwrap();
        assert!(c.columns("orders' OR '1'='1").await.is_err());
        let seen = seen.lock().unwrap();
        let queries: Vec<&str> =
            seen.iter().filter(|b| b["dryRun"] == json!(false)).filter_map(|b| b["query"].as_str()).collect();
        assert!(queries[0].contains("`acme-analytics.sales`.INFORMATION_SCHEMA.TABLES"), "{queries:?}");
        assert!(
            queries[1].contains("INFORMATION_SCHEMA.COLUMNS") && queries[1].contains("table_name = 'orders'"),
            "{queries:?}"
        );
        let types: Vec<ColType> = [
            "STRING",
            "INT64",
            "FLOAT64",
            "NUMERIC",
            "BIGNUMERIC",
            "BOOL",
            "DATE",
            "TIMESTAMP",
            "DATETIME",
            "ARRAY<STRING>",
        ]
        .iter()
        .map(|t| type_named(t))
        .collect();
        use ColType::*;
        assert_eq!(types, [Text, Int, Float, Numeric, Numeric, Bool, Date, Timestamptz, Timestamp, Text]);
    }

    #[tokio::test]
    async fn queries_over_the_byte_cap_are_refused_before_running() {
        let (url, seen) = mock(5_000_000).await;
        let e = connector(&url, 1_000_000).run("SELECT 1", &[]).await.unwrap_err();
        assert!(e.to_string().contains("over this connection's limit"), "{e}");
        assert_eq!(seen.lock().unwrap().len(), 1, "only the dry run was sent");
    }
}
