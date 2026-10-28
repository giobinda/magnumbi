//! Organisations' own models, made in the app: proposed from a table's columns, then adjusted.
//!
//! Unlike the project's YAML models (trusted configuration, written by the platform's team),
//! these are written by an organisation's admins in the browser, so they carry no SQL: a
//! dimension is a column (by its name), a measure aggregates a column, and the model reads a
//! table by name. The compiler quotes all of them.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::compiler::{Dialect, humanize};
use crate::materialize::ColType;
use crate::schema::{Dimension, Measure, Model, NAME_RE};

/// schema.table (up to three parts), letters, digits and underscores only
static TABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_]+(\.[A-Za-z0-9_]+){0,2}$").unwrap());
/// a column, as written in a measure: bare, or quoted for its dialect
static COLUMN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^([A-Za-z0-9_]+|"[A-Za-z0-9_]+"|`[A-Za-z0-9_]+`)$"#).unwrap());
const CALENDAR: [&str; 7] = ["hour", "day", "week", "weekday", "month", "quarter", "year"];
const MEASURE_KINDS: [&str; 6] = ["count", "count_distinct", "sum", "avg", "min", "max"];

/// `schema.table` as a model names it, split into its parts (letters, digits, underscores).
pub fn table_parts(table: &str) -> Option<Vec<&str>> {
    TABLE.is_match(table).then(|| table.split('.').collect())
}

/// The rules an organisation's model follows on top of any model's: see the module's doc.
pub fn check(m: &Model) -> Result<(), String> {
    if m.extends.is_some() {
        return Err("an organisation's model can't extend another model".into());
    }
    if m.sql.is_some() {
        return Err("an organisation's model reads a table by name: no `sql`".into());
    }
    m.validate()?;
    let table = m.table.as_deref().unwrap_or_default();
    if !TABLE.is_match(table) {
        return Err(format!("invalid table name {table:?}: use schema.table, with letters, digits and underscores"));
    }
    for d in &m.dimensions {
        if d.sql.is_some() {
            return Err(format!("dimension {:?}: a dimension is a column, named by its name (no SQL)", d.name));
        }
    }
    // drill paths: a name, then two dimensions or more, each once (that they're the model's own is
    // any model's rule)
    for (h, path) in &m.hierarchies {
        if !NAME_RE.is_match(h) {
            return Err(format!("drill path {h:?}: name it with letters, digits and underscores"));
        }
        if path.len() < 2 {
            return Err(format!("drill path {h:?}: give it two dimensions or more, in drilling order"));
        }
        if let Some(twice) = path.iter().enumerate().find(|(i, d)| path[..*i].contains(d)).map(|(_, d)| d) {
            return Err(format!("drill path {h:?}: {twice:?} is in it twice"));
        }
    }
    for x in &m.measures {
        if !MEASURE_KINDS.contains(&x.kind.as_str()) {
            return Err(format!("measure {:?}: use count, count_distinct, sum, avg, min or max", x.name));
        }
        if let Some(sql) = &x.sql
            && !COLUMN.is_match(sql.trim())
        {
            return Err(format!("measure {:?}: name a column to aggregate, not SQL", x.name));
        }
    }
    Ok(())
}

/// Write each measure's column the way its connection quotes it (the editor sends bare names).
pub fn quote_columns(m: &mut Model, dialect: Dialect) {
    for x in &mut m.measures {
        if let Some(sql) = &x.sql {
            x.sql = Some(dialect.ident(sql.trim().trim_matches(|c| c == '"' || c == '`')));
        }
    }
}

