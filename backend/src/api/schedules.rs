//! Scheduled reports: a dashboard sent on a schedule (schedule.rs), as its maker, by email and
//! webhooks, through the tenant's own delivery settings (api/delivery.rs). Each run is the zip of
//! the charts' data, a fresh guest link (signed) or a public link. Recipients are the tenant's
//! members and the email domains its Org admins allow, checked when made and on every run.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use super::dashboards::load;
use crate::access::{AccessConfig, Role};
use crate::auth::{Auth, Caller, LinkAccess, link_caller, password};
use crate::delivery::{Mail, Message, WebhookKind, payload, post_webhook, send_mail};
use crate::error::{ApiError, ApiResult};
use crate::schedule::{AlertRule, ScheduleConfig, When, may_receive};
use crate::state::{AppState, Shared};
use crate::store::{GuestLink, Schedule, Store, random_token, sha256_hex};
use crate::tile_query::TileRequest;

const MAX_RECIPIENTS: usize = 50;
/// failed runs in a row before a schedule pauses
const MAX_FAILURES: i64 = 5;
/// larger zips aren't attached (mail servers refuse them); the email links to the dashboard
const MAX_ATTACHMENT: usize = 10 << 20;
/// a run's status starting with this pauses the schedule
const PAUSED: &str = "paused: ";

fn unsubscribe_context(org: &str) -> String {
    format!("{org}/unsubscribe")
}

fn key(state: &AppState) -> ApiResult<&crate::crypto::SecretKey> {
    state.config.secret_key.as_ref().ok_or_else(|| ApiError::bad_request("scheduled reports need MAGNUM_SECRET_KEY"))
}

/// Where links in reports point: MAGNUM_PUBLIC_URL (reports leave without a request to go by).
fn base_url(state: &AppState) -> String {
    state.config.public_url.clone().unwrap_or_else(|| format!("http://localhost:{}", state.config.port))
}

/// Who a tenant's reports may go to: its members (system owners work in every organisation),
/// and the domains its people join by.
fn audience(access: &AccessConfig, org: &str) -> (Vec<String>, Vec<String>) {
    let owners = access.platform.system_owners.clone();
    if org.is_empty() {
        return (owners, vec![]);
    }
    let members = access.members_of(org).users.keys().cloned().chain(owners).collect();
    let domains = access.platform.organisations.get(org).map(|o| o.domains.clone()).unwrap_or_default();
    (members, domains)
}

fn describe(s: &Schedule) -> Value {
    json!({
        "id": s.id, "dashboard": s.dashboard_id, "name": s.config.name, "when": s.config.when,
        "format": s.config.format, "recipients": s.config.recipients, "webhooks": s.config.webhooks,
        "guest_role": s.config.guest_role, "share_link": s.config.share_link, "alert": s.config.alert,
        "firing": s.firing, "active": s.active,
        "next_run_at": s.next_run_at.to_rfc3339(), "last_run_at": s.last_run_at.map(|t| t.to_rfc3339()),
        "last_status": s.last_status, "failures": s.failures, "created_by": s.created_by,
        "created_at": s.created_at.to_rfc3339(),
    })
}

async fn store_of(state: &AppState, caller: &Caller) -> ApiResult<Store> {
    Ok(state.orgs.store(caller.org()).await?)
}

/// A dashboard's schedules (anyone who may share it sees them), and what reports can use here:
/// whether there's a mail server, and the webhooks' names (never their addresses).
pub async fn list(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    caller.require("share")?;
    load(&state, &caller, &id).await?;
    let store = store_of(&state, &caller).await?;
    let all = store.schedules(caller.org(), Some(&id)).await?;
    let webhooks: Vec<String> = store.webhooks(caller.org()).await?.into_iter().map(|(name, _, _)| name).collect();
    let mail = store.mail_server(caller.org()).await?.is_some();
    Ok(Json(json!({"schedules": all.iter().map(describe).collect::<Vec<_>>(), "webhooks": webhooks, "mail": mail})))
}

