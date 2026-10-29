//! Calculations on a chart's results: running totals, moving averages, rank, % of total and the
//! value before (formula.rs's result functions).
//!
//! The chart's query runs as always (on its engine, or from the cache) without them. Then its
//! result is copied into a work table on the organisation's ScramDB, where the calculations run
//! as window functions, keyed by row. Their values join the rows as new columns; the result's
//! own values are never read back, so they keep their exact form. Changing a calculation never
//! queries the warehouse again when the result is cached.

use serde_json::Value;

use crate::compiler::Column;
use crate::formula::{self, Type, Window};
use crate::materialize::ColType;
use crate::schema::{ADHOC_MEASURE, Filter, Formula, Query, TopN};

/// A query with calculations on its results, taken apart.
#[derive(Debug)]
pub struct Split {
    /// what runs on the engine
    pub inner: Query,
    pub calcs: Vec<Formula>,
    /// measures the calculations use that the chart doesn't show
    pub hidden: Vec<String>,
    /// filters on calculations: applied to the result's rows once they're calculated
    pub filters: Vec<Filter>,
    /// top N by a calculation (charts with one dimension): its best rows
    pub top: Option<TopN>,
}

/// How a filter on a calculation (a number) may compare, and how many values it needs.
const NUMBER_OPS: [(&str, usize); 9] = [
    ("equals", 1),
    ("not_equals", 1),
    ("gt", 1),
    ("gte", 1),
    ("lt", 1),
    ("lte", 1),
    ("between", 2),
    ("is_null", 0),
    ("not_null", 0),
];

/// The query's calculations on results apart from the rest (None: it has none).
pub fn split(q: &Query) -> Result<Option<Split>, String> {
    let names = calculations(&q.formulas)?;
    let (calcs, others): (Vec<Formula>, Vec<Formula>) =
        q.formulas.iter().cloned().partition(|f| names.contains(&f.name));
    if calcs.is_empty() {
        return Ok(None);
    }
    layers_of(&calcs)?;
    let is_calc = |name: &str| calcs.iter().any(|c: &Formula| c.name == name);
    let (filters, kept): (Vec<Filter>, Vec<Filter>) = q.filters.iter().cloned().partition(|f| is_calc(&f.field));
    for f in &filters {
        let Some((_, need)) = NUMBER_OPS.iter().find(|(op, _)| *op == f.op) else {
            return Err(format!(
                "{}: a calculation on results is a number: filter it with =, ≠, <, ≤, >, ≥, between or empty",
                f.field
            ));
        };
        if f.values.len() < *need {
            return Err(format!("filter {} {} needs {need} value(s)", f.field, f.op));
        }
    }
    let top = q.top.clone().filter(|t| is_calc(&t.by));
    if let Some(t) = &top
        && q.dimensions.len() != 1
    {
        return Err(format!(
            "{}: top N by a calculation on results needs a chart with one dimension (it differs from row to row \
             within a value); rank by a measure",
            t.by
        ));
    }
    let mut inner = q.clone();
    inner.formulas = others;
    inner.filters = kept;
    if top.is_some() {
        inner.top = None;
    }
    inner.measures.retain(|m| !is_calc(m));
    inner.sort.retain(|s| !is_calc(s.trim_start_matches(['-', '+'])));
    let mut hidden = vec![];
    for c in &calcs {
        for r in formula::references(&c.expr).map_err(|e| format!("{}: {e}", c.name))? {
            if is_calc(&r) {
                continue; // calculated before it (sql: in layers)
            }
            // a comparison column comes with its measure
            let measure = r.strip_suffix("__previous").or_else(|| r.strip_suffix("__change")).unwrap_or(&r).to_string();
            let shown = inner.measures.contains(&measure) || inner.formulas.iter().any(|f| f.name == measure);
            if !shown && !inner.dimensions.contains(&measure) {
                hidden.push(column_name(&measure));
                inner.measures.push(measure);
            }
        }
    }
    Ok(Some(Split { inner, calcs, hidden, filters, top }))
}