/// A model for a table, from its columns: text, true/false and dates are dimensions, numbers are
/// summed (ids are dimensions), plus a count of rows. Columns whose names can't be fields are
/// left out and returned.
pub fn propose(
    name: &str,
    connection: &str,
    table: &str,
    dialect: Dialect,
    columns: &[(String, ColType)],
) -> (Model, Vec<String>) {
    let mut dimensions = vec![];
    let mut measures = vec![];
    let mut skipped = vec![];
    for (col, ty) in columns {
        if !NAME_RE.is_match(col) {
            skipped.push(col.clone());
            continue;
        }
        let c = col.to_lowercase();
        let id = c == "id" || c.ends_with("_id");
        // numbers people group by, not add up: hour, day, week, month, quarter, year, weekday
        let calendar = CALENDAR.iter().any(|p| c == *p || c.ends_with(&format!("_{p}")));
        let kind = match ty {
            ColType::Text => "string",
            ColType::Bool => "boolean",
            ColType::Date => "date",
            ColType::Timestamptz => "time",
            ColType::Timestamp => "datetime",
            ColType::Int | ColType::Float | ColType::Numeric if id => "string",
            ColType::Int if calendar => "number",
            ColType::Int | ColType::Float | ColType::Numeric => {
                measures.push(measure(col, "sum", Some(dialect.ident(col))));
                continue;
            }
            ColType::Json => {
                skipped.push(col.clone());
                continue;
            }
        };
        dimensions.push(Dimension {
            name: col.clone(),
            label: Some(humanize(col)),
            description: None,
            kind: kind.into(),
            sql: None,
            formula: None,
            hidden: false,
            aggregation: None,
            synonyms: vec![],
            tags: vec![],
        });
    }
    let taken = |n: &str| columns.iter().any(|(c, _)| c == n);
    let count = ["count", "row_count", "records"].into_iter().find(|n| !taken(n)).unwrap_or("magnum_count");
    measures.insert(0, Measure { label: Some("Count".into()), ..measure(count, "count", None) });
    let model = Model {
        name: name.into(),
        label: Some(humanize(name)),
        description: None,
        extends: None,
        connection: connection.into(),
        table: Some(table.into()),
        sql: None,
        dimensions,
        measures,
        hierarchies: BTreeMap::new(),
        cache: None,
        tags: vec![],
    };
    (model, skipped)
}

