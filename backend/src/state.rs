//! The application state shared by every request.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::access::{AccessConfig, Members, Platform, lint};
use crate::auth::google::Google;
use crate::auth::password::LoginLimiter;
use crate::config::Config;
use crate::connectors::Connectors;
use crate::orgdb::OrgDbs;
use crate::project::{Project, ProjectState, load_yaml};
use crate::store::Store;
use crate::usage::Usage;

pub type Shared = Arc<AppState>;

pub struct AppState {
    pub config: Config,
    /// the primary ScramDB: identity, platform settings, members, templates, usage
    pub store: Store,
    /// each organisation's own ScramDB: its dashboards and query cache
    pub orgs: OrgDbs,
    pub usage: Usage,
    pub project: Project,
    pub access: AccessCache,
    pub connectors: Connectors,
    pub google: Option<Google>,
    /// failed sign-ins per email
    pub limiter: LoginLimiter,
    /// failed sign-ins per client address (password spraying across many accounts)
    pub ip_limiter: LoginLimiter,
    /// password hashing is deliberately slow: at most this many at once
    pub hashing: tokio::sync::Semaphore,
    /// organisations' own blends and themes, read from their databases
    pub blends: OrgBlends,
    pub themes: OrgCached<crate::themes::OrgThemes>,
    /// organisations' own models (org_models.rs), read from their databases
    pub models: OrgCached<Vec<crate::schema::Model>>,
    /// each model's governance per organisation (api/governance.rs): (model, governance)
    pub governance: OrgCached<Vec<(String, crate::governance::Governance)>>,
    /// what each organisation says about its models (governance.rs): (model, docs)
    pub docs: OrgCached<Vec<(String, crate::governance::ModelDocs)>>,
    /// organisations' shared formulas (api/formulas.rs): (model, formula)
    pub formulas: OrgCached<Vec<(String, crate::schema::Formula)>>,
    /// organisations' own sign-in providers (discovery and keys, cached)
    pub oidc: crate::auth::oidc::Oidc,
    /// the relying party for passkeys (None: unavailable)
    pub passkeys: Option<webauthn_rs::Webauthn>,
    /// the NYC demo's setup, when one runs (or ran) on this server
    pub demo: crate::demo::Progress,
    /// public links' visitors arriving together for one tile wait for one query
    pub flights: Flights,
}

/// Locks by key (64, picked by the key's hash): whoever holds one runs the query, the others then
/// find its result. Per process and rebuilt at will: replicas may each run one.
pub struct Flights(Vec<tokio::sync::Mutex<()>>);

impl Default for Flights {
    fn default() -> Self {
        Flights((0..64).map(|_| tokio::sync::Mutex::new(())).collect())
    }
}

impl Flights {
    pub async fn lock(&self, key: &str) -> tokio::sync::MutexGuard<'_, ()> {
        let h = key.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
        self.0[(h % self.0.len() as u64) as usize].lock().await
    }
}

/// Something each organisation keeps in its own database (its blends, its themes), cached
/// briefly: a save on this server forgets it at once, other servers see it within `TTL`.
pub struct OrgCached<T> {
    cached: Mutex<BTreeMap<String, (Instant, Arc<T>)>>,
}

impl<T> Default for OrgCached<T> {
    fn default() -> Self {
        OrgCached { cached: Mutex::new(BTreeMap::new()) }
    }
}

impl<T> OrgCached<T> {
    const TTL: Duration = Duration::from_secs(10);

    pub async fn get<F: std::future::Future<Output = anyhow::Result<T>>>(
        &self,
        org: &str,
        load: impl FnOnce() -> F,
    ) -> anyhow::Result<Arc<T>> {
        if let Some((at, v)) = self.cached.lock().expect("org cache lock").get(org)
            && at.elapsed() < Self::TTL
        {
            return Ok(v.clone());
        }
        let v = Arc::new(load().await?);
        self.cached.lock().expect("org cache lock").insert(org.to_string(), (Instant::now(), v.clone()));
        Ok(v)
    }

    pub fn forget(&self, org: &str) {
        self.cached.lock().expect("org cache lock").remove(org);
    }
}

pub type OrgBlends = OrgCached<Vec<crate::blend::Blend>>;

impl AppState {
    /// The organisation's own blends.
    pub async fn org_blends(&self, org: &str) -> anyhow::Result<Arc<Vec<crate::blend::Blend>>> {
        self.blends
            .get(org, || async {
                let store = self.orgs.store(org).await?;
                Ok(store.blends(org).await?.into_iter().map(|(b, _, _)| b).collect())
            })
            .await
    }

    /// The organisation's own models.
    pub async fn org_models(&self, org: &str) -> anyhow::Result<Arc<Vec<crate::schema::Model>>> {
        self.models
            .get(org, || async {
                let store = self.orgs.store(org).await?;
                Ok(store.org_models(org).await?.into_iter().map(|(m, _, _)| m).collect())
            })
            .await
    }

    /// Each model's governance in the organisation.
    pub async fn org_governance(&self, org: &str) -> anyhow::Result<Arc<Vec<(String, crate::governance::Governance)>>> {
        self.governance
            .get(org, || async {
                let store = self.orgs.store(org).await?;
                Ok(store.governance(org).await?.into_iter().map(|(m, g, _, _)| (m, g)).collect())
            })
            .await
    }

    /// What the organisation says about its models.
    pub async fn org_docs(&self, org: &str) -> anyhow::Result<Arc<Vec<(String, crate::governance::ModelDocs)>>> {
        self.docs.get(org, || async { self.orgs.store(org).await?.model_docs(org).await }).await
    }

