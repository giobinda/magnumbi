//! Loads a Magnum BI project directory:
//!
//! ```text
//! project/
//!   models/*.yaml      semantic models (one per file)
//!   themes/*.yaml      extra themes (optional)
//!   blends/*.yaml      models joined on shared fields, across connections (blend.rs)
//!   dashboards/*.yaml  dashboards seeded into ScramDB on first start (file stem = id)
//!   access.yaml        optional: initial organisations and members, adopted on first start
//! ```
//!
//! Data connections aren't project files: each organisation has its own (connections.rs), and a
//! model only names the connection it reads from.
//!
//! Files are re-read when they change on disk, so editing YAML (or pulling from git) is picked up
//! without a restart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::SystemTime;

use regex::Regex;
use serde_json::Value;

use crate::blend::{Blend, MAX_SOURCE_ROWS, Plan, plan};
use crate::compiler::{Dialect, compile_query, compile_query_up_to, expand};
use crate::period::now_rounded;
use crate::schema::{Dashboard, Drill, Formula, Model, Query, is_temporal};
use crate::themes::{Theme, builtin};

pub const MAX_YAML_BYTES: usize = 512 * 1024;
static ENV_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").unwrap());
pub static DASHBOARD_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,80}$").unwrap());

pub fn expand_env(text: &str) -> String {
    ENV_RE
        .replace_all(text, |c: &regex::Captures| {
            std::env::var(&c[1])
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| c.get(2).map_or("", |m| m.as_str()).into())
        })
        .into_owned()
}

/// How deep `[...]` and `{...}` may nest. Nothing real comes close, and the YAML parser takes
/// quadratic time on deep nesting (100,000 levels: 40 seconds of CPU), so it never sees more.
const MAX_YAML_DEPTH: usize = 100;

