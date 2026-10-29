//! Blends: models joined on shared dimensions, across connections and engines.
//!
//! A blend names a base model and the models joined to it, each on one or more keys:
//!
//! ```yaml
//! name: taxi_weather
//! base: taxi
//! joins:
//!   - model: weather
//!     on: ["pickup_borough = borough", "pickup_at:hour = observed_at:hour"]
//! ```
//!
//! Its fields are the models' fields as `model.field` ("taxi.trips", "weather.condition").
//! A query on a blend runs in two steps (aggregate, then join):
//!
//! 1. each model involved gets its own query, run on its own connection (DuckDB, BigQuery, ...)
//!    with that model's data restrictions: grouped by the join keys plus the dimensions asked
//!    for, with measures split into parts that add up again (an average becomes sum + count);
//! 2. those results are written to the organisation's ScramDB as tables, joined there (the
//!    base LEFT JOIN each other model; INNER when the query filters that model), and the
//!    person's query is compiled over the join as an ordinary model: grouping, sorting, top N,
//!    period comparison all work as usual.
//!
//! A query that only touches one of the models skips the join and runs on that model directly.
//! Measures must re-aggregate (count, sum, min, max, avg); distinct counts and custom-SQL
//! measures stay available on the models themselves. When a model has several rows per join
//! key, the other models' measures would be counted more than once: the executor checks that
//! with `fanout_checks` before joining and refuses.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::compiler::{QueryError, humanize, ident, split_grain};
use crate::period::resolve_period;
use crate::schema::{Dimension, Filter, Formula, Measure, Model, NAME_RE, Query, TIME_GRAINS, is_temporal};

/// Rows a model's part of a blend may return (aggregated to the join keys).
pub const MAX_SOURCE_ROWS: i64 = 1_000_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Blend {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub base: String,
    pub joins: Vec<Join>,
    /// how long results may be reused, like a model's `cache`
    #[serde(default, deserialize_with = "crate::schema::duration")]
    pub cache: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub model: String,
    /// "base_field[:grain] = field[:grain]"
    pub on: Vec<String>,
}

/// One join key: a base field and the joined model's field, at a grain for dates and times.
#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub base: String,
    pub other: String,
    pub grain: Option<String>,
}

fn err<T>(msg: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError(msg.into()))
}

/// Can values truncated to `key` be truncated further to `wanted` without mixing periods?
fn nests(key: Option<&str>, wanted: Option<&str>) -> bool {
    match (key, wanted) {
        (None, _) => true,
        (Some(_), None) => true, // the key's own values
        (Some(k), Some(w)) if k == w => true,
        (Some(k), Some(w)) => match k {
            "hour" => true,
            "day" => w != "hour",
            "month" => matches!(w, "quarter" | "year"),
            "quarter" => w == "year",
            _ => false, // weeks straddle months and years
        },
    }
}

fn model_label(m: &Model) -> String {
    m.label.clone().unwrap_or_else(|| humanize(&m.name))
}

fn field_label(m: &Model, label: Option<&String>, name: &str) -> String {
    format!("{} · {}", model_label(m), label.cloned().unwrap_or_else(|| humanize(name)))
}

/// The type a key compares as: a date once truncated to a day or longer.
fn key_kind(kind: &str, grain: Option<&str>) -> String {
    match grain {
        Some(g) if g != "hour" => "date".into(),
        _ => kind.into(),
    }
}

/// Measures whose parts add up again after the join.
fn blendable(kind: &str) -> bool {
    matches!(kind, "count" | "sum" | "min" | "max" | "avg")
}

impl Blend {
    pub fn keys(join: &Join) -> Result<Vec<Key>, String> {
        if join.on.is_empty() {
            return Err(format!("join {}: at least one key (base_field = field)", join.model));
        }
        join.on
            .iter()
            .map(|s| {
                let (a, b) = s
                    .split_once('=')
                    .ok_or_else(|| format!("join key {s:?}: write it as base_field = field (= between them)"))?;
                let ((base, ga), (other, gb)) = (split_grain(a.trim()), split_grain(b.trim()));
                if ga != gb {
                    return Err(format!("join key {s:?}: both sides need the same grain"));
                }
                if !NAME_RE.is_match(base) || !NAME_RE.is_match(other) {
                    return Err(format!("join key {s:?}: field names"));
                }
                Ok(Key { base: base.into(), other: other.into(), grain: ga.map(String::from) })
            })
            .collect()
    }

    /// Checks the blend against the models it joins (`models`: everything that exists).
    pub fn validate(&self, models: &BTreeMap<String, Model>) -> Result<(), String> {
        if !NAME_RE.is_match(&self.name) {
            return Err(format!("invalid blend name {:?}: use letters, digits and underscores", self.name));
        }
        let base = models.get(&self.base).ok_or_else(|| format!("unknown model {:?}", self.base))?;
        if self.joins.is_empty() {
            return Err("a blend joins at least one model to its base".into());
        }
        let mut seen = vec![&self.base];
        for j in &self.joins {
            if j.model == self.base {
                return Err(format!("{}: a model can't be joined to itself", j.model));
            }
            if seen.contains(&&j.model) {
                return Err(format!("{} is joined twice", j.model));
            }
            seen.push(&j.model);
            let other = models.get(&j.model).ok_or_else(|| format!("unknown model {:?}", j.model))?;
            for k in Blend::keys(j)? {
                let a =
                    base.dimension(&k.base).ok_or_else(|| format!("{} has no dimension {:?}", base.name, k.base))?;
                let b = other
                    .dimension(&k.other)
                    .ok_or_else(|| format!("{} has no dimension {:?}", other.name, k.other))?;
                let show = format!("{} = {}", k.base, k.other);
                if let Some(g) = &k.grain {
                    if !TIME_GRAINS.contains(&g.as_str()) {
                        return Err(format!("join key {show}: unknown grain {g:?}"));
                    }
                    if !is_temporal(&a.kind)
                        || !is_temporal(&b.kind)
                        || (g == "hour" && (a.kind == "date" || b.kind == "date"))
                    {
                        return Err(format!("join key {show}: the grain {g:?} needs date/time fields"));
                    }
                }
                let (ka, kb) = (key_kind(&a.kind, k.grain.as_deref()), key_kind(&b.kind, k.grain.as_deref()));
                if ka != kb {
                    return Err(format!("join key {show}: different types ({ka} and {kb})"));
                }
            }
        }
        Ok(())
    }