    /// The organisation's shared formulas, with the model each belongs to.
    pub async fn org_formulas(&self, org: &str) -> anyhow::Result<Arc<Vec<(String, crate::schema::Formula)>>> {
        self.formulas
            .get(org, || async {
                let store = self.orgs.store(org).await?;
                Ok(store.formulas(org).await?.into_iter().map(|(m, f, _, _)| (m, f)).collect())
            })
            .await
    }

    /// The organisation's own themes and default themes.
    pub async fn org_themes(&self, org: &str) -> anyhow::Result<Arc<crate::themes::OrgThemes>> {
        self.themes.get(org, || async { self.orgs.store(org).await?.org_themes(org).await }).await
    }
}

/// Password hashes allowed at once: half the CPUs (at least 2), so sign-in floods can't starve
/// everything else.
pub fn hashing_permits() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(2))
}

/// Access settings change from any replica: re-read them from ScramDB every few seconds.
const ACCESS_RELOAD: Duration = Duration::from_secs(3);

/// (loaded at, settings, problems worth showing to admins)
type Cached = (Instant, Arc<AccessConfig>, BTreeMap<String, String>);

pub struct AccessCache {
    cached: Mutex<Option<Cached>>,
    /// tests: settings held in memory, never reloaded (the test database's stay untouched)
    pinned: std::sync::atomic::AtomicBool,
}

impl AccessCache {
    pub fn new() -> AccessCache {
        AccessCache { cached: Mutex::new(None), pinned: false.into() }
    }

    #[cfg(test)]
    pub fn pin(&self, cfg: AccessConfig) {
        self.pinned.store(true, std::sync::atomic::Ordering::SeqCst);
        *self.cached.lock().expect("access lock") = Some((Instant::now(), Arc::new(cfg), BTreeMap::new()));
    }

    pub fn invalidate(&self) {
        if self.pinned.load(std::sync::atomic::Ordering::SeqCst) {
            return; // tests: the pinned settings stay
        }
        *self.cached.lock().expect("access lock") = None;
    }

    /// The access settings, and problems worth showing to admins. Fails closed: settings that
    /// can't be read hand out no data.
    pub async fn get(&self, state: &AppState, project: &ProjectState) -> (Arc<AccessConfig>, BTreeMap<String, String>) {
        if let Some((at, cfg, errors)) = self.cached.lock().expect("access lock").as_ref()
            && (at.elapsed() < ACCESS_RELOAD || self.pinned.load(std::sync::atomic::Ordering::SeqCst))
        {
            return (cfg.clone(), errors.clone());
        }
        let (cfg, mut errors) = match read_access(state).await {
            Ok((cfg, _source)) => (cfg, BTreeMap::new()),
            Err(e) => {
                tracing::error!("access settings unusable: {e:#}");
                let msg = format!("{e:#} (no data is shown until this is fixed)");
                (
                    AccessConfig { broken: true, ..AccessConfig::default() },
                    BTreeMap::from([("access".to_string(), msg)]),
                )
            }
        };
        errors.extend(lint(&cfg, project));
        let cfg = Arc::new(cfg);
        *self.cached.lock().expect("access lock") = Some((Instant::now(), cfg.clone(), errors.clone()));
        (cfg, errors)
    }
}

impl Default for AccessCache {
    fn default() -> Self {
        Self::new()
    }
}

/// From ScramDB ("platform" + "members:<org>"); on the first start adopted from
/// project/access.yaml when there is one; otherwise MAGNUM_SYSTEM_OWNERS and no organisations.
async fn read_access(state: &AppState) -> anyhow::Result<(AccessConfig, &'static str)> {
    let Some(platform) = state.store.setting("platform").await? else {
        let path = state.project.root.join("access.yaml");
        if path.exists() {
            let text = crate::project::expand_env(&std::fs::read_to_string(&path)?);
            let doc: Value = load_yaml(&text).map_err(anyhow::Error::msg)?;
            let cfg = AccessConfig::from_document(doc).map_err(|e| anyhow::anyhow!("access.yaml: {e}"))?;
            store_access(state, &cfg, None, Some("seed")).await?;
            tracing::info!("adopted access settings from {}", path.display());
            return Ok((cfg, "access.yaml"));
        }
        let platform = Platform { system_owners: state.config.bootstrap_owners.clone(), ..Platform::default() };
        return Ok((AccessConfig { platform, ..AccessConfig::default() }, "bootstrap"));
    };
    let platform: Platform = serde_json::from_value(platform).map_err(|e| anyhow::anyhow!("platform settings: {e}"))?;
    platform.validate().map_err(anyhow::Error::msg)?;
    let mut members = BTreeMap::new();
    for oid in platform.organisations.keys() {
        let m: Members = match state.store.setting(&format!("members:{oid}")).await? {
            Some(v) => serde_json::from_value(v).map_err(|e| anyhow::anyhow!("members of {oid}: {e}"))?,
            None => Members::default(),
        };
        members.insert(oid.clone(), m);
    }
    Ok((AccessConfig { platform, members, broken: false }, "database"))
}

/// Write the platform part and/or members. `only`: Some("platform") or Some(org id); None: all.
pub async fn store_access(
    state: &AppState,
    cfg: &AccessConfig,
    only: Option<&str>,
    by: Option<&str>,
) -> anyhow::Result<()> {
    let mut values: Vec<(String, Option<Value>)> = vec![];
    if only.is_none() || only == Some("platform") {
        values.push(("platform".into(), Some(serde_json::to_value(&cfg.platform)?)));
    }
    for oid in cfg.platform.organisations.keys() {
        if only.is_none() || only == Some(oid.as_str()) {
            values.push((format!("members:{oid}"), Some(serde_json::to_value(cfg.members_of(oid))?)));
        }
    }
    state.store.put_settings(&values, by).await?;
    state.access.invalidate();
    Ok(())
}
