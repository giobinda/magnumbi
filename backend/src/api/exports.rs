//! Downloads: a tile's data as CSV, a dashboard's as a zip of CSVs (one per chart, a folder per
//! tab). The same queries as on screen (the saved tile with the person's filters, period and
//! drill), under the same restrictions; only for roles with `download` (users and up).

use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

use super::Body;
use super::dashboards::{load, run_saved};
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::export::{csv, file_name, zip};
use crate::state::{AppState, Shared};
use crate::tile_query::TileRequest;

/// A file to save, never kept by a browser or proxy.
pub(super) fn attachment(content_type: &'static str, name: &str, bytes: Vec<u8>) -> Response {
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    // names are built from file_name(): letters, digits, '_', '-' and '.'
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap());
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// A tile's data as CSV: (file name, text).
pub async fn tile_csv(
    state: &AppState,
    caller: &Caller,
    id: &str,
    tile_id: &str,
    req: &TileRequest,
) -> ApiResult<Response> {
    caller.require("download")?;
    let result = run_saved(state, caller, id, tile_id, req, "export").await?;
    let name = format!("{}-{}.csv", file_name(id, "dashboard"), file_name(tile_id, "tile"));
    Ok(attachment("text/csv; charset=utf-8", &name, csv(&result).into_bytes()))
}

/// A dashboard's charts as a zip of CSVs, with the person's filters and period (no drilling).
/// A tile that fails is listed in errors.txt rather than failing the rest.
pub async fn dashboard_zip(state: &AppState, caller: &Caller, id: &str, req: &TileRequest) -> ApiResult<Response> {
    caller.require("download")?;
    let (d, _) = load(state, caller, id).await?;
    let req = TileRequest { drill: vec![], ..req.clone() };
    let (mut files, mut errors): (Vec<(String, String)>, Vec<String>) = (vec![], vec![]);
    for t in d.tiles.iter().filter(|t| t.query.is_some()) {
        let mut name = file_name(t.title.as_deref().unwrap_or(&t.id), &file_name(&t.id, "tile"));
        if let Some(first) = d.tabs.first() {
            let tab = t.tab.as_ref().filter(|x| d.tabs.contains(x)).unwrap_or(first);
            name = format!("{}/{name}", file_name(tab, "tab"));
        }
        if files.iter().any(|(n, _)| *n == format!("{name}.csv")) {
            name = format!("{name}_{}", file_name(&t.id, "tile"));
        }
        match run_saved(state, caller, id, &t.id, &req, "export").await {
            Ok(result) => files.push((format!("{name}.csv"), csv(&result))),
            Err(e) => {
                let detail = e.into_parts().1["detail"].as_str().unwrap_or("failed").to_string();
                errors.push(format!("{}: {detail}", t.title.as_deref().unwrap_or(&t.id)));
            }
        }
    }
    if !errors.is_empty() {
        files.push(("errors.txt".into(), errors.join("\n") + "\n"));
    }
    let bytes = zip(&files).map_err(|e| ApiError::Internal(e.into()))?;
    Ok(attachment("application/zip", &format!("{}.zip", file_name(id, "dashboard")), bytes))
}

pub async fn tile(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((id, tile_id)): Path<(String, String)>,
    Body(req): Body<TileRequest>,
) -> ApiResult<Response> {
    tile_csv(&state, &caller, &id, &tile_id, &req).await
}

pub async fn dashboard(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    Body(req): Body<TileRequest>,
) -> ApiResult<Response> {
    dashboard_zip(&state, &caller, &id, &req).await
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Read;

    use axum::http::{HeaderMap, StatusCode};
    use serde_json::{Value, json};

    use crate::api::share::tests::{clean_up, org_with};

    const DASHBOARD: &str = "title: Export sales\ntabs: [Main, Detail]\ntiles:\n  - {id: n, title: All transactions, type: kpi, query: {model: sales, measures: [transactions]}}\n  - {id: regions, type: bar, tab: Detail, drill: location, query: {model: sales, dimensions: [region], measures: [transactions]}}\n  - {id: note, type: text, text: Hello}\n";

    /// A request as the signed-in owner, viewing as `role`: (status, headers, body bytes).
    pub(crate) async fn raw(
        app: &axum::Router,
        cookie: &str,
        org: &str,
        path: &str,
        body: Value,
        view_as: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("cookie", cookie)
            .header("content-type", "application/json");
        if !org.is_empty() {
            req = req.header("x-magnum-org", org);
        }
        if let Some(r) = view_as {
            req = req.header("x-magnum-view-as", r);
        }
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, headers, bytes.to_vec())
    }

    pub(crate) fn unzip(bytes: Vec<u8>) -> Vec<(String, String)> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|i| {
                let mut f = archive.by_index(i).unwrap();
                let mut text = String::new();
                f.read_to_string(&mut text).unwrap();
                (f.name().to_string(), text)
            })
            .collect()
    }

    #[tokio::test]
    async fn users_download_what_they_see() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let emea = json!({"filters": {}, "drill": [{"field": "region", "value": "EMEA"}]});

        // a tile as CSV: labels first, the person's drill applied
        let (s, h, body) = raw(&app, &cookie, &org, "/api/dashboards/shared/tiles/regions/export", emea, None).await;
        assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        assert_eq!(h["content-type"], "text/csv; charset=utf-8");
        assert_eq!(h["content-disposition"], "attachment; filename=\"shared-regions.csv\"");
        assert_eq!(h["cache-control"], "no-store");
        let text = String::from_utf8(body).unwrap();
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').trim_end().split("\r\n").collect();
        assert_eq!(lines[0], "Store,Transactions", "drilled from regions down to stores: {text}");
        assert!(lines.len() > 2, "{text}");

        // the dashboard as a zip: a CSV per chart, in a folder per tab, no text tiles
        let (s, h, body) = raw(&app, &cookie, &org, "/api/dashboards/shared/export", json!({}), None).await;
        assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        assert_eq!(h["content-type"], "application/zip");
        assert_eq!(h["content-disposition"], "attachment; filename=\"shared.zip\"");
        let files = unzip(body);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["main/all_transactions.csv", "detail/regions.csv"]);
        assert_eq!(files[1].1.trim_end().lines().count(), 4, "three regions: {}", files[1].1);

        // viewers only look; a text tile has no data
        let (s, _, _) = raw(&app, &cookie, &org, "/api/dashboards/shared/export", json!({}), Some("viewer")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let path = "/api/dashboards/shared/tiles/regions/export";
        assert_eq!(raw(&app, &cookie, &org, path, json!({}), Some("viewer")).await.0, StatusCode::FORBIDDEN);
        let path = "/api/dashboards/shared/tiles/note/export";
        assert_eq!(raw(&app, &cookie, &org, path, json!({}), None).await.0, StatusCode::BAD_REQUEST);
        clean_up(&state, &org).await;
    }
}
