//! Compile a semantic Query against a Model into SQL, in the dialect of the model's connection.
//!
//! Only fields declared in the model can be referenced, and filter values are always emitted as
//! escaped literals, so a query spec can never inject SQL. The SQL fragments inside models
//! (`sql:` of dimensions, measures and derived tables) come from the project files, which are
//! trusted configuration, and are embedded as written (in parentheses).
//!
//! Dialects differ in identifier quoting, string escaping, type names, date truncation and date
//! arithmetic; everything else (CASE, COUNT(DISTINCT), NULLIF, GROUP BY ordinals, LIMIT) is shared.

use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sqlparser::ast::{Expr, FunctionArg, FunctionArgExpr, FunctionArguments, visit_expressions_mut};
use sqlparser::dialect::{BigQueryDialect, DuckDbDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;

use crate::period::{ResolvedPeriod, resolve_period};
use crate::schema::{ADHOC_MEASURE, Aggregation, Dimension, Filter, Measure, Model, Query, TIME_GRAINS, is_temporal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// PostgreSQL: ScramDB and Postgres
    Postgres,
    DuckDb,
    /// GoogleSQL
    BigQuery,
}

impl Dialect {
    pub fn ident(self, name: &str) -> String {
        match self {
            Dialect::BigQuery => format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`")),
            _ => ident(name),
        }
    }

    /// A string literal. Postgres and DuckDB only double quotes; GoogleSQL uses backslash escapes.
    pub fn quote(self, s: &str) -> Result<String, QueryError> {
        match self {
            Dialect::BigQuery => {
                if s.contains('\0') {
                    return err("text values can't contain NUL characters");
                }
                let escaped = s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n").replace('\r', "\\r");
                Ok(format!("'{escaped}'"))
            }
            _ => quote(s),
        }
    }

    fn double(self) -> &'static str {
        match self {
            Dialect::Postgres => "DOUBLE PRECISION",
            Dialect::DuckDb => "DOUBLE",
            Dialect::BigQuery => "FLOAT64",
        }
    }

    fn text(self) -> &'static str {
        match self {
            Dialect::BigQuery => "STRING",
            Dialect::DuckDb => "VARCHAR",
            Dialect::Postgres => "TEXT",
        }
    }

    /// The SQL type of a date/time dimension type: date, time (UTC instant), datetime (naive).
    fn temporal(self, ftype: &str) -> &'static str {
        match (self, ftype) {
            (_, "date") => "DATE",
            (Dialect::BigQuery, "time") => "TIMESTAMP",
            (Dialect::BigQuery, _) => "DATETIME",
            (_, "time") => "TIMESTAMPTZ",
            _ => "TIMESTAMP",
        }
    }

    /// Truncate a date/time column to the start of its day/week/month/... (weeks start on Monday).
    pub fn trunc(self, grain: &str, col: &str, ftype: &str) -> String {
        match self {
            Dialect::BigQuery => {
                let unit = if grain == "week" { "ISOWEEK".to_string() } else { grain.to_uppercase() };
                match ftype {
                    "date" => format!("DATE_TRUNC({col}, {unit})"),
                    "time" => format!("TIMESTAMP_TRUNC({col}, {unit}, 'UTC')"),
                    _ => format!("DATETIME_TRUNC({col}, {unit})"),
                }
            }
            _ if ftype == "date" => format!("CAST(DATE_TRUNC('{grain}', {col}) AS DATE)"),
            _ => format!("DATE_TRUNC('{grain}', {col})"),
        }
    }

    /// A date/time column moved forward by whole days, keeping its type.
    fn shift(self, col: &str, days: i64, ftype: &str) -> String {
        match (self, ftype) {
            (Dialect::BigQuery, "date") => format!("DATE_ADD({col}, INTERVAL {days} DAY)"),
            (Dialect::BigQuery, "time") => format!("TIMESTAMP_ADD({col}, INTERVAL {days} DAY)"),
            (Dialect::BigQuery, _) => format!("DATETIME_ADD({col}, INTERVAL {days} DAY)"),
            (_, "date") => format!("CAST({col} + INTERVAL '{days} days' AS DATE)"),
            _ => format!("{col} + INTERVAL '{days} days'"),
        }
    }

    /// Case-insensitive "contains"; `raw` is the searched text, LIKE wildcards not yet escaped.
    fn contains(self, target: &str, raw: &str) -> Result<String, QueryError> {
        let escaped = raw.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let pattern = self.quote(&format!("%{escaped}%"))?;
        Ok(match self {
            // GoogleSQL: backslash is LIKE's escape character already (no ESCAPE clause)
            Dialect::BigQuery => format!("LOWER(CAST({target} AS STRING)) LIKE {pattern}"),
            _ => format!("LOWER(CAST({target} AS {})) LIKE {pattern} ESCAPE '\\'", self.text()),
        })
    }

    fn parse_expr(self, sql: &str) -> Result<Expr, sqlparser::parser::ParserError> {
        match self {
            Dialect::Postgres => Parser::new(&PostgreSqlDialect {}).try_with_sql(sql)?.parse_expr(),
            Dialect::DuckDb => Parser::new(&DuckDbDialect {}).try_with_sql(sql)?.parse_expr(),
            Dialect::BigQuery => Parser::new(&BigQueryDialect {}).try_with_sql(sql)?.parse_expr(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct QueryError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError(msg.into()))
}

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    /// output alias, e.g. "created_at__month"
    pub name: String,
    /// field reference as written in the query, e.g. "created_at:month"
    pub field: String,
    pub label: String,
    /// "dimension" | "measure"
    pub kind: &'static str,
    /// dimension type, or "number" for measures
    #[serde(rename = "type")]
    pub kind_type: String,
    pub format: Option<String>,
    pub grain: Option<String>,
    /// "previous" | "change" for period-comparison columns
    pub compare: Option<&'static str>,
    /// the measure a comparison column belongs to
    pub base: Option<String>,
    pub higher_is_better: Option<bool>,
    /// measures: whether values add up across rows (sum, count), so totals mean something
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additive: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Compiled {
    pub sql: String,
    pub columns: Vec<Column>,
    /// resolved windows, for display
    pub period: Option<Value>,
}

pub fn humanize(name: &str) -> String {
    let s = name.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => s,
    }
}

pub fn split_grain(r: &str) -> (&str, Option<&str>) {
    match r.split_once(':') {
        Some((n, g)) if !g.is_empty() => (n, Some(g)),
        Some((n, _)) => (n, None),
        None => (r, None),
    }
}

// --------------------------------------------------------------------------- SQL pieces

/// A PostgreSQL identifier (also what ScramDB-side code like the cache uses).
pub fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A PostgreSQL string literal (standard_conforming_strings: only quotes need doubling).
pub fn quote(s: &str) -> Result<String, QueryError> {
    if s.contains('\0') {
        return err("text values can't contain NUL characters");
    }
    Ok(format!("'{}'", s.replace('\'', "''")))
}

/// The column (or expression) behind a dimension.
fn col_expr(dl: Dialect, d: &Dimension) -> String {
    let col = match d.sql.as_deref() {
        Some(sql) if !sql.trim().is_empty() => format!("({})", sql.trim()),
        // lowercase names quoted (same meaning, and safe for reserved words like "order");
        // mixed case is left as written, like any unquoted identifier
        _ if d.name == d.name.to_lowercase() => dl.ident(&d.name),
        _ => d.name.clone(),
    };
    // DuckDB without ICU has no time-zone arithmetic: work on instants as UTC timestamps
    if dl == Dialect::DuckDb && d.kind == "time" { format!("CAST({col} AS TIMESTAMP)") } else { col }
}

fn from_clause(dl: Dialect, model: &Model) -> Result<String, QueryError> {
    if let Some(sql) = &model.sql {
        return Ok(format!("({}) AS base", sql.trim().trim_end_matches(';')));
    }
    let table = model.table.as_deref().unwrap_or_default().trim_matches(|c| c == '`' || c == '"' || c == ' ');
    let parts: Vec<&str> = table.split('.').collect();
    if !(1..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty()) {
        return err(format!("invalid table name {table:?}"));
    }
    Ok(parts.iter().map(|p| dl.ident(p.trim_matches(|c| c == '"' || c == '`'))).collect::<Vec<_>>().join("."))
}

fn dim_expr(dl: Dialect, d: &Dimension, grain: Option<&str>, col: Option<String>) -> Result<String, QueryError> {
    let col = col.unwrap_or_else(|| col_expr(dl, d));
    let Some(grain) = grain else { return Ok(col) };
    if !is_temporal(&d.kind) {
        return err(format!("grain {grain:?} is only valid on date/time dimensions, not {:?}", d.name));
    }
    if !TIME_GRAINS.contains(&grain) {
        return err(format!("unknown grain {grain:?}; use one of {TIME_GRAINS:?}"));
    }
    if grain == "hour" && d.kind == "date" {
        return err(format!("{:?} is a date: the hour grain needs a date-time dimension", d.name));
    }
    Ok(dl.trunc(grain, &col, &d.kind))
}

/// What a dimension is grouped by. ScramDB 1.0 corrupts GROUP BY and DISTINCT on a bare NUMERIC
/// column after their first run (made-up values, extra groups); a computed value groups
/// correctly, and `x + 0` is exact for every number type. Not needed elsewhere, so only there.
fn group_key(dl: Dialect, d: &Dimension, expr: String) -> String {
    if dl == Dialect::Postgres && d.kind == "number" { format!("({expr} + 0)") } else { expr }
}

/// A measure's aggregate, optionally restricted to rows matching `scope` (period comparison):
/// SUM(x) -> SUM(CASE WHEN scope THEN x END), COUNT(*) -> COUNT(CASE WHEN scope THEN 1 END).
fn measure_expr(dl: Dialect, m: &Measure, scope: Option<&str>) -> Result<String, QueryError> {
    let inner = m.sql.as_deref().map(|s| format!("({})", s.trim()));
    if m.kind == "number" {
        let sql = m.sql.as_deref().unwrap_or_default();
        return match scope {
            None => Ok(format!("({})", sql.trim())),
            Some(cond) => scope_custom(dl, sql, cond).map(|s| format!("({s})")),
        };
    }
    let arg = match (scope, inner) {
        (None, Some(x)) => x,
        (None, None) => "*".into(),
        (Some(c), Some(x)) => format!("CASE WHEN {c} THEN {x} END"),
        (Some(c), None) => format!("CASE WHEN {c} THEN 1 END"),
    };
    Ok(match m.kind.as_str() {
        "count" => format!("COUNT({arg})"),
        "count_distinct" => format!("COUNT(DISTINCT {arg})"),
        "sum" => format!("SUM({arg})"),
        "avg" => format!("AVG({arg})"),
        "min" => format!("MIN({arg})"),
        "max" => format!("MAX({arg})"),
        other => return err(format!("unknown measure type {other:?}")),
    })
}

const AGGREGATES: [&str; 18] = [
    "sum",
    "avg",
    "min",
    "max",
    "count",
    "countif",
    "every",
    "bool_and",
    "bool_or",
    "logical_and",
    "string_agg",
    "array_agg",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "variance",
    "var_pop",
    "var_samp",
];

/// Restrict every aggregate inside a custom (`type: number`) measure to rows matching `cond`.
fn scope_custom(dl: Dialect, sql: &str, cond: &str) -> Result<String, QueryError> {
    let parse = |s: &str| -> Result<Expr, QueryError> {
        dl.parse_expr(s).map_err(|e| QueryError(format!("cannot parse model expression {s:?}: {e}")))
    };
    let mut expr = parse(sql)?;
    let mut failure = None;
    let _ = visit_expressions_mut(&mut expr, |e| {
        let Expr::Function(f) = e else { return ControlFlow::<()>::Continue(()) };
        let name = f.name.to_string().to_lowercase();
        if f.over.is_some() || !AGGREGATES.contains(&name.as_str()) {
            return ControlFlow::Continue(());
        }
        if let FunctionArguments::List(list) = &mut f.args {
            // only the first argument is the aggregated value (string_agg's second is a separator)
            if let Some(FunctionArg::Unnamed(arg)) = list.args.first_mut() {
                let then = match arg {
                    FunctionArgExpr::Expr(x) => x.to_string(),
                    _ => "1".into(), // COUNT(*)
                };
                // COUNTIF(p) counts rows where p holds: scope it as COUNTIF(cond AND (p))
                let scoped = if name == "countif" {
                    format!("{cond} AND ({then})")
                } else {
                    format!("CASE WHEN {cond} THEN {then} END")
                };
                match parse(&scoped) {
                    Ok(case) => *arg = FunctionArgExpr::Expr(case),
                    Err(e) => failure = Some(e),
                }
            }
        }
        ControlFlow::Continue(())
    });
    match failure {
        Some(e) => Err(e),
        None => Ok(expr.to_string()),
    }
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+(\.\d+)?([eE][-+]?\d+)?$").unwrap());

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn literal(dl: Dialect, v: &Value, ftype: &str) -> Result<String, QueryError> {
    // DuckDB compares instants as UTC timestamps (see col_expr)
    let ftype = if dl == Dialect::DuckDb && ftype == "time" { "datetime" } else { ftype };
    if v.is_null() {
        return Ok("NULL".into());
    }
    match ftype {
        "number" => {
            // only plain decimal numbers reach the SQL (no nan/inf/whitespace tricks)
            let text = value_text(v).trim().to_string();
            if v.is_boolean() || !NUMBER.is_match(&text) {
                return err(format!("expected a number, got {v}"));
            }
            // and one every database can hold (beyond, NUMERIC overflows in the warehouse)
            if text.parse::<f64>().map_or(true, |x| !x.is_finite() || x.abs() >= 1e30) {
                return err(format!("the number {text} is too large"));
            }
            Ok(text)
        }
        "boolean" => {
            let b = match v {
                Value::Bool(b) => *b,
                Value::String(s) => matches!(s.to_lowercase().as_str(), "true" | "1" | "yes"),
                Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
                _ => false,
            };
            Ok(if b { "TRUE" } else { "FALSE" }.into())
        }
        "date" => {
            let s = value_text(v);
            Ok(format!("CAST({} AS DATE)", dl.quote(s.get(..10).unwrap_or(&s))?))
        }
        "time" => {
            let s = value_text(v);
            // GoogleSQL wants a full offset: "+00" becomes "+00:00"
            let s = match dl {
                Dialect::BigQuery if s.ends_with("+00") || s.ends_with("-00") => format!("{s}:00"),
                _ => s,
            };
            Ok(format!("CAST({} AS {})", dl.quote(&s)?, dl.temporal("time")))
        }
        "datetime" => {
            // naive: drop any UTC offset / 'T'
            let s = value_text(v).replace('T', " ");
            let s = s.strip_suffix("+00:00").or_else(|| s.strip_suffix('Z')).unwrap_or(&s).to_string();
            let s = if s.ends_with("+00") || s.ends_with("-00") { s[..s.len() - 3].to_string() } else { s };
            Ok(format!("CAST({} AS {})", dl.quote(&s)?, dl.temporal("datetime")))
        }
        _ => dl.quote(&value_text(v)),
    }
}

fn condition(dl: Dialect, target: &str, f: &Filter, ftype: &str, now: DateTime<Utc>) -> Result<String, QueryError> {
    let vals = &f.values;
    let need = match f.op.as_str() {
        "gt" | "gte" | "lt" | "lte" | "last_n_days" | "contains" => 1,
        "between" => 2,
        _ => 0,
    };
    if vals.len() < need {
        return err(format!("filter {} {} needs {need} value(s)", f.field, f.op));
    }
    let lit = |v: &Value| literal(dl, v, ftype);
    Ok(match f.op.as_str() {
        "equals" | "not_equals" => {
            if vals.is_empty() {
                return err(format!("filter {} {} needs at least one value", f.field, f.op));
            }
            let non_null: Vec<&Value> = vals.iter().filter(|v| !v.is_null()).collect();
            let mut parts = vec![];
            if !non_null.is_empty() {
                let list = non_null.iter().map(|v| lit(v)).collect::<Result<Vec<_>, _>>()?.join(", ");
                parts.push(format!("{target} IN ({list})"));
            }
            if non_null.len() < vals.len() {
                parts.push(format!("{target} IS NULL"));
            }
            let cond = format!("({})", parts.join(" OR "));
            if f.op == "equals" { cond } else { format!("NOT {cond}") }
        }
        "contains" => dl.contains(target, &value_text(&vals[0]))?,
        "gt" => format!("{target} > {}", lit(&vals[0])?),
        "gte" => format!("{target} >= {}", lit(&vals[0])?),
        "lt" => format!("{target} < {}", lit(&vals[0])?),
        "lte" => format!("{target} <= {}", lit(&vals[0])?),
        "between" => format!("{target} BETWEEN {} AND {}", lit(&vals[0])?, lit(&vals[1])?),
        "is_null" => format!("{target} IS NULL"),
        "not_null" => format!("NOT {target} IS NULL"),
        "last_n_days" => {
            if !is_temporal(ftype) {
                return err("last_n_days only applies to date/time dimensions");
            }
            let days = match &vals[0] {
                Value::Number(n) => n.as_i64(),
                Value::String(s) => s.trim().parse().ok(),
                _ => None,
            }
            .filter(|d| (1..=36600).contains(d))
            .ok_or_else(|| QueryError("last_n_days needs a number of days between 1 and 36600".into()))?;
            let start = now - Duration::days(days);
            let v = match ftype {
                "date" => start.date_naive().to_string(),
                "datetime" => start.format("%Y-%m-%d %H:%M:%S").to_string(),
                _ => start.format("%Y-%m-%d %H:%M:%S+00").to_string(),
            };
            format!("{target} >= {}", lit(&Value::String(v))?)
        }
        other => return err(format!("unsupported filter op {other:?}")),
    })
}

// --------------------------------------------------------------------------- periods

fn ts_literal(dl: Dialect, d: DateTime<Utc>, ftype: &str) -> Result<String, QueryError> {
    let v = match ftype {
        "date" => d.date_naive().to_string(),
        "datetime" => d.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
        _ => d.format("%Y-%m-%d %H:%M:%S%.6f+00").to_string(),
    };
    literal(dl, &Value::String(v), ftype)
}

fn window(dl: Dialect, col: &str, start: DateTime<Utc>, end: DateTime<Utc>, ftype: &str) -> Result<String, QueryError> {
    Ok(format!("({col} >= {} AND {col} < {})", ts_literal(dl, start, ftype)?, ts_literal(dl, end, ftype)?))
}

/// (cur - prev) / prev as a float; NULL when prev is 0 or NULL.
fn change(dl: Dialect, cur: &str, prev: &str) -> String {
    format!("(CAST({cur} AS {}) - {prev}) / NULLIF({prev}, 0)", dl.double())
}

// --------------------------------------------------------------------------- ad-hoc measures

fn adhoc_measure(dl: Dialect, r: &str, model: &Model) -> Result<Option<Measure>, QueryError> {
    let Some(c) = ADHOC_MEASURE.captures(r) else { return Ok(None) };
    let (agg, col) = (Aggregation::parse(&c[1]).expect("regex"), &c[2]);
    let d = match model.dimension(col) {
        Some(d) if !d.hidden => d,
        _ => return err(format!("unknown column {col:?} in {}", model.label.as_deref().unwrap_or(&model.name))),
    };
    let label = d.label.clone().unwrap_or_else(|| d.name.clone());
    if matches!(agg, Aggregation::Sum | Aggregation::Avg) && d.kind != "number" {
        return err(format!("{} needs a number column; {label} isn't one (try count or count_distinct)", agg.as_str()));
    }
    static MONEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"_(usd|eur|gbp|aud|cad|dkk)$|price|amount|refund|value|cost").unwrap());
    let money = d.kind == "number"
        && matches!(agg, Aggregation::Sum | Aggregation::Avg | Aggregation::Min | Aggregation::Max)
        && MONEY.is_match(col);
    Ok(Some(Measure {
        name: format!("{}__{col}", agg.as_str()),
        label: Some(format!("{label} ({})", agg.label())),
        description: None,
        kind: agg.as_str().into(),
        sql: Some(d.sql.clone().unwrap_or_else(|| col_expr(dl, d))),
        format: if money { "currency" } else { "number" }.into(),
        higher_is_better: true,
        hidden: false,
    }))
}

/// The model as this query sees it: plus ad-hoc aggregations ("sum(col)") as ordinary measures,
/// which the query then refers to by name (so period comparison, filters and sorting just work).
pub fn expand<'m>(model: &'m Model, q: &Query, dl: Dialect) -> Result<(Cow<'m, Model>, Query), QueryError> {
    let strip = |s: &str| s.trim_start_matches(['-', '+']).to_string();
    let mut refs: Vec<String> = q.measures.clone();
    refs.extend(q.sort.iter().map(|s| strip(s)));
    refs.extend(q.filters.iter().map(|f| f.field.clone()));
    refs.extend(q.top.iter().map(|t| t.by.clone()));
    let mut extra: Vec<Measure> = vec![];
    let mut rename: Vec<(String, String)> = vec![];
    for r in refs {
        if model.measure(&r).is_some() || rename.iter().any(|(from, _)| *from == r) {
            continue;
        }
        if let Some(m) = adhoc_measure(dl, &r, model)? {
            rename.push((r, m.name.clone()));
            if !extra.iter().any(|x| x.name == m.name) {
                extra.push(m);
            }
        }
    }
    if extra.is_empty() {
        return Ok((Cow::Borrowed(model), q.clone()));
    }
    let ren = |s: &str| rename.iter().find(|(from, _)| from == s).map_or(s.to_string(), |(_, to)| to.clone());
    let mut m2 = model.clone();
    m2.measures.extend(extra);
    let mut q2 = q.clone();
    q2.measures = q.measures.iter().map(|m| ren(m)).collect();
    q2.sort = q
        .sort
        .iter()
        .map(|s| {
            let sign = if s.starts_with('-') { "-" } else { "" };
            format!("{sign}{}", ren(&strip(s)))
        })
        .collect();
    for f in &mut q2.filters {
        f.field = ren(&f.field);
    }
    if let Some(t) = &mut q2.top {
        t.by = ren(&t.by);
    }
    Ok((Cow::Owned(m2), q2))
}

// --------------------------------------------------------------------------- compile

struct PeriodParts<'a> {
    dim: &'a Dimension,
    resolved: ResolvedPeriod,
    cur: String,
    prev: Option<String>,
}

