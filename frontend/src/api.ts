import type { Theme } from './theme'
import { guestPath } from './guest'
import type { ModelDef } from './modelEditor'
// Typed client for the Magnum BI backend. Types mirror backend/src/schema.rs.

// time = TIMESTAMP (UTC instant); datetime = naive local date-time (e.g. store-local)
export type DimType = 'string' | 'number' | 'boolean' | 'date' | 'time' | 'datetime'
export type FilterOp =
  | 'equals' | 'not_equals' | 'contains' | 'gt' | 'gte' | 'lt' | 'lte' | 'between'
  | 'is_null' | 'not_null' | 'last_n_days'
export type ChartType = 'kpi' | 'table' | 'bar' | 'hbar' | 'line' | 'area' | 'pie' | 'scatter'
export type Value = string | number | boolean | null

export type Aggregation = 'sum' | 'avg' | 'min' | 'max' | 'count' | 'count_distinct'
export const AGG_LABELS: Record<Aggregation, string> = {
  sum: 'Sum', avg: 'Average', min: 'Min', max: 'Max', count: 'Count', count_distinct: 'Unique count',
}
export interface Dimension {
  name: string; label?: string | null; description?: string | null; type: DimType
  /** how it aggregates when used as a number (numbers: sum; identifiers: count_distinct) */
  aggregation?: Aggregation | null
}
export interface TopN { dimension: string; n: number; by: string; bottom?: boolean }
export interface Measure { name: string; label?: string | null; description?: string | null; type: string; format: 'number' | 'currency' | 'percent'; higher_is_better?: boolean }
export interface ModelInfo {
  name: string; label: string; description?: string | null; connection: string
  /** the organisation's connection of that name: null when it has none yet */
  connection_type: string | null
  extends?: string | null  // extension models inherit their base model's data restrictions
  dimensions: Dimension[]; measures: Measure[]; hierarchies: Record<string, string[]>
  /** a blend: models joined on shared fields (its fields are written model.field) */
  blend?: { base: string; joins: BlendJoin[] } | null
  /** the organisation's own (made in the app, on Models) */
  own?: boolean
}

/** A join key: "base_field = field", with a grain for dates and times ("sold_at:day = day:day") */
export interface BlendJoin { model: string; on: string[] }
export interface Blend {
  name: string; label?: string | null; description?: string | null; base: string; joins: BlendJoin[]
  /** seconds results may be reused */
  cache?: number | null
  /** the organisation's own (defined here); otherwise defined in the project's code */
  editable?: boolean
}

export interface Filter { field: string; op?: FilterOp; values?: Value[] }
export type Preset = 'month_to_date' | 'quarter_to_date' | 'year_to_date'
export interface PeriodRange { last_n_days?: number; preset?: Preset; from?: string; to?: string }
export type Compare = 'previous_period' | 'previous_year' | { from: string } | null
export interface Period { field: string; current: PeriodRange; compare?: Compare }
export interface ResolvedPeriod { current: [string, string]; previous: [string, string] | null; offset_days: number | null }
export interface Query {
  model: string; dimensions?: string[]; measures?: string[]; filters?: Filter[]; sort?: string[]; limit?: number
  period?: Period | null
  /** keep the N best (or worst) values of a dimension, ranked by a measure */
  top?: TopN | null
}
export interface Column {
  name: string; field: string; label: string; kind: 'dimension' | 'measure'; type: string
  format?: string | null; grain?: string | null
  compare?: 'previous' | 'change' | null; base?: string | null; higher_is_better?: boolean | null
}
export interface QueryResult {
  columns: Column[]; rows: Value[][]
  stats: { duration_ms?: number; cache_hit?: boolean; cached_at?: string; cache_ttl_s?: number }
  sql: string | null  // only for roles that may see SQL
  period?: ResolvedPeriod | null
}