fn measure(name: &str, kind: &str, sql: Option<String>) -> Measure {
    Measure {
        name: name.into(),
        label: Some(humanize(name)),
        description: None,
        kind: kind.into(),
        sql,
        formula: None,
        format: "number".into(),
        higher_is_better: true,
        hidden: false,
        synonyms: vec![],
        tags: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Dialect;
    use crate::materialize::ColType;

    fn cols(list: &[(&str, ColType)]) -> Vec<(String, ColType)> {
        list.iter().map(|(n, t)| (n.to_string(), *t)).collect()
    }

    #[test]
    fn proposes_dimensions_and_measures_from_columns() {
        let columns = cols(&[
            ("trip_id", ColType::Int),
            ("pickup_borough", ColType::Text),
            ("picked_up_at", ColType::Timestamptz),
            ("sold_on", ColType::Date),
            ("is_shared", ColType::Bool),
            ("fare", ColType::Numeric),
            ("Passengers", ColType::Int),
            ("drop off", ColType::Text), // no name Magnum BI can use
        ]);
        let (m, skipped) = propose("trips", "warehouse", "public.trips", Dialect::Postgres, &columns);
        assert_eq!(
            (m.name.as_str(), m.connection.as_str(), m.table.as_deref()),
            ("trips", "warehouse", Some("public.trips"))
        );
        assert_eq!(m.label.as_deref(), Some("Trips"));
        let dims: Vec<(&str, &str)> = m.dimensions.iter().map(|d| (d.name.as_str(), d.kind.as_str())).collect();
        assert_eq!(
            dims,
            [
                ("trip_id", "string"),
                ("pickup_borough", "string"),
                ("picked_up_at", "time"),
                ("sold_on", "date"),
                ("is_shared", "boolean")
            ]
        );
        let measures: Vec<(&str, &str, Option<&str>)> =
            m.measures.iter().map(|x| (x.name.as_str(), x.kind.as_str(), x.sql.as_deref())).collect();
        assert_eq!(
            measures,
            [
                ("count", "count", None),
                ("fare", "sum", Some("\"fare\"")),
                ("Passengers", "sum", Some("\"Passengers\""))
            ]
        );
        assert_eq!(skipped, ["drop off"]);
        assert!(check(&m).is_ok(), "{:?}", check(&m));
        // BigQuery quotes with backticks
        let (bq, _) = propose("trips", "warehouse", "trips", Dialect::BigQuery, &columns);
        assert_eq!(bq.measures[1].sql.as_deref(), Some("`fare`"));
        assert!(check(&bq).is_ok());
    }

    #[test]
    fn organisations_models_name_columns_never_sql() {
        let (good, _) =
            propose("t", "warehouse", "t", Dialect::Postgres, &cols(&[("a", ColType::Text), ("n", ColType::Int)]));
        let refused = |edit: &dyn Fn(&mut Model)| {
            let mut m = good.clone();
            edit(&mut m);
            check(&m).unwrap_err()
        };
        assert!(refused(&|m| m.sql = Some("SELECT * FROM secrets".into())).contains("table"));
        assert!(
            refused(&|m| {
                m.table = Some("t; DROP TABLE x".into());
            })
            .contains("table")
        );
        assert!(refused(&|m| m.extends = Some("sales".into())).contains("extend"));
        assert!(refused(&|m| m.dimensions[0].sql = Some("pg_read_file('/etc/passwd')".into())).contains("column"));
        assert!(refused(&|m| m.measures[1].sql = Some("SUM(n) + (SELECT 1)".into())).contains("column"));
        assert!(refused(&|m| m.measures[1].kind = "number".into()).contains("count, count_distinct"));
        assert!(refused(&|m| m.connection = String::new()).contains("connection"));
    }

    #[test]
    fn they_join_the_project_without_replacing_anything_and_are_the_organisations() {
        let (sales, _) = propose("sales", "warehouse", "sales", Dialect::Postgres, &cols(&[("region", ColType::Text)]));
        let project = crate::project::ProjectState {
            models: [("sales".to_string(), sales.clone())].into(),
            ..Default::default()
        };
        let (trips, _) =
            propose("trips", "warehouse", "trips", Dialect::Postgres, &cols(&[("borough", ColType::Text)]));
        let mut sneaky = trips.clone();
        sneaky.name = "sneaky".into();
        sneaky.sql = Some("SELECT * FROM secrets".into());
        let p = project.with_org_models(&[sales.clone(), trips, sneaky]);
        assert!(p.models.contains_key("trips") && p.org_models.contains("trips"));
        assert!(!p.org_models.contains("sales"), "a platform model stays the platform's");
        assert!(!p.models.contains_key("sneaky"));
        assert!(p.errors.contains_key("model sales") && p.errors.contains_key("model sneaky"), "{:?}", p.errors);

        // an organisation limited to some of the platform's models still has its own
        let scope = crate::access::Scope {
            role: Some(crate::access::Role::Viewer),
            org: Some("acme".into()),
            models: Some(["sales".to_string()].into()),
            ..Default::default()
        };
        assert!(crate::access::model_available(&scope, &p, "trips"));
        assert!(crate::access::model_available(&scope, &p, "sales"));
        assert!(!crate::access::model_available(&crate::access::Scope::none(), &p, "trips"));
    }

    #[test]
    fn restrictions_follow_the_data_into_the_organisations_models() {
        // the platform's `sales` and an organisation's model on the same table
        let columns = cols(&[("region", ColType::Text), ("amount", ColType::Numeric)]);
        let (sales, _) = propose("sales", "warehouse", "sample_sales", Dialect::Postgres, &columns);
        let (mine, _) = propose("my_sales", "warehouse", "sample_sales", Dialect::Postgres, &columns);
        let project =
            crate::project::ProjectState { models: [("sales".to_string(), sales)].into(), ..Default::default() };
        let p = project.with_org_models(&[mine]);
        let rule =
            crate::access::RowRule { model: "sales".into(), dimension: "region".into(), values: vec!["EMEA".into()] };
        let scope = crate::access::Scope {
            role: Some(crate::access::Role::Viewer),
            org: Some("acme".into()),
            personal_filters: vec![rule],
            ..Default::default()
        };
        let filters = crate::access::row_filters_for(&scope, &p, "my_sales").unwrap();
        assert_eq!(filters.len(), 1, "a restriction on the table's data covers a new model on it: {filters:?}");
        assert_eq!(filters[0].field, "region");
        // the same table written another way (with its schema) is still the same data
        let (other, _) = propose("my_sales_2", "warehouse", "public.SAMPLE_SALES", Dialect::Postgres, &columns);
        let p = p.with_org_models(&[other]);
        assert_eq!(crate::access::row_filters_for(&scope, &p, "my_sales_2").unwrap().len(), 1);
    }

    #[test]
    fn measures_columns_are_quoted_for_their_connection() {
        let (mut m, _) = propose("t", "warehouse", "t", Dialect::Postgres, &cols(&[("amount", ColType::Int)]));
        m.measures[1].sql = Some("amount".into()); // as the editor sends it
        let mut bq = m.clone();
        quote_columns(&mut m, Dialect::Postgres);
        quote_columns(&mut bq, Dialect::BigQuery);
        assert_eq!(m.measures[1].sql.as_deref(), Some("\"amount\""));
        assert_eq!(bq.measures[1].sql.as_deref(), Some("`amount`"));
        assert_eq!(m.measures[0].sql, None, "count has no column");
    }

    #[test]
    fn numbers_that_are_calendar_parts_are_dimensions() {
        let columns = cols(&[
            ("pickup_hour", ColType::Int),
            ("year", ColType::Int),
            ("weekday", ColType::Int),
            ("fare", ColType::Float),
        ]);
        let (m, _) = propose("t", "warehouse", "t", Dialect::Postgres, &columns);
        let dims: Vec<(&str, &str)> = m.dimensions.iter().map(|d| (d.name.as_str(), d.kind.as_str())).collect();
        assert_eq!(dims, [("pickup_hour", "number"), ("year", "number"), ("weekday", "number")]);
        assert_eq!(m.measures.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["count", "fare"]);
    }

    #[test]
    fn the_platforms_app_models_are_the_platforms_and_an_organisations_own_come_first() {
        let columns = cols(&[("region", ColType::Text), ("amount", ColType::Numeric)]);
        let (sales, _) = propose("sales", "warehouse", "sample_sales", Dialect::Postgres, &columns);
        let (shared, _) = propose("shared", "warehouse", "public.sample_sales", Dialect::Postgres, &columns);
        let (clash, _) = propose("sales", "warehouse", "other", Dialect::Postgres, &columns);
        let project =
            crate::project::ProjectState { models: [("sales".to_string(), sales)].into(), ..Default::default() };
        let p = project.with_platform_models(&[shared.clone(), clash], &[]);
        assert!(p.models.contains_key("shared") && !p.org_models.contains("shared"));
        assert!(p.errors.contains_key("model sales"), "a YAML model keeps its name: {:?}", p.errors);
        // like the platform's YAML models, they follow each organisation's list of models
        let scope = crate::access::Scope {
            role: Some(crate::access::Role::Viewer),
            org: Some("acme".into()),
            models: Some(["sales".to_string()].into()),
            ..Default::default()
        };
        assert!(!crate::access::model_available(&scope, &p, "shared"));
        // restrictions on the table's data cover them, however the table is written
        let rule =
            crate::access::RowRule { model: "sales".into(), dimension: "region".into(), values: vec!["EMEA".into()] };
        let scope = crate::access::Scope { models: None, personal_filters: vec![rule], ..scope };
        assert_eq!(crate::access::row_filters_for(&scope, &p, "shared").unwrap().len(), 1);
        // an organisation's own model of the same name stays its own (the platform's is left out there)
        let (mine, _) = propose("shared", "warehouse", "mine", Dialect::Postgres, &columns);
        let p = project.with_platform_models(&[shared], &["shared".to_string()]).with_org_models(&[mine]);
        assert_eq!(p.models["shared"].table.as_deref(), Some("mine"));
        assert!(p.org_models.contains("shared"));
    }
}