/// Every schedule in the organisation (Org admins: to see what leaves, and pause it).
pub async fn list_all(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let all = store_of(&state, &caller).await?.schedules(caller.org(), None).await?;
    Ok(Json(json!({"schedules": all.iter().map(describe).collect::<Vec<_>>()})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSchedule {
    name: String,
    when: When,
    /// csv | signed | public
    format: String,
    #[serde(default)]
    recipients: Vec<String>,
    #[serde(default)]
    webhooks: Vec<String>,
    /// the dashboard's filters and period, as for a tile ({"filters": ..., "period": ...})
    #[serde(default)]
    request: Value,
    /// signed: the guests' password (empty: made up and shown once)
    #[serde(default)]
    password: Option<String>,
    /// signed: the guests' role (Viewer by default)
    #[serde(default)]
    guest_role: Option<String>,
    /// public: which of the dashboard's public links to send
    #[serde(default)]
    share_link: Option<String>,
    /// alert: the number to watch, and the line
    #[serde(default)]
    alert: Option<AlertRule>,
}

/// A new report or alert on a dashboard (`?dry_run=1`: checked like the real thing, and what it
/// would be is answered; nothing saved).
pub async fn create(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(n): Body<NewSchedule>,
) -> ApiResult<Json<Value>> {
    caller.require("share")?;
    // who may send which kind of report, before anything else is looked at
    match n.format.as_str() {
        "csv" => caller.require("download")?,
        "signed" => super::guest::require_maker(&caller)?,
        "public" => caller.require("manage_members")?,
        "alert" => {}
        other => return Err(ApiError::bad_request(format!("{other:?}: the format is csv, signed, public or alert"))),
    }
    // a dashboard the files being applied add (api/code.rs) counts as there
    let d = match caller.planned.dashboards.get(&id) {
        Some(text) => crate::project::parse_dashboard(text).map_err(ApiError::bad_request)?,
        None => load(&state, &caller, &id).await?.0,
    };
    let org = caller.org().to_string();
    let store = store_of(&state, &caller).await?;
    let name = n.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request("give the report a name (up to 100 characters)"));
    }
    n.when.check().map_err(ApiError::bad_request)?;
    let mut recipients: Vec<String> =
        n.recipients.iter().map(|r| r.trim().to_lowercase()).filter(|r| !r.is_empty()).collect();
    recipients.sort();
    recipients.dedup();
    if recipients.is_empty() && n.webhooks.is_empty() {
        return Err(ApiError::bad_request("say who gets it: email addresses or webhooks"));
    }
    if recipients.len() > MAX_RECIPIENTS {
        return Err(ApiError::bad_request(format!("at most {MAX_RECIPIENTS} recipients")));
    }
    if !recipients.is_empty() {
        let server = match &caller.planned.mail {
            Some(planned) => planned.clone(),
            None => store
                .mail_server(&org)
                .await?
                .map(|(m, _)| m)
                .ok_or_else(|| ApiError::bad_request("set up a mail server first (Delivery) to email reports"))?,
        };
        let (members, domains) = audience(&caller.access, &org);
        if let Some(r) = recipients.iter().find(|r| !may_receive(r, &members, &domains, &server.allowed_domains)) {
            return Err(ApiError::bad_request(format!(
                "{r} isn't a member here or of an email domain your Org admins allow (Delivery)"
            )));
        }
    }
    let mut hooks: Vec<String> = store.webhooks(&org).await?.into_iter().map(|(name, _, _)| name).collect();
    hooks.extend(caller.planned.webhooks.iter().cloned());
    if let Some(w) = n.webhooks.iter().find(|w| !hooks.contains(w)) {
        return Err(ApiError::bad_request(format!("no webhook named {w:?} (Delivery)")));
    }
    if !n.request.is_null() {
        serde_json::from_value::<TileRequest>(n.request.clone())
            .map_err(|e| ApiError::bad_request(format!("request: {e}")))?;
    }
    let alert = match (n.format.as_str(), n.alert) {
        ("alert", Some(rule)) => Some(alert_on(&d, &rule)?),
        ("alert", None) => return Err(ApiError::bad_request("an alert needs its rule (tile, op, value)")),
        _ => None,
    };
    let (mut made_up, mut password_hash, mut guest_role, mut share_link) = (None, None, None, None);
    match n.format.as_str() {
        "signed" => {
            guest_role = Some(super::guest::guest_role(&caller, n.guest_role.as_deref())?.as_str().to_string());
            let pw = match n.password.as_deref().map(str::trim) {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => {
                    let p = password::generate();
                    made_up = Some(p.clone());
                    p
                }
            };
            password::check_policy(&pw, caller.email()).map_err(ApiError::bad_request)?;
            password_hash = Some(super::auth::hashing(&state, move || password::hash(&pw)).await??);
        }
        "public" => {
            let wanted = n.share_link.clone().unwrap_or_default();
            let links = store.share_links(&org, Some(&id)).await?;
            if !links.iter().any(|l| l.id() == wanted) {
                return Err(ApiError::bad_request("choose one of this dashboard's public links (Share publicly)"));
            }
            share_link = Some(wanted);
        }
        _ => {}
    }
    let now = Utc::now();
    let s = Schedule {
        id: random_token(9),
        dashboard_id: id.clone(),
        created_by: caller.email().to_string(),
        config: ScheduleConfig {
            name,
            when: n.when.clone(),
            format: n.format,
            recipients,
            webhooks: n.webhooks,
            request: n.request,
            guest_role,
            share_link,
            alert,
        },
        password_hash,
        active: true,
        next_run_at: n.when.next_after(now).trunc_subsecs(0),
        last_run_at: None,
        last_status: None,
        failures: 0,
        created_at: now.trunc_subsecs(6),
        firing: false,
    };
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true, "schedule": describe(&s)})));
    }
    store.put_schedule(&org, &s).await?;
    state.usage.record(crate::usage::Event {
        dashboard_id: Some(id),
        detail: Some(json!({"schedule": s.id, "format": s.config.format, "recipients": s.config.recipients.len(),
                            "webhooks": s.config.webhooks, "title": d.title})),
        ..caller.event("schedule_created")
    });
    let mut out = describe(&s);
    if let Some(p) = made_up {
        out["password"] = json!(p); // shown this once
    }
    Ok(Json(out))
}