export interface TileLayout { x: number; y: number; w: number; h: number }
export interface Tile {
  id: string; title?: string | null; type: ChartType; query: Query
  drill?: string | string[] | null; layout: TileLayout; options?: Record<string, unknown>
}
export interface DashboardFilter { model: string; field: string; label?: string | null; op?: FilterOp; default?: Value[] }
export interface DashboardPeriod extends Period { model: string }
export interface Dashboard { title: string; description?: string | null; theme?: string | null; period?: DashboardPeriod | null; filters: DashboardFilter[]; tiles: Tile[] }
export interface DashboardResponse {
  id: string; dashboard: Dashboard; yaml: string | null; warnings: string[]
  /** send back as If-Match to save only over this version */
  version?: string
}
export interface DashboardSummary { id: string; title: string; description?: string | null; tiles?: number; error?: string }

export type Role = 'viewer' | 'user' | 'power_user' | 'org_admin' | 'system_owner'
export type Permission = 'view' | 'filter' | 'drill' | 'share' | 'explore' | 'create' | 'edit' | 'view_sql' | 'manage_members' | 'manage_orgs' | 'manage_system'
export interface AuthState {
  required: boolean; authenticated: boolean; user: { name?: string; email?: string } | null
  /** the sign-in methods this server offers */
  methods: { password: boolean; google: boolean }
  method?: string; has_password?: boolean
  /** signs in with an authenticator code after the password */
  two_factor?: boolean
  /** the organisation now requires an authenticator code: set one up before anything else */
  two_factor_setup?: boolean
  /** an admin issued a one-time password: a new one must be chosen before anything else */
  must_change_password?: boolean
  role?: Role; role_label?: string; actual_role?: Role; permissions?: Permission[]; view_as?: Role[]
  restrictions?: Restriction[]
  /** false: signed in, but not a member of any organisation */
  member?: boolean
  org?: OrgRef | null  // the organisation this session works in (null: platform workspace)
  orgs?: OrgRef[]      // system owners: organisations they can switch into
}
export interface OrgRef { id: string; name: string }
/** Only rows of `model` whose `dimension` is one of `values` (empty: no rows). */
export interface RowRule { model: string; dimension: string; values: Value[] }
export interface PersonAccess { role: Role; filters: RowRule[] }
/** An organisation's people (Org admins manage this). */
export interface Members { default_role: Role; default_filters: RowRule[]; users: Record<string, PersonAccess> }
/** A sign-in account (members without one haven't been given a password nor signed in with Google) */
export interface Account { last_login: number | null; password: boolean; google: boolean; disabled: boolean }
export interface RoleInfo { id: Role; label: string; permissions: Permission[] }
export interface MembersResponse {
  org: OrgRef & { domains: string[]; filters: RowRule[]; models: string[] }
  members: Members; roles: RoleInfo[]; errors: Record<string, string>
  last_login: Record<string, number>  // epoch seconds; people missing here haven't signed in yet
  accounts: Record<string, Account>
  password_login: boolean
}
/** An organisation's definition (system owners only). */
export interface Organisation {
  name: string; domains: string[]; models: string[]; filters: RowRule[]
  /** the platform's sign-in rules for this organisation (its admins can only tighten them) */
  sign_in?: SignInPolicy | null
}
export interface Platform {
  system_owners: string[]; organisations: Record<string, Organisation>
  /** sign-in rules for everyone */
  sign_in?: SignInPolicy | null
}
export interface PlatformResponse {
  platform: Platform; members: Record<string, number>; models: { name: string; label: string }[]
  errors: Record<string, string>
  /** each organisation's own ScramDB (connection string redacted) */
  databases: Record<string, { display: string; updated_at: string; updated_by: string | null }>
  /** MAGNUM_SECRET_KEY is set (needed to store organisation databases) */
  secret_key: boolean
}
export interface UsageSummary {
  scope: string; days: number
  totals: { people: number; queries: number; cache_hit_rate: number | null; failed_queries: number; avg_query_ms: number | null
    dashboard_views: number; sign_ins: number }
  by_day: { day: string; people: number; queries: number; dashboard_views: number }[]
  dashboards: { org: string; dashboard_id: string | null; views: number; people: number }[]
  people: { email: string; org: string; role: string | null; queries: number; dashboard_views: number; last_seen: string }[]
  models: { model: string | null; queries: number; cache_hits: number; failed: number; avg_ms: number | null }[]
  orgs: { org: string; people: number; queries: number; dashboard_views: number }[]
}
/** A data connection of the organisation (its secret is never sent back) */
export type ConnectionType = 'scramdb' | 'postgres' | 'bigquery' | 'duckdb'
export interface ConnectionInfo {
  name: string; type: ConnectionType; config: Record<string, unknown>; has_secret: boolean; dialect: string
  updated_at: string; updated_by: string | null
}
export interface CacheStats {
  ttl_seconds: Record<string, number>
  cached: Record<string, { results: number; rows: number | null; hits: number | null }>
}
export interface Restriction {
  model: string; model_label: string; dimension: string; label: string; values: Value[]
  source?: 'organisation' | 'personal'
}
export interface TileRequest {
  filters?: Record<string, Value[]>; period?: { current: PeriodRange; compare?: Compare } | null
  drill?: { field: string; value: Value }[]
}

