//! Loads a Magnum BI project directory:
//!
//! ```text
//! project/
//!   models/*.yaml      semantic models (one per file)
//!   themes/*.yaml      extra themes (optional)
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

use crate::compiler::{Dialect, compile_query, expand};
use crate::period::now_rounded;
use crate::schema::{Dashboard, Drill, Model, Query, is_temporal};
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

pub fn load_yaml<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    if text.len() > MAX_YAML_BYTES {
        return Err(format!("larger than {} KB", MAX_YAML_BYTES / 1024));
    }
    serde_yaml::from_str(text).map_err(|e| format!("invalid YAML: {e}"))
}

/// What the project files define, as one consistent snapshot.
#[derive(Debug, Default)]
pub struct ProjectState {
    pub models: BTreeMap<String, Model>,
    /// extension model -> the model it extends
    pub bases: BTreeMap<String, String>,
    pub themes: Vec<Theme>,
    /// file -> error, surfaced in the UI for builders
    pub errors: BTreeMap<String, String>,
}

impl ProjectState {
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
            let check = || -> Result<(), String> {
                if !available.contains(&t.query.model) {
                    return Err(format!("unknown model {:?}", t.query.model));
                }
                let m = &self.models[&t.query.model];
                let q = with_dashboard_period(d, t.query.clone(), &t.options);
                let dialect = dialect_of(&m.connection)
                    .ok_or_else(|| format!("this organisation has no connection named {:?}", m.connection))?;
                let (m2, q2) = expand(m, &q, dialect).map_err(|e| e.0)?;
                compile_query(&m2, &q2, dialect, now_rounded(), &[]).map_err(|e| e.0)?;
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
pub fn with_dashboard_period(d: &Dashboard, mut q: Query, options: &serde_json::Map<String, Value>) -> Query {
    if let Some(p) = &d.period
        && q.period.is_none()
        && q.model == p.model
        && options.get("period") != Some(&Value::Bool(false))
    {
        q.period = Some(crate::schema::Period {
            field: p.field.clone(),
            current: p.current.clone(),
            compare: p.compare.clone(),
        });
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
        ProjectState { models, bases, themes, errors }
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
            let warehouse = |c: &str| (c == "warehouse").then_some(Dialect::Postgres);
            assert_eq!(s.validate_dashboard(&d, &all, &warehouse), Vec::<String>::new(), "{}", path.display());
            // an organisation without the model's connection gets told
            let none = |_: &str| None;
            assert!(
                s.validate_dashboard(&d, &all, &none).iter().any(|w| w.contains("no connection named \"warehouse\""))
            );
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
}