/// The formulas that calculate on a chart's results: those calling a function on results, and
/// those using one of them (`revenue / so_far`), which can only run after it.
pub fn calculations(formulas: &[Formula]) -> Result<Vec<String>, String> {
    let mut names = vec![];
    for f in formulas.iter().filter(|f| !f.per_row) {
        if formula::is_on_results(&f.expr).map_err(|e| format!("{}: {e}", f.name))? {
            names.push(f.name.clone());
        }
    }
    loop {
        let before = names.len();
        for f in formulas.iter().filter(|f| !f.per_row) {
            if names.contains(&f.name) {
                continue;
            }
            if formula::references(&f.expr).map_err(|e| format!("{}: {e}", f.name))?.iter().any(|r| names.contains(r)) {
                names.push(f.name.clone());
            }
        }
        if names.len() == before {
            return Ok(names);
        }
    }
}

/// The calculations in the order they can run: each layer uses only those in the layers before.
fn layers_of(calcs: &[Formula]) -> Result<Vec<Vec<usize>>, String> {
    let refs: Vec<Vec<String>> = calcs
        .iter()
        .map(|c| formula::references(&c.expr).map_err(|e| format!("{}: {e}", c.name)))
        .collect::<Result<_, _>>()?;
    let index = |name: &str| calcs.iter().position(|c| c.name == name);
    let mut done = vec![false; calcs.len()];
    let mut layers = vec![];
    while done.iter().any(|d| !d) {
        let ready: Vec<usize> = (0..calcs.len())
            .filter(|&i| !done[i] && refs[i].iter().filter_map(|r| index(r)).all(|j| done[j]))
            .collect();
        if ready.is_empty() {
            let stuck = (0..calcs.len()).find(|&i| !done[i]).expect("one left");
            return Err(format!("{}: uses itself, through the calculations it uses", calcs[stuck].name));
        }
        for &i in &ready {
            done[i] = true;
        }
        layers.push(ready);
    }
    Ok(layers)
}

/// The result's column a formula's field is: "sum(qty)" is the column `sum__qty`.
fn column_name(field: &str) -> String {
    match ADHOC_MEASURE.captures(field) {
        Some(c) => format!("{}__{}", &c[1], &c[2]),
        None => field.to_string(),
    }
}

/// The work table's column types: numbers as doubles, the rest as they sort (dates as ISO text).
pub fn column_types(columns: &[Column]) -> Vec<ColType> {
    columns
        .iter()
        .map(|c| match (c.kind, c.kind_type.as_str()) {
            ("measure", _) | (_, "number") => ColType::Float,
            (_, "boolean") => ColType::Bool,
            _ => ColType::Text,
        })
        .collect()
}

/// ScramDB SQL for the calculations over the result's work table (columns c0, c1, ... and
/// `_row`): one row per result row, `_row` first.
pub fn sql(table: &str, columns: &[Column], calcs: &[Formula]) -> Result<String, String> {
    let col = |i: usize| format!("\"c{i}\"");
    let dims: Vec<usize> = (0..columns.len()).filter(|&i| columns[i].kind == "dimension").collect();
    let along = dims
        .iter()
        .copied()
        .find(|&i| crate::schema::is_temporal(&columns[i].kind_type) || columns[i].grain.is_some())
        .or(dims.first().copied());
    let window = Window {
        along: along.map(col).unwrap_or_default(),
        across: dims.iter().filter(|&&i| Some(i) != along).map(|&i| col(i)).collect(),
        // by field name, whatever the grain: along(sold_at) for sold_at:month
        dims: dims
            .iter()
            .map(|&i| (columns[i].field.split(':').next().unwrap_or_default().to_string(), col(i)))
            .collect(),
    };
    // in layers: a calculation runs once those it uses have (each one's value: column k<i>)
    let index = |name: &str| calcs.iter().position(|c| c.name == name);
    let mut layers: Vec<Vec<(usize, String)>> = vec![];
    for ready in layers_of(calcs)? {
        let mut layer = vec![];
        for i in ready {
            let c = &calcs[i];
            let (expr, _) = formula::compile_on_results(&c.expr, &window, &mut |name| {
                if let Some(j) = index(name) {
                    return Ok((format!("\"k{j}\""), Type::Number));
                }
                let wanted = column_name(name);
                match columns.iter().position(|x| x.name == wanted) {
                    Some(i) if columns[i].kind == "measure" => Ok((col(i), Type::Number)),
                    Some(_) => Err(format!("{name} is a dimension; calculations on results use measures")),
                    None => Err(format!("unknown field {name:?}")),
                }
            })
            .map_err(|e| format!("{}: {e}", c.name))?;
            layer.push((i, expr));
        }
        layers.push(layer);
    }
    if let [layer] = layers.as_slice() {
        let selects: Vec<String> = layer.iter().map(|(_, e)| e.clone()).collect();
        return Ok(format!("SELECT \"_row\", {} FROM {table} ORDER BY \"_row\"", selects.join(", ")));
    }
    // each layer a subquery of the one before, its columns named (ScramDB loses track of them
    // through SELECT * from one WITH part into the next)
    let mut known: Vec<String> = std::iter::once("\"_row\"".to_string()).chain((0..columns.len()).map(col)).collect();
    let mut from = table.to_string();
    for (n, layer) in layers.iter().enumerate() {
        let selects: Vec<String> =
            known.iter().cloned().chain(layer.iter().map(|(i, e)| format!("{e} AS \"k{i}\""))).collect();
        from = format!("(SELECT {} FROM {from}) AS l{}", selects.join(", "), n + 1);
        known.extend(layer.iter().map(|(i, _)| format!("\"k{i}\"")));
    }
    let values: Vec<String> = (0..calcs.len()).map(|i| format!("\"k{i}\"")).collect();
    Ok(format!("SELECT \"_row\", {} FROM {from} ORDER BY \"_row\"", values.join(", ")))
}

