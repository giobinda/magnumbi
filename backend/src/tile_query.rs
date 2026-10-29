//! Run a SAVED dashboard tile with only the inputs a dashboard viewer can change.
//!
//! Roles without `explore` never send queries. They ask for (dashboard, tile) plus:
//! - values for the filters the dashboard declares (nothing else can be filtered),
//! - a different current/compare window for the dashboard's period (same field),
//! - drill clicks (roles with `drill` only), replayed with the tile's own drill path.
//!
//! The server rebuilds the query from the saved definition, so the model, fields and measures
//! can't be changed from the browser.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::compiler::split_grain;
use crate::schema::{Compare, Dashboard, Drill, Filter, Model, Period, PeriodRange, Query, Tile, is_temporal};

const MAX_VALUES: usize = 500;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrillClick {
    /// the clicked column as shown, e.g. "country" or "created_at:month"
    pub field: String,
    pub value: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TilePeriod {
    pub current: PeriodRange,
    #[serde(default)]
    pub compare: Option<Compare>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileRequest {
    /// "model.field" -> values
    #[serde(default)]
    pub filters: BTreeMap<String, Vec<Value>>,
    /// window for the dashboard's period (its field is fixed)
    #[serde(default)]
    pub period: Option<TilePeriod>,
    #[serde(default)]
    pub drill: Vec<DrillClick>,
}

#[derive(Debug, PartialEq)]
pub enum TileRequestError {
    Invalid(String),
    /// a drill click from a role without `drill`
    NoDrill,
}

fn invalid<T>(msg: String) -> Result<T, TileRequestError> {
    Err(TileRequestError::Invalid(msg))
}

fn finer(grain: &str) -> Option<&'static str> {
    match grain {
        "year" => Some("quarter"),
        "quarter" => Some("month"),
        "month" | "week" => Some("day"),
        _ => None,
    }
}

fn drill_path(tile: &Tile, model: &Model) -> Vec<String> {
    match &tile.drill {
        None => vec![],
        Some(Drill::Path(p)) => p.clone(),
        Some(Drill::Hierarchy(h)) => model.hierarchies.get(h).cloned().unwrap_or_default(),
    }
}

fn parse_instant(v: &str) -> Option<DateTime<Utc>> {
    let v = v.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(&v.replace(' ', "T")) {
        return Some(d.with_timezone(&Utc));
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%#z"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(v, f) {
            return Some(Utc.from_utc_datetime(&d));
        }
    }
    let day = chrono::NaiveDate::parse_from_str(v.get(..10)?, "%Y-%m-%d").ok()?;
    Some(Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0)?))
}

/// [start, end) of the bucket starting at `value`, one `grain` long.
fn bucket(value: &str, grain: &str) -> Option<(String, String)> {
    let d = parse_instant(value)?;
    let first = |y: i32, m: u32| Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0).single();
    let (y, m) = (d.year(), d.month());
    let end = match grain {
        "year" => first(y + 1, 1)?,
        "quarter" => first(y + ((m + 2) / 12) as i32, (m + 2) % 12 + 1)?,
        "month" => first(y + (m / 12) as i32, m % 12 + 1)?,
        _ => d + Duration::days(7),
    };
    let fmt = "%Y-%m-%d %H:%M:%S+00";
    Some((d.format(fmt).to_string(), end.format(fmt).to_string()))
}

/// A dashboard-level field (the period's, a filter's), written on model `owner`, as the tile's
/// model names it: the field itself, or `owner.field` on a blend that contains `owner`.
pub fn tile_field(q_model: &str, model: &Model, owner: &str, field: &str) -> Option<String> {
    if q_model == owner {
        return Some(field.to_string());
    }
    let name = format!("{owner}.{field}");
    model.dimension(&name).is_some().then_some(name)
}