// "view as" a lower role (the server only ever lets a role go down)
const VIEW_AS_KEY = 'magnum.viewAs'
export const getViewAs = () => { try { return localStorage.getItem(VIEW_AS_KEY) } catch { return null } }
export const setViewAs = (r: Role | null) => {
  try { if (r) localStorage.setItem(VIEW_AS_KEY, r); else localStorage.removeItem(VIEW_AS_KEY) } catch { /* ignore */ }
}

// system owners: which organisation to work in (none: the platform workspace). Members are always
// in their own organisation; the server ignores this for them.
const ORG_KEY = 'magnum.org'
export const getOrg = () => { try { return localStorage.getItem(ORG_KEY) } catch { return null } }
export const setOrg = (id: string | null) => {
  try { if (id) localStorage.setItem(ORG_KEY, id); else localStorage.removeItem(ORG_KEY) } catch { /* ignore */ }
}

export type SignInMethod = 'password' | 'google' | 'oidc' | 'passkey' | 'token'
export const SIGN_IN_METHODS: SignInMethod[] = ['password', 'google', 'oidc', 'passkey', 'token']
export const SIGN_IN_LABELS: Record<SignInMethod, string> = {
  password: 'Password', google: 'Google', oidc: "Organisation's single sign-on", passkey: 'Passkeys', token: 'API tokens (agents)',
}
/** An organisation's own themes and its default light and dark theme. */
export interface OrgThemes { themes: Theme[]; default_light?: string | null; default_dark?: string | null }
export interface OrgThemesResponse extends OrgThemes { version: string; available: Theme[] }

/** The NYC demo's setup (taxi trips x weather), as it goes. */
export interface DemoStatus { running: boolean; org: string; step: string; done: number; total: number; log: string[]; error: string | null }

/** An API token (never its secret, except once when created). */
/** A public link: anyone with `url` sees the dashboard, from results kept `cache_seconds`. */
export interface ShareLink {
  id: string; dashboard: string; url: string; qr_svg: string
  cache_seconds: number; created_by: string; created_at: string
}

/** What a public link shows: the layout (no queries) and the organisation's theme. */
export interface PublicDashboard {
  title: string; description?: string | null; cache_seconds: number; theme: Theme | null
  tiles: { id: string; title?: string | null; type: ChartType; layout: TileLayout; options?: Record<string, unknown> }[]
}
export type PublicResult = QueryResult & { as_of: string; expires_at?: string; from_cache: boolean }

/** A guest link: anyone with it and its password uses the dashboard, until `expires_at`. */
export interface GuestLink {
  id: string; dashboard: string; url: string; qr_svg: string; role: Role
  filters: { model: string; dimension: string; values: Value[] }[]
  expires_at: string; created_by: string; created_at: string
  /** only when Magnum BI made the password up: shown this once */
  password?: string
}