/// `mandatory`: the person's data restrictions, always applied as WHERE conditions on the
/// dimension's raw expression; unlike q.filters they are never replaced by the period window.
pub fn compile_query(
    model: &Model,
    q: &Query,
    dl: Dialect,
    now: DateTime<Utc>,
    mandatory: &[Filter],
) -> Result<Compiled, QueryError> {
    compile_query_up_to(model, q, dl, now, mandatory, crate::schema::MAX_ROWS)
}

/// compile_query for internal queries that may return up to `max_rows` (blend sources).
pub fn compile_query_up_to(
    model: &Model,
    q: &Query,
    dl: Dialect,
    now: DateTime<Utc>,
    mandatory: &[Filter],
    max_rows: i64,
) -> Result<Compiled, QueryError> {
    q.validate_up_to(max_rows).map_err(QueryError)?;
    let mut selects: Vec<String> = vec![];
    let mut columns: Vec<Column> = vec![];
    let mut aliases: Vec<(String, String)> = vec![]; // field ref (and bare name) -> output alias
    let alias_of =
        |aliases: &Vec<(String, String)>, r: &str| aliases.iter().find(|(k, _)| k == r).map(|(_, v)| v.clone());

    // ---- period: current window filter, and the previous window when comparing
    let period = match &q.period {
        None => None,
        Some(p) => {
            let dim = model.dimension(&p.field).filter(|d| is_temporal(&d.kind)).ok_or_else(|| {
                QueryError(format!("period field {:?} must be a date/time dimension of {:?}", p.field, model.name))
            })?;
            let resolved = resolve_period(&p.current, p.compare.as_ref(), &dim.kind, now).map_err(QueryError)?;
            let pcol = col_expr(dl, dim);
            let cur = window(dl, &pcol, resolved.start, resolved.end, &dim.kind)?;
            let prev = match resolved.offset_days {
                Some(o) => {
                    let sh = Duration::days(o);
                    Some(window(dl, &pcol, resolved.start - sh, resolved.end - sh, &dim.kind)?)
                }
                None => None,
            };
            Some(PeriodParts { dim, resolved, cur, prev })
        }
    };
    let comparing = period.as_ref().is_some_and(|p| p.prev.is_some());

    for r in &q.dimensions {
        let (name, grain) = split_grain(r);
        let d = model
            .dimension(name)
            .ok_or_else(|| QueryError(format!("unknown dimension {name:?} in model {:?}", model.name)))?;
        let alias = grain.map_or(name.to_string(), |g| format!("{name}__{g}"));
        let expr = match &period {
            // put previous-window rows on the current timeline so both periods share an axis
            Some(p) if comparing && p.dim.name == d.name => {
                let col = col_expr(dl, d);
                let shifted = dl.shift(&col, p.resolved.offset_days.unwrap_or(0), &d.kind);
                dim_expr(dl, d, grain, Some(format!("CASE WHEN {} THEN {col} ELSE {shifted} END", p.cur)))?
            }
            _ => group_key(dl, d, dim_expr(dl, d, grain, None)?),
        };
        selects.push(format!("{expr} AS {}", dl.ident(&alias)));
        let label =
            d.label.clone().unwrap_or_else(|| humanize(name)) + &grain.map_or(String::new(), |g| format!(" ({g})"));
        columns.push(Column {
            name: alias.clone(),
            field: r.clone(),
            label,
            kind: "dimension",
            // truncated to a day or longer it's a date; an hour is still a point in time
            kind_type: if grain.is_some_and(|g| g != "hour") { "date".into() } else { d.kind.clone() },
            format: None,
            grain: grain.map(String::from),
            compare: None,
            base: None,
            higher_is_better: None,
            additive: None,
        });
        aliases.push((r.clone(), alias.clone()));
        if alias_of(&aliases, name).is_none() {
            aliases.push((name.to_string(), alias));
        }
    }

    let mut measure_exprs: Vec<(String, String)> = vec![]; // used by HAVING: current-window value
    for name in &q.measures {
        let m = model
            .measure(name)
            .ok_or_else(|| QueryError(format!("unknown measure {name:?} in model {:?}", model.name)))?;
        let label = m.label.clone().unwrap_or_else(|| humanize(name));
        let col = |n: String, l: String, fmt: &str, compare: Option<&'static str>| Column {
            name: n.clone(),
            field: n,
            label: l,
            kind: "measure",
            kind_type: "number".into(),
            format: Some(fmt.into()),
            grain: None,
            compare,
            base: compare.map(|_| name.clone()),
            higher_is_better: Some(m.higher_is_better),
            additive: Some(compare.is_none_or(|c| c == "previous") && matches!(m.kind.as_str(), "sum" | "count")),
        };
        match &period {
            Some(PeriodParts { cur: cc, prev: Some(pc), .. }) => {
                let (cur, prev) = (measure_expr(dl, m, Some(cc))?, measure_expr(dl, m, Some(pc))?);
                selects.push(format!("{cur} AS {}", dl.ident(name)));
                selects.push(format!("{prev} AS {}", dl.ident(&format!("{name}__previous"))));
                selects.push(format!("{} AS {}", change(dl, &cur, &prev), dl.ident(&format!("{name}__change"))));
                columns.push(col(name.clone(), label.clone(), &m.format, None));
                columns.push(col(
                    format!("{name}__previous"),
                    format!("{label} (previous)"),
                    &m.format,
                    Some("previous"),
                ));
                columns.push(col(format!("{name}__change"), format!("{label} Δ%"), "percent", Some("change")));
                for suffix in ["", "__previous", "__change"] {
                    aliases.push((format!("{name}{suffix}"), format!("{name}{suffix}")));
                }
                measure_exprs.push((name.clone(), cur));
            }
            _ => {
                let e = measure_expr(dl, m, None)?;
                selects.push(format!("{e} AS {}", dl.ident(name)));
                columns.push(col(name.clone(), label, &m.format, None));
                aliases.push((name.clone(), name.clone()));
                measure_exprs.push((name.clone(), e));
            }
        }
    }

    let mut names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| w[0] == w[1]) {
        return err("the same field is selected twice");
    }

    let from = from_clause(dl, model)?;
    let mut wheres: Vec<String> = vec![];
    let mut having: Vec<String> = vec![];
    if let Some(p) = &period {
        wheres.push(match &p.prev {
            Some(prev) => format!("({} OR {prev})", p.cur),
            None => p.cur.clone(),
        });
    }
    let mut base_conds: Vec<String> = vec![]; // row filters (not the period): reused by the Top N ranking
    for f in &q.filters {
        let (name, grain) = split_grain(&f.field);
        if period.as_ref().is_some_and(|p| p.dim.name == name) {
            continue; // the period owns this field's window (e.g. a dashboard date filter)
        }
        if let Some(d) = model.dimension(name) {
            let target = dim_expr(dl, d, grain, None)?;
            base_conds.push(condition(dl, &target, f, if grain.is_some() { "date" } else { &d.kind }, now)?);
        } else if let Some(m) = model.measure(name) {
            let target = match measure_exprs.iter().find(|(n, _)| n == name) {
                Some((_, e)) => e.clone(),
                None => measure_expr(dl, m, period.as_ref().filter(|_| comparing).map(|p| p.cur.as_str()))?,
            };
            having.push(condition(dl, &target, f, "number", now)?);
        } else {
            return err(format!("unknown filter field {name:?} in model {:?}", model.name));
        }
    }
    for f in mandatory {
        match model.dimension(&f.field) {
            Some(d) if !f.values.is_empty() && f.op == "equals" => {
                base_conds.push(condition(dl, &dim_expr(dl, d, None, None)?, f, &d.kind, now)?);
            }
            _ => return err("a data restriction can't be applied to this model"), // fail closed
        }
    }
    wheres.extend(base_conds.iter().cloned());

    let mut limit = q.limit;
    let mut sort = if q.sort.is_empty() { default_sort(&columns) } else { q.sort.clone() };
    if let Some(t) = &q.top {
        let (name, grain) = split_grain(&t.dimension);
        if !q.dimensions.iter().any(|x| split_grain(x).0 == name) {
            return err(format!("top {}: {:?} must be one of the query's dimensions", t.n, t.dimension));
        }
        let d = model.dimension(name).ok_or_else(|| QueryError(format!("top {}: unknown dimension {name:?}", t.n)))?;
        dim_expr(dl, d, grain, None)?; // its grain, if any, is a real one for this field
        let m = model.measure(&t.by).ok_or_else(|| QueryError(format!("top {}: unknown measure {:?}", t.n, t.by)))?;
        let direction = if t.bottom { "" } else { "-" };
        let selected = alias_of(&aliases, &t.by).is_some();
        if q.dimensions.len() == 1 && selected {
            // one dimension: ORDER BY the measure, LIMIT N (cheapest)
            sort = vec![format!("{direction}{}", t.by)];
            limit = limit.min(t.n);
        } else {
            // more dimensions: `dimension IN (the N best values)`, ranked with the same row
            // filters, data restrictions and current period window, so every row of those N stays
            if period.as_ref().is_some_and(|p| p.dim.name == d.name) {
                return err("top N can't rank the period's own date field; rank another dimension");
            }
            let dim = group_key(dl, d, dim_expr(dl, d, grain, None)?);
            let mut conds: Vec<String> = period.iter().map(|p| p.cur.clone()).collect();
            conds.extend(base_conds.iter().cloned());
            let wh = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
            let order = if t.bottom { "" } else { " DESC" };
            wheres.push(format!(
                "{dim} IN (SELECT {dim} FROM {from}{wh} GROUP BY 1 ORDER BY {}{order} LIMIT {})",
                measure_expr(dl, m, None)?,
                t.n
            ));
            if q.sort.is_empty() && selected {
                sort = vec![format!("{direction}{}", t.by)]; // rows of the best values first
            }
        }
    }

    let mut order_by: Vec<String> = vec![];
    for s in &sort {
        let desc = s.starts_with('-');
        let r = s.trim_start_matches(['-', '+']);
        if !comparing && (r.ends_with("__previous") || r.ends_with("__change")) {
            let base = r.rsplit_once("__").map_or(r, |(b, _)| b);
            if measure_exprs.iter().any(|(n, _)| n == base) {
                continue; // comparison sort while the viewer has comparison switched off
            }
        }
        let alias =
            alias_of(&aliases, r).ok_or_else(|| QueryError(format!("cannot sort by {r:?}: it is not selected")))?;
        order_by.push(format!("{}{}", dl.ident(&alias), if desc { " DESC" } else { "" }));
    }

    let mut sql = String::from("SELECT");
    if !q.dimensions.is_empty() && q.measures.is_empty() {
        sql.push_str(" DISTINCT"); // dimensions only: distinct values
    }
    sql.push_str(&format!("\n  {}\nFROM {from}", selects.join(",\n  ")));
    if !wheres.is_empty() {
        sql.push_str(&format!("\nWHERE\n  {}", wheres.join("\n  AND ")));
    }
    if !q.dimensions.is_empty() && !q.measures.is_empty() {
        let groups: Vec<String> = (1..=q.dimensions.len()).map(|i| i.to_string()).collect();
        sql.push_str(&format!("\nGROUP BY\n  {}", groups.join(",\n  ")));
    }
    if !having.is_empty() {
        sql.push_str(&format!("\nHAVING\n  {}", having.join("\n  AND ")));
    }
    if !order_by.is_empty() {
        sql.push_str(&format!("\nORDER BY\n  {}", order_by.join(",\n  ")));
    }
    sql.push_str(&format!("\nLIMIT {limit}"));

    Ok(Compiled { sql, columns, period: period.map(|p| p.resolved.as_json(&p.dim.kind)) })
}

