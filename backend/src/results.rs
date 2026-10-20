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
use crate::schema::{ADHOC_MEASURE, Formula, Query};

/// A query with calculations on its results, taken apart.
#[derive(Debug)]
pub struct Split {
    /// what runs on the engine
    pub inner: Query,
    pub calcs: Vec<Formula>,
    /// measures the calculations use that the chart doesn't show
    pub hidden: Vec<String>,
}

/// The query's calculations on results apart from the rest (None: it has none).
pub fn split(q: &Query) -> Result<Option<Split>, String> {
    let mut calcs = vec![];
    let mut others = vec![];
    for f in &q.formulas {
        if !f.per_row && formula::is_on_results(&f.expr).map_err(|e| format!("{}: {e}", f.name))? {
            calcs.push(f.clone());
        } else {
            others.push(f.clone());
        }
    }
    if calcs.is_empty() {
        return Ok(None);
    }
    let is_calc = |name: &str| calcs.iter().any(|c: &Formula| c.name == name);
    if let Some(f) = q.filters.iter().find(|f| is_calc(&f.field)) {
        return Err(format!("{}: a calculation on results can't be filtered on (yet)", f.field));
    }
    if let Some(t) = q.top.as_ref().filter(|t| is_calc(&t.by)) {
        return Err(format!("{}: top N can't rank by a calculation on results; rank by a measure", t.by));
    }
    let mut inner = q.clone();
    inner.formulas = others;
    inner.measures.retain(|m| !is_calc(m));
    inner.sort.retain(|s| !is_calc(s.trim_start_matches(['-', '+'])));
    let mut hidden = vec![];
    for c in &calcs {
        for r in formula::references(&c.expr).map_err(|e| format!("{}: {e}", c.name))? {
            if is_calc(&r) {
                return Err(format!("{}: uses {r}, another calculation on results; use the measure itself", c.name));
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
    Ok(Some(Split { inner, calcs, hidden }))
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
    };
    let mut selects = vec!["\"_row\"".to_string()];
    for c in calcs {
        let (expr, _) = formula::compile_on_results(&c.expr, &window, &mut |name| {
            let wanted = column_name(name);
            match columns.iter().position(|x| x.name == wanted) {
                Some(i) if columns[i].kind == "measure" => Ok((col(i), Type::Number)),
                Some(_) => Err(format!("{name} is a dimension; calculations on results use measures")),
                None => Err(format!("unknown field {name:?}")),
            }
        })
        .map_err(|e| format!("{}: {e}", c.name))?;
        selects.push(expr);
    }
    Ok(format!("SELECT {} FROM {table} ORDER BY \"_row\"", selects.join(", ")))
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
                .map(|&(i, desc)| {
                    let (x, y) = (a[i].as_f64(), b[i].as_f64());
                    // empty values last, either way
                    match (x, y) {
                        (Some(x), Some(y)) => {
                            if desc {
                                y.total_cmp(&x)
                            } else {
                                x.total_cmp(&y)
                            }
                        }
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, _) => std::cmp::Ordering::Greater,
                        (_, None) => std::cmp::Ordering::Less,
                    }
                })
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    (out_columns, out_rows)
}

/// Shares are shown as percentages unless the formula says otherwise.
fn default_format(expr: &str) -> &'static str {
    let e = expr.trim().to_lowercase();
    let Some(inside) = e.strip_prefix("percent_of_total(") else { return "number" };
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
        // what can't be done (yet)
        let e = |v: Value| split(&q(v)).unwrap_err();
        let running = json!([{"name": "running", "expr": "running_sum(revenue)"}]);
        assert!(
            e(json!({"model": "sales", "measures": ["revenue"], "formulas": running,
            "filters": [{"field": "running", "op": "gt", "values": [1]}]}))
            .contains("filtered")
        );
        assert!(
            e(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"], "formulas": running,
            "top": {"dimension": "region", "n": 2, "by": "running"}}))
            .contains("top N")
        );
        let chained = json!([{"name": "a", "expr": "running_sum(revenue)"}, {"name": "b", "expr": "rank(a)"}]);
        assert!(e(json!({"model": "sales", "measures": ["revenue"], "formulas": chained})).contains("another"));
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
}
