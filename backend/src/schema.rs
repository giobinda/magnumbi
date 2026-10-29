//! Serde schemas for the YAML project files (connections, models, dashboards) and for the query
//! API. These are the single source of truth for the file formats: unknown keys are refused.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub static NAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());
pub const TIME_GRAINS: [&str; 6] = ["hour", "day", "week", "month", "quarter", "year"];
pub const TEMPORAL: [&str; 3] = ["date", "time", "datetime"];
/// An ad-hoc aggregation of a column, written where a measure goes: "sum(sale_price)".
pub static ADHOC_MEASURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(sum|avg|min|max|count|count_distinct)\(([A-Za-z_][A-Za-z0-9_]*)\)$").unwrap());

pub fn check_name(v: &str) -> Result<(), String> {
    if NAME_RE.is_match(v) { Ok(()) } else { Err(format!("invalid name {v:?}: use letters, digits and underscores")) }
}

pub fn is_temporal(t: &str) -> bool {
    TEMPORAL.contains(&t)
}

fn default_true() -> bool {
    true
}

// --------------------------------------------------------------------------- semantic models

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Sum,
    Avg,
    Min,
    Max,
    Count,
    CountDistinct,
}

impl Aggregation {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sum" => Self::Sum,
            "avg" => Self::Avg,
            "min" => Self::Min,
            "max" => Self::Max,
            "count" => Self::Count,
            "count_distinct" => Self::CountDistinct,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
            Self::Count => "count",
            Self::CountDistinct => "count_distinct",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sum => "Sum",
            Self::Avg => "Average",
            Self::Min => "Min",
            Self::Max => "Max",
            Self::Count => "Count",
            Self::CountDistinct => "Unique count",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dimension {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// string | number | boolean | date | time (UTC instant) | datetime (naive local date-time)
    #[serde(default = "default_dim_type", rename = "type")]
    pub kind: String,
    /// SQL expression over the model's base; defaults to the column `name`
    #[serde(default)]
    pub sql: Option<String>,
    /// a lookup's column (`from` its `as`), not the main table's
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// the column, when it isn't named as the field (two lookups may both have a `name`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// or a formula on the row's other fields (no SQL): e.g. `amount * qty`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    /// how the column is aggregated when used as a number (None: sum for numbers)
    #[serde(default)]
    pub aggregation: Option<Aggregation>,
    /// other words people use for it (search, agents); organisations add theirs (governance.rs)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    /// classification such as pii or finance: information only
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn default_dim_type() -> String {
    "string".into()
}

pub const DIM_TYPES: [&str; 6] = ["string", "number", "boolean", "date", "time", "datetime"];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Measure {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// count | count_distinct | sum | avg | min | max | number (a full aggregate expression) |
    /// formula (a calculation on other measures, in the formula language)
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub sql: Option<String>,
    /// for type formula: e.g. `(revenue - cost) / revenue`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(default = "default_format")]
    pub format: String,
    /// colours period-over-period changes (e.g. false for a return rate)
    #[serde(default = "default_true")]
    pub higher_is_better: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn default_format() -> String {
    "number".into()
}

pub const MEASURE_TYPES: [&str; 8] = ["count", "count_distinct", "sum", "avg", "min", "max", "number", "formula"];
/// The most formulas one query may add.
pub const MAX_FORMULAS: usize = 50;
pub const FORMATS: [&str; 3] = ["number", "currency", "percent"];

/// A lookup a model joins: one row per key, so the main table's numbers are never counted twice;
/// rows without a match are kept (LEFT JOIN).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    /// schema.table, on the model's connection
    pub table: String,
    /// how its fields name it (`from`)
    #[serde(rename = "as")]
    pub alias: String,
    /// the columns that match: [the main table's, the lookup's], each pair equal
    pub on: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// inherit connection, table/sql, fields and hierarchies from another model and add to them
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub connection: String,
    /// fully qualified table, e.g. schema.table
    #[serde(default)]
    pub table: Option<String>,
    /// or a derived table (SELECT ...)
    #[serde(default)]
    pub sql: Option<String>,
    #[serde(default)]
    pub dimensions: Vec<Dimension>,
    #[serde(default)]
    pub measures: Vec<Measure>,
    /// named drill paths, e.g. location: [region, store]
    #[serde(default)]
    pub hierarchies: BTreeMap<String, Vec<String>>,
    /// lookups: tables of the same connection with one row per key (customers for orders),
    /// joined to the main table, whose columns are dimensions (`from`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joins: Vec<Join>,
    /// how long results may be reused ("15m", "6h", 0/off). Parsed now, used once caching lands.
    #[serde(default, deserialize_with = "duration")]
    pub cache: Option<u64>,
    /// classification such as finance: information only
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Model {
    /// Lookups: each a name of its own (not `base`, the main table's), the columns to match, all
    /// plain column names; a field from a lookup names one, and reads a column (no SQL).
    fn check_joins(&self) -> Result<(), String> {
        let at = |what: String| Err(format!("model {:?}: {what}", self.name));
        if !self.joins.is_empty() && self.table.is_none() {
            return at("lookups join a table: use `table`, not `sql`".into());
        }
        for (i, j) in self.joins.iter().enumerate() {
            if !NAME_RE.is_match(&j.alias) || j.alias.eq_ignore_ascii_case("base") {
                return at(format!("lookup {:?}: name it with letters, digits and _ (not \"base\")", j.alias));
            }
            if self.joins[..i].iter().any(|x| x.alias == j.alias) {
                return at(format!("lookup {:?} is named twice", j.alias));
            }
            if j.on.is_empty() {
                return at(format!("lookup {:?}: say which columns match (on)", j.alias));
            }
            for pair in &j.on {
                if pair.len() != 2 {
                    return at(format!(
                        "lookup {:?}: each match is a pair [main table's column, lookup's column]",
                        j.alias
                    ));
                }
                if let Some(bad) = pair.iter().find(|c| !NAME_RE.is_match(c)) {
                    return at(format!("lookup {:?}: {bad:?} isn't a column name", j.alias));
                }
            }
        }
        for d in &self.dimensions {
            if let Some(c) = d.column.as_deref().filter(|c| !NAME_RE.is_match(c)) {
                return at(format!("dimension {:?}: {c:?} isn't a column name", d.name));
            }
            let Some(from) = &d.from else { continue };
            if !self.joins.iter().any(|j| &j.alias == from) {
                return at(format!("dimension {:?}: there's no lookup {from:?}", d.name));
            }
            if d.sql.is_some() || d.formula.is_some() {
                return at(format!(
                    "dimension {:?}: a lookup's field is one of its columns (no SQL, no formula)",
                    d.name
                ));
            }
        }
        Ok(())
    }

    pub fn dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.iter().find(|d| d.name == name)
    }

    pub fn measure(&self, name: &str) -> Option<&Measure> {
        self.measures.iter().find(|m| m.name == name)
    }

    /// Field-level checks (the project loader checks connections and `extends`).
    pub fn validate(&self) -> Result<(), String> {
        check_name(&self.name)?;
        crate::governance::check_tags("the model's tags", &self.tags)?;
        for (name, synonyms, tags) in self
            .dimensions
            .iter()
            .map(|d| (&d.name, &d.synonyms, &d.tags))
            .chain(self.measures.iter().map(|m| (&m.name, &m.synonyms, &m.tags)))
        {
            crate::governance::check_synonyms(&format!("{name}'s synonyms"), synonyms)?;
            crate::governance::check_tags(&format!("{name}'s tags"), tags)?;
        }
        for d in &self.dimensions {
            check_name(&d.name)?;
            if d.formula.is_some() && d.sql.is_some() {
                return Err(format!("dimension {:?}: set `formula` or `sql`, not both", d.name));
            }
            if !DIM_TYPES.contains(&d.kind.as_str()) {
                return Err(format!("dimension {:?}: unknown type {:?}", d.name, d.kind));
            }
        }
        for m in &self.measures {
            check_name(&m.name)?;
            if !MEASURE_TYPES.contains(&m.kind.as_str()) {
                return Err(format!("measure {:?}: unknown type {:?}", m.name, m.kind));
            }
            if !FORMATS.contains(&m.format.as_str()) {
                return Err(format!("measure {:?}: unknown format {:?}", m.name, m.format));
            }
            if m.kind == "formula" {
                if m.formula.as_deref().is_none_or(str::is_empty) || m.sql.is_some() {
                    return Err(format!("measure {:?} of type formula needs `formula` (and no `sql`)", m.name));
                }
            } else if m.formula.is_some() {
                return Err(format!("measure {:?}: `formula` is for measures of type formula", m.name));
            } else if m.kind != "count" && m.sql.as_deref().is_none_or(str::is_empty) {
                return Err(format!("measure {:?} of type {} needs `sql`", m.name, m.kind));
            }
        }
        if self.extends.is_some() {
            return Ok(()); // completed from the base model by the project loader
        }
        if self.connection.is_empty() {
            return Err(format!("model {:?}: `connection` is required", self.name));
        }
        if self.table.is_some() == self.sql.is_some() {
            return Err(format!("model {:?}: set exactly one of `table` or `sql`", self.name));
        }
        let mut names: Vec<&str> = self.dimensions.iter().map(|d| d.name.as_str()).collect();
        names.extend(self.measures.iter().map(|m| m.name.as_str()));
        let mut dupes: Vec<&str> =
            names.iter().copied().filter(|n| names.iter().filter(|x| *x == n).count() > 1).collect();
        dupes.sort_unstable();
        dupes.dedup();
        if !dupes.is_empty() {
            return Err(format!("model {:?}: duplicate field names {dupes:?}", self.name));
        }
        self.check_joins()?;
        crate::compiler::check_formulas(self).map_err(|e| e.0)?;
        for (h, path) in &self.hierarchies {
            let missing: Vec<&String> = path.iter().filter(|p| self.dimension(p).is_none()).collect();
            if !missing.is_empty() {
                return Err(format!("hierarchy {h:?} references unknown dimensions {missing:?}"));
            }
        }
        Ok(())
    }
}

/// Seconds from 900, "900", "15m", "6h", "1d"; 0 for off/false/no.
pub fn parse_duration(v: &serde_yaml::Value) -> Result<Option<u64>, String> {
    static DURATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\s*(s|m|h|d)?$").unwrap());
    let seconds = match v {
        serde_yaml::Value::Null => return Ok(None),
        serde_yaml::Value::Bool(false) => 0,
        serde_yaml::Value::Bool(true) => return Err("cache: use a duration like 15m, or off".into()),
        serde_yaml::Value::Number(n) => n.as_u64().ok_or("cache: a whole number of seconds")?,
        serde_yaml::Value::String(s) => {
            let s = s.trim().to_lowercase();
            if ["off", "false", "no", "none", ""].contains(&s.as_str()) {
                0
            } else {
                let c = DURATION
                    .captures(&s)
                    .ok_or_else(|| format!("cache: {s:?} is not a duration (e.g. 30s, 15m, 6h, 1d, off)"))?;
                let n: u64 = c[1].parse().map_err(|_| "cache: number too large")?;
                let unit = match c.get(2).map(|m| m.as_str()) {
                    Some("m") => 60,
                    Some("h") => 3600,
                    Some("d") => 86400,
                    _ => 1,
                };
                n.saturating_mul(unit)
            }
        }
        _ => return Err("cache: use a duration like 15m, or off".into()),
    };
    if seconds > 30 * 86400 {
        return Err("cache: between 0 and 30 days".into());
    }
    Ok(Some(seconds))
}

pub fn duration<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = serde_yaml::Value::deserialize(d)?;
    parse_duration(&v).map_err(serde::de::Error::custom)
}

// --------------------------------------------------------------------------- queries

pub const FILTER_OPS: [&str; 11] =
    ["equals", "not_equals", "contains", "gt", "gte", "lt", "lte", "between", "is_null", "not_null", "last_n_days"];

fn default_op() -> String {
    "equals".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// dimension or measure name (measures become HAVING)
    pub field: String,
    #[serde(default = "default_op")]
    pub op: String,
    #[serde(default, deserialize_with = "listify")]
    pub values: Vec<Value>,
}

impl Filter {
    pub fn equals(field: &str, values: Vec<Value>) -> Self {
        Filter { field: field.into(), op: "equals".into(), values }
    }
}

/// A single value or a list of values.
pub fn listify<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Value>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a,
        Value::Null => vec![Value::Null],
        v => vec![v],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)] // the names are the file format's: month_to_date, ...
