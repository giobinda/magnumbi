//! The data catalog's computed part: where a model and its fields are used (lineage), and
//! search across models, fields, glossary terms and dashboards. Nothing here is typed by people:
//! it's read from the models, dashboards and documentation as they are.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::schema::{ADHOC_MEASURE, Dashboard, Drill, Model, Query};

/// A dashboard that uses a model, and which of its tiles do.
#[derive(Debug, Serialize)]
pub struct DashboardUse {
    pub id: String,
    pub title: String,
    pub tiles: Vec<String>,
}

/// Where a model is used: its dashboards, and for each field where it's used
/// ("dashboard/tile", or "dashboard (filter)" for a dashboard's own filter or period).
#[derive(Debug, Default, Serialize)]
pub struct Uses {
    pub dashboards: Vec<DashboardUse>,
    pub fields: BTreeMap<String, Vec<String>>,
}

/// The fields a query names, as written: the model's own, or `model.field` on a blend.
fn refs(q: &Query, model: &Model) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let strip = |r: &str| {
        let r = r.trim_start_matches(['-', '+']);
        let r = r.split(':').next().unwrap_or(r);
        r.strip_suffix("__previous").or_else(|| r.strip_suffix("__change")).unwrap_or(r).to_string()
    };
    let mut add = |r: &str| match ADHOC_MEASURE.captures(r) {
        Some(c) => out.push(c[2].to_string()),
        None => out.push(strip(r)),
    };
    q.dimensions.iter().for_each(|r| add(r));
    q.measures.iter().for_each(|r| add(r));
    q.filters.iter().for_each(|f| add(&f.field));
    q.sort.iter().for_each(|r| add(r));
    if let Some(t) = &q.top {
        add(&t.dimension);
        add(&t.by);
    }
    if let Some(p) = &q.period {
        add(&p.field);
    }
    for f in &q.formulas {
        for r in crate::formula::references(&f.expr).unwrap_or_default() {
            add(&r);
        }
    }
    let own: Vec<&str> = q.formulas.iter().map(|f| f.name.as_str()).collect();
    out.retain(|r| !own.contains(&r.as_str()));
    // on a blend, this model's fields are written model.field
    let prefix = format!("{}.", model.name);
    out.into_iter()
        .filter_map(|r| if q.model == model.name { Some(r) } else { r.strip_prefix(&prefix).map(String::from) })
        .collect()
}

/// Where `model` is used in these dashboards (id, dashboard).
pub fn uses(model: &Model, dashboards: &[(String, Dashboard)]) -> Uses {
    let mut u = Uses::default();
    let is_field = |f: &str| model.dimension(f).is_some() || model.measure(f).is_some();
    for (id, d) in dashboards {
        let mut note = |field: &str, place: String| {
            if is_field(field) {
                let at = u.fields.entry(field.to_string()).or_default();
                if !at.contains(&place) {
                    at.push(place);
                }
            }
        };
        for f in d.filters.iter().filter(|f| f.model == model.name) {
            note(&f.field, format!("{id} (filter)"));
        }
        if let Some(p) = d.period.as_ref().filter(|p| p.model == model.name) {
            note(&p.field, format!("{id} (period)"));
        }
        let mut tiles = vec![];
        for t in &d.tiles {
            let Some(q) = &t.query else { continue };
            let mut fields = refs(q, model);
            if q.model == model.name {
                match &t.drill {
                    Some(Drill::Hierarchy(h)) => fields.extend(model.hierarchies.get(h).cloned().unwrap_or_default()),
                    Some(Drill::Path(p)) => fields.extend(p.iter().cloned()),
                    None => {}
                }
            }
            if q.model != model.name && fields.is_empty() {
                continue;
            }
            for f in &fields {
                note(f, format!("{id}/{}", t.id));
            }
            tiles.push(t.id.clone());
        }
        if !tiles.is_empty() {
            u.dashboards.push(DashboardUse { id: id.clone(), title: d.title.clone(), tiles });
        }
    }
    u
}

/// One thing the catalog search can find.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// model | field | term | dashboard
    pub kind: &'static str,
    /// a field's model
    pub model: Option<String>,
    pub name: String,
    pub label: String,
    /// other words for it: synonyms, glossary terms, tags
    pub words: Vec<String>,
    /// its description or definition
    pub text: String,
}

/// How well one word matches: names first, then other words, then descriptions (0: not at all).
fn word_score(item: &Item, word: &str) -> u32 {
    let (name, label) = (item.name.to_lowercase(), item.label.to_lowercase());
    let whole = |s: &str| s.split(|c: char| !c.is_alphanumeric()).any(|w| w == word);
    if name == word || label == word {
        12
    } else if whole(&name) || whole(&label) {
        8
    } else if name.contains(word) || label.contains(word) {
        6
    } else if item.words.iter().any(|w| w.to_lowercase().contains(word)) {
        5
    } else if item.text.to_lowercase().contains(word) {
        2
    } else {
        0
    }
}