/// The result with the calculations' values (`values`: one row per result row, in order) as new
/// columns, without the hidden measures, sorted by calculations if the query asks.
pub fn finish(
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    values: Vec<Vec<Value>>,
    split: &Split,
    sort: &[String],
) -> (Vec<Column>, Vec<Vec<Value>>) {
    let hidden = |name: &String| split.hidden.contains(name);
    let keep: Vec<bool> = columns.iter().map(|c| !hidden(&c.name) && !c.base.as_ref().is_some_and(hidden)).collect();
    let mut out_columns: Vec<Column> = columns.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| c).collect();
    for f in &split.calcs {
        out_columns.push(Column {
            name: f.name.clone(),
            field: f.name.clone(),
            label: f.label.clone().unwrap_or_else(|| crate::compiler::humanize(&f.name)),
            kind: "measure",
            kind_type: "number".into(),
            format: Some(f.format.clone().unwrap_or_else(|| default_format(&f.expr).into())),
            grain: None,
            compare: None,
            base: None,
            higher_is_better: Some(true),
            additive: Some(false),
        });
    }
    let mut out_rows: Vec<Vec<Value>> = rows
        .into_iter()
        .zip(values)
        .map(|(row, extra)| {
            let mut r: Vec<Value> = row.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(v, _)| v).collect();
            r.extend(extra);
            r
        })
        .collect();
    let at = |name: &str| out_columns.iter().position(|c| c.name == name).expect("a calculation's column");
    for f in &split.filters {
        let i = at(&f.field);
        out_rows.retain(|r| keeps(number(&r[i]), f));
    }
    if let Some(t) = &split.top {
        let i = at(&t.by);
        let mut best: Vec<usize> = (0..out_rows.len()).collect();
        best.sort_by(|&a, &b| by_value(number(&out_rows[a][i]), number(&out_rows[b][i]), !t.bottom));
        best.truncate(usize::try_from(t.n).unwrap_or(0));
        // best first, unless the query sorts them its own way (then its order, kept)
        if !sort.is_empty() {
            best.sort_unstable();
        }
        let mut rows: Vec<Option<Vec<Value>>> = out_rows.into_iter().map(Some).collect();
        out_rows = best.into_iter().filter_map(|k| rows[k].take()).collect();
    }
    // sorted by a calculation: the engine couldn't (stable, so ties keep the query's order)
    let keys: Vec<(usize, bool)> = sort
        .iter()
        .filter_map(|s| {
            let name = s.trim_start_matches(['-', '+']);
            let i = out_columns.iter().position(|c| c.name == name)?;
            split.calcs.iter().any(|c| c.name == name).then_some((i, s.starts_with('-')))
        })
        .collect();
    if !keys.is_empty() {
        out_rows.sort_by(|a, b| {
            keys.iter()
                .map(|&(i, desc)| by_value(number(&a[i]), number(&b[i]), desc))
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    (out_columns, out_rows)
}

/// A result's number (a filter's value may come as text).
fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Ordered by value, largest first if `desc`; empty values last, either way.
fn by_value(x: Option<f64>, y: Option<f64>, desc: bool) -> std::cmp::Ordering {
    match (x, y) {
        (Some(x), Some(y)) if desc => y.total_cmp(&x),
        (Some(x), Some(y)) => x.total_cmp(&y),
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
    }
}

/// Whether a calculated value passes a filter (checked by `split`: a number's operator).
fn keeps(v: Option<f64>, f: &Filter) -> bool {
    let given: Vec<f64> = f.values.iter().filter_map(number).collect();
    // calculated values are doubles: 0.1 + 0.2 equals 0.3
    let same = |x: f64, y: f64| (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0);
    let first = given.first().copied();
    match (f.op.as_str(), v) {
        ("is_null", v) => v.is_none(),
        ("not_null", v) => v.is_some(),
        (_, None) => false,
        ("equals", Some(x)) => given.iter().any(|&y| same(x, y)),
        ("not_equals", Some(x)) => !given.iter().any(|&y| same(x, y)),
        ("gt", Some(x)) => first.is_some_and(|y| x > y),
        ("gte", Some(x)) => first.is_some_and(|y| x >= y || same(x, y)),
        ("lt", Some(x)) => first.is_some_and(|y| x < y),
        ("lte", Some(x)) => first.is_some_and(|y| x <= y || same(x, y)),
        ("between", Some(x)) => given.len() >= 2 && x >= given[0] && x <= given[1],
        _ => false,
    }
}

/// Shares are shown as percentages unless the formula says otherwise.
fn default_format(expr: &str) -> &'static str {
    let e = expr.trim().to_lowercase();
    let Some(inside) = e.strip_prefix("percent_of_total(").or_else(|| e.strip_prefix("percent_change(")) else {
        return "number";
    };
    // the whole formula is that one call: its bracket closes at the very end
    let mut depth = 1;
    for (i, c) in inside.char_indices() {
        depth += match c {
            '(' => 1,
            ')' => -1,
            _ => 0,
        };
        if depth == 0 {
            return if i == inside.len() - 1 { "percent" } else { "number" };
        }
    }
    "number"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q(v: Value) -> Query {
        serde_json::from_value(v).unwrap()
    }

    fn column(name: &str, kind: &'static str, kind_type: &str) -> Column {
        Column {
            name: name.into(),
            field: name.into(),
            label: name.into(),
            kind,
            kind_type: kind_type.into(),
            format: None,
            grain: None,
            compare: None,
            base: None,
            higher_is_better: None,
            additive: None,
        }
    }

    #[test]
    fn calculations_on_results_are_taken_apart_from_the_query() {
        let query = q(json!({"model": "sales", "dimensions": ["sold_at:month"], "measures": ["revenue", "running"],
            "sort": ["-running", "sold_at:month"],
            "formulas": [{"name": "running", "expr": "running_sum(revenue)"},
                         {"name": "share", "expr": "percent_of_total(sum(qty)) * 100"},
                         {"name": "per_sale", "expr": "revenue / transactions"}]}));
        let s = split(&query).unwrap().unwrap();
        assert_eq!(s.calcs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["running", "share"]);
        // the engine gets the rest, and the measures the calculations need
        assert_eq!(s.inner.measures, ["revenue", "sum(qty)"]);
        assert_eq!(s.inner.formulas.len(), 1);
        assert_eq!(s.inner.sort, ["sold_at:month"]);
        assert_eq!(s.hidden, ["sum__qty"]);
        // none: nothing to do
        assert!(split(&q(json!({"model": "sales", "measures": ["revenue"]}))).unwrap().is_none());
        // filtered on, and top N by, a calculation: after it runs, on the result's rows
        let running = json!([{"name": "running", "expr": "running_sum(revenue)"}]);
        let s = split(&q(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue", "running"],
            "formulas": running, "filters": [{"field": "running", "op": "gt", "values": [1]},
                                             {"field": "region", "op": "equals", "values": ["EMEA"]}],
            "top": {"dimension": "region", "n": 2, "by": "running"}})))
        .unwrap()
        .unwrap();
        assert_eq!(s.filters.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(), ["running"]);
        assert_eq!(s.inner.filters.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(), ["region"]);
        assert_eq!(s.top.as_ref().map(|t| t.n), Some(2));
        assert!(s.inner.top.is_none());
        // what can't be done
        let e = |v: Value| split(&q(v)).unwrap_err();
        assert!(
            e(json!({"model": "sales", "measures": ["revenue"], "formulas": running,
            "filters": [{"field": "running", "op": "contains", "values": ["1"]}]}))
            .contains("number")
        );
        assert!(
            e(json!({"model": "sales", "dimensions": ["region", "store"], "measures": ["revenue"], "formulas": running,
            "top": {"dimension": "region", "n": 2, "by": "running"}}))
            .contains("one dimension")
        );
        // calculations may use one another; one using itself can't be
        let circle = json!([{"name": "a", "expr": "running_sum(b)"}, {"name": "b", "expr": "rank(a)"}]);
        assert!(e(json!({"model": "sales", "measures": ["revenue"], "formulas": circle})).contains("itself"));
    }

    #[test]
    fn calculations_run_along_the_first_date_and_across_the_rest() {
        let columns = vec![
            column("region", "dimension", "string"),
            Column { grain: Some("month".into()), ..column("sold_at__month", "dimension", "date") },
            column("revenue", "measure", "number"),
            column("sum__qty", "measure", "number"),
        ];
        let calcs: Vec<Formula> = serde_json::from_value(json!([
            {"name": "running", "expr": "running_sum(revenue)"}, {"name": "share", "expr": "percent_of_total(sum(qty))"}]))
        .unwrap();
        assert_eq!(
            sql("magnum_work_1_x", &columns, &calcs).unwrap(),
            "SELECT \"_row\", SUM(\"c2\") OVER (PARTITION BY \"c0\" ORDER BY \"c1\" ROWS BETWEEN UNBOUNDED PRECEDING AND \
             CURRENT ROW), (CAST(\"c3\" AS DOUBLE PRECISION) / NULLIF(SUM(\"c3\") OVER (PARTITION BY \"c1\"), 0)) \
             FROM magnum_work_1_x ORDER BY \"_row\""
        );
        let bad: Vec<Formula> = serde_json::from_value(json!([{"name": "x", "expr": "rank(region)"}])).unwrap();
        assert!(sql("t", &columns, &bad).unwrap_err().contains("dimension"));
        assert_eq!(column_types(&columns), [ColType::Text, ColType::Text, ColType::Float, ColType::Float]);
    }

    #[test]
    fn values_join_the_rows_and_hidden_measures_go() {
        let columns = vec![column("region", "dimension", "string"), column("sum__qty", "measure", "number")];
        let rows = vec![vec![json!("EMEA"), json!(3)], vec![json!("APAC"), json!(1)], vec![json!("AMER"), json!(6)]];
        let values = vec![vec![json!(0.3)], vec![json!(0.1)], vec![json!(0.6)]];
        let s = Split {
            inner: q(json!({"model": "sales", "measures": ["sum(qty)"]})),
            calcs: serde_json::from_value(json!([{"name": "share", "expr": "percent_of_total(sum(qty))"}])).unwrap(),
            hidden: vec!["sum__qty".into()],
            filters: vec![],
            top: None,
        };
        let (cols, out) = finish(columns, rows, values, &s, &["-share".into()]);
        assert_eq!(
            cols.iter().map(|c| (c.name.as_str(), c.format.as_deref())).collect::<Vec<_>>(),
            [("region", None), ("share", Some("percent"))]
        );
        assert_eq!(
            out,
            [vec![json!("AMER"), json!(0.6)], vec![json!("EMEA"), json!(0.3)], vec![json!("APAC"), json!(0.1)]]
        );
    }

    #[test]
    fn filters_and_top_n_on_calculations_keep_the_rows_they_say() {
        let columns = vec![column("region", "dimension", "string"), column("revenue", "measure", "number")];
        let rows: Vec<Vec<Value>> =
            ["A", "B", "C", "D"].iter().zip([5, 9, 2, 7]).map(|(r, v)| vec![json!(r), json!(v)]).collect();
        let values: Vec<Vec<Value>> = [0.2, 0.39, 0.08, 0.3].iter().map(|v| vec![json!(v)]).collect();
        let base = |filters: Value, top: Value| Split {
            inner: q(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"]})),
            calcs: serde_json::from_value(json!([{"name": "share", "expr": "percent_of_total(revenue)"}])).unwrap(),
            hidden: vec![],
            filters: serde_json::from_value(filters).unwrap(),
            top: serde_json::from_value(top).unwrap(),
        };
        let regions = |s: &Split, sort: &[String]| {
            let (_, out) = finish(columns.clone(), rows.clone(), values.clone(), s, sort);
            out.iter().map(|r| r[0].as_str().unwrap().to_string()).collect::<Vec<_>>()
        };
        let f = |op: &str, v: Value| base(json!([{"field": "share", "op": op, "values": v}]), json!(null));
        assert_eq!(regions(&f("gt", json!([0.25])), &[]), ["B", "D"]);
        assert_eq!(regions(&f("lte", json!([0.2])), &[]), ["A", "C"]);
        assert_eq!(regions(&f("between", json!([0.1, 0.35])), &[]), ["A", "D"]);
        assert_eq!(regions(&f("equals", json!([0.08, 0.3])), &[]), ["C", "D"]);
        assert_eq!(regions(&f("not_equals", json!([0.08])), &[]), ["A", "B", "D"]);
        assert_eq!(regions(&f("not_null", json!([])), &[]), ["A", "B", "C", "D"]);
        // numbers given as text, as filter bars send them
        assert_eq!(regions(&f("gt", json!(["0.25"])), &[]), ["B", "D"]);
        // top N: the best rows, best first; bottom: the worst first
        let top = |n: i64, bottom: bool| {
            base(json!([]), json!({"dimension": "region", "n": n, "by": "share", "bottom": bottom}))
        };
        assert_eq!(regions(&top(2, false), &[]), ["B", "D"]);
        assert_eq!(regions(&top(2, true), &[]), ["C", "A"]);
        // the query's own sort still orders the rows kept
        assert_eq!(regions(&top(3, false), &["region".into()]), ["A", "B", "D"]);
    }

    #[test]
    fn calculations_may_use_one_another() {
        // b ranks a; c divides by a without a function of its own: still on results
        let query = q(json!({"model": "sales", "dimensions": ["sold_at:month"], "measures": ["revenue", "b", "c"],
            "formulas": [{"name": "a", "expr": "running_sum(revenue)"}, {"name": "b", "expr": "rank(a)"},
                         {"name": "c", "expr": "revenue / a"}]}));
        let s = split(&query).unwrap().unwrap();
        assert_eq!(s.calcs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(s.inner.measures, ["revenue"], "a, b and c run after the query");
        // every one a column, as a query's formulas are
        let rows = vec![vec![json!("2026-01-01"), json!(5)], vec![json!("2026-02-01"), json!(7)]];
        let values = vec![vec![json!(5), json!(2), json!(1)], vec![json!(12), json!(1), json!(0.58)]];
        let cols = vec![column("sold_at__month", "dimension", "date"), column("revenue", "measure", "number")];
        let (out_cols, out) = finish(cols, rows, values, &s, &["-a".into()]);
        assert_eq!(
            out_cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["sold_at__month", "revenue", "a", "b", "c"]
        );
        assert_eq!(out[0], [json!("2026-02-01"), json!(7), json!(12), json!(1), json!(0.58)]);
        let columns = vec![
            Column { grain: Some("month".into()), ..column("sold_at__month", "dimension", "date") },
            column("revenue", "measure", "number"),
        ];
        // in layers: each sees the ones before it
        assert_eq!(
            sql("t", &columns, &s.calcs).unwrap(),
            "SELECT \"_row\", \"k0\", \"k1\", \"k2\" FROM (SELECT \"_row\", \"c0\", \"c1\", \"k0\", \
             RANK() OVER (ORDER BY \"k0\" DESC) AS \"k1\", (CAST(\"c1\" AS DOUBLE PRECISION) / NULLIF(\"k0\", 0)) AS \"k2\" \
             FROM (SELECT \"_row\", \"c0\", \"c1\", SUM(\"c1\") OVER (ORDER BY \"c0\" ROWS BETWEEN UNBOUNDED PRECEDING \
             AND CURRENT ROW) AS \"k0\" FROM t) AS l1) AS l2 ORDER BY \"_row\""
        );
    }
}