pub enum Preset {
    MonthToDate,
    QuarterToDate,
    YearToDate,
}

/// The current window. Exactly one of: last_n_days, preset, or from/to (inclusive dates).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodRange {
    #[serde(default)]
    pub last_n_days: Option<i64>,
    #[serde(default)]
    pub preset: Option<Preset>,
    #[serde(default, rename = "from")]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
}

impl PeriodRange {
    pub fn validate(&self) -> Result<(), String> {
        let forms = [self.last_n_days.is_some(), self.preset.is_some(), self.from.is_some()];
        if forms.iter().filter(|x| **x).count() != 1 {
            return Err("set exactly one of last_n_days, preset, or from/to".into());
        }
        if let Some(n) = self.last_n_days
            && !(1..=3660).contains(&n)
        {
            return Err("last_n_days: between 1 and 3660".into());
        }
        if self.to.is_some() && self.from.is_none() {
            return Err("`to` needs `from`".into());
        }
        if let (Some(f), Some(t)) = (self.from, self.to)
            && t < f
        {
            return Err("`to` is before `from`".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareNamed {
    /// the window just before
    PreviousPeriod,
    /// 364 days back (weekday-aligned)
    PreviousYear,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompareFrom {
    /// custom previous window: same length as the current one, starting on this date
    pub from: NaiveDate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Compare {
    Named(CompareNamed),
    From(CompareFrom),
}

/// Restricts a query to a time window and optionally compares it with an earlier one of the same
/// length, shifted back by whole days, so per-day/week/month buckets line up on one axis.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub field: String,
    pub current: PeriodRange,
    #[serde(default)]
    pub compare: Option<Compare>,
}

/// Keep only the N values of `dimension` with the highest (bottom: lowest) `by` measure.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TopN {
    pub dimension: String,
    pub n: i64,
    pub by: String,
    #[serde(default)]
    pub bottom: bool,
}

/// A calculation a query adds, in the formula language: a measure of that query only.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Formula {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    pub expr: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// a field of each row (grouped by, filtered, summed), not a calculation on totals
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub per_row: bool,
}

impl Formula {
    /// The formula as a measure of its model.
    pub fn as_measure(&self) -> Measure {
        Measure {
            name: self.name.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            kind: "formula".into(),
            sql: None,
            formula: Some(self.expr.clone()),
            format: self.format.clone().unwrap_or_else(default_format),
            higher_is_better: true,
            hidden: false,
            synonyms: vec![],
            tags: vec![],
        }
    }

    /// A per-row formula as a dimension of its model (its type is what it gives: see
    /// `compiler::with_row_formulas`).
    pub fn as_dimension(&self) -> Dimension {
        Dimension {
            name: self.name.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            kind: String::new(),
            sql: None,
            from: None,
            column: None,
            formula: Some(self.expr.clone()),
            hidden: false,
            aggregation: None,
            synonyms: vec![],
            tags: vec![],
        }
    }

    pub fn check(&self) -> Result<(), String> {
        check_name(&self.name).map_err(|e| format!("formula {e}"))?;
        match self.format.as_deref() {
            Some(x) if !FORMATS.contains(&x) => Err(format!("formula {:?}: unknown format {x:?}", self.name)),
            _ => Ok(()),
        }
    }
}

fn default_limit() -> i64 {
    1000
}

/// The most rows a person's query may ask for.
pub const MAX_ROWS: i64 = 50_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub model: String,
    /// dimension names; time dimensions accept a grain suffix, e.g. "created_at:month"
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub measures: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// field names, prefix "-" for descending
    #[serde(default)]
    pub sort: Vec<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub period: Option<Period>,
    #[serde(default)]
    pub formulas: Vec<Formula>,
    #[serde(default)]
    pub top: Option<TopN>,
}

impl Query {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_up_to(MAX_ROWS)
    }

    /// `max_rows`: MAX_ROWS for people's queries; more for internal ones (blend sources).
    pub fn validate_up_to(&self, max_rows: i64) -> Result<(), String> {
        if self.dimensions.is_empty() && self.measures.is_empty() && self.formulas.is_empty() {
            return Err("a query needs at least one dimension or measure".into());
        }
        if !(1..=max_rows).contains(&self.limit) {
            return Err(format!("limit: between 1 and {max_rows}"));
        }
        if self.formulas.len() > MAX_FORMULAS {
            return Err(format!("at most {MAX_FORMULAS} formulas in a query"));
        }
        for f in &self.formulas {
            f.check()?;
        }
        for f in &self.filters {
            if !FILTER_OPS.contains(&f.op.as_str()) {
                return Err(format!("unsupported filter op {:?}", f.op));
            }
        }
        if let Some(p) = &self.period {
            p.current.validate()?;
        }
        if let Some(t) = &self.top
            && !(1..=1000).contains(&t.n)
        {
            return Err("top: n between 1 and 1000".into());
        }
        Ok(())
    }
}

// --------------------------------------------------------------------------- dashboards

pub const CHART_TYPES: [&str; 18] = [
    "kpi",
    "table",
    "bar",
    "hbar",
    "line",
    "area",
    "pie",
    "scatter",
    "pivot",
    "heatmap",
    "combo",
    "gauge",
    "trend",
    "treemap",
    "funnel",
    "waterfall",
    "histogram",
    "text",
];

/// The longest text a text tile may hold (characters).
pub const MAX_TEXT: usize = 10_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileLayout {
    #[serde(default)]
    pub x: i64,
    #[serde(default)]
    pub y: i64,
    #[serde(default = "default_w")]
    pub w: i64,
    #[serde(default = "default_h")]
    pub h: i64,
}

fn default_w() -> i64 {
    6
}
fn default_h() -> i64 {
    4
}

impl Default for TileLayout {
    fn default() -> Self {
        TileLayout { x: 0, y: 0, w: 6, h: 4 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Drill {
    Hierarchy(String),
    Path(Vec<String>),
}

fn default_chart() -> String {
    "table".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_chart", rename = "type")]
    pub kind: String,
    /// every tile but a text tile has one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<Query>,
    /// a text tile's words, in markdown
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// the tab it is on (none: the first)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// drill path: a hierarchy name from the model, or an explicit list of dimensions
    #[serde(default)]
    pub drill: Option<Drill>,
    #[serde(default)]
    pub layout: TileLayout,
    /// chart-specific (stacked, period: false, ...)
    #[serde(default)]
    pub options: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardFilter {
    pub model: String,
    pub field: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "default_op")]
    pub op: String,
    #[serde(default)]
    pub default: Vec<Value>,
}

/// Dashboard-wide period: applies to every tile on `model` without a period of its own.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardPeriod {
    pub model: String,
    pub field: String,
    pub current: PeriodRange,
    #[serde(default)]
    pub compare: Option<Compare>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dashboard {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// theme id; pins the look for every viewer (else: the viewer's choice)
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub period: Option<DashboardPeriod>,
    #[serde(default)]
    pub filters: Vec<DashboardFilter>,
    /// tab names, in order (none: one page)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<String>,
    #[serde(default)]
    pub tiles: Vec<Tile>,
}

impl Dashboard {
    pub fn validate(&self) -> Result<(), String> {
        let mut ids: Vec<&str> = self.tiles.iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err("tile ids must be unique".into());
        }
        let mut tabs: Vec<&str> = self.tabs.iter().map(|t| t.trim()).collect();
        tabs.sort_unstable();
        if tabs.iter().any(|t| t.is_empty()) || tabs.windows(2).any(|w| w[0] == w[1]) {
            return Err("tab names must be unique and not empty".into());
        }
        for t in &self.tiles {
            // the dashboard filters a tile doesn't follow: its own ones only
            if let Some(ignored) = t.options.get("ignore_filters") {
                let keys = ignored.as_array().filter(|a| a.iter().all(Value::is_string)).ok_or_else(|| {
                    format!("tile {}: ignore_filters is a list of the dashboard's filters (model.field)", t.id)
                })?;
                for k in keys.iter().filter_map(Value::as_str) {
                    if !self.filters.iter().any(|f| format!("{}.{}", f.model, f.field) == k) {
                        return Err(format!("tile {}: ignore_filters: {k} isn't one of the dashboard's filters", t.id));
                    }
                }
            }
            if !CHART_TYPES.contains(&t.kind.as_str()) {
                return Err(format!("tile {}: unknown chart type {:?}", t.id, t.kind));
            }
            if !(1..=12).contains(&t.layout.w) || t.layout.h < 1 {
                return Err(format!("tile {}: layout w is 1-12 and h at least 1", t.id));
            }
            if let Some(tab) = &t.tab
                && !self.tabs.contains(tab)
            {
                return Err(format!("tile {}: unknown tab {tab:?}", t.id));
            }
            match (t.kind.as_str(), &t.query, &t.text) {
                ("text", Some(_), _) => return Err(format!("tile {}: a text tile has no query", t.id)),
                ("text", None, None) => return Err(format!("tile {}: a text tile needs `text`", t.id)),
                ("text", None, Some(text)) if text.chars().count() > MAX_TEXT => {
                    return Err(format!("tile {}: text of at most {MAX_TEXT} characters", t.id));
                }
                ("text", ..) => {}
                (kind, None, _) => return Err(format!("tile {}: a {kind} tile needs a query", t.id)),
                (kind, Some(_), Some(_)) => return Err(format!("tile {}: only text tiles have `text` ({kind})", t.id)),
                (_, Some(q), None) => q.validate().map_err(|e| format!("tile {}: {e}", t.id))?,
            }
        }
        for f in &self.filters {
            if !FILTER_OPS.contains(&f.op.as_str()) {
                return Err(format!("filter {}: unsupported op {:?}", f.field, f.op));
            }
        }
        if let Some(p) = &self.period {
            p.current.validate().map_err(|e| format!("period: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let y = |s: &str| serde_yaml::from_str::<serde_yaml::Value>(s).unwrap();
        assert_eq!(parse_duration(&y("15m")), Ok(Some(900)));
        assert_eq!(parse_duration(&y("900")), Ok(Some(900)));
        assert_eq!(parse_duration(&y("off")), Ok(Some(0)));
        assert_eq!(parse_duration(&y("false")), Ok(Some(0)));
        assert!(parse_duration(&y("31d")).is_err());
        assert!(parse_duration(&y("soon")).is_err());
    }

    #[test]
    fn unknown_keys_are_refused() {
        let err = serde_yaml::from_str::<Dashboard>("title: x\ntilez: []").unwrap_err();
        assert!(err.to_string().contains("tilez"));
    }

    #[test]
    fn compare_forms() {
        let p: Period = serde_yaml::from_str("field: d\ncurrent: {last_n_days: 7}\ncompare: previous_year").unwrap();
        assert!(matches!(p.compare, Some(Compare::Named(CompareNamed::PreviousYear))));
        let p: Period =
            serde_yaml::from_str("field: d\ncurrent: {from: 2026-01-01}\ncompare: {from: 2025-01-01}").unwrap();
        assert!(matches!(p.compare, Some(Compare::From(_))));
    }

    fn dashboard(yaml: &str) -> Result<(), String> {
        serde_yaml::from_str::<Dashboard>(yaml).map_err(|e| e.to_string())?.validate()
    }

    #[test]
    fn text_tiles_have_text_and_no_query() {
        let q = "query: {model: m, measures: [n]}";
        assert_eq!(dashboard("title: x\ntiles:\n  - {id: a, type: text, text: '## Notes'}"), Ok(()));
        let no_text = dashboard("title: x\ntiles:\n  - {id: a, type: text}").unwrap_err();
        assert!(no_text.contains("tile a: a text tile needs `text`"), "{no_text}");
        let both = dashboard(&format!("title: x\ntiles:\n  - {{id: a, type: text, text: hi, {q}}}")).unwrap_err();
        assert!(both.contains("tile a: a text tile has no query"), "{both}");
        let chart = dashboard("title: x\ntiles:\n  - {id: a, type: bar, text: hi}").unwrap_err();
        assert!(chart.contains("tile a: a bar tile needs a query"), "{chart}");
        assert_eq!(dashboard(&format!("title: x\ntiles:\n  - {{id: a, type: bar, {q}}}")), Ok(()));
        let long = format!("title: x\ntiles:\n  - {{id: a, type: text, text: '{}'}}", "a".repeat(MAX_TEXT + 1));
        assert!(dashboard(&long).unwrap_err().contains("at most"));
    }

    #[test]
    fn tiles_go_on_listed_tabs() {
        let t = "{id: a, type: text, text: hi, tab: Detail}";
        assert_eq!(dashboard(&format!("title: x\ntabs: [Overview, Detail]\ntiles:\n  - {t}")), Ok(()));
        let unknown = dashboard(&format!("title: x\ntabs: [Overview]\ntiles:\n  - {t}")).unwrap_err();
        assert!(unknown.contains("tile a: unknown tab \"Detail\""), "{unknown}");
        let twice = dashboard("title: x\ntabs: [A, A]").unwrap_err();
        assert!(twice.contains("tab names must be unique"), "{twice}");
        assert!(dashboard("title: x\ntabs: ['']").unwrap_err().contains("tab names"));
        // no tab: the first one
        assert_eq!(dashboard("title: x\ntabs: [A]\ntiles:\n  - {id: a, type: text, text: hi}"), Ok(()));
    }

    #[test]
    fn single_filter_value_becomes_a_list() {
        let f: Filter = serde_json::from_str(r#"{"field": "region", "values": "EMEA"}"#).unwrap();
        assert_eq!(f.values, vec![Value::String("EMEA".into())]);
    }

    #[test]
    fn a_tile_ignores_only_the_dashboards_own_filters() {
        let d = |ignore: &str| {
            let yaml = format!(
                "title: t\nfilters: [{{model: sales, field: region}}]\ntiles:\n  - {{id: a, type: bar, query: {{model: sales, measures: [revenue]}}, options: {{ignore_filters: {ignore}}}}}\n"
            );
            serde_yaml::from_str::<Dashboard>(&yaml).unwrap().validate()
        };
        assert!(d("[sales.region]").is_ok());
        assert!(d("[]").is_ok());
        let e = d("[sales.store]").unwrap_err();
        assert!(e.contains("sales.store") && e.contains("filters"), "{e}");
        assert!(d("sales.region").unwrap_err().contains("list"));
    }
}