    fn models<'m>(&self, models: &'m BTreeMap<String, Model>) -> Vec<&'m Model> {
        std::iter::once(&self.base).chain(self.joins.iter().map(|j| &j.model)).filter_map(|n| models.get(n)).collect()
    }

    /// The blend as a model, for listing its fields: `model.field`, labelled "Model · Field".
    /// Joined models' key fields are hidden (they're the base's), and so are measures that
    /// can't be blended.
    pub fn as_model(&self, models: &BTreeMap<String, Model>) -> Model {
        let mut out = Model {
            name: self.name.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            extends: None,
            connection: String::new(),
            table: None,
            sql: None,
            dimensions: vec![],
            measures: vec![],
            hierarchies: BTreeMap::new(),
            joins: vec![],
            cache: self.cache,
            tags: vec![],
        };
        for (i, m) in self.models(models).into_iter().enumerate() {
            let keys: Vec<String> = match i {
                0 => vec![],
                _ => self
                    .joins
                    .iter()
                    .find(|j| j.model == m.name)
                    .and_then(|j| Blend::keys(j).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|k| k.other)
                    .collect(),
            };
            for d in &m.dimensions {
                out.dimensions.push(Dimension {
                    name: format!("{}.{}", m.name, d.name),
                    label: Some(field_label(m, d.label.as_ref(), &d.name)),
                    hidden: d.hidden || keys.contains(&d.name),
                    sql: None,
                    formula: None,
                    ..d.clone()
                });
            }
            for x in &m.measures {
                out.measures.push(Measure {
                    name: format!("{}.{}", m.name, x.name),
                    label: Some(field_label(m, x.label.as_ref(), &x.name)),
                    hidden: x.hidden || !blendable(&x.kind),
                    sql: None,
                    ..x.clone()
                });
            }
            for (h, fields) in &m.hierarchies {
                out.hierarchies
                    .insert(format!("{}.{h}", m.name), fields.iter().map(|f| format!("{}.{f}", m.name)).collect());
            }
        }
        out
    }
}

/// A model's part of a joined query.
#[derive(Debug, Clone)]
pub struct Side {
    pub model: String,
    /// the model plus the measure parts the query asks for
    pub source: Model,
    pub query: Query,
    /// INNER JOIN: the query filters this model's own fields
    pub inner: bool,
    /// output column names (`model.field`, measure parts) in the query's column order
    columns: Vec<String>,
    /// for joined models: (this side's key column, the base's key column), by position
    on: Vec<(usize, usize)>,
    label: String,
}

#[derive(Debug, Clone)]
pub enum Plan {
    /// Only one model is involved: run `query` on it directly; its result columns get the
    /// `model.` prefix back (prefix_columns).
    Single {
        model: String,
        query: Query,
    },
    Joined(Joined),
}

#[derive(Debug, Clone)]
pub struct Joined {
    /// the base first
    pub sides: Vec<Side>,
    /// the person's query, over `joined_model`
    pub query: Query,
    /// the joined model's dimensions and measures (without the SQL, which needs table names)
    dimensions: Vec<Dimension>,
    measures: Vec<Measure>,
    /// sides (positions) whose measures the query asks for
    measured: Vec<usize>,
}

/// A field reference: which model (index: 0 = base), the field, its grain.
struct Ref<'q> {
    side: usize,
    field: &'q str,
    grain: Option<&'q str>,
}

/// What a plan knows about the blend's models.
struct Ctx<'a> {
    blend: &'a Blend,
    models: Vec<&'a Model>,
    keys: Vec<Vec<Key>>, // per side; empty for the base
}

impl<'a> Ctx<'a> {
    fn parse<'q>(&self, r: &'q str) -> Result<Ref<'q>, QueryError> {
        let Some((m, rest)) = r.split_once('.') else {
            return err(format!("{r:?}: blend fields are written model.field"));
        };
        let side = self
            .models
            .iter()
            .position(|x| x.name == m)
            .ok_or_else(|| QueryError(format!("unknown model {m:?} in blend {:?}", self.blend.name)))?;
        let (field, grain) = split_grain(rest);
        Ok(Ref { side, field, grain })
    }

    fn dim(&self, r: &Ref) -> Result<&'a Dimension, QueryError> {
        let m = self.models[r.side];
        m.dimension(r.field).ok_or_else(|| QueryError(format!("unknown dimension {:?} in model {:?}", r.field, m.name)))
    }

    /// The base key a joined model's key field stands for.
    fn key_of(&self, r: &Ref) -> Option<&Key> {
        self.keys[r.side].iter().find(|k| k.other == r.field)
    }

    fn name(&self, side: usize, field: &str) -> String {
        format!("{}.{field}", self.models[side].name)
    }
}

fn with_grain(field: &str, grain: Option<&str>) -> String {
    grain.map_or(field.to_string(), |g| format!("{field}:{g}"))
}

fn rename_sort(s: &str, f: impl Fn(&str) -> String) -> String {
    let sign = if s.starts_with('-') { "-" } else { "" };
    format!("{sign}{}", f(s.trim_start_matches(['-', '+'])))
}

fn time_text(d: DateTime<Utc>, kind: &str) -> String {
    match kind {
        "date" => d.date_naive().to_string(),
        "datetime" => d.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => d.format("%Y-%m-%d %H:%M:%S+00").to_string(),
    }
}

pub fn plan(
    blend: &Blend,
    models: &BTreeMap<String, Model>,
    q: &Query,
    now: DateTime<Utc>,
) -> Result<Plan, QueryError> {
    if !q.formulas.is_empty() {
        return with_formulas(blend, models, q, now);
    }
    blend.validate(models).map_err(QueryError)?;
    let ms = blend.models(models);
    let mut keys = vec![vec![]];
    for j in &blend.joins {
        keys.push(Blend::keys(j).map_err(QueryError)?);
    }
    let cx = Ctx { blend, models: ms, keys };

    // which models the query needs: its fields' models; a joined model's key field is the
    // base's field, so on its own it needs no join
    let mut definite: Vec<usize> = vec![];
    let mut loose: Vec<usize> = vec![]; // joined models of key fields
    let mut note = |side: usize, key: bool| {
        let list = if key { &mut loose } else { &mut definite };
        if !list.contains(&side) {
            list.push(side);
        }
    };
    for r in q.dimensions.iter().chain(q.filters.iter().map(|f| &f.field)).chain(q.period.iter().map(|p| &p.field)) {
        let r = cx.parse(r)?;
        if cx.models[r.side].measure(r.field).is_none() {
            cx.dim(&r)?;
        }
        note(r.side, cx.key_of(&r).is_some());
    }
    for r in &q.measures {
        let r = cx.parse(r)?;
        cx.models[r.side].measure(r.field).ok_or_else(|| {
            QueryError(format!("unknown measure {:?} in model {:?}", r.field, cx.models[r.side].name))
        })?;
        note(r.side, false);
    }
    let mut sides = definite.clone();
    for s in loose {
        if !sides.contains(&s) && !sides.contains(&0) {
            sides.push(0); // stands for the base's field
        }
    }
    if sides.is_empty() {
        sides.push(0);
    }
    if sides.len() == 1 {
        return Ok(Plan::Single { model: cx.models[sides[0]].name.clone(), query: single(&cx, sides[0], q)? });
    }
    if !sides.contains(&0) {
        sides.push(0);
    }
    sides.sort_unstable();
    joined(&cx, &sides, q, now).map(Plan::Joined)
}