pub fn build(
    dash: &Dashboard,
    tile: &Tile,
    model: &Model,
    req: &TileRequest,
    allow_drill: bool,
) -> Result<Query, TileRequestError> {
    let mut q =
        tile.query.clone().ok_or_else(|| TileRequestError::Invalid(format!("tile {} has no query", tile.id)))?;

    // period: the tile's own, else the dashboard's (with the viewer's window), unless opted out
    let opted_out = tile.options.get("period") == Some(&Value::Bool(false));
    if q.period.is_none()
        && !opted_out
        && let Some(p) = dash.period.as_ref()
        && let Some(field) = tile_field(&q.model, model, &p.model, &p.field)
    {
        let (current, compare) = match &req.period {
            Some(w) => (w.current.clone(), w.compare.clone()),
            None => (p.current.clone(), p.compare.clone()),
        };
        q.period = Some(Period { field, current, compare });
    }
    let period_field = q.period.as_ref().map(|p| p.field.clone());

    // dashboard filters only: unknown keys are refused rather than ignored
    let declared: BTreeMap<String, _> = dash.filters.iter().map(|f| (format!("{}.{}", f.model, f.field), f)).collect();
    let unknown: Vec<&String> = req.filters.keys().filter(|k| !declared.contains_key(*k)).collect();
    if !unknown.is_empty() {
        return invalid(format!("not a filter of this dashboard: {unknown:?}"));
    }
    // those the tile ignores (its author's choice: dashboard filters only, never restrictions)
    let ignored = |key: &str| {
        tile.options.get("ignore_filters").and_then(Value::as_array).is_some_and(|a| a.iter().any(|k| k == key))
    };
    for (key, f) in &declared {
        if ignored(key) {
            continue;
        }
        let Some(field) = tile_field(&q.model, model, &f.model, &f.field) else { continue };
        let values = req.filters.get(key).unwrap_or(&f.default);
        if values.len() > MAX_VALUES {
            return invalid(format!("too many values for {key}"));
        }
        if !values.is_empty() || f.op == "is_null" || f.op == "not_null" {
            q.filters.push(Filter { field, op: f.op.clone(), values: values.clone() });
        }
    }

    // drill: replay clicks with the tile's own drill path / finer time grains
    if !req.drill.is_empty() && !allow_drill {
        return Err(TileRequestError::NoDrill);
    }
    if req.drill.len() > 10 {
        return invalid("at most 10 drill steps".into());
    }
    let path = drill_path(tile, model);
    for click in &req.drill {
        if !q.dimensions.contains(&click.field) {
            return invalid(format!("{:?} is not shown in this tile", click.field));
        }
        let (name, grain) = split_grain(&click.field);
        let (name, grain) = (name.to_string(), grain.map(String::from));
        let i = path.iter().position(|p| *p == name);
        if let Some(i) = i.filter(|i| i + 1 < path.len()) {
            let next = path[i + 1].clone();
            q.dimensions =
                q.dimensions.iter().map(|d| if *d == click.field { next.clone() } else { d.clone() }).collect();
            q.filters.push(Filter::equals(&name, vec![click.value.clone()]));
        } else if let (Some(g), Some(Value::String(v))) = (grain.as_deref().and_then(finer), Some(&click.value)) {
            if period_field.as_deref() == Some(name.as_str()) {
                return invalid(format!("{:?} can't be drilled into in this tile", click.field));
            }
            if !model.dimension(&name).is_some_and(|d| is_temporal(&d.kind)) {
                return invalid(format!("can't drill into {:?}", click.field));
            }
            let (start, end) = bucket(v, grain.as_deref().unwrap_or_default())
                .ok_or_else(|| TileRequestError::Invalid(format!("not a date: {v:?}")))?;
            q.dimensions = q
                .dimensions
                .iter()
                .map(|d| if *d == click.field { format!("{name}:{g}") } else { d.clone() })
                .collect();
            q.filters.push(Filter { field: name.clone(), op: "gte".into(), values: vec![Value::String(start)] });
            q.filters.push(Filter { field: name.clone(), op: "lt".into(), values: vec![Value::String(end)] });
        } else {
            return invalid(format!("{:?} can't be drilled into in this tile", click.field));
        }
        q.sort.retain(|s| s.trim_start_matches(['-', '+']) != click.field);
        // drilling into one of the top N: show everything inside it, not the top N of the next level
        if q.top.as_ref().is_some_and(|t| t.dimension == click.field) {
            q.top = None;
        }
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixtures() -> (Dashboard, Model) {
        let d: Dashboard = serde_yaml::from_str(
            r#"
title: t
period: {model: sales, field: sold_at, current: {last_n_days: 30}}
filters: [{model: sales, field: category, default: [Shoes]}]
tiles:
  - {id: geo, type: bar, drill: location, query: {model: sales, dimensions: [region], measures: [revenue]}}
  - {id: trend, type: line, query: {model: sales, dimensions: ["sold_at:month"], measures: [revenue]}, options: {period: false}}
"#,
        )
        .unwrap();
        let m: Model = serde_yaml::from_str(
            "name: sales\nconnection: c\ntable: t\ndimensions: [{name: sold_at, type: time}, {name: region}, {name: store}, {name: category}]\n\
             measures: [{name: revenue, type: sum, sql: amount}]\nhierarchies: {location: [region, store]}",
        )
        .unwrap();
        (d, m)
    }

    #[test]
    fn filters_period_and_drill() {
        let (d, m) = fixtures();
        let req: TileRequest = serde_json::from_value(json!({
            "filters": {"sales.category": ["Bags"]},
            "period": {"current": {"last_n_days": 7}, "compare": "previous_year"},
            "drill": [{"field": "region", "value": "EMEA"}]}))
        .unwrap();
        let q = build(&d, &d.tiles[0], &m, &req, true).unwrap();
        assert_eq!(q.dimensions, ["store"]);
        assert_eq!(q.filters.len(), 2);
        assert_eq!(q.filters[0].values, [json!("Bags")]);
        assert_eq!(q.period.unwrap().current.last_n_days, Some(7));
        assert_eq!(build(&d, &d.tiles[0], &m, &req, false).unwrap_err(), TileRequestError::NoDrill);
    }

    #[test]
    fn time_drill_narrows_the_bucket() {
        let (d, m) = fixtures();
        let req: TileRequest = serde_json::from_value(
            json!({"drill": [{"field": "sold_at:month", "value": "2026-12-01T00:00:00+00:00"}]}),
        )
        .unwrap();
        let q = build(&d, &d.tiles[1], &m, &req, true).unwrap();
        assert!(q.period.is_none(), "opted out of the dashboard period");
        assert_eq!(q.dimensions, ["sold_at:day"]);
        assert_eq!(q.filters[1].values, [json!("2026-12-01 00:00:00+00")]);
        assert_eq!(q.filters[2].values, [json!("2027-01-01 00:00:00+00")]);
    }

    #[test]
    fn blends_get_their_models_period_and_filters() {
        let (mut d, m) = fixtures();
        let targets: Model =
            serde_yaml::from_str("name: targets\nconnection: c\ntable: u\ndimensions: [{name: region}]").unwrap();
        let blend: crate::blend::Blend =
            serde_yaml::from_str("name: st\nbase: sales\njoins: [{model: targets, on: [region = region]}]").unwrap();
        let bm = blend.as_model(&BTreeMap::from([("sales".to_string(), m), ("targets".to_string(), targets)]));
        d.tiles[0].query = serde_json::from_value(json!({"model": "st", "dimensions": ["sales.region"],
            "measures": ["sales.revenue"]}))
        .unwrap();
        d.tiles[0].drill = None;
        let q = build(&d, &d.tiles[0], &bm, &TileRequest::default(), true).unwrap();
        assert_eq!(q.period.unwrap().field, "sales.sold_at");
        assert_eq!(q.filters.len(), 1);
        assert_eq!((q.filters[0].field.as_str(), &q.filters[0].values), ("sales.category", &vec![json!("Shoes")]));
    }

    #[test]
    fn undeclared_filters_are_refused() {
        let (d, m) = fixtures();
        let req: TileRequest = serde_json::from_value(json!({"filters": {"sales.store": ["x"]}})).unwrap();
        assert!(matches!(build(&d, &d.tiles[0], &m, &req, true), Err(TileRequestError::Invalid(_))));
        let req: TileRequest = serde_json::from_value(json!({"drill": [{"field": "store", "value": "x"}]})).unwrap();
        assert!(build(&d, &d.tiles[0], &m, &req, true).is_err(), "only fields shown in the tile");
    }

    #[test]
    fn a_tile_may_ignore_dashboard_filters() {
        let (mut d, m) = fixtures();
        d.filters.push(serde_yaml::from_str("{model: sales, field: region}").unwrap());
        d.tiles[0].options.insert("ignore_filters".into(), json!(["sales.category"]));
        let req = TileRequest {
            filters: BTreeMap::from([
                ("sales.category".into(), vec![json!("Bags")]),
                ("sales.region".into(), vec![json!("EMEA")]),
            ]),
            ..Default::default()
        };
        // the tile that ignores the category: the region only, the viewer's category or not
        let q = build(&d, &d.tiles[0], &m, &req, true).unwrap();
        assert_eq!(q.filters.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(), ["region"]);
        let q = build(&d, &d.tiles[0], &m, &TileRequest::default(), true).unwrap();
        assert!(q.filters.is_empty(), "nor the category's default: {:?}", q.filters);
        // the others follow every filter
        let q = build(&d, &d.tiles[1], &m, &req, true).unwrap();
        assert_eq!(q.filters.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(), ["category", "region"]);
    }
}