/// The deepest nesting of brackets, counted everywhere (telling quoted text apart would need a
/// YAML parser; a string with 100 unclosed brackets in a row is refused too, which is fine).
fn flow_depth(text: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    for c in text.chars() {
        match c {
            '[' | '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

pub fn load_yaml<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    if text.len() > MAX_YAML_BYTES {
        return Err(format!("larger than {} KB", MAX_YAML_BYTES / 1024));
    }
    if flow_depth(text) > MAX_YAML_DEPTH {
        return Err(format!("nested more than {MAX_YAML_DEPTH} levels deep"));
    }
    serde_yaml::from_str(text).map_err(|e| format!("invalid YAML: {e}"))
}

/// What the project files define, as one consistent snapshot (plus, per request, the
/// organisation's own blends: with_org_blends).
#[derive(Debug, Default, Clone)]
pub struct ProjectState {
    pub models: BTreeMap<String, Model>,
    /// extension model -> the model it extends
    pub bases: BTreeMap<String, String>,
    /// blends; each is also in `models`, as its `model.field` view (Blend::as_model)
    pub blends: BTreeMap<String, Blend>,
    /// the blends that are the organisation's own (defined in the UI, editable there)
    pub org_blends: BTreeSet<String>,
    /// the models that are the organisation's own (org_models.rs: made in the UI, no SQL)
    pub org_models: BTreeSet<String>,
    /// the organisation's shared formulas: (model, name)
    pub formulas: BTreeSet<(String, String)>,
    /// the default (light, dark) themes: the platform's, or the organisation's own choice
    pub theme_defaults: (String, String),
    pub themes: Vec<Theme>,
    /// file -> error, surfaced in the UI for builders
    pub errors: BTreeMap<String, String>,
}

impl ProjectState {
    /// Can `b` be added? Its name must be free, and it joins models (not other blends).
    pub fn check_new_blend(&self, b: &Blend) -> Result<(), String> {
        if self.models.contains_key(&b.name) {
            return Err(format!("the name {:?} is already a model's or blend's", b.name));
        }
        for m in std::iter::once(&b.base).chain(b.joins.iter().map(|j| &j.model)) {
            if self.blends.contains_key(m) {
                return Err(format!("{m} is a blend: a blend joins models"));
            }
        }
        b.validate(&self.models)
    }

    /// This snapshot plus an organisation's own themes and default themes (a theme whose id is
    /// taken here is left out).
    pub fn with_org_themes(&self, t: &crate::themes::OrgThemes) -> ProjectState {
        let mut p = self.clone();
        for theme in &t.themes {
            if !p.themes.iter().any(|x| x.id == theme.id) {
                p.themes.push(theme.clone());
            }
        }
        if let Some(l) = t.default_light.as_ref().filter(|id| p.themes.iter().any(|x| &x.id == *id)) {
            p.theme_defaults.0 = l.clone();
        }
        if let Some(d) = t.default_dark.as_ref().filter(|id| p.themes.iter().any(|x| &x.id == *id)) {
            p.theme_defaults.1 = d.clone();
        }
        p
    }

    /// This snapshot plus an organisation's own models (one whose name is taken, or that breaks
    /// the rules for organisations' models, is left out and reported).
    pub fn with_org_models(&self, models: &[Model]) -> ProjectState {
        let mut p = self.clone();
        for m in models {
            let fits = if self.models.contains_key(&m.name) {
                Err(format!("the name {:?} is already a model's or blend's", m.name))
            } else {
                crate::org_models::check(m)
            };
            match fits {
                Ok(()) => {
                    p.models.insert(m.name.clone(), m.clone());
                    p.org_models.insert(m.name.clone());
                }
                Err(e) => {
                    p.errors.insert(format!("model {}", m.name), e);
                }
            }
        }
        p
    }

    /// This snapshot plus an organisation's shared formulas, as measures of their models (one
    /// that no longer fits, e.g. after its model changed, is left out and reported).
    pub fn with_formulas(&self, formulas: &[(String, Formula)]) -> ProjectState {
        let mut p = self.clone();
        for (model, f) in formulas {
            match p.formula_fits(model, f) {
                Ok(m) => {
                    p.models.insert(model.clone(), m);
                    p.formulas.insert((model.clone(), f.name.clone()));
                }
                Err(e) => {
                    p.errors.insert(format!("formula {model}.{}", f.name), e);
                }
            }
        }
        p
    }

    /// The model with the formula added (or replacing the shared formula of that name), if it fits.
    pub fn formula_fits(&self, model: &str, f: &Formula) -> Result<Model, String> {
        f.check()?;
        let mut m = match self.models.get(model) {
            Some(_) if self.blends.contains_key(model) => {
                return Err(format!("{model} is a blend: formulas go on models"));
            }
            Some(m) => m.clone(),
            None => return Err(format!("no model named {model:?}")),
        };
        if self.formulas.contains(&(model.to_string(), f.name.clone())) {
            m.measures.retain(|x| x.name != f.name);
            m.dimensions.retain(|x| x.name != f.name);
        } else if m.dimension(&f.name).is_some() || m.measure(&f.name).is_some() {
            return Err(format!("{model} already has a field named {:?}", f.name));
        }
        if f.per_row {
            m.dimensions.push(f.as_dimension());
            // its type, for everyone who lists the model's fields
            let kind = crate::compiler::with_row_formulas(&m, crate::compiler::Dialect::Postgres)
                .map_err(|e| e.0)?
                .dimension(&f.name)
                .map(|d| d.kind.clone())
                .unwrap_or_default();
            m.dimensions.last_mut().expect("pushed").kind = kind;
        } else {
            m.measures.push(f.as_measure());
        }
        crate::compiler::check_formulas(&m).map_err(|e| e.0)?;
        Ok(m)
    }

    /// This snapshot plus an organisation's own blends (one that no longer fits, e.g. after a
    /// model changed, is left out and reported).
    pub fn with_org_blends(&self, blends: &[Blend]) -> ProjectState {
        let mut p = self.clone();
        for b in blends {
            match self.check_new_blend(b) {
                Ok(()) => {
                    p.models.insert(b.name.clone(), b.as_model(&self.models));
                    p.blends.insert(b.name.clone(), b.clone());
                    p.org_blends.insert(b.name.clone());
                }
                Err(e) => {
                    p.errors.insert(format!("blend {}", b.name), e);
                }
            }
        }
        p
    }

    /// Compile a blend query's parts, each in its connection's dialect, and the join.
    fn check_blend(&self, b: &Blend, q: &Query, dialect_of: &dyn Fn(&str) -> Option<Dialect>) -> Result<(), String> {
        let now = now_rounded();
        match plan(b, &self.models, q, now).map_err(|e| e.0)? {
            Plan::Single { model, query } => check_query(&self.models[&model], &query, dialect_of).map(|_| ()),
            Plan::Joined(j) => {
                for side in &j.sides {
                    let dialect = dialect_of(&side.source.connection).ok_or_else(|| no_connection(&side.source))?;
                    compile_query_up_to(&side.source, &side.query, dialect, now, &[], MAX_SOURCE_ROWS)
                        .map_err(|e| e.0)?;
                }
                let tables: Vec<String> = (0..j.sides.len()).map(|i| format!("source_{i}")).collect();
                compile_query(&j.joined_model(&tables), &j.query, Dialect::Postgres, now, &[]).map_err(|e| e.0)?;
                Ok(())
            }
        }
    }

    /// Semantic checks of a dashboard against the models (`available`: the organisation's) and
    /// the organisation's connections (`dialect_of`: connection name -> its SQL dialect).
    /// Warnings, not errors, so a dashboard stays editable while a model it uses is broken.
    pub fn validate_dashboard(
        &self,
        d: &Dashboard,
        available: &BTreeSet<String>,
        dialect_of: &dyn Fn(&str) -> Option<Dialect>,
    ) -> Vec<String> {
        let mut warnings = vec![];
        for t in &d.tiles {
            let Some(tq) = &t.query else { continue }; // text
            let check = || -> Result<(), String> {
                if !available.contains(&tq.model) {
                    return Err(format!("unknown model {:?}", tq.model));
                }
                let m = &self.models[&tq.model];
                let q = with_dashboard_period(d, tq.clone(), &t.options, m);
                // calculations on results apart: checked against the columns of the rest
                let split = crate::results::split(&q)?;
                let inner = split.as_ref().map_or(&q, |s| &s.inner);
                match self.blends.get(&q.model) {
                    Some(b) => self.check_blend(b, inner, dialect_of)?,
                    None => {
                        let columns = check_query(m, inner, dialect_of)?;
                        if let Some(s) = &split {
                            crate::results::sql("results", &columns, &s.calcs)?;
                        }
                    }
                }
                if let Some(Drill::Hierarchy(h)) = &t.drill
                    && !m.hierarchies.contains_key(h)
                {
                    return Err(format!("unknown hierarchy {h:?}"));
                }
                Ok(())
            };
            if let Err(e) = check() {
                warnings.push(format!("tile {}: {e}", t.id));
            }
        }
        if let Some(theme) = &d.theme
            && !self.themes.iter().any(|t| &t.id == theme)
        {
            warnings.push(format!("theme {theme:?} isn't available here; viewers will see their own theme"));
        }
        if let Some(p) = &d.period {
            let ok =
                self.models.get(&p.model).and_then(|m| m.dimension(&p.field)).is_some_and(|d| is_temporal(&d.kind));
            if !ok {
                warnings.push(format!("period: {:?} is not a date/time dimension of model {:?}", p.field, p.model));
            }
        }
        for f in &d.filters {
            if let Some(m) = self.models.get(&f.model)
                && m.dimension(&f.field).is_none()
                && m.measure(&f.field).is_none()
            {
                warnings.push(format!("filter {}: not a field of model {:?}", f.field, f.model));
            }
        }
        warnings
    }
}

/// Tiles inherit the dashboard period (same model, no period of their own, not opted out).
fn no_connection(m: &Model) -> String {
    format!("this organisation has no connection named {:?}", m.connection)
}

/// The query's columns, if it compiles.
fn check_query(
    m: &Model,
    q: &Query,
    dialect_of: &dyn Fn(&str) -> Option<Dialect>,
) -> Result<Vec<crate::compiler::Column>, String> {
    let dialect = dialect_of(&m.connection).ok_or_else(|| no_connection(m))?;
    let (m2, q2) = expand(m, q, dialect).map_err(|e| e.0)?;
    Ok(compile_query(&m2, &q2, dialect, now_rounded(), &[]).map_err(|e| e.0)?.columns)
}

/// The tile's query with the dashboard's period, when the tile's model has the period's field
/// (`model`: the tile's; a blend has it as `model.field`).
pub fn with_dashboard_period(
    d: &Dashboard,
    mut q: Query,
    options: &serde_json::Map<String, Value>,
    model: &Model,
) -> Query {
    if let Some(p) = &d.period
        && q.period.is_none()
        && options.get("period") != Some(&Value::Bool(false))
        && let Some(field) = crate::tile_query::tile_field(&q.model, model, &p.model, &p.field)
    {
        q.period = Some(crate::schema::Period { field, current: p.current.clone(), compare: p.compare.clone() });
    }
    q
}

pub fn parse_dashboard(text: &str) -> Result<Dashboard, String> {
    let d: Dashboard = load_yaml(text)?;
    d.validate()?;
    Ok(d)
}

/// Compact YAML: nulls and empty lists/maps left out, keys in definition order.
pub fn dump_dashboard(d: &Dashboard) -> String {
    fn prune(v: Value) -> Option<Value> {
        match v {
            Value::Null => None,
            Value::Array(a) if a.is_empty() => None,
            Value::Object(o) => {
                let o: serde_json::Map<String, Value> =
                    o.into_iter().filter_map(|(k, v)| prune(v).map(|v| (k, v))).collect();
                (!o.is_empty()).then_some(Value::Object(o))
            }
            Value::Array(a) => Some(Value::Array(a.into_iter().map(|v| prune(v).unwrap_or(Value::Null)).collect())),
            v => Some(v),
        }
    }
    let v = prune(serde_json::to_value(d).expect("dashboards serialize")).unwrap_or(Value::Null);
    serde_yaml::to_string(&v).expect("json values serialize to YAML")
}

type Signature = Vec<(PathBuf, Option<SystemTime>, u64)>;

pub struct Project {
    pub root: PathBuf,
    state: RwLock<(Signature, Arc<ProjectState>)>,
}

impl Project {
    pub fn new(root: impl Into<PathBuf>) -> Project {
        let root = root.into();
        let p = Project { root, state: RwLock::new((vec![], Arc::new(ProjectState::default()))) };
        p.current();
        p
    }

    pub fn dashboards_dir(&self) -> PathBuf {
        self.root.join("dashboards")
    }

    fn yaml_files(&self, dir: &str) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(self.root.join(dir))
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    fn signature(&self) -> Signature {
        let mut files = self.yaml_files("models");
        files.extend(self.yaml_files("blends"));
        files.extend(self.yaml_files("themes"));
        files
            .into_iter()
            .map(|f| {
                let meta = std::fs::metadata(&f).ok();
                let mtime = meta.as_ref().and_then(|m| m.modified().ok());
                let len = meta.map_or(0, |m| m.len());
                (f, mtime, len)
            })
            .collect()
    }

    /// The current snapshot, re-read first if any project file changed.
    pub fn current(&self) -> Arc<ProjectState> {
        let sig = self.signature();
        {
            let guard = self.state.read().expect("project lock");
            if guard.0 == sig && !sig.is_empty() {
                return guard.1.clone();
            }
        }
        let state = Arc::new(self.load());
        let mut guard = self.state.write().expect("project lock");
        *guard = (sig, state.clone());
        state
    }

    fn load(&self) -> ProjectState {
        let mut errors = BTreeMap::new();
        let mut models: BTreeMap<String, Model> = BTreeMap::new();
        let mut extensions: Vec<(String, Model)> = vec![];
        for path in self.yaml_files("models") {
            let file = format!("models/{}", file_name(&path));
            let parsed = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| load_yaml::<Model>(&t));
            let result = parsed.and_then(|m| {
                m.validate()?;
                if models.contains_key(&m.name) || extensions.iter().any(|(_, x)| x.name == m.name) {
                    return Err(format!("duplicate model name {:?}", m.name));
                }
                Ok(m)
            });
            match result {
                Ok(m) if m.extends.is_some() => extensions.push((file, m)),
                Ok(m) => {
                    models.insert(m.name.clone(), m);
                }
                Err(e) => {
                    errors.insert(file, e);
                }
            }
        }
        let mut bases = BTreeMap::new();
        for (file, ext) in extensions {
            // one level: extend a regular model
            match extend(&models, &ext) {
                Ok(m) => {
                    bases.insert(ext.name.clone(), ext.extends.clone().unwrap_or_default());
                    models.insert(ext.name.clone(), m);
                }
                Err(e) => {
                    errors.insert(file, e);
                }
            }
        }

        let mut themes = builtin();
        for path in self.yaml_files("themes") {
            let file = format!("themes/{}", file_name(&path));
            let result = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| load_yaml::<Theme>(&t))
                .and_then(|mut t| {
                    t.validate()?;
                    if themes.iter().any(|x| x.id == t.id) {
                        return Err(format!("theme id {:?} is built in or already used; pick a new id", t.id));
                    }
                    Ok(t)
                });
            match result {
                Ok(t) => themes.push(t),
                Err(e) => {
                    errors.insert(file, e);
                }
            }
        }
        let mut blends = BTreeMap::new();
        for path in self.yaml_files("blends") {
            let file = format!("blends/{}", file_name(&path));
            let result = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| load_yaml::<Blend>(&t))
                .and_then(|b| {
                    if models.contains_key(&b.name) || blends.contains_key(&b.name) {
                        return Err(format!("the name {:?} is already a model's or blend's", b.name));
                    }
                    b.validate(&models)?;
                    Ok(b)
                });
            match result {
                Ok(b) => {
                    blends.insert(b.name.clone(), b);
                }
                Err(e) => {
                    errors.insert(file, e);
                }
            }
        }
        for b in blends.values() {
            models.insert(b.name.clone(), b.as_model(&models));
        }
        let theme_defaults = (crate::themes::DEFAULT_LIGHT.to_string(), crate::themes::DEFAULT_DARK.to_string());
        ProjectState {
            models,
            bases,
            blends,
            org_blends: BTreeSet::new(),
            org_models: BTreeSet::new(),
            formulas: BTreeSet::new(),
            theme_defaults,
            themes,
            errors,
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn extend(models: &BTreeMap<String, Model>, ext: &Model) -> Result<Model, String> {
    let base_name = ext.extends.as_deref().unwrap_or_default();
    let base = models
        .get(base_name)
        .ok_or_else(|| format!("extends unknown model {base_name:?} (an extension can't extend another extension)"))?;
    let names: BTreeSet<&str> =
        base.dimensions.iter().map(|d| d.name.as_str()).chain(base.measures.iter().map(|m| m.name.as_str())).collect();
    let mut clash: Vec<&str> = ext
        .dimensions
        .iter()
        .map(|d| d.name.as_str())
        .chain(ext.measures.iter().map(|m| m.name.as_str()))
        .filter(|n| names.contains(n))
        .collect();
    clash.sort_unstable();
    if !clash.is_empty() {
        return Err(format!("redefines fields of {:?}: {clash:?}", base.name));
    }
    let mut m = base.clone();
    m.name = ext.name.clone();
    m.extends = None;
    m.label = Some(
        ext.label.clone().unwrap_or_else(|| format!("{} (extended)", base.label.as_deref().unwrap_or(&base.name))),
    );
    m.description = ext.description.clone().or_else(|| base.description.clone());
    m.dimensions.extend(ext.dimensions.iter().cloned());
    m.measures.extend(ext.measures.iter().cloned());
    m.hierarchies.extend(ext.hierarchies.clone());
    m.cache = ext.cache.or(base.cache);
    m.validate()?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeply_nested_yaml_is_refused_before_parsing() {
        // the YAML parser takes quadratic time on deep nesting: 100k brackets took 40 s
        let deep = format!("title: {}{}", "[".repeat(100_000), "]".repeat(100_000));
        let t = std::time::Instant::now();
        let e = load_yaml::<Value>(&deep).unwrap_err();
        assert!(t.elapsed() < std::time::Duration::from_millis(200), "{:?}", t.elapsed());
        assert!(e.contains("nested"), "{e}");
        // an apostrophe can't hide what follows it
        let sneaky = format!("a: it's x\nb: {}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(load_yaml::<Value>(&sneaky).unwrap_err().contains("nested"));
        // text full of closed brackets is fine (markdown links), as is ordinary nesting
        let links = format!("text: \"{}\"\n", "[a](b) ".repeat(2000));
        assert!(load_yaml::<Value>(&links).is_ok());
        assert!(load_yaml::<Value>("a: {b: [1, {c: [2, 3]}]}").is_ok());
    }

    fn tmp_project(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("magnum-test-{}", uuid::Uuid::new_v4()));
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn loads_the_shipped_project() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../project");
        let s = Project::new(&root).current();
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.models.contains_key("sales"));
        let all: BTreeSet<String> = s.models.keys().cloned().collect();
        for path in Project::new(&root).yaml_files("dashboards") {
            let d = parse_dashboard(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let connections = |c: &str| match c {
                "warehouse" => Some(Dialect::Postgres),
                "nyc_taxi" => Some(Dialect::DuckDb),
                "meteo" => Some(Dialect::Postgres),
                _ => None,
            };
            assert_eq!(s.validate_dashboard(&d, &all, &connections), Vec::<String>::new(), "{}", path.display());
            // an organisation without the model's connection gets told
            let none = |_: &str| None;
            assert!(s.validate_dashboard(&d, &all, &none).iter().any(|w| w.contains("no connection named")));
            // the compact YAML round-trips
            let again = parse_dashboard(&dump_dashboard(&d)).unwrap();
            assert_eq!(serde_json::to_value(&again).unwrap(), serde_json::to_value(&d).unwrap());
        }
    }

    #[test]
    fn extends_and_errors() {
        // connections aren't project files: a model only names one, resolved per organisation
        let dir = tmp_project(&[
            (
                "models/a.yaml",
                "name: a\nconnection: db\ntable: t\ndimensions: [{name: x}]\nmeasures: [{name: n, type: count}]\n",
            ),
            ("models/b.yaml", "name: b\nextends: a\nmeasures: [{name: s, type: sum, sql: x}]\n"),
            ("models/c.yaml", "name: c\nextends: a\ndimensions: [{name: x}]\n"),
            ("models/d.yaml", "name: d\nconnection: anywhere\ntable: t\n"),
            ("models/e.yaml", "name: e\ntable: t\n"),
        ]);
        let s = Project::new(&dir).current();
        assert!(s.models.contains_key("d"), "{:?}", s.errors);
        assert!(s.errors["models/e.yaml"].contains("`connection` is required"));
        assert_eq!(s.models["b"].measures.len(), 2);
        assert_eq!(s.bases["b"], "a");
        assert!(s.errors["models/c.yaml"].contains("redefines"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn blends_are_models_of_models() {
        let a = "name: a\nconnection: db\ntable: t\ndimensions: [{name: k}, {name: x}]\nmeasures: [{name: n, type: count}]\n";
        let b = "name: b\nconnection: other\ntable: u\ndimensions: [{name: k}, {name: y}]\n";
        let dir = tmp_project(&[
            ("models/a.yaml", a),
            ("models/b.yaml", b),
            ("blends/ab.yaml", "name: ab\nlabel: A and B\nbase: a\njoins: [{model: b, on: [k = k]}]\ncache: 5m\n"),
            ("blends/bad.yaml", "name: bad\nbase: a\njoins: [{model: b, on: [k = nope]}]\n"),
            ("blends/a.yaml", "name: a\nbase: b\njoins: [{model: a, on: [k = k]}]\n"),
        ]);
        let s = Project::new(&dir).current();
        let ab = &s.models["ab"];
        assert_eq!(ab.label.as_deref(), Some("A and B"));
        assert!(ab.dimension("b.y").is_some());
        assert_eq!(ab.cache, Some(300));
        assert_eq!(s.blends["ab"].base, "a");
        assert!(s.errors["blends/bad.yaml"].contains("nope"));
        assert!(s.errors["blends/a.yaml"].contains("already"), "{:?}", s.errors);
        assert_eq!(s.models["a"].connection, "db", "a blend can't replace a model");

        // dashboards check blend tiles part by part, each on its model's connection
        let d = parse_dashboard(
            "title: t\ntiles:\n\
             - {id: ok, type: table, query: {model: ab, dimensions: [b.y], measures: [a.n]}}\n\
             - {id: one, type: table, query: {model: ab, dimensions: [a.x], measures: [a.n]}}\n\
             - {id: bad, type: table, query: {model: ab, dimensions: [b.nope], measures: [a.n]}}\n",
        )
        .unwrap();
        let all: BTreeSet<String> = s.models.keys().cloned().collect();
        let both = |c: &str| match c {
            "db" => Some(Dialect::Postgres),
            "other" => Some(Dialect::DuckDb),
            _ => None,
        };
        let w = s.validate_dashboard(&d, &all, &both);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].starts_with("tile bad:") && w[0].contains("nope"), "{w:?}");
        let only_db = |c: &str| (c == "db").then_some(Dialect::Postgres);
        let w = s.validate_dashboard(&d, &all, &only_db);
        assert!(w.iter().any(|w| w.starts_with("tile ok:") && w.contains("\"other\"")), "{w:?}");
        assert!(!w.iter().any(|w| w.starts_with("tile one:")), "one model: the other connection isn't needed");
        std::fs::remove_dir_all(dir).ok();
    }
}