/// The items matching every word of the query, best first (at most 50).
pub fn search<'a>(items: &'a [Item], query: &str) -> Vec<&'a Item> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).take(10).collect();
    if words.is_empty() {
        return vec![];
    }
    let mut found: Vec<(u32, &Item)> = items
        .iter()
        .filter_map(|i| {
            let scores: Vec<u32> = words.iter().map(|w| word_score(i, w)).collect();
            scores.iter().all(|s| *s > 0).then(|| (scores.iter().sum(), i))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().take(50).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Dashboard, Model};

    fn model() -> Model {
        serde_yaml::from_str(
            r#"
name: sales
connection: warehouse
table: sample_sales
dimensions:
  - {name: sold_at, type: time}
  - {name: region}
  - {name: store}
  - {name: amount, type: number}
measures:
  - {name: revenue, type: sum, sql: amount}
  - {name: transactions, type: count}
hierarchies:
  location: [region, store]
"#,
        )
        .unwrap()
    }

    fn dashboard(yaml: &str) -> Dashboard {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn fields_are_traced_to_the_tiles_that_use_them() {
        let d = dashboard(
            r#"
title: Sales
filters: [{model: sales, field: region}]
tiles:
  - id: by_region
    query: {model: sales, dimensions: [region, "sold_at:month"], measures: [revenue, "sum(amount)"],
            sort: ["-revenue__change"], filters: [{field: store, values: [x]}],
            formulas: [{name: per_sale, expr: "revenue / transactions"}]}
    drill: location
  - id: notes
    type: text
    text: hello
  - id: other
    query: {model: weather, dimensions: [borough]}
  - id: blended
    query: {model: sales_by_store, dimensions: [sales.region, stores.city], measures: [sales.transactions]}
"#,
        );
        let u = uses(&model(), &[("overview".into(), d)]);
        assert_eq!(u.dashboards.len(), 1);
        assert_eq!((u.dashboards[0].id.as_str(), u.dashboards[0].title.as_str()), ("overview", "Sales"));
        assert_eq!(u.dashboards[0].tiles, ["by_region", "blended"]);
        let at = |f: &str| u.fields.get(f).cloned().unwrap_or_default();
        // grouped by, drilled through, filtered on, by the dashboard too
        assert_eq!(at("region"), ["overview (filter)", "overview/by_region", "overview/blended"]);
        assert_eq!(at("sold_at"), ["overview/by_region"]);
        assert_eq!(at("store"), ["overview/by_region"]); // a filter, and the drill path
        assert_eq!(at("amount"), ["overview/by_region"]); // sum(amount)
        assert_eq!(at("revenue"), ["overview/by_region"]);
        assert_eq!(at("transactions"), ["overview/by_region", "overview/blended"]); // in a formula; a blend
        assert!(!u.fields.contains_key("borough") && !u.fields.contains_key("city"));
    }

    #[test]
    fn search_finds_by_words_and_ranks_names_first() {
        let items = vec![
            Item {
                kind: "field",
                model: Some("sales".into()),
                name: "revenue".into(),
                label: "Revenue".into(),
                words: vec!["takings".into(), "net revenue".into()],
                text: "What customers paid".into(),
            },
            Item {
                kind: "field",
                model: Some("sales".into()),
                name: "region".into(),
                label: "Region".into(),
                words: vec![],
                text: "Where the revenue was made".into(),
            },
            Item {
                kind: "term",
                model: None,
                name: "Net revenue".into(),
                label: "Net revenue".into(),
                words: vec!["net sales".into()],
                text: "Sales after discounts".into(),
            },
            Item {
                kind: "dashboard",
                model: None,
                name: "overview".into(),
                label: "Sales overview".into(),
                words: vec![],
                text: String::new(),
            },
        ];
        let names = |q: &str| search(&items, q).into_iter().map(|i| i.name.clone()).collect::<Vec<_>>();
        // a name beats a description
        assert_eq!(names("revenue"), ["revenue", "Net revenue", "region"]);
        // other words people use
        assert_eq!(names("takings"), ["revenue"]);
        assert_eq!(names("NET SALES"), ["Net revenue"]);
        // every word must match somewhere
        assert_eq!(names("sales overview"), ["overview"]);
        assert!(names("nothing like it").is_empty());
        assert!(names("  ").is_empty());
    }
}