/// A blend query's formulas. A field of each row uses one model's fields: it becomes a field of
/// that model, computed in that model's own query on its engine before it aggregates. A formula on
/// totals uses the blend's measures (`model.measure`): they're fetched from their models as any
/// other, and it runs after the join, on ScramDB (or on the one model, when only one is needed).
fn with_formulas(
    blend: &Blend,
    models: &BTreeMap<String, Model>,
    q: &Query,
    now: DateTime<Utc>,
) -> Result<Plan, QueryError> {
    let in_blend: Vec<&str> =
        std::iter::once(blend.base.as_str()).chain(blend.joins.iter().map(|j| j.model.as_str())).collect();
    let mut models = models.clone();
    let mut rows: Vec<(String, Formula)> = vec![]; // (model, the field with its own names)
    let mut totals: Vec<Formula> = vec![];
    for f in &q.formulas {
        let what = |e: crate::formula::FormulaError| QueryError(format!("{}: {e}", f.name));
        let refs = crate::formula::references(&f.expr).map_err(what)?;
        if !f.per_row {
            if let Some(c) = crate::formula::calls(&f.expr)
                .map_err(what)?
                .into_iter()
                .find(|c| crate::formula::OVER_ROWS.contains(&c.as_str()))
            {
                return err(format!(
                    "{}: {c}(…) adds up a model's rows, and after the join they're totals already: \
                     use it on the model itself",
                    f.name
                ));
            }
            totals.push(f.clone());
            continue;
        }
        let mut owners: Vec<&str> = vec![];
        for r in &refs {
            let owner = match r.split_once('.') {
                Some((m, _)) => m,
                // another field of each row
                None => match rows.iter().find(|(_, x)| x.name == *r) {
                    Some((m, _)) => m.as_str(),
                    None => return err(format!("{}: {r:?}: blend fields are written model.field", f.name)),
                },
            };
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        let model = match owners.as_slice() {
            [m] if in_blend.contains(m) => m.to_string(),
            [m] => return err(format!("{}: unknown model {m:?} in blend {:?}", f.name, blend.name)),
            [] => return err(format!("{}: a field of each row uses a model's fields, written model.field", f.name)),
            _ => {
                return err(format!(
                    "{}: a field of each row uses one model's fields; {} are different models",
                    f.name,
                    owners.join(" and ")
                ));
            }
        };
        let prefix = format!("{model}.");
        let expr =
            crate::formula::rename(&f.expr, |n| n.strip_prefix(&prefix).unwrap_or(n).to_string()).map_err(what)?;
        let own = Formula { expr, ..f.clone() };
        let m = models.get_mut(&model).expect("a model of the blend");
        if m.dimension(&f.name).is_some() || m.measure(&f.name).is_some() {
            return err(format!("formula {:?}: {model} already has a field with this name", f.name));
        }
        m.dimensions.push(own.as_dimension());
        // what it gives, as for the model's own fields of each row
        let kind = crate::compiler::with_row_formulas(m, crate::compiler::Dialect::Postgres)?
            .dimension(&f.name)
            .map(|d| d.kind.clone())
            .unwrap_or_default();
        m.dimensions.last_mut().expect("just added").kind = kind;
        rows.push((model, own));
    }
    let is_total = |r: &str| totals.iter().any(|f| f.name == r);
    // a field of each row is its model's
    let field = |r: &str| {
        let (name, grain) = split_grain(r);
        match rows.iter().find(|(_, f)| f.name == name) {
            Some((m, _)) => with_grain(&format!("{m}.{name}"), grain),
            None => r.to_string(),
        }
    };
    // the query without the formulas: the measures they use instead
    let mut inner = q.clone();
    inner.formulas = vec![];
    inner.measures = q.measures.iter().filter(|m| !is_total(m)).cloned().collect();
    for f in &totals {
        for r in crate::formula::references(&f.expr).map_err(|e| QueryError(format!("{}: {e}", f.name)))? {
            if is_total(&r) || inner.measures.contains(&r) {
                continue;
            }
            if !r.contains('.') {
                return err(format!("{}: {r:?}: blend fields are written model.field", f.name));
            }
            inner.measures.push(r);
        }
    }
    inner.dimensions = q.dimensions.iter().map(|d| field(d)).collect();
    inner.filters = q
        .filters
        .iter()
        .filter(|f| !is_total(&f.field))
        .map(|f| Filter { field: field(&f.field), ..f.clone() })
        .collect();
    inner.sort =
        q.sort.iter().filter(|s| !is_total(s.trim_start_matches(['-', '+']))).map(|s| rename_sort(s, field)).collect();
    if let Some(t) = &mut inner.top {
        t.dimension = field(&t.dimension);
    }
    let sort: Vec<String> = q.sort.iter().map(|s| rename_sort(s, field)).collect();
    let on_totals: Vec<Filter> = q.filters.iter().filter(|f| is_total(&f.field)).cloned().collect();
    Ok(match plan(blend, &models, &inner, now)? {
        Plan::Single { model, mut query } => {
            // the model's own query: its names without the model's
            let prefix = format!("{model}.");
            let own = |n: &str| n.strip_prefix(&prefix).unwrap_or(n).to_string();
            query.measures = q.measures.iter().map(|m| own(m)).collect();
            query.sort = sort.iter().map(|s| rename_sort(s, own)).collect();
            query.filters.extend(on_totals);
            query.formulas = rows.iter().filter(|(m, _)| *m == model).map(|(_, f)| f.clone()).collect();
            for f in &totals {
                let expr = crate::formula::rename(&f.expr, own).map_err(|e| QueryError(format!("{}: {e}", f.name)))?;
                query.formulas.push(Formula { expr, ..f.clone() });
            }
            Plan::Single { model, query }
        }
        Plan::Joined(mut j) => {
            j.measures.extend(totals.iter().map(Formula::as_measure));
            j.query.measures = q.measures.clone();
            j.query.sort = sort;
            j.query.filters.extend(on_totals);
            Plan::Joined(j)
        }
    })
}

/// The query on one model: fields renamed to that model's (key fields of joined models to the
/// base's).
fn single(cx: &Ctx, side: usize, q: &Query) -> Result<Query, QueryError> {
    let field = |r: &str| -> Result<String, QueryError> {
        let p = cx.parse(r)?;
        if p.side == side {
            return Ok(with_grain(p.field, p.grain));
        }
        match cx.key_of(&p) {
            Some(k) if side == 0 => Ok(with_grain(&k.base, p.grain)),
            _ => err(format!("{r:?} can't be used here")),
        }
    };
    let measure = |r: &str| -> String {
        let prefix = format!("{}.", cx.models[side].name);
        r.strip_prefix(&prefix).unwrap_or(r).to_string()
    };
    let mut out = q.clone();
    out.model = cx.models[side].name.clone();
    out.dimensions = q.dimensions.iter().map(|d| field(d)).collect::<Result<_, _>>()?;
    out.measures = q.measures.iter().map(|m| measure(m)).collect();
    for f in &mut out.filters {
        f.field = match cx.parse(&f.field) {
            Ok(p) if cx.models[p.side].measure(p.field).is_some() => measure(&f.field),
            _ => field(&f.field)?,
        };
    }
    if let Some(p) = &mut out.period {
        p.field = field(&p.field)?;
    }
    out.sort =
        q.sort.iter().map(|s| rename_sort(s, |r| measure(&field(r).unwrap_or_else(|_| r.to_string())))).collect();
    if let Some(t) = &mut out.top {
        t.dimension = field(&t.dimension)?;
        t.by = measure(&t.by);
    }
    Ok(out)
}

fn joined(cx: &Ctx, sides: &[usize], q: &Query, now: DateTime<Utc>) -> Result<Joined, QueryError> {
    let base = cx.models[0];
    let n = cx.models.len();
    // the base's key columns: (field, grain), from the joins in play
    let mut base_keys: Vec<(String, Option<String>)> = vec![];
    for &s in &sides[1..] {
        for k in &cx.keys[s] {
            if !base_keys.iter().any(|(f, _)| *f == k.base) {
                base_keys.push((k.base.clone(), k.grain.clone()));
            } else if !base_keys.iter().any(|(f, g)| *f == k.base && *g == k.grain) {
                return err(format!("{} is joined at two different grains", k.base));
            }
        }
    }
    let key_index = |field: &str| base_keys.iter().position(|(f, _)| f == field);

    // per model: sub-query dimensions (refs), measures, filters, column names
    let mut dims: Vec<Vec<String>> = vec![vec![]; n];
    let mut cols: Vec<Vec<String>> = vec![vec![]; n];
    let mut on: Vec<Vec<(usize, usize)>> = vec![vec![]; n];
    for (field, grain) in &base_keys {
        dims[0].push(with_grain(field, grain.as_deref()));
        cols[0].push(cx.name(0, field));
    }
    for &s in &sides[1..] {
        for k in &cx.keys[s] {
            on[s].push((dims[s].len(), key_index(&k.base).expect("base key")));
            dims[s].push(with_grain(&k.other, k.grain.as_deref()));
            cols[s].push(cx.name(s, &k.other));
        }
    }

    // the joined model's dimensions: keys (the base's columns, and the joined models' names
    // for them), then the other fields asked for
    let mut jdims: Vec<Dimension> = vec![];
    let dim_named = |side: usize, d: &Dimension, kind: String, column: &str, hidden: bool| Dimension {
        name: cx.name(side, &d.name),
        label: Some(field_label(cx.models[side], d.label.as_ref(), &d.name)),
        description: d.description.clone(),
        kind,
        sql: Some(ident(column)),
        from: None,
        column: None,
        formula: None,
        hidden,
        aggregation: None,
        synonyms: vec![],
        tags: vec![],
    };
    for (field, grain) in &base_keys {
        let d = base.dimension(field).expect("validated");
        jdims.push(dim_named(0, d, key_kind(&d.kind, grain.as_deref()), &cx.name(0, field), false));
    }
    for &s in &sides[1..] {
        for k in &cx.keys[s] {
            let d = cx.models[s].dimension(&k.other).expect("validated");
            jdims.push(dim_named(s, d, key_kind(&d.kind, k.grain.as_deref()), &cx.name(0, &k.base), true));
        }
    }

    // a reference resolved to the base's key column, if it is one
    let as_key = |r: &Ref| -> Option<usize> {
        match r.side {
            0 => key_index(r.field),
            s if sides.contains(&s) => cx.key_of(r).and_then(|k| key_index(&k.base)),
            _ => None,
        }
    };
    for r in &q.dimensions {
        let p = cx.parse(r)?;
        let d = cx.dim(&p)?;
        if let Some(i) = as_key(&p) {
            let kg = base_keys[i].1.as_deref();
            if !nests(kg, p.grain) {
                return err(format!(
                    "{r}: the models are joined by {} at the {} grain; use it or a longer one",
                    base_keys[i].0,
                    kg.unwrap_or("raw")
                ));
            }
            continue;
        }
        // a joined model's key field when that model isn't in play: the base's field
        let (side, field) = match cx.key_of(&p) {
            Some(k) if !sides.contains(&p.side) => (0, k.base.as_str()),
            _ => (p.side, p.field),
        };
        let column = cx.name(side, field);
        if cols[side].contains(&column) {
            return err(format!("{r}: a field can be used once in a blend query (at one grain)"));
        }
        dims[side].push(with_grain(field, p.grain));
        cols[side].push(column.clone());
        let src = cx.models[side].dimension(field).expect("checked");
        jdims.push(dim_named(side, src, key_kind(&src.kind, p.grain), &column, false));
        if side != p.side {
            jdims.push(dim_named(p.side, d, key_kind(&d.kind, p.grain), &column, true));
        }
    }

    // measures: parts that add up again, per model
    let mut sources: Vec<Model> = cx.models.iter().map(|m| (*m).clone()).collect();
    let mut parts: Vec<Vec<String>> = vec![vec![]; n];
    let mut jmeasures: Vec<Measure> = vec![];
    let mut measured: Vec<usize> = vec![];
    for r in &q.measures {
        let p = cx.parse(r)?;
        let m = cx.models[p.side].measure(p.field).expect("checked").clone();
        if !blendable(&m.kind) {
            return err(format!(
                "{r}: {} measures can't be combined across models; use it on {} alone",
                m.kind.replace('_', " "),
                cx.models[p.side].name
            ));
        }
        if !measured.contains(&p.side) {
            measured.push(p.side);
        }
        let label = field_label(cx.models[p.side], m.label.as_ref(), &m.name);
        let col = cx.name(p.side, &m.name);
        let (kind, sql) = if m.kind == "avg" {
            let Some(arg) = m.sql.clone() else { return err(format!("{r}: an average needs sql")) };
            for (suffix, kind) in [("sum", "sum"), ("count", "count")] {
                let name = format!("{}__{suffix}", m.name);
                if sources[p.side].measure(&name).is_none() {
                    sources[p.side].measures.push(Measure {
                        name: name.clone(),
                        kind: kind.into(),
                        sql: Some(arg.clone()),
                        hidden: true,
                        ..m.clone()
                    });
                }
                parts[p.side].push(name.clone());
                cols[p.side].push(format!("{col}__{suffix}"));
            }
            (
                "number",
                format!("SUM({}) / NULLIF(SUM({}), 0)", ident(&format!("{col}__sum")), ident(&format!("{col}__count"))),
            )
        } else {
            parts[p.side].push(m.name.clone());
            cols[p.side].push(col.clone());
            (if m.kind == "count" { "sum" } else { m.kind.as_str() }, ident(&col))
        };
        jmeasures.push(Measure {
            name: col,
            label: Some(label),
            kind: kind.into(),
            sql: Some(sql),
            hidden: false,
            ..m
        });
    }

    // the period: on a time key, fetched for both windows from every model in play
    let mut final_query = q.clone();
    let mut filters: Vec<Vec<Filter>> = vec![vec![]; n];
    let mut inner = vec![false; n];
    let mut period_key: Option<usize> = None;
    if let Some(p) = &q.period {
        let r = cx.parse(&p.field)?;
        let i = as_key(&r)
            .filter(|i| base_keys[*i].1.is_some() || is_temporal(&base.dimension(&base_keys[*i].0).expect("key").kind))
            .ok_or_else(|| {
                QueryError(format!("period: {} must be a date/time field the models are joined on", p.field))
            })?;
        let kind = &base.dimension(&base_keys[i].0).expect("key").kind;
        let w = resolve_period(&p.current, p.compare.as_ref(), kind, now).map_err(QueryError)?;
        let start = w.start - Duration::days(w.offset_days.unwrap_or(0));
        let range = |field: &str| {
            vec![
                Filter { field: field.into(), op: "gte".into(), values: vec![json!(time_text(start, kind))] },
                Filter { field: field.into(), op: "lt".into(), values: vec![json!(time_text(w.end, kind))] },
            ]
        };
        filters[0].extend(range(&base_keys[i].0));
        for &s in &sides[1..] {
            if let Some(k) = cx.keys[s].iter().find(|k| k.base == base_keys[i].0) {
                filters[s].extend(range(&k.other));
            }
        }
        final_query.period = Some(crate::schema::Period { field: cx.name(0, &base_keys[i].0), ..p.clone() });
        period_key = Some(i);
    }

    // filters: measures after the join; keys on every model; other fields on their model
    final_query.filters = vec![];
    for f in &q.filters {
        let r = cx.parse(&f.field)?;
        if cx.models[r.side].measure(r.field).is_some() {
            final_query.filters.push(f.clone());
            continue;
        }
        if let Some(i) = as_key(&r) {
            if period_key == Some(i) {
                continue; // the period owns this field's window
            }
            let key = &base_keys[i].0;
            filters[0].push(Filter { field: with_grain(key, r.grain), ..f.clone() });
            for &s in &sides[1..] {
                if let Some(k) = cx.keys[s].iter().find(|k| &k.base == key) {
                    filters[s].push(Filter { field: with_grain(&k.other, r.grain), ..f.clone() });
                }
            }
            continue;
        }
        let (side, field) = match cx.key_of(&r) {
            Some(k) if !sides.contains(&r.side) => (0, k.base.as_str()),
            _ => (r.side, r.field),
        };
        filters[side].push(Filter { field: with_grain(field, r.grain), ..f.clone() });
        inner[side] = side != 0;
    }

    let sides_out = sides
        .iter()
        .map(|&s| {
            let query = Query {
                model: cx.models[s].name.clone(),
                dimensions: dims[s].clone(),
                measures: parts[s].clone(),
                filters: filters[s].clone(),
                sort: vec![],
                limit: MAX_SOURCE_ROWS,
                period: None,
                formulas: vec![],
                top: None,
            };
            Side {
                model: cx.models[s].name.clone(),
                source: sources[s].clone(),
                query,
                inner: inner[s],
                columns: cols[s].clone(),
                on: on[s].clone(),
                label: model_label(cx.models[s]),
            }
        })
        .collect();
    // positions in `sides`
    let measured = measured.iter().filter_map(|m| sides.iter().position(|s| s == m)).collect();
    Ok(Joined { sides: sides_out, query: final_query, dimensions: jdims, measures: jmeasures, measured })
}

impl Joined {
    /// The join as a model over the sides' result tables (`tables[i]`: side i's table, with
    /// columns c0, c1, ... in the side's column order).
    pub fn joined_model(&self, tables: &[String]) -> Model {
        let mut select = vec![];
        for (i, s) in self.sides.iter().enumerate() {
            for (p, name) in s.columns.iter().enumerate() {
                if i > 0 && s.on.iter().any(|(mine, _)| *mine == p) {
                    continue; // the base's key column stands for it
                }
                select.push(format!("s{i}.c{p} AS {}", ident(name)));
            }
        }
        let mut sql = format!("SELECT\n  {}\nFROM {} AS s0", select.join(",\n  "), tables[0]);
        for (i, s) in self.sides.iter().enumerate().skip(1) {
            let on: Vec<String> = s.on.iter().map(|(mine, base)| format!("s{i}.c{mine} = s0.c{base}")).collect();
            let kind = if s.inner { "INNER" } else { "LEFT" };
            sql.push_str(&format!("\n{kind} JOIN {} AS s{i} ON {}", tables[i], on.join(" AND ")));
        }
        Model {
            name: "blend".into(),
            label: None,
            description: None,
            extends: None,
            connection: String::new(),
            table: None,
            sql: Some(sql),
            dimensions: self.dimensions.clone(),
            measures: self.measures.clone(),
            hierarchies: BTreeMap::new(),
            joins: vec![],
            cache: None,
            tags: vec![],
        }
    }

    /// Queries that find a row when the join would count measures more than once, each with
    /// the explanation to give.
    pub fn fanout_checks(&self, tables: &[String]) -> Vec<(String, String)> {
        let unique = |i: usize, cols: Vec<usize>| {
            let list: Vec<String> = cols.iter().map(|c| format!("c{c}")).collect();
            format!("SELECT 1 FROM {} GROUP BY {} HAVING COUNT(*) > 1 LIMIT 1", tables[i], list.join(", "))
        };
        let mut out: Vec<(String, String)> = vec![];
        let mut add = |sql: String, msg: String| {
            if !out.iter().any(|(s, _)| *s == sql) {
                out.push((sql, msg));
            }
        };
        for (si, s) in self.sides.iter().enumerate() {
            if !self.measured.contains(&si) {
                continue;
            }
            let why = |other: &Side| {
                format!(
                    "these fields can't be combined: \u{201c}{}\u{201d} has several rows for each value of the join keys, \
                     so measures from \u{201c}{}\u{201d} would be counted more than once. Remove the \u{201c}{}\u{201d} \
                     fields that vary within a key, or the \u{201c}{}\u{201d} measures",
                    other.label, s.label, other.label, s.label
                )
            };
            for (oi, o) in self.sides.iter().enumerate().skip(1) {
                if oi != si && o.query.dimensions.len() > o.on.len() {
                    add(unique(oi, o.on.iter().map(|(mine, _)| *mine).collect()), why(o));
                }
            }
            if si > 0 {
                let base_cols: Vec<usize> = s.on.iter().map(|(_, b)| *b).collect();
                if self.sides[0].query.dimensions.len() > base_cols.len() {
                    add(unique(0, base_cols), why(&self.sides[0]));
                }
            }
        }
        out
    }
}

/// Result columns of a single-model plan, renamed back to the blend's names and labels.
pub fn prefix_columns(model: &str, label: &str, columns: &mut [crate::compiler::Column]) {
    for c in columns {
        c.name = format!("{model}.{}", c.name);
        c.field = format!("{model}.{}", c.field);
        c.base = c.base.take().map(|b| format!("{model}.{b}"));
        c.label = format!("{label} · {}", c.label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::{Dialect, compile_query, compile_query_up_to};
    use chrono::TimeZone;
    use serde_json::Value;

    fn models() -> BTreeMap<String, Model> {
        let trips: Model = serde_yaml::from_str(
            r#"
name: trips
label: Taxi
connection: lake
table: trips
dimensions:
  - {name: pickup_at, type: datetime}
  - {name: pickup_borough, label: Pickup borough}
  - {name: pickup_zone}
  - {name: fare, type: number}
measures:
  - {name: trips, type: count}
  - {name: revenue, type: sum, sql: total, format: currency}
  - {name: avg_fare, type: avg, sql: fare}
  - {name: zones, type: count_distinct, sql: pickup_zone}
hierarchies:
  location: [pickup_borough, pickup_zone]
"#,
        )
        .unwrap();
        let weather: Model = serde_yaml::from_str(
            r#"
name: weather
connection: meteo
table: hourly
dimensions:
  - {name: observed_at, type: datetime}
  - {name: observed_on, type: date}
  - {name: borough}
  - {name: condition}
measures:
  - {name: hours, type: count}
  - {name: avg_temperature, type: avg, sql: temperature_c}
  - {name: max_gust, type: max, sql: wind_gusts_kmh}
"#,
        )
        .unwrap();
        BTreeMap::from([("trips".into(), trips), ("weather".into(), weather)])
    }

    fn blend() -> Blend {
        serde_yaml::from_str(
            r#"
name: trips_weather
base: trips
joins:
  - model: weather
    on: ["pickup_borough = borough", "pickup_at:hour = observed_at:hour"]
"#,
        )
        .unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, 7, 30, 0).unwrap()
    }

    fn q(v: Value) -> Query {
        serde_json::from_value(v).unwrap()
    }

    fn joined(v: Value) -> Joined {
        match plan(&blend(), &models(), &q(v), now()).unwrap() {
            Plan::Joined(j) => j,
            Plan::Single { model, .. } => panic!("expected a join, got a query on {model}"),
        }
    }

    fn tables(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("magnum_cache.t{i}")).collect()
    }

    #[test]
    fn keys_and_validation() {
        let b = blend();
        b.validate(&models()).unwrap();
        assert_eq!(
            Blend::keys(&b.joins[0]).unwrap(),
            vec![
                Key { base: "pickup_borough".into(), other: "borough".into(), grain: None },
                Key { base: "pickup_at".into(), other: "observed_at".into(), grain: Some("hour".into()) },
            ]
        );
        let bad = |on: &[&str], model: &str| {
            let mut b = blend();
            b.joins[0].model = model.into();
            b.joins[0].on = on.iter().map(|s| s.to_string()).collect();
            b.validate(&models()).unwrap_err()
        };
        assert!(bad(&["pickup_borough = nope"], "weather").contains("nope"));
        assert!(bad(&["pickup_borough = borough"], "nowhere").contains("nowhere"));
        assert!(bad(&["pickup_borough = borough"], "trips").contains("itself"));
        assert!(bad(&["pickup_at:hour = observed_at:day"], "weather").contains("grain"));
        assert!(bad(&["pickup_borough = observed_at"], "weather").contains("type"));
        assert!(bad(&["pickup_borough:day = borough:day"], "weather").contains("grain"));
        assert!(bad(&["pickup_borough borough"], "weather").contains("="));
        assert!(bad(&[], "weather").contains("key"));
        let mut b = blend();
        b.name = "no.dots".into();
        assert!(b.validate(&models()).is_err());
    }

    #[test]
    fn fields_are_model_dot_field() {
        let m = blend().as_model(&models());
        assert_eq!(m.name, "trips_weather");
        let d = m.dimension("weather.condition").unwrap();
        assert_eq!(d.label.as_deref(), Some("Weather · Condition"));
        assert_eq!(m.dimension("trips.pickup_borough").unwrap().label.as_deref(), Some("Taxi · Pickup borough"));
        assert!(m.dimension("weather.borough").unwrap().hidden, "the base's key stands for it");
        assert!(m.measure("weather.avg_temperature").is_some());
        assert!(m.measure("trips.zones").is_none_or(|m| m.hidden), "distinct counts don't add up");
        assert_eq!(m.hierarchies["trips.location"], ["trips.pickup_borough", "trips.pickup_zone"]);
    }

    #[test]
    fn one_model_runs_directly() {
        let p = plan(
            &blend(),
            &models(),
            &q(json!({"model": "trips_weather",
            "dimensions": ["weather.condition"], "measures": ["weather.avg_temperature"],
            "filters": [{"field": "weather.borough", "values": ["Manhattan"]}], "sort": ["-weather.avg_temperature"]})),
            now(),
        )
        .unwrap();
        let Plan::Single { model, query } = p else { panic!("one model: no join") };
        assert_eq!(model, "weather");
        assert_eq!(query.dimensions, ["condition"]);
        assert_eq!(query.measures, ["avg_temperature"]);
        assert_eq!(query.filters[0].field, "borough");
        assert_eq!(query.sort, ["-avg_temperature"]);

        let mut cols = vec![crate::compiler::Column {
            name: "avg_temperature__change".into(),
            field: "avg_temperature__change".into(),
            label: "x".into(),
            kind: "measure",
            kind_type: "number".into(),
            format: None,
            grain: None,
            compare: Some("change"),
            base: Some("avg_temperature".into()),
            higher_is_better: None,
            additive: None,
        }];
        prefix_columns("weather", "Weather", &mut cols);
        assert_eq!(
            (cols[0].name.as_str(), cols[0].field.as_str(), cols[0].base.as_deref()),
            ("weather.avg_temperature__change", "weather.avg_temperature__change", Some("weather.avg_temperature"))
        );
    }

    #[test]
    fn aggregates_per_model_then_joins() {
        let j = joined(json!({"model": "trips_weather", "dimensions": ["weather.condition"],
            "measures": ["trips.trips", "weather.avg_temperature"]}));
        assert_eq!(j.sides.len(), 2);
        let (t, w) = (&j.sides[0], &j.sides[1]);
        assert_eq!(t.query.dimensions, ["pickup_borough", "pickup_at:hour"]);
        assert_eq!(t.query.measures, ["trips"]);
        assert_eq!(w.query.dimensions, ["borough", "observed_at:hour", "condition"]);
        assert_eq!(w.query.measures, ["avg_temperature__sum", "avg_temperature__count"]);
        assert_eq!(t.query.limit, MAX_SOURCE_ROWS);
        assert!(!w.inner);

        // each side compiles on its own model, in its own dialect
        let wsql = compile_query_up_to(&w.source, &w.query, Dialect::DuckDb, now(), &[], MAX_SOURCE_ROWS).unwrap().sql;
        assert!(wsql.contains("SUM((temperature_c)) AS \"avg_temperature__sum\""), "{wsql}");
        assert!(wsql.contains("COUNT((temperature_c)) AS \"avg_temperature__count\""), "{wsql}");

        // the join, then the person's query over it on ScramDB
        let m = j.joined_model(&tables(2));
        let sql = m.sql.as_deref().unwrap();
        assert!(
            sql.contains(
                "FROM magnum_cache.t0 AS s0\nLEFT JOIN magnum_cache.t1 AS s1 ON s1.c0 = s0.c0 AND s1.c1 = s0.c1"
            ),
            "{sql}"
        );
        let c = compile_query(&m, &j.query, Dialect::Postgres, now(), &[]).unwrap();
        assert!(c.sql.contains("SUM((\"trips.trips\")) AS \"trips.trips\""), "{}", c.sql);
        assert!(
            c.sql
                .contains("SUM(\"weather.avg_temperature__sum\") / NULLIF(SUM(\"weather.avg_temperature__count\"), 0)"),
            "{}",
            c.sql
        );
        assert_eq!(c.columns[0].name, "weather.condition");
        assert_eq!(c.columns[0].label, "Weather · Condition");

        // trips' measures would double if weather had several rows per borough-hour: checked
        let checks = j.fanout_checks(&tables(2));
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].0, "SELECT 1 FROM magnum_cache.t1 GROUP BY c0, c1 HAVING COUNT(*) > 1 LIMIT 1");
        assert!(checks[0].1.contains("Weather"), "{}", checks[0].1);
    }

    #[test]
    fn base_rows_finer_than_the_keys_are_checked_too() {
        let j = joined(json!({"model": "trips_weather", "dimensions": ["trips.pickup_zone"],
            "measures": ["weather.max_gust"]}));
        let checks = j.fanout_checks(&tables(2));
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].0, "SELECT 1 FROM magnum_cache.t0 GROUP BY c0, c1 HAVING COUNT(*) > 1 LIMIT 1");
        // nothing to check when no model has finer rows than the keys
        let j = joined(json!({"model": "trips_weather", "dimensions": ["trips.pickup_borough"],
            "measures": ["trips.trips", "weather.max_gust"]}));
        assert!(j.fanout_checks(&tables(2)).is_empty());
    }

    #[test]
    fn keys_from_either_side_are_one_column() {
        let j =
            joined(json!({"model": "trips_weather", "dimensions": ["weather.observed_at:day", "weather.condition"],
            "measures": ["trips.trips"]}));
        assert_eq!(j.sides[0].query.dimensions, ["pickup_borough", "pickup_at:hour"], "no extra base column");
        let c = compile_query(&j.joined_model(&tables(2)), &j.query, Dialect::Postgres, now(), &[]).unwrap();
        assert!(
            c.sql.contains("DATE_TRUNC('day', (\"trips.pickup_at\")) AS \"weather.observed_at__day\""),
            "{}",
            c.sql
        );
        // finer than the key: those values don't exist
        let e = plan(
            &blend(),
            &models(),
            &q(json!({"model": "trips_weather",
            "dimensions": ["trips.pickup_at:week", "weather.condition"]})),
            now(),
        );
        assert!(e.is_ok(), "weeks nest hours");
    }

    #[test]
    fn filters_go_to_their_models() {
        let j =
            joined(json!({"model": "trips_weather", "dimensions": ["trips.pickup_zone"], "measures": ["trips.trips"],
            "filters": [
                {"field": "weather.borough", "values": ["Manhattan"]},
                {"field": "weather.condition", "values": ["Rain"]},
                {"field": "trips.trips", "op": "gt", "values": [100]}
            ]}));
        let (t, w) = (&j.sides[0], &j.sides[1]);
        // a key: both models
        assert!(t.query.filters.iter().any(|f| f.field == "pickup_borough" && f.values == [json!("Manhattan")]));
        assert!(w.query.filters.iter().any(|f| f.field == "borough"));
        // a joined model's own field: there, and only matching rows count
        assert!(w.query.filters.iter().any(|f| f.field == "condition"));
        assert!(!t.query.filters.iter().any(|f| f.field.contains("condition")));
        assert!(w.inner);
        // a measure: after the join
        assert_eq!(j.query.filters.len(), 1);
        assert_eq!(j.query.filters[0].field, "trips.trips");
        let m = j.joined_model(&tables(2));
        assert!(m.sql.unwrap().contains("INNER JOIN magnum_cache.t1 AS s1"));
    }

    #[test]
    fn periods_on_a_time_key() {
        let j = joined(json!({"model": "trips_weather", "dimensions": ["trips.pickup_at:day"],
            "measures": ["trips.trips", "weather.avg_temperature"],
            "period": {"field": "trips.pickup_at", "current": {"from": "2026-07-01", "to": "2026-07-31"},
                       "compare": "previous_year"}}));
        // both windows are fetched from every model; the comparison happens after the join
        for (s, field) in [(&j.sides[0], "pickup_at"), (&j.sides[1], "observed_at")] {
            let f: Vec<(&str, &str)> =
                s.query.filters.iter().map(|f| (f.op.as_str(), f.values[0].as_str().unwrap())).collect();
            assert_eq!(f, [("gte", "2025-07-02 00:00:00"), ("lt", "2026-08-01 00:00:00")], "{field}");
            assert!(s.query.filters.iter().all(|f| f.field == field));
            assert!(s.query.period.is_none());
        }
        assert_eq!(j.query.period.as_ref().unwrap().field, "trips.pickup_at");
        let c = compile_query(&j.joined_model(&tables(2)), &j.query, Dialect::Postgres, now(), &[]).unwrap();
        assert!(c.columns.iter().any(|c| c.name == "trips.trips__change"));

        let e = plan(
            &blend(),
            &models(),
            &q(json!({"model": "trips_weather", "dimensions": ["weather.condition"],
            "measures": ["trips.trips"], "period": {"field": "weather.observed_on", "current": {"last_n_days": 7}}})),
            now(),
        )
        .unwrap_err();
        assert!(e.0.contains("join"), "{}", e.0);
    }

    #[test]
    fn what_cannot_be_blended() {
        let e = |v: Value| plan(&blend(), &models(), &q(v), now()).unwrap_err().0;
        assert!(
            e(json!({"model": "trips_weather", "dimensions": ["weather.condition"], "measures": ["trips.zones"]}))
                .contains("can't be combined")
        );
        assert!(e(json!({"model": "trips_weather", "dimensions": ["nope.x"]})).contains("nope"));
        assert!(e(json!({"model": "trips_weather", "dimensions": ["trips.nope"]})).contains("nope"));
        assert!(e(json!({"model": "trips_weather", "dimensions": ["condition"]})).contains("model.field"));
        assert!(
            e(json!({"model": "trips_weather", "dimensions": ["weather.condition", "weather.observed_on:month",
            "weather.observed_on:year"], "measures": ["trips.trips"]}))
            .contains("once")
        );
        // distinct counts are fine on one model
        assert!(
            plan(&blend(), &models(), &q(json!({"model": "trips_weather", "measures": ["trips.zones"]})), now())
                .is_ok()
        );
    }

    #[test]
    fn grains_nest() {
        assert!(nests(Some("hour"), Some("week")));
        assert!(nests(Some("day"), Some("month")));
        assert!(!nests(Some("day"), Some("hour")));
        assert!(!nests(Some("week"), Some("month")));
        assert!(nests(Some("month"), Some("year")));
        assert!(nests(None, Some("day")));
    }

    #[test]
    fn formulas_on_totals_run_after_the_join() {
        let j = joined(json!({"model": "trips_weather", "dimensions": ["weather.condition"], "measures": ["per_hour"],
            "formulas": [{"name": "per_hour", "label": "Trips per hour", "expr": "trips.trips / weather.hours"}],
            "filters": [{"field": "per_hour", "op": "gt", "values": [1]}], "sort": ["-per_hour"]}));
        // each model brings what the formula uses, aggregated on its own engine as always
        assert_eq!(j.sides[0].query.measures, ["trips"]);
        assert_eq!(j.sides[1].query.measures, ["hours"]);
        assert_eq!(j.query.measures, ["per_hour"]);
        let c = compile_query(&j.joined_model(&tables(2)), &j.query, Dialect::Postgres, now(), &[]).unwrap();
        // only the formula is shown; it's filtered on and sorted by after the join
        assert_eq!(c.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["weather.condition", "per_hour"]);
        assert_eq!(c.columns[1].label, "Trips per hour");
        assert!(
            c.sql.contains(
                "(CAST((SUM((\"trips.trips\"))) AS DOUBLE PRECISION) / NULLIF((SUM((\"weather.hours\"))), 0))"
            ),
            "{}",
            c.sql
        );
        assert!(c.sql.contains("HAVING") && c.sql.contains("ORDER BY"), "{}", c.sql);
    }

    #[test]
    fn formulas_on_one_model_run_on_it() {
        let p = plan(
            &blend(),
            &models(),
            &q(json!({"model": "trips_weather", "dimensions": ["trips.pickup_borough"],
            "measures": ["trips.trips", "per_trip"], "sort": ["-per_trip"],
            "formulas": [{"name": "per_trip", "expr": "trips.revenue / trips.trips"}]})),
            now(),
        )
        .unwrap();
        let Plan::Single { model, query } = p else { panic!("one model: no join") };
        assert_eq!(model, "trips");
        assert_eq!(query.measures, ["trips", "per_trip"]);
        assert_eq!(query.sort, ["-per_trip"]);
        assert_eq!(query.formulas[0].expr, "(revenue / trips)");
    }

    #[test]
    fn fields_of_each_row_go_to_their_model() {
        let j = joined(
            json!({"model": "trips_weather", "dimensions": ["long"], "measures": ["trips.trips", "weather.hours"],
            "filters": [{"field": "long", "op": "equals", "values": [true]}],
            "formulas": [{"name": "long", "label": "Long trip", "expr": "trips.fare > 20", "per_row": true}]}),
        );
        let t = &j.sides[0];
        let d = t.source.dimension("long").expect("on the trips side");
        assert_eq!((d.formula.as_deref(), d.kind.as_str()), (Some("(fare > 20)"), "boolean"));
        assert!(t.query.dimensions.contains(&"long".to_string()), "{:?}", t.query.dimensions);
        assert_eq!(t.query.filters[0].field, "long");
        // computed on that model's engine, before it aggregates
        let sql = compile_query_up_to(&t.source, &t.query, Dialect::DuckDb, now(), &[], MAX_SOURCE_ROWS).unwrap().sql;
        assert!(sql.contains("(\"fare\" > 20)"), "{sql}");
        let c = compile_query(&j.joined_model(&tables(2)), &j.query, Dialect::Postgres, now(), &[]).unwrap();
        assert_eq!((c.columns[0].name.as_str(), c.columns[0].label.as_str()), ("trips.long", "Taxi · Long trip"));
        // and on one model alone, the query's own field
        let p = plan(
            &blend(),
            &models(),
            &q(json!({"model": "trips_weather", "dimensions": ["long"], "measures": ["trips.trips"],
            "formulas": [{"name": "long", "expr": "trips.fare > 20", "per_row": true}]})),
            now(),
        )
        .unwrap();
        let Plan::Single { query, .. } = p else { panic!("one model") };
        assert_eq!((query.dimensions[0].as_str(), query.formulas[0].expr.as_str()), ("long", "(fare > 20)"));
    }

    #[test]
    fn formulas_that_cant_be_blended() {
        let e = |f: Value, measures: Value| {
            plan(
                &blend(),
                &models(),
                &q(json!({"model": "trips_weather", "dimensions": ["weather.condition"],
                "measures": measures, "formulas": f})),
                now(),
            )
            .unwrap_err()
            .0
        };
        let mixed = json!([{"name": "x", "expr": "concat(trips.pickup_borough, weather.condition)", "per_row": true}]);
        assert!(
            e(mixed, json!(["trips.trips"])).contains("one model's fields"),
            "{}",
            e(
                json!([{"name": "x", "expr": "concat(trips.pickup_borough, weather.condition)", "per_row": true}]),
                json!(["trips.trips"])
            )
        );
        // after the join the rows are totals already: adding up rows there would be wrong
        let rows = json!([{"name": "x", "expr": "sum_if(trips.fare, trips.fare > 1)"}]);
        assert!(e(rows, json!(["x"])).contains("sum_if"));
        assert!(e(json!([{"name": "x", "expr": "trips.nope / 2"}]), json!(["x"])).contains("nope"));
        assert!(e(json!([{"name": "x", "expr": "trips.zones / 2"}]), json!(["x"])).contains("can't be combined"));
    }
}