/// Time series by time, the other dimensions breaking ties (the same order on every engine);
/// otherwise the first measure, largest first.
fn default_sort(columns: &[Column]) -> Vec<String> {
    if let Some(t) = columns.iter().find(|c| c.kind == "dimension" && is_temporal(&c.kind_type)) {
        let others = columns.iter().filter(|c| c.kind == "dimension" && c.field != t.field).map(|c| c.field.clone());
        return std::iter::once(t.field.clone()).chain(others).collect();
    }
    columns
        .iter()
        .find(|c| c.kind == "measure" && c.compare.is_none())
        .map(|m| vec![format!("-{}", m.field)])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn model() -> Model {
        let m: Model = serde_yaml::from_str(
            r#"
name: sales
connection: scramdb
table: public.sample_sales
dimensions:
  - {name: sold_at, type: time}
  - {name: sold_on, type: date, sql: "CAST(sold_at AS DATE)"}
  - {name: region}
  - {name: store}
  - {name: qty, type: number}
  - {name: amount, type: number}
measures:
  - {name: revenue, type: sum, sql: amount, format: currency}
  - {name: transactions, type: count}
  - {name: margin, type: number, sql: "SUM(amount - cost) / NULLIF(SUM(amount), 0)"}
hierarchies:
  location: [region, store]
"#,
        )
        .unwrap();
        m.validate().unwrap();
        m
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, 7, 30, 0).unwrap()
    }

    fn q(v: Value) -> Query {
        serde_json::from_value(v).unwrap()
    }

    fn compile(v: Value) -> Result<Compiled, QueryError> {
        let m = model();
        let q = q(v);
        let (m2, q2) = expand(&m, &q, Dialect::Postgres)?;
        compile_query(&m2, &q2, Dialect::Postgres, now(), &[])
    }

    #[test]
    fn number_values_must_fit_a_database_number() {
        let f = |v: Value| {
            compile(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"],
                                         "filters": [{"field": "amount", "op": "gt", "values": [v]}]}))
        };
        assert!(f(json!(12.5)).is_ok() && f(json!("-3e5")).is_ok());
        for v in [json!("1e400"), json!(format!("1{}", "0".repeat(60))), json!("-1e31")] {
            let e = f(v.clone()).unwrap_err().0;
            assert!(e.contains("number"), "{v}: {e}");
        }
    }

    #[test]
    fn number_dimensions_are_grouped_by_value_on_scramdb() {
        // ScramDB 1.0 corrupts GROUP BY / DISTINCT on a bare NUMERIC column after its first run;
        // a computed value (x + 0, exact for every number type) groups correctly
        let c = compile(json!({"model": "sales", "dimensions": ["amount"], "measures": ["revenue"]})).unwrap();
        assert!(c.sql.contains("(\"amount\" + 0) AS \"amount\""), "{}", c.sql);
        let top = compile(json!({"model": "sales", "dimensions": ["amount", "region"], "measures": ["revenue"],
                                 "top": {"dimension": "amount", "n": 3, "by": "revenue"}}))
        .unwrap();
        assert!(!top.sql.contains("GROUP BY \"amount\""), "{}", top.sql);
        // Top N names a dimension the way the query does: no made-up grains
        let junk = compile(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"],
                                  "top": {"dimension": "region:'; --", "n": 3, "by": "revenue"}}));
        assert!(junk.is_err(), "{junk:?}");
        // filters stay on the column itself
        let f = compile(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"],
                               "filters": [{"field": "amount", "op": "gt", "values": [10]}]}))
        .unwrap();
        assert!(f.sql.contains("\"amount\" > 10"), "{}", f.sql);
        // other engines group the column as is
        let m = model();
        let q = q(json!({"model": "sales", "dimensions": ["amount"], "measures": ["revenue"]}));
        let duck = compile_query(&m, &q, Dialect::DuckDb, now(), &[]).unwrap();
        assert!(duck.sql.contains("\"amount\" AS \"amount\""), "{}", duck.sql);
    }

    #[test]
    fn group_by_with_filters_and_sort() {
        let c = compile(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"],
            "filters": [{"field": "store", "op": "equals", "values": ["Store 01", null]}],
            "sort": ["-revenue"], "limit": 10}))
        .unwrap();
        assert_eq!(
            c.sql,
            "SELECT\n  \"region\" AS \"region\",\n  SUM((amount)) AS \"revenue\"\nFROM \"public\".\"sample_sales\"\n\
             WHERE\n  (\"store\" IN ('Store 01') OR \"store\" IS NULL)\nGROUP BY\n  1\nORDER BY\n  \"revenue\" DESC\nLIMIT 10"
        );
        assert_eq!(c.columns.len(), 2);
        assert_eq!(c.columns[1].format.as_deref(), Some("currency"));
    }

    #[test]
    fn values_are_escaped_literals() {
        let c = compile(json!({"model": "sales", "dimensions": ["region"],
            "filters": [{"field": "region", "values": ["x'); DROP TABLE users; --"]}]}))
        .unwrap();
        assert!(c.sql.contains("'x''); DROP TABLE users; --'"));
        assert!(c.sql.starts_with("SELECT DISTINCT"));
        let e = compile(json!({"model": "sales", "measures": ["revenue"],
            "filters": [{"field": "qty", "op": "gt", "values": ["1 OR 1=1"]}]}))
        .unwrap_err();
        assert!(e.0.contains("expected a number"));
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(compile(json!({"model": "sales", "dimensions": ["password_hash"]})).is_err());
        assert!(compile(json!({"model": "sales", "measures": ["revenue"], "sort": ["region"]})).is_err());
    }

    #[test]
    fn time_grains() {
        let c = compile(
            json!({"model": "sales", "dimensions": ["sold_at:month", "sold_on:week"], "measures": ["transactions"]}),
        )
        .unwrap();
        assert!(c.sql.contains("DATE_TRUNC('month', \"sold_at\") AS \"sold_at__month\""));
        assert!(c.sql.contains("CAST(DATE_TRUNC('week', (CAST(sold_at AS DATE))) AS DATE) AS \"sold_on__week\""));
        assert!(c.sql.contains("COUNT(*)"));
        assert!(c.sql.contains("ORDER BY\n  \"sold_at__month\""), "time series sort by time: {}", c.sql);
        assert!(compile(json!({"model": "sales", "dimensions": ["region:month"]})).is_err());
    }

    #[test]
    fn internal_queries_may_return_more_rows() {
        let big = q(json!({"model": "sales", "dimensions": ["store"], "measures": ["revenue"], "limit": 1_000_000}));
        assert!(compile_query(&model(), &big, Dialect::Postgres, now(), &[]).unwrap_err().0.contains("limit"));
        let c = compile_query_up_to(&model(), &big, Dialect::Postgres, now(), &[], 1_000_000).unwrap();
        assert!(c.sql.ends_with("LIMIT 1000000"));
    }

    #[test]
    fn hour_grain_on_times_only() {
        let c =
            compile(json!({"model": "sales", "dimensions": ["sold_at:hour"], "measures": ["transactions"]})).unwrap();
        assert!(c.sql.contains("DATE_TRUNC('hour', \"sold_at\") AS \"sold_at__hour\""), "{}", c.sql);
        assert_eq!(c.columns[0].kind_type, "time", "an hour is still a point in time, not a date");
        assert!(compile(json!({"model": "sales", "dimensions": ["sold_on:hour"]})).unwrap_err().0.contains("hour"));
        assert_eq!(Dialect::BigQuery.trunc("hour", "t", "datetime"), "DATETIME_TRUNC(t, HOUR)");
        assert_eq!(Dialect::DuckDb.trunc("hour", "t", "time"), "DATE_TRUNC('hour', t)");
    }

    #[test]
    fn period_comparison() {
        let c = compile(json!({"model": "sales", "dimensions": ["sold_at:day"], "measures": ["revenue", "margin"],
            "period": {"field": "sold_at", "current": {"last_n_days": 7}, "compare": "previous_period"}}))
        .unwrap();
        let cur = "(\"sold_at\" >= CAST('2026-10-09 00:00:00.000000+00' AS TIMESTAMPTZ) AND \"sold_at\" < CAST('2026-10-15 07:30:00.000000+00' AS TIMESTAMPTZ))";
        assert!(c.sql.contains(&format!("SUM(CASE WHEN {cur} THEN (amount) END) AS \"revenue\"")), "{}", c.sql);
        assert!(c.sql.contains("AS \"revenue__previous\""));
        assert!(c.sql.contains("NULLIF("));
        assert!(c.sql.contains("INTERVAL '7 days'"), "previous rows shifted onto the current axis");
        // custom aggregates are scoped too
        assert!(c.sql.contains("SUM(CASE WHEN"), "{}", c.sql);
        assert!(!c.sql.contains("SUM(amount - cost) /"), "number measure must be rewritten: {}", c.sql);
        let names: Vec<_> = c.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "sold_at__day",
                "revenue",
                "revenue__previous",
                "revenue__change",
                "margin",
                "margin__previous",
                "margin__change"
            ]
        );
        assert_eq!(c.period.unwrap()["offset_days"], json!(7));
    }

    #[test]
    fn adhoc_aggregations() {
        let c = compile(
            json!({"model": "sales", "dimensions": ["region"], "measures": ["sum(amount)", "count_distinct(store)"],
            "sort": ["-sum(amount)"]}),
        )
        .unwrap();
        assert!(c.sql.contains("SUM((\"amount\")) AS \"sum__amount\""), "{}", c.sql);
        assert!(c.sql.contains("COUNT(DISTINCT (\"store\")) AS \"count_distinct__store\""));
        assert!(c.sql.contains("\"sum__amount\" DESC"));
        assert_eq!(c.columns[1].format.as_deref(), Some("currency"));
        assert!(compile(json!({"model": "sales", "measures": ["sum(region)"]})).is_err());
    }

    #[test]
    fn top_n() {
        let c = compile(json!({"model": "sales", "dimensions": ["store"], "measures": ["revenue"],
            "top": {"dimension": "store", "n": 5, "by": "revenue"}}))
        .unwrap();
        assert!(c.sql.ends_with("ORDER BY\n  \"revenue\" DESC\nLIMIT 5"));
        let c = compile(json!({"model": "sales", "dimensions": ["store", "sold_at:month"], "measures": ["revenue"],
            "top": {"dimension": "store", "n": 3, "by": "revenue", "bottom": true}}))
        .unwrap();
        assert!(c.sql.contains("\"store\" IN (SELECT \"store\" FROM \"public\".\"sample_sales\" GROUP BY 1 ORDER BY SUM((amount)) LIMIT 3)"), "{}", c.sql);
    }

    #[test]
    fn mandatory_filters_fail_closed() {
        let m = model();
        let query = q(json!({"model": "sales", "measures": ["revenue"]}));
        let c = compile_query(&m, &query, Dialect::Postgres, now(), &[Filter::equals("region", vec![json!("EMEA")])])
            .unwrap();
        assert!(c.sql.contains("(\"region\" IN ('EMEA'))"));
        assert!(
            compile_query(&m, &query, Dialect::Postgres, now(), &[Filter::equals("tenant", vec![json!("x")])]).is_err()
        );
        assert!(compile_query(&m, &query, Dialect::Postgres, now(), &[Filter::equals("region", vec![])]).is_err());
    }

    #[test]
    fn contains_escapes_like_wildcards() {
        let c = compile(json!({"model": "sales", "dimensions": ["store"], "filters": [{"field": "store", "op": "contains", "values": ["50%_O'k"]}]})).unwrap();
        assert!(c.sql.contains(r"LIKE '%50\%\_o''k%' ESCAPE '\'"), "{}", c.sql);
    }

    fn compile_in(v: Value, dl: Dialect) -> Compiled {
        let m = model();
        let q = q(v);
        let (m2, q2) = expand(&m, &q, dl).unwrap();
        compile_query(&m2, &q2, dl, now(), &[Filter::equals("region", vec![json!("EMEA")])]).unwrap()
    }

    #[test]
    fn bigquery_dialect() {
        let c = compile_in(
            json!({"model": "sales", "dimensions": ["sold_at:week", "sold_on:month"], "measures": ["revenue", "margin", "sum(qty)"],
                   "filters": [{"field": "store", "op": "contains", "values": ["It's 50%"]}],
                   "period": {"field": "sold_at", "current": {"last_n_days": 7}, "compare": "previous_year"},
                   "sort": ["-revenue__change"]}),
            Dialect::BigQuery,
        );
        let sql = &c.sql;
        for part in [
            "TIMESTAMP_TRUNC(CASE WHEN",
            "TIMESTAMP_ADD(`sold_at`, INTERVAL 364 DAY)",
            "ISOWEEK, 'UTC') AS `sold_at__week`",
            "DATE_TRUNC((CAST(sold_at AS DATE)), MONTH) AS `sold_on__month`",
            "CAST('2026-10-09 00:00:00.000000+00:00' AS TIMESTAMP)",
            "AS FLOAT64)",
            "THEN (`qty`) END) AS `sum__qty`",
            "FROM `public`.`sample_sales`",
            r"LOWER(CAST(`store` AS STRING)) LIKE '%it\'s 50\\%%'",
            "(`region` IN ('EMEA'))",
            "`revenue__change` DESC",
        ] {
            assert!(sql.contains(part), "missing {part:?} in\n{sql}");
        }
        assert!(!sql.contains('"') && !sql.contains("ESCAPE") && !sql.contains("TIMESTAMPTZ"), "{sql}");
        assert!(!sql.contains("SUM(amount - cost) /"), "custom measure scoped: {sql}");
    }

    #[test]
    fn duckdb_dialect() {
        let c = compile_in(
            json!({"model": "sales", "dimensions": ["sold_at:month"], "measures": ["revenue"],
                   "filters": [{"field": "store", "op": "contains", "values": ["o'k"]}],
                   "period": {"field": "sold_at", "current": {"last_n_days": 7}, "compare": "previous_period"}}),
            Dialect::DuckDb,
        );
        // no ICU in this DuckDB: instants are compared and truncated as UTC timestamps
        for part in [
            "DATE_TRUNC('month', CASE WHEN (CAST(&quot;sold_at&quot; AS TIMESTAMP) >= CAST('2026-10-09 00:00:00.000000' AS TIMESTAMP)",
            "CAST(&quot;sold_at&quot; AS TIMESTAMP) + INTERVAL '7 days'",
            "AS DOUBLE)",
            r"LOWER(CAST(&quot;store&quot; AS VARCHAR)) LIKE '%o''k%' ESCAPE '\'",
        ] {
            let part = part.replace("&quot;", "\"");
            assert!(c.sql.contains(&part), "missing {part:?} in\n{}", c.sql);
        }
    }

    #[test]
    fn bigquery_strings_escape_backslashes_and_newlines() {
        assert_eq!(Dialect::BigQuery.quote("a\\b'c\nd").unwrap(), r"'a\\b\'c\nd'");
        assert_eq!(Dialect::BigQuery.ident("we`ird"), r"`we\`ird`");
        assert_eq!(Dialect::Postgres.quote("a\\b'c").unwrap(), r"'a\b''c'");
    }

    #[test]
    fn default_order_is_deterministic() {
        // time first, then the other dimensions break ties (same order on every engine)
        let c = compile(json!({"model": "sales", "dimensions": ["sold_at:week", "region"], "measures": ["revenue"]}))
            .unwrap();
        assert!(c.sql.contains("ORDER BY\n  \"sold_at__week\",\n  \"region\""), "{}", c.sql);
    }

    #[test]
    fn measures_say_whether_they_add_up() {
        let c = compile(
            json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue", "transactions", "margin"]}),
        )
        .unwrap();
        let additive: Vec<(&str, Option<bool>)> = c.columns.iter().map(|c| (c.name.as_str(), c.additive)).collect();
        assert_eq!(
            additive,
            [("region", None), ("revenue", Some(true)), ("transactions", Some(true)), ("margin", Some(false))]
        );
    }

    #[test]
    fn formulas_are_not_supported_yet() {
        let e = compile(json!({"model": "sales", "measures": ["revenue"], "formulas": [{"name": "x", "expr": "1"}]}))
            .unwrap_err();
        assert!(e.0.contains("formulas"));
    }
}