/// An alert's rule, checked against the dashboard: a number to watch (a tile without dimensions),
/// one of its measures, and a comparison period for rises and falls.
fn alert_on(d: &crate::schema::Dashboard, rule: &AlertRule) -> ApiResult<AlertRule> {
    rule.check().map_err(ApiError::bad_request)?;
    let tile = d
        .tiles
        .iter()
        .find(|t| t.id == rule.tile)
        .ok_or_else(|| ApiError::bad_request(format!("no tile {:?} on this dashboard", rule.tile)))?;
    let q = tile
        .query
        .as_ref()
        .filter(|q| q.dimensions.is_empty() && !q.measures.is_empty())
        .ok_or_else(|| ApiError::bad_request("an alert watches a number: a tile without dimensions (a KPI)"))?;
    let measure = rule.measure.clone().unwrap_or_else(|| q.measures[0].clone());
    if !q.measures.contains(&measure) {
        return Err(ApiError::bad_request(format!("the tile has no measure {measure:?}")));
    }
    let compared = q.period.as_ref().is_some_and(|p| p.compare.is_some())
        || (q.period.is_none()
            && tile.options.get("period") != Some(&Value::Bool(false))
            && d.period.as_ref().is_some_and(|p| p.compare.is_some()));
    if rule.op.ends_with("_by") && !compared {
        return Err(ApiError::bad_request("a rise or fall needs a comparison period on the tile or the dashboard"));
    }
    Ok(AlertRule { measure: Some(measure), ..rule.clone() })
}

/// A number as people read it in a message.
fn shown(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.0}") } else { format!("{v:.2}") }
}

/// A schedule its maker or an Org admin may change.
async fn own(state: &AppState, caller: &Caller, id: &str) -> ApiResult<(Store, Schedule)> {
    caller.require("share")?;
    let store = store_of(state, caller).await?;
    let s = store.schedule(caller.org(), id).await?.ok_or_else(|| ApiError::not_found("no such scheduled report"))?;
    if s.created_by != caller.email() && !caller.can("manage_members") {
        return Err(ApiError::forbidden("only its maker or an Org admin can change this report"));
    }
    Ok((store, s))
}