/** What a guest link opens: the dashboard, its models, the guest's role and limits. */
export interface GuestDashboard {
  id: string; dashboard: Dashboard; models: ModelInfo[]; theme: Theme | null
  role: Role; permissions: Permission[]; filters: GuestLink['filters']; expires_at: string; shared_by: string
}

/** While a guest link is open: its API (/api/guest/<org>/<token>), where tiles, filter values and
 *  periods are asked for instead of the signed-in endpoints. */
let guestBase: string | null = null
export const setGuestBase = (base: string | null) => { guestBase = base }

/** Public pages: no session, no organisation header. */
async function publicCall<T>(path: string, signal?: AbortSignal): Promise<T> {
  const r = await fetch(path, { signal })
  const body = await r.json().catch(() => ({}))
  if (!r.ok) throw new ApiError(r.status, typeof body.detail === 'string' ? body.detail : r.statusText)
  return body as T
}

export interface ApiToken {
  id: string; name: string; user_email?: string; role: Role; scope: 'read' | 'write'
  created_at?: string; expires_at: string; last_used_at?: string | null; revoked_at?: string | null
}
/** One level's sign-in rules: allowed mechanisms (omitted: no restriction), required second factor. */
export interface SignInPolicy { methods?: SignInMethod[] | null; require_two_factor?: boolean }
export interface SignInMethods { methods: SignInMethod[]; oidc: { label: string; url: string } | null }
export interface PasskeyInfo { id: string; name: string; created_at: string; last_used_at: string | null }
export interface OidcInput {
  label: string; issuer: string; client_id: string; client_secret?: string
  groups_claim?: string; group_roles?: Record<string, Role>
}
export interface SignInSettings {
  /** what the platform allows this organisation */
  platform: { methods: SignInMethod[]; require_two_factor: boolean }
  sign_in: SignInPolicy
  oidc: (Omit<OidcInput, 'client_secret'> & { has_secret: boolean }) | null
  domains: string[]
  callback_url: string
  secret_key: boolean
}

/** Fired when the server says the user must (re-)sign in; the AuthGate shows the login screen. */
export const AUTH_REQUIRED_EVENT = 'magnum:auth-required'

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) { super(message); this.status = status }
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const viewAs = getViewAs()
  const headers = new Headers(init?.headers)
  if (viewAs) headers.set('x-magnum-view-as', viewAs)
  const org = getOrg()
  if (org) headers.set('x-magnum-org', org)
  const r = await fetch(path, { ...init, headers })
  if (!r.ok) {
    let msg = r.statusText
    try {
      const body = await r.json()
      // not signed in, or a new password is required: refresh the auth state
      if ((r.status === 401 && body.login) || (r.status === 403 && body.change_password)) window.dispatchEvent(new CustomEvent(AUTH_REQUIRED_EVENT))
      msg = typeof body.detail === 'string' ? body.detail
        : Array.isArray(body.detail) ? body.detail.map((d: { loc: unknown[]; msg: string }) => `${d.loc.slice(1).join('.')}: ${d.msg}`).join('; ')
        : JSON.stringify(body)
    } catch { /* not json */ }
    throw new ApiError(r.status, msg)
  }
  const ct = r.headers.get('content-type') ?? ''
  return (ct.includes('json') ? r.json() : r.text()) as Promise<T>
}

const json = (method: string, body: unknown): RequestInit =>
  ({ method, headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })

export const api = {
  models: () => call<{ models: ModelInfo[]; errors: Record<string, string> }>('/api/models'),
  query: (q: Query, signal?: AbortSignal) => call<QueryResult>('/api/query', { ...json('POST', q), signal }),
  tileQuery: (dash: string, tile: string, req: TileRequest, signal?: AbortSignal) =>
    call<QueryResult>(guestPath(guestBase, dash, { tile }), { ...json('POST', req), signal }),
  filterValues: (dash: string, key: string) => call<QueryResult>(guestPath(guestBase, dash, { key })),
  members: () => call<MembersResponse>('/api/members'),
  saveMembers: (m: Members) => call<MembersResponse>('/api/members', json('PUT', m)),
  platform: () => call<PlatformResponse>('/api/platform'),
  savePlatform: (p: Platform) => call<PlatformResponse>('/api/platform', json('PUT', p)),
  publishTemplate: (id: string, orgs: string[], as?: string, overwrite = false) =>
    call<{ results: Record<string, string> }>(`/api/platform/dashboards/${encodeURIComponent(id)}/publish`,
      json('POST', { orgs, as: as || undefined, overwrite })),
  setOrgDatabase: (org: string, dsn: string) =>
    call<{ display: string; version: string; dashboards_moved: number }>(
      `/api/platform/organisations/${encodeURIComponent(org)}/database`, json('PUT', { dsn })),
  orgThemes: () => call<OrgThemesResponse>('/api/org-themes'),
  saveOrgThemes: (doc: OrgThemes, version?: string) => {
    const init = json('PUT', doc)
    if (version) init.headers = { ...(init.headers as Record<string, string>), 'if-match': version }
    return call<{ ok: boolean; version: string }>('/api/org-themes', init)
  },
  importThemeCss: (css: string, base?: string) => call<Theme>('/api/themes/import', json('POST', { css, base })),
  themeCss: (id: string) => call<string>(`/api/themes/${encodeURIComponent(id)}/css`),
  demoStatus: () => call<DemoStatus>('/api/platform/demo'),
  startDemo: (org: string, months: number) => call<DemoStatus>('/api/platform/demo', json('POST', { org, months })),
  usage: (days: number) => call<UsageSummary>(`/api/usage?days=${days}`),
  cacheStats: () => call<CacheStats>('/api/cache'),
  clearCache: (model?: string) => call<{ cleared: number }>(`/api/cache${model ? `?model=${encodeURIComponent(model)}` : ''}`, { method: 'DELETE' }),
  connections: () => call<{ connections: ConnectionInfo[]; secret_key: boolean }>('/api/connections'),
  saveConnection: (name: string, config: Record<string, unknown>, secret?: string) =>
    call<{ ok: boolean }>(`/api/connections/${encodeURIComponent(name)}`, json('PUT', { config, secret: secret || undefined })),
  testConnection: (name: string, config: Record<string, unknown>, secret?: string) =>
    call<{ ok: boolean; message: string }>(`/api/connections/${encodeURIComponent(name)}/test`, json('POST', { config, secret: secret || undefined })),
  deleteConnection: (name: string) => call<{ ok: boolean }>(`/api/connections/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  resolvePeriod: (model: string, p: Period) => call<ResolvedPeriod>(guestPath(guestBase, '', { model }), json('POST', p)),
  dashboards: () => call<{ dashboards: DashboardSummary[] }>('/api/dashboards'),
  blends: () => call<{ blends: Blend[] }>('/api/blends'),
  saveBlend: (b: Blend) => call<{ ok: boolean }>(`/api/blends/${encodeURIComponent(b.name)}`, json('PUT', { ...b, editable: undefined })),
  deleteBlend: (name: string) => call<{ ok: boolean }>(`/api/blends/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  me: () => call<AuthState>('/api/auth/me'),
  /** the ways this email can sign in (sign-in page, step 2) */
  signInMethods: (email: string) => call<SignInMethods>('/api/auth/methods', json('POST', { email })),
  login: (email: string, password: string) =>
    call<{ ok: boolean; must_change_password?: boolean; two_factor?: boolean; ticket?: string }>('/api/auth/login', json('POST', { email, password })),
  loginTwoFactor: (ticket: string, code: string) => call<{ ok: boolean }>('/api/auth/login/two-factor', json('POST', { ticket, code })),
  twoFactorSetup: () => call<{ secret: string; qr_svg: string; uri: string }>('/api/auth/two-factor/setup', json('POST', {})),
  twoFactorEnable: (code: string) => call<{ ok: boolean; recovery_codes: string[] }>('/api/auth/two-factor/enable', json('POST', { code })),
  twoFactorDisable: (password: string) => call<{ ok: boolean }>('/api/auth/two-factor/disable', json('POST', { password })),
  passkeys: () => call<{ passkeys: PasskeyInfo[]; available: boolean }>('/api/auth/passkeys'),
  deletePasskey: (id: string) => call<{ ok: boolean }>(`/api/auth/passkeys/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  passkeyRegisterStart: () => call<{ ticket: string; options: unknown }>('/api/auth/passkeys/register/start', json('POST', {})),
  passkeyRegisterFinish: (ticket: string, name: string, credential: unknown) =>
    call<{ ok: boolean }>('/api/auth/passkeys/register/finish', json('POST', { ticket, name, credential })),
  passkeyLoginStart: (email: string) => call<{ ticket: string; options: unknown }>('/api/auth/passkeys/login/start', json('POST', { email })),
  passkeyLoginFinish: (ticket: string, credential: unknown) =>
    call<{ ok: boolean }>('/api/auth/passkeys/login/finish', json('POST', { ticket, credential })),
  signInSettings: () => call<SignInSettings>('/api/members/sign-in'),
  saveSignInSettings: (s: { sign_in: SignInPolicy | null; oidc: OidcInput | null }) =>
    call<SignInSettings>('/api/members/sign-in', json('PUT', s)),
  changePassword: (current: string | null, next: string) =>
    call<{ ok: boolean }>('/api/auth/password', json('POST', { current, new: next })),
  logout: () => call<{ ok: boolean }>('/api/auth/logout', { method: 'POST' }),
  /** Org admins: a one-time password for a member, shown once */
  resetMemberPassword: (email: string) => call<{ email: string; password: string }>('/api/members/password', json('POST', { email })),
  dashboard: (id: string) => call<DashboardResponse>(`/api/dashboards/${encodeURIComponent(id)}`),
  /** `version`: the one read; the save is refused (409) if someone changed it since */
  saveDashboard: (id: string, d: Dashboard, version?: string | null) => {
    const init = json('PUT', d)
    if (version) init.headers = { ...(init.headers as Record<string, string>), 'if-match': version }
    return call<DashboardResponse>(`/api/dashboards/${encodeURIComponent(id)}`, init)
  },
  saveDashboardYaml: (id: string, text: string, version?: string | null) => call<DashboardResponse>(`/api/dashboards/${encodeURIComponent(id)}/yaml`,
    { method: 'PUT', headers: { 'content-type': 'text/plain', ...(version ? { 'if-match': version } : {}) }, body: text }),
  tokens: (all = false) => call<{ tokens: ApiToken[]; default_days: number; max_days: number }>(`/api/tokens${all ? '?all=true' : ''}`),
  createToken: (t: { name: string; scope: 'read' | 'write'; role?: Role; expires_in_days?: number }) =>
    call<ApiToken & { token: string }>('/api/tokens', json('POST', t)),
  revokeToken: (id: string) => call<{ ok: boolean }>(`/api/tokens/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  shareLinks: (dashboard: string) =>
    call<{ links: ShareLink[]; allowed: boolean; cache_periods: number[] }>(`/api/dashboards/${encodeURIComponent(dashboard)}/share-links`),
  createShareLink: (dashboard: string, l: { cache_seconds: number; accept_uncached_risk?: boolean }) =>
    call<ShareLink>(`/api/dashboards/${encodeURIComponent(dashboard)}/share-links`, json('POST', l)),
  revokeShareLink: (id: string) => call<{ ok: boolean }>(`/api/share-links/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  tables: (connection: string) => call<{ tables: string[] }>(`/api/connections/${encodeURIComponent(connection)}/tables`),
  proposeModel: (connection: string, table: string, name: string) =>
    call<{ model: ModelDef; skipped: string[] }>(`/api/connections/${encodeURIComponent(connection)}/tables/${encodeURIComponent(table)}/model?name=${encodeURIComponent(name)}`),
  orgModels: () => call<{ models: { model: ModelDef; updated_at: string; updated_by: string | null }[] }>('/api/org-models'),
  saveOrgModel: (m: ModelDef, dryRun = false) =>
    call<{ ok: boolean }>(`/api/org-models/${encodeURIComponent(m.name)}${dryRun ? '?dry_run=1' : ''}`, json('PUT', m)),
  deleteOrgModel: (name: string) => call<{ ok: boolean }>(`/api/org-models/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  guestLinks: (dashboard: string) =>
    call<{ links: GuestLink[]; max_hours: number }>(`/api/dashboards/${encodeURIComponent(dashboard)}/guest-links`),
  createGuestLink: (dashboard: string, l: { expires_in_hours: number; password: string; role: Role; filters: GuestLink['filters'] }) =>
    call<GuestLink>(`/api/dashboards/${encodeURIComponent(dashboard)}/guest-links`, json('POST', l)),
  revokeGuestLink: (id: string) => call<{ ok: boolean }>(`/api/guest-links/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  guestDashboard: (base: string) => publicCall<GuestDashboard>(base),
  guestUnlock: async (base: string, password: string) => {
    const r = await fetch(`${base}/unlock`, json('POST', { password }))
    const body = await r.json().catch(() => ({}))
    if (!r.ok) throw new ApiError(r.status, typeof body.detail === 'string' ? body.detail : r.statusText)
  },
  publicDashboard: (org: string, token: string) =>
    publicCall<PublicDashboard>(`/api/public/${encodeURIComponent(org)}/${encodeURIComponent(token)}`),
  publicTile: (org: string, token: string, tile: string, signal?: AbortSignal) =>
    publicCall<PublicResult>(`/api/public/${encodeURIComponent(org)}/${encodeURIComponent(token)}/tiles/${encodeURIComponent(tile)}`, signal),
  deleteDashboard: (id: string) => call<{ ok: boolean }>(`/api/dashboards/${encodeURIComponent(id)}`, { method: 'DELETE' }),
}

// --------------------------------------------------------------------------- helpers

export const isTemporal = (t?: string | null) => t === 'date' || t === 'time' || t === 'datetime'

/** Parse ISO strings; naive values (no offset) are wall-clock times, read them as UTC. */
export const parseIso = (v: string) => new Date(/[zZ]|[+-]\d\d:?\d\d$/.test(v) || v.length <= 10 ? v : v + 'Z')

export const fieldName = (ref: string) => ref.split(':')[0]
export const fieldGrain = (ref: string) => ref.split(':')[1] as string | undefined

/** "sum(amount)": an ad-hoc aggregation of a column */
export const ADHOC = /^(sum|avg|min|max|count|count_distinct)\(([A-Za-z_][A-Za-z0-9_]*)\)$/
export function parseAdhoc(ref: string): { agg: Aggregation; field: string } | null {
  const m = ADHOC.exec(ref)
  return m ? { agg: m[1] as Aggregation, field: m[2] } : null
}

export function fieldLabel(model: ModelInfo | undefined, ref: string): string {
  const adhoc = parseAdhoc(ref.replace(/^-/, ''))
  if (adhoc) return `${fieldLabel(model, adhoc.field)} (${AGG_LABELS[adhoc.agg]})`
  const [name, grain] = ref.split(':')
  const f = model?.dimensions.find(d => d.name === name) ?? model?.measures.find(m => m.name === name)
  const base = f?.label || (name.charAt(0).toUpperCase() + name.slice(1)).replace(/_/g, ' ')
  return grain ? `${base} (${grain})` : base
}

/** Resolve a tile's drill path (hierarchy name or explicit list). */
export function drillPath(tile: Tile, model?: ModelInfo): string[] {
  if (!tile.drill) return []
  if (Array.isArray(tile.drill)) return tile.drill
  return model?.hierarchies[tile.drill] ?? []
}