pub async fn delete(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (store, s) = own(&state, &caller, &id).await?;
    store.delete_schedule(caller.org(), &s.id).await?;
    state.usage.record(crate::usage::Event {
        dashboard_id: Some(s.dashboard_id.clone()),
        detail: Some(json!({"schedule": s.id})),
        ..caller.event("schedule_deleted")
    });
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Active {
    active: bool,
}

/// Pause, or resume (from now on, with a clean slate).
pub async fn set_active(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    Body(a): Body<Active>,
) -> ApiResult<Json<Value>> {
    let (store, mut s) = own(&state, &caller, &id).await?;
    s.active = a.active;
    if a.active {
        s.failures = 0;
        s.next_run_at = s.config.when.next_after(Utc::now()).trunc_subsecs(0);
    }
    store.put_schedule(caller.org(), &s).await?;
    Ok(Json(describe(&s)))
}

/// Send it now (its schedule stays as it is).
pub async fn run_now(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let (store, mut s) = own(&state, &caller, &id).await?;
    let result = run(&state, caller.org(), &mut s).await;
    let s = record(&store, caller.org(), s, &result, Utc::now()).await?;
    Ok(Json(json!({"ok": result.is_ok(), "schedule": describe(&s)})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unsubscribe {
    org: String,
    token: String,
}

/// From the link in a report: stop sending it to this address. No account needed; the token is
/// sealed for this organisation, this report and this address.
pub async fn unsubscribe(State(state): State<Shared>, Body(u): Body<Unsubscribe>) -> ApiResult<Json<Value>> {
    let gone = || ApiError::not_found("this link doesn't work any more");
    let opened = key(&state)?.open(&u.token, &unsubscribe_context(&u.org)).map_err(|_| gone())?;
    let (id, email) = opened.split_once('|').ok_or_else(gone)?;
    let store = state.orgs.store(&u.org).await?;
    let mut s = store.schedule(&u.org, id).await?.ok_or_else(gone)?;
    s.config.recipients.retain(|r| r != email);
    store.put_schedule(&u.org, &s).await?;
    Ok(Json(json!({"ok": true, "report": s.config.name, "email": email})))
}

/// Remember how a run went; a schedule pauses when its maker can't send it any more, or after
/// MAX_FAILURES failed runs in a row.
async fn record(
    store: &Store,
    org: &str,
    mut s: Schedule,
    result: &Result<String, String>,
    now: DateTime<Utc>,
) -> ApiResult<Schedule> {
    s.last_run_at = Some(now.trunc_subsecs(6));
    match result {
        Ok(status) => {
            s.failures = 0;
            s.last_status = Some(status.clone());
        }
        Err(e) => {
            s.failures += 1;
            s.last_status = Some(e.clone());
            if e.starts_with(PAUSED) {
                s.active = false;
            } else if s.failures >= MAX_FAILURES {
                s.active = false;
                s.last_status = Some(format!("{PAUSED}{MAX_FAILURES} runs failed in a row; the last: {e}"));
            }
        }
    }
    store.put_schedule(org, &s).await?;
    Ok(s)
}

fn text(e: ApiError) -> String {
    e.into_parts().1["detail"].as_str().unwrap_or("failed").to_string()
}

/// One run: the report, as its maker, to its recipients and webhooks.
pub async fn run(state: &AppState, org: &str, s: &mut Schedule) -> Result<String, String> {
    let min = match s.config.format.as_str() {
        "signed" => Role::PowerUser,
        "public" => Role::OrgAdmin,
        _ => Role::User,
    };
    let access = LinkAccess { min, cap: Role::SystemOwner, filters: &[], name: format!("report {}", s.config.name) };
    let caller = link_caller(state, org, &s.created_by, access)
        .await
        .map_err(text)?
        .ok_or_else(|| format!("{PAUSED}its maker ({}) can't send this dashboard any more", s.created_by))?;
    let store = state.orgs.store(org).await.map_err(|e| text(e.into()))?;
    let (d, _) = load(state, &caller, &s.dashboard_id).await.map_err(text)?;
    let base = base_url(state);
    let key = key(state).map_err(text)?;
    let request: TileRequest = serde_json::from_value(s.config.request.clone()).unwrap_or_default();
    let mut link = format!("{base}/dashboards/{}", s.dashboard_id);
    let mut lines: Vec<String> = d.description.iter().cloned().collect();
    let mut attachment = None;
    let mut subject = format!("{}: {}", s.config.name, d.title);
    let mut done = String::new();
    match s.config.format.as_str() {
        "csv" => {
            let (file, bytes) =
                super::exports::zip_of(state, &caller, &s.dashboard_id, &request).await.map_err(text)?;
            if bytes.len() <= MAX_ATTACHMENT {
                lines.push(format!("Each chart's data is in {file}, as CSV."));
                attachment = Some((file, "application/zip".to_string(), bytes));
            } else {
                lines.push("The data is too large to attach: open the dashboard.".into());
            }
        }
        "signed" => {
            let token = random_token(32);
            // until the next report, and never longer than a guest link may last
            let until = (s.config.when.next_after(Utc::now()) + Duration::days(1))
                .min(Utc::now() + Duration::hours(super::guest::MAX_HOURS));
            let guest = GuestLink {
                token_hash: sha256_hex(&token),
                token_sealed: key.seal(&token, &super::guest::sealed_for(org)).map_err(|e| e.to_string())?,
                dashboard_id: s.dashboard_id.clone(),
                password_hash: s.password_hash.clone().ok_or("this report has no password")?,
                role: s.config.guest_role.clone().unwrap_or_else(|| "viewer".into()),
                filters: vec![],
                expires_at: until.trunc_subsecs(6),
                created_by: s.created_by.clone(),
                created_at: Utc::now().trunc_subsecs(6),
            };
            store.put_guest_link(org, &guest).await.map_err(|e| e.to_string())?;
            link = format!("{base}/g/{org}/{token}");
            lines.push(format!(
                "Open it with the password you were given. The link works until {}.",
                until.format("%Y-%m-%d %H:%M UTC")
            ));
        }
        "public" => {
            let wanted = s.config.share_link.clone().unwrap_or_default();
            let links = store.share_links(org, Some(&s.dashboard_id)).await.map_err(|e| e.to_string())?;
            let l = links.iter().find(|l| l.id() == wanted).ok_or("its public link was removed")?;
            let token = key.open(&l.token_sealed, &super::share::sealed_for(org)).map_err(|e| e.to_string())?;
            link = format!("{base}/s/{org}/{token}");
        }
        "alert" => {
            let rule = s.config.alert.clone().ok_or("this alert has no rule")?;
            let result = super::dashboards::tile_for(state, &caller, &s.dashboard_id, &rule.tile, &request)
                .await
                .map_err(text)?;
            let measure = rule.measure.clone().unwrap_or_default();
            let columns = result["columns"].as_array().cloned().unwrap_or_default();
            let i = columns.iter().position(|c| c["field"] == json!(measure)).ok_or("the tile no longer shows it")?;
            let name = columns[i]["name"].as_str().unwrap_or_default().to_string();
            let label = columns[i]["label"].as_str().unwrap_or(&measure).to_string();
            let number = |v: &Value| v.as_f64().or_else(|| v.as_str().and_then(|t| t.parse().ok()));
            let row = &result["rows"][0];
            let value = number(&row[i]);
            let change = columns
                .iter()
                .position(|c| c["name"] == json!(format!("{name}__change")))
                .and_then(|j| number(&row[j]));
            let said = rule.describe(&label);
            let now = value.map_or("no data".to_string(), shown);
            let met = rule.met(value, change);
            let was = std::mem::replace(&mut s.firing, met);
            if !met {
                return Ok(format!("checked: {label} is {now}"));
            }
            if was {
                return Ok(format!("still {said} (now {now}); already told"));
            }
            let change = change.map(|c| format!(", {:+.1}% on the previous period", c * 100.0)).unwrap_or_default();
            subject = format!("{}: {said}", s.config.name);
            lines = vec![format!("{said}: it is {now}{change}, on {}.", d.title)];
            done = format!("alerted: {said} (now {now}): ");
        }
        other => return Err(format!("unknown format {other:?}")),
    }

    let (mut sent, mut problems) = (vec![], vec![]);
    if !s.config.recipients.is_empty() {
        match store.mail_server(org).await.map_err(|e| e.to_string())? {
            None => problems.push("no mail server is set up (Delivery)".to_string()),
            Some((server, sealed)) => {
                let password = match sealed {
                    Some(p) => Some(key.open(&p, &format!("{org}/mail")).map_err(|e| e.to_string())?),
                    None => None,
                };
                let (members, domains) = audience(&caller.access, org);
                for r in &s.config.recipients {
                    if !may_receive(r, &members, &domains, &server.allowed_domains) {
                        problems.push(format!("{r} may no longer receive it"));
                        continue;
                    }
                    let token =
                        key.seal(&format!("{}|{r}", s.id), &unsubscribe_context(org)).map_err(|e| e.to_string())?;
                    // a query string: the platform workspace's org is empty, and tokens hold / and +
                    let stop = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("o", org)
                        .append_pair("t", &token)
                        .finish();
                    let body = format!(
                        "{}\n\n{}\n\n{link}\n\n--\n{}, sent for {}. Stop receiving it: {base}/unsubscribe?{stop}\n",
                        d.title,
                        lines.join("\n"),
                        s.config.name,
                        s.created_by
                    );
                    let mail = Mail {
                        to: vec![r.clone()],
                        subject: subject.clone(),
                        text: body,
                        attachments: attachment.iter().cloned().collect(),
                    };
                    match send_mail(state.connectors.net_guard(), &server, password.as_deref(), &mail).await {
                        Ok(()) => sent.push(r.clone()),
                        Err(e) => problems.push(format!("{r}: {e}")),
                    }
                }
            }
        }
    }
    let mut posted = 0;
    for w in &s.config.webhooks {
        let Some((kind, sealed)) = store.webhook(org, w).await.map_err(|e| e.to_string())? else {
            problems.push(format!("the webhook {w:?} was removed"));
            continue;
        };
        let url = key.open(&sealed, &format!("{org}/webhook/{w}")).map_err(|e| e.to_string())?;
        let kind: WebhookKind = serde_json::from_value(json!(kind)).map_err(|e| e.to_string())?;
        let m = Message { title: subject.clone(), text: lines.join("\n"), link: Some(link.clone()) };
        match post_webhook(state.connectors.net_guard(), &url, &payload(kind, &m)).await {
            Ok(()) => posted += 1,
            Err(e) => problems.push(format!("webhook {w}: {e}")),
        }
    }
    state.usage.record(crate::usage::Event {
        ok: problems.is_empty(),
        dashboard_id: Some(s.dashboard_id.clone()),
        detail: Some(json!({"schedule": s.id, "sent": sent.len(), "webhooks": posted, "problems": problems})),
        ..caller.event("schedule_run")
    });
    if !done.is_empty() && sent.is_empty() && posted == 0 {
        s.firing = false; // nobody was told: try again at the next check
    }
    let done = format!("{done}sent to {} people and {posted} webhooks", sent.len());
    if problems.is_empty() { Ok(done) } else { Err(format!("{done}; {}", problems.join("; "))) }
}

/// Run what's due, in every tenant (the server calls this every minute). Each run is claimed
/// first, so with several servers only one sends it.
pub async fn tick(state: &AppState, now: DateTime<Utc>) {
    let mut tenants = vec![String::new()];
    match state.store.org_databases().await {
        Ok(dbs) => tenants.extend(dbs.into_keys()),
        Err(e) => tracing::warn!("scheduled reports: organisations unavailable: {e:#}"),
    }
    for org in tenants {
        tick_tenant(state, &org, now).await;
    }
}

/// One tenant's due reports (an organisation that can't be reached waits; the others don't).
pub async fn tick_tenant(state: &AppState, org: &str, now: DateTime<Utc>) {
    let store = match state.orgs.store(org).await {
        Ok(s) => s,
        Err(e) => return tracing::warn!(org, "scheduled reports: database unavailable: {e}"),
    };
    let due = match store.due_schedules(org, now).await {
        Ok(d) => d,
        Err(e) => return tracing::warn!(org, "scheduled reports: {e:#}"),
    };
    for s in due {
        let next = s.config.when.next_after(now).trunc_subsecs(0);
        if !store.claim_schedule(org, &s.id, s.next_run_at, next).await.unwrap_or(false) {
            continue; // another server took it
        }
        let mut s = Schedule { next_run_at: next, ..s };
        let result = run(state, org, &mut s).await;
        if let Err(e) = &result {
            tracing::warn!(org, schedule = s.id, "scheduled report: {e}");
        }
        if let Err(e) = record(&store, org, s, &result, now).await {
            tracing::warn!(org, "scheduled report not recorded: {}", text(e));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use axum::http::StatusCode;
    use base64::Engine;
    use serde_json::{Value, json};

    use crate::api::delivery::tests::{fake_smtp, fake_webhook};
    use crate::api::share::tests::{clean_up, org_with, send};
    use crate::api::tests::OWNER;

    const DASHBOARD: &str = "title: Weekly sales\ntiles:\n  - {id: r, type: bar, query: {model: sales, dimensions: [region], measures: [transactions]}}\n";

    /// The next message a stand-in received (failing rather than waiting forever).
    async fn arrived<T>(rx: &mut tokio::sync::mpsc::UnboundedReceiver<T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv()).await.expect("nothing arrived").unwrap()
    }

    /// An email's text as a mail client shows it (long lines come quoted-printable).
    fn shown(mail: &str) -> String {
        mail.replace("=\n", "").replace("=3D", "=")
    }

    fn weekly() -> Value {
        json!({"every": "weekly", "day": 1, "at": "08:00", "timezone": "Europe/Rome"})
    }

    /// The text of an email's zip attachment's CSV files: (name, csv).
    fn attached_csv(mail: &str) -> Vec<(String, String)> {
        let start = mail.find("Content-Disposition: attachment").expect("an attachment");
        let body = &mail[start..];
        let body = &body[body.find("\n\n").unwrap() + 2..];
        let b64: String = body.lines().take_while(|l| !l.starts_with("--")).collect();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut f = zip.by_index(i).unwrap();
                let mut s = String::new();
                f.read_to_string(&mut s).unwrap();
                (f.name().to_string(), s)
            })
            .collect()
    }

    /// The (org, token) in an email's unsubscribe link: a query string, so any organisation
    /// (the platform workspace's is empty) and any token survive.
    fn unsubscribe_params(mail: &str) -> (String, String) {
        let i = mail.find("/unsubscribe?").expect("an unsubscribe link");
        let link = mail[i..].split_whitespace().next().unwrap();
        let url = url::Url::parse(&format!("http://x{link}")).unwrap();
        let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()).unwrap();
        (get("o"), get("t"))
    }

    #[tokio::test]
    async fn reports_go_by_email_to_members_and_allowed_domains() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let (smtp, mut mails) = fake_smtp().await;
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "bi@example.com",
                            "allowed_domains": ["partner.example"]});
        assert_eq!(send(&app, c, &org, "PUT", "/api/delivery/mail", Some(server), None).await.0, StatusCode::OK);
        let path = "/api/dashboards/shared/schedules";
        let report = |recipients: Value| json!({"name": "Monday numbers", "when": weekly(), "format": "csv", "recipients": recipients});

        // Users and up; members and allowed domains only; someone to send it to
        let (s, _, _) = send(&app, c, &org, "POST", path, Some(report(json!([OWNER]))), Some("viewer")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        for bad in [json!(["eve@evil.example"]), json!(["eve@sub.partner.example"]), json!([])] {
            let (s, b, _) = send(&app, c, &org, "POST", path, Some(report(bad.clone())), Some("user")).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {b}");
        }
        let (s, r, _) =
            send(&app, c, &org, "POST", path, Some(report(json!([OWNER, "Eve@Partner.example"]))), Some("user")).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["recipients"], json!(["eve@partner.example", OWNER]));
        let next = chrono::DateTime::parse_from_rfc3339(r["next_run_at"].as_str().unwrap()).unwrap();
        assert_eq!(chrono::Datelike::weekday(&next), chrono::Weekday::Mon);
        let id = r["id"].as_str().unwrap().to_string();

        // sent now: one email each, with the data attached
        let (s, b, _) = send(&app, c, &org, "POST", &format!("/api/schedules/{id}/run"), None, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(b["schedule"]["last_status"], json!("sent to 2 people and 0 webhooks"), "{b}");
        let (first, second) = (arrived(&mut mails).await, arrived(&mut mails).await);
        for m in [&first, &second] {
            assert!(m.contains("Subject: Monday numbers: Weekly sales"), "{m}");
            let files = attached_csv(m);
            assert_eq!(files[0].0, "r.csv");
            assert!(files[0].1.contains("Region,Transactions"), "{}", files[0].1);
        }
        assert!(first.contains("To: eve@partner.example") || second.contains("To: eve@partner.example"));

        // when it's due, the server sends it (once) and moves to the next Monday
        let store = state.orgs.store(&org).await.unwrap();
        let mut s = store.schedule(&org, &id).await.unwrap().unwrap();
        s.next_run_at = chrono::Utc::now() - chrono::Duration::minutes(1);
        store.put_schedule(&org, &s).await.unwrap();
        super::tick_tenant(&state, &org, chrono::Utc::now()).await;
        super::tick_tenant(&state, &org, chrono::Utc::now()).await;
        let eve = arrived(&mut mails).await;
        arrived(&mut mails).await;
        assert!(mails.try_recv().is_err(), "sent once");
        let s = store.schedule(&org, &id).await.unwrap().unwrap();
        assert!(
            s.next_run_at > chrono::Utc::now() && s.last_status.as_deref() == Some("sent to 2 people and 0 webhooks")
        );

        // the unsubscribe link in the email works without signing in, for that address only
        let mail = if eve.contains("To: eve@partner.example") { eve } else { first };
        let (o, token) = unsubscribe_params(&shown(&mail));
        assert_eq!(o, org);
        let (s, b, _) =
            send(&app, None, "", "POST", "/api/unsubscribe", Some(json!({"org": org, "token": token})), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(store.schedule(&org, &id).await.unwrap().unwrap().config.recipients, [OWNER]);
        let forged = json!({"org": org, "token": "x".repeat(40)});
        assert_eq!(send(&app, None, "", "POST", "/api/unsubscribe", Some(forged), None).await.0, StatusCode::NOT_FOUND);
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn a_dry_run_checks_a_report_and_saves_nothing() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let (smtp, _mails) = fake_smtp().await;
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "bi@example.com"});
        send(&app, c, &org, "PUT", "/api/delivery/mail", Some(server), None).await;
        let path = "/api/dashboards/shared/schedules";
        let body = json!({"name": "Monday numbers", "when": weekly(), "format": "csv", "recipients": [OWNER]});
        let (s, b, _) = send(&app, c, &org, "POST", &format!("{path}?dry_run=1"), Some(body), None).await;
        assert_eq!((s, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        // what it would be: when it would first run
        assert_eq!(b["schedule"]["name"], json!("Monday numbers"));
        assert!(b["schedule"]["next_run_at"].is_string(), "{b}");
        let (_, l, _) = send(&app, c, &org, "GET", path, None, None).await;
        assert_eq!(l["schedules"], json!([]));
        // and checked like the real thing
        let bad = json!({"name": "x", "when": weekly(), "format": "csv", "webhooks": ["nope"]});
        let (s, b, _) = send(&app, c, &org, "POST", &format!("{path}?dry_run=1"), Some(bad), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn signed_and_public_links_and_webhooks() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let (smtp, mut mails) = fake_smtp().await;
        let (hook, mut posts) = fake_webhook().await;
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "bi@example.com"});
        send(&app, c, &org, "PUT", "/api/delivery/mail", Some(server), None).await;
        let w = json!({"kind": "slack", "url": format!("http://127.0.0.1:{hook}/hook")});
        send(&app, c, &org, "PUT", "/api/delivery/webhooks/team", Some(w), None).await;
        let path = "/api/dashboards/shared/schedules";
        let (_, l, _) = send(&app, c, &org, "GET", path, None, Some("user")).await;
        assert_eq!((&l["webhooks"], &l["mail"]), (&json!(["team"]), &json!(true)), "{l}");
        assert!(!l.to_string().contains("/hook"), "names only: {l}");

        // a fresh guest link each time, with the report's password: Power users and up
        let signed = json!({"name": "For partners", "when": weekly(), "format": "signed", "recipients": [OWNER],
                            "webhooks": ["team"], "guest_role": "viewer"});
        assert_eq!(
            send(&app, c, &org, "POST", path, Some(signed.clone()), Some("user")).await.0,
            StatusCode::FORBIDDEN
        );
        let (s, r, _) = send(&app, c, &org, "POST", path, Some(signed), Some("power_user")).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let password = r["password"].as_str().expect("made up and shown once").to_string();
        let id = r["id"].as_str().unwrap();
        send(&app, c, &org, "POST", &format!("/api/schedules/{id}/run"), None, None).await;
        let mail = arrived(&mut mails).await;
        let link = shown(&mail).split_whitespace().find(|w| w.contains("/g/")).expect("a guest link").to_string();
        assert!(!mail.contains(&password), "the password never travels with the link");
        let api = link[link.find("/g/").unwrap()..].replacen("/g/", "/api/guest/", 1);
        let (s, _, _) =
            send(&app, None, "", "POST", &format!("{api}/unlock"), Some(json!({"password": password})), None).await;
        assert_eq!(s, StatusCode::OK);
        let post = arrived(&mut posts).await;
        assert!(post["text"].as_str().unwrap().contains("/g/"), "the webhook gets the link: {post}");

        // a public link: Org admins, and only one of the dashboard's own
        let public = |link: &str| json!({"name": "Public", "when": weekly(), "format": "public", "webhooks": ["team"], "share_link": link});
        assert_eq!(
            send(&app, c, &org, "POST", path, Some(public("x")), Some("power_user")).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(send(&app, c, &org, "POST", path, Some(public("nope")), None).await.0, StatusCode::BAD_REQUEST);
        let (_, l, _) = send(
            &app,
            c,
            &org,
            "POST",
            "/api/dashboards/shared/share-links",
            Some(json!({"cache_seconds": 300})),
            None,
        )
        .await;
        let (s, r, _) = send(&app, c, &org, "POST", path, Some(public(l["id"].as_str().unwrap())), None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        send(&app, c, &org, "POST", &format!("/api/schedules/{}/run", r["id"].as_str().unwrap()), None, None).await;
        let post = arrived(&mut posts).await;
        let token = l["url"].as_str().unwrap().rsplit('/').next().unwrap();
        assert!(post["text"].as_str().unwrap().contains(&format!("/s/{org}/{token}")), "{post}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn reports_run_as_their_maker_and_pause_when_they_cannot() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let (smtp, mut mails) = fake_smtp().await;
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "bi@example.com"});
        send(&app, Some(&cookie), &org, "PUT", "/api/delivery/mail", Some(server), None).await;
        // a member who sees EMEA only
        let ana = format!("ana-{}@sched.example", &org[5..]);
        state
            .store
            .set_password(&ana, None, &crate::auth::password::hash("ana-password-1").unwrap(), None, false)
            .await
            .unwrap();
        let session = state.store.create_session_as(&ana, "password", chrono::Duration::hours(1), None).await.unwrap();
        let ana_cookie = format!("magnum_session={session}");
        let pin = |with_ana: bool| {
            let members = if with_ana {
                json!({"users": {ana.clone(): {"role": "user", "filters": [{"model": "sales", "dimension": "region", "values": ["EMEA"]}]}}})
            } else {
                json!({"users": {}})
            };
            let access = json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "S", "models": ["*"], "members": members}}});
            state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        };
        pin(true);
        let body = json!({"name": "Mine", "when": weekly(), "format": "csv", "recipients": [ana]});
        let (s, r, _) =
            send(&app, Some(&ana_cookie), &org, "POST", "/api/dashboards/shared/schedules", Some(body), None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let id = r["id"].as_str().unwrap().to_string();
        let run = format!("/api/schedules/{id}/run");
        send(&app, Some(&ana_cookie), &org, "POST", &run, None, None).await;
        let csv = &attached_csv(&arrived(&mut mails).await)[0].1;
        assert!(csv.contains("EMEA") && !csv.contains("APAC") && !csv.contains("AMER"), "her restriction: {csv}");

        // she leaves: the next run pauses it instead of sending
        pin(false);
        let (_, b, _) = send(&app, Some(&cookie), &org, "POST", &run, None, None).await;
        assert_eq!(b["schedule"]["active"], json!(false), "{b}");
        assert!(b["schedule"]["last_status"].as_str().unwrap().starts_with("paused"), "{b}");
        assert!(mails.try_recv().is_err());
        // other organisations don't see it; its own Org admins do
        let (_, all, _) = send(&app, Some(&cookie), &org, "GET", "/api/schedules", None, None).await;
        assert_eq!(all["schedules"].as_array().unwrap().len(), 1);
        let (_, none, _) = send(&app, Some(&cookie), "", "GET", "/api/schedules", None, None).await;
        assert!(!none.to_string().contains(&id), "{none}");
        let c = state.store.pool.get().await.unwrap();
        c.execute("DELETE FROM sessions WHERE user_email = $1", &[&ana]).await.unwrap();
        c.execute("DELETE FROM users WHERE email = $1", &[&ana]).await.unwrap();
        clean_up(&state, &org).await;
    }

    const ALERTS: &str = "title: Watch\ntiles:\n  - {id: n, type: kpi, query: {model: sales, measures: [transactions]}}\n  - {id: cmp, type: kpi, query: {model: sales, measures: [transactions], period: {field: sold_at, current: {last_n_days: 30}, compare: previous_period}}}\n  - {id: r, type: bar, query: {model: sales, dimensions: [region], measures: [transactions]}}\n";

    #[tokio::test]
    async fn alerts_tell_once_when_a_number_crosses_the_line() {
        let Some((app, state, cookie, org)) = org_with(ALERTS).await else { return };
        let c = Some(cookie.as_str());
        let (smtp, mut mails) = fake_smtp().await;
        let (hook, mut posts) = fake_webhook().await;
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "bi@example.com"});
        send(&app, c, &org, "PUT", "/api/delivery/mail", Some(server), None).await;
        let w = json!({"kind": "webhook", "url": format!("http://127.0.0.1:{hook}/hook")});
        send(&app, c, &org, "PUT", "/api/delivery/webhooks/ops", Some(w), None).await;
        let path = "/api/dashboards/shared/schedules";
        let hourly = json!({"every": "hourly", "at": "00:05", "timezone": "UTC"});
        let alert = |rule: Value| {
            json!({"name": "Sales watch", "when": hourly, "format": "alert", "recipients": [OWNER],
                                         "webhooks": ["ops"], "alert": rule})
        };

        // a number to watch: a tile without dimensions; a change needs a comparison period
        for (bad, why) in [
            (json!({"tile": "r", "op": "above", "value": 1}), "a number"),
            (json!({"tile": "nope", "op": "above", "value": 1}), "no tile"),
            (json!({"tile": "n", "measure": "units", "op": "above", "value": 1}), "measure"),
            (json!({"tile": "n", "op": "rises_by", "value": 10}), "comparison"),
            (json!({"tile": "n", "op": "equals", "value": 1}), "above, below"),
        ] {
            let (s, b, _) = send(&app, c, &org, "POST", path, Some(alert(bad.clone())), Some("user")).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {b}");
            assert!(b["detail"].as_str().unwrap().contains(why), "{bad}: {b}");
        }
        let rule = json!({"tile": "n", "op": "above", "value": 1});
        assert_eq!(
            send(&app, c, &org, "POST", path, Some(alert(rule.clone())), Some("viewer")).await.0,
            StatusCode::FORBIDDEN
        );
        let (s, r, _) = send(&app, c, &org, "POST", path, Some(alert(rule)), Some("user")).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let id = r["id"].as_str().unwrap().to_string();
        let run = format!("/api/schedules/{id}/run");

        // over the line: told, by email and webhook
        let (_, b, _) = send(&app, c, &org, "POST", &run, None, None).await;
        assert!(b["schedule"]["last_status"].as_str().unwrap().starts_with("alerted"), "{b}");
        let m = arrived(&mut mails).await;
        assert!(m.contains("Subject: Sales watch: Transactions above 1"), "{m}");
        let p = arrived(&mut posts).await;
        assert!(p["title"].as_str().unwrap().contains("Transactions above 1") && p["link"].is_string(), "{p}");
        // still over: not told again
        let (_, b, _) = send(&app, c, &org, "POST", &run, None, None).await;
        assert!(b["schedule"]["last_status"].as_str().unwrap().starts_with("still"), "{b}");
        assert!(tokio::time::timeout(std::time::Duration::from_millis(500), mails.recv()).await.is_err(), "told twice");
        // back under the line, then over again: told again
        let store = state.orgs.store(&org).await.unwrap();
        let set = |value: f64| {
            let store = store.clone();
            let (org, id) = (org.clone(), id.clone());
            async move {
                let mut s = store.schedule(&org, &id).await.unwrap().unwrap();
                s.config.alert.as_mut().unwrap().value = value;
                store.put_schedule(&org, &s).await.unwrap();
            }
        };
        set(1e12).await;
        let (_, b, _) = send(&app, c, &org, "POST", &run, None, None).await;
        assert!(b["schedule"]["last_status"].as_str().unwrap().starts_with("checked"), "{b}");
        set(1.0).await;
        send(&app, c, &org, "POST", &run, None, None).await;
        assert!(arrived(&mut mails).await.contains("Transactions above 1"));

        // a change on the comparison period
        let rule = json!({"tile": "cmp", "op": "falls_by", "value": 1000});
        let (s, r, _) = send(&app, c, &org, "POST", path, Some(alert(rule)), Some("user")).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let (_, b, _) =
            send(&app, c, &org, "POST", &format!("/api/schedules/{}/run", r["id"].as_str().unwrap()), None, None).await;
        assert!(
            b["schedule"]["last_status"].as_str().unwrap().starts_with("checked"),
            "a 1000% fall can't happen: {b}"
        );

        // over the line but nobody could be told: not settled, so it tries again next time
        send(&app, c, &org, "DELETE", "/api/delivery/mail", None, None).await;
        let mut sched = store.schedule(&org, &id).await.unwrap().unwrap();
        sched.config.webhooks.clear();
        sched.firing = false;
        store.put_schedule(&org, &sched).await.unwrap();
        let (_, b, _) = send(&app, c, &org, "POST", &run, None, None).await;
        assert!(b["schedule"]["last_status"].as_str().unwrap().contains("no mail server"), "{b}");
        assert_eq!(b["schedule"]["firing"], json!(false), "{b}");
        clean_up(&state, &org).await;
    }
}
