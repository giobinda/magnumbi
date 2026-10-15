# Magnum BI

A lightweight BI tool: semantic models and dashboards as YAML, queries on
[ScramDB](https://scramdb.com), a drag-and-drop explorer, drill-down, period comparison, and
one-line embedding. The backend is Rust (axum + tokio-postgres); the UI is React.

## Quick start

Prerequisites: Rust 1.88+, Node 20+, and Podman or Docker (for the local ScramDB).

```bash
make install                                  # frontend dependencies
make db-up                                    # the primary ScramDB on 127.0.0.1:55432 (container)
make seed                                     # the `sample_sales` demo table (≈46k rows)
echo 'MAGNUM_SYSTEM_OWNERS=you@example.com' > .env.local
make key                                      # MAGNUM_SECRET_KEY, for organisation databases
make user EMAIL=you@example.com               # choose a password
make dev-backend                              # API on :8000 (migrates ScramDB on start)
make dev-frontend                             # UI on :5173, in another terminal
```

Open http://localhost:5173 and sign in. The **Sales overview** dashboard runs on the sample data.
To try organisations: create one on **Organisations** (e.g. `acme`), then give it its own
ScramDB with `make org-db ORG=acme PORT=55433` (or **Database › Set…** on that page).
For one process serving the API and the built UI on :8000, run `make serve`. Run `make test`
for the backend tests and the frontend typecheck, and `make check` to validate the project files.

## Project layout

```
project/                    your content (git it)
  connections.yaml          named connections (${ENV_VARS} allowed, never secrets in the file)
  models/*.yaml             semantic models: dimensions, measures, drill hierarchies
  dashboards/*.yaml         dashboards seeded into ScramDB on first start (file stem = id)
  themes/*.yaml             optional extra themes
  access.yaml               optional: initial organisations/members, adopted on first start
backend/                    Rust: the API server and CLI (crate `magnum-bi`)
  migrations/primary/       schema of the primary ScramDB } embedded in the binary,
  migrations/org/           schema of each organisation's  } applied automatically
  src/
    api/                    HTTP handlers (auth, models & queries, dashboards, admin, usage)
    auth/                   sessions, password sign-in (Argon2id), Google sign-in (OIDC)
    connectors/             where queries run: the Connector trait, ScramDB
    access.rs               organisations, roles, row filters
    compiler.rs             semantic query → SQL
    orgdb.rs                each organisation's own ScramDB (routing, registration)
    cache.rs, materialize.rs  the query cache as typed tables (the future federation bridge)
    usage.rs                usage metrics
    crypto.rs               sealing organisations' connection strings (AES-256-GCM)
    project.rs, schema.rs   loading and validating the YAML files
    store.rs, db.rs         tables and migrations
frontend/                   React + Vite + ECharts + react-grid-layout
devtools/scramdb.sh         local ScramDB containers: the primary and one per organisation
```

## Architecture

```mermaid
flowchart TB
  UI["Browser: React UI"]
  subgraph MB["Magnum BI: one binary, one URL"]
    MW["Hardening: CSRF origin check · body limits · security headers"]
    AUTH["Sessions: password (Argon2id) · Google (OIDC)"]
    ACC["Access: organisations · roles · row filters"]
    SEM["Semantic layer: YAML models → SQL"]
    CON["Connectors"]
    MW --> AUTH --> ACC --> SEM --> CON
  end
  PRIMARY[("Primary ScramDB<br/>users · sessions · platform settings · organisations & members<br/>platform templates · usage metrics")]
  ORGA[("ScramDB of org A<br/>dashboards + history · query cache")]
  ORGB[("ScramDB of org B<br/>dashboards + history · query cache")]
  DATA[("Data sources<br/>ScramDB today; more connectors later")]
  UI -- "same origin · HttpOnly session cookie" --> MW
  AUTH <--> PRIMARY
  ACC <--> PRIMARY
  SEM <-- "cached results" --> ORGA
  SEM <--> ORGB
  CON -- "read-only transactions" --> DATA
```

- **One service, one URL.** The server serves the built UI and `/api`, so the browser only ever
  talks to one origin. Cookies, CSRF checks and the CSP rely on that.
- **A primary ScramDB** (`MAGNUM_DATABASE_URL`) is the configuration and system database:
  identity (users, sessions), platform settings, organisations with their members, roles and row
  filters, platform templates, and the usage metrics of the whole platform.
- **One ScramDB instance per organisation** holds what is only that organisation's: its
  dashboards (with every saved version) and its query cache. System owners register each
  instance on the Organisations page; the connection string is tested, the instance migrated,
  the organisation's existing dashboards moved there, and the string stored sealed with
  `MAGNUM_SECRET_KEY` (AES-256-GCM, bound to the organisation). It fails closed: an organisation
  without an instance can't open dashboards, and an unreachable instance only affects its own
  organisation (503), never sign-in or other organisations. Members stay in the primary database
  because sign-in must find a person's organisation before it can open that organisation's
  database. (ScramDB doesn't reliably isolate several databases on one instance, so this is one
  *instance* per organisation; for development, several organisations may share one, even the
  primary: rows carry the organisation id.)
- **Stateless server.** All state is in ScramDB, so replicas are interchangeable; each replica
  re-reads the organisation registry every few seconds. Migrations run at startup
  (`MAGNUM_DB_AUTO_MIGRATE=0` to turn off; `magnum-bi migrate` for the primary alone;
  organisation databases migrate when first used). ScramDB DDL isn't transactional, so every
  migration statement is idempotent.
- **Data.** Queries are compiled from the models. Only declared fields can be referenced and every
  value is an escaped literal; SQL inside models (`sql:`) is trusted configuration. Queries run in
  `READ ONLY` transactions with a statement timeout. By default the `scramdb` connection reads
  the primary database (where `make seed` puts the sample); set `MAGNUM_DATA_DSN` to read
  another ScramDB.

## Query cache (and the federation bridge)

A model opts in with `cache: 15m` (any duration up to 30 days; off by default, so live data
stays live). Results are cached in the organisation's own ScramDB, keyed by organisation +
connection + compiled SQL. The SQL already contains the person's row filters and the
(5-minute-rounded) period window, so people share a cached result only when they'd get the
same rows anyway. Builders can bypass it (`x-magnum-refresh: 1`); Org admins see and clear it
on **Usage**. Up to 2,000 results per instance, least recently used dropped first.

Each cached result is a **typed table** (`magnum_cache.r_…`, indexed by
`magnum_cache.entries`), not a JSON blob. That's on purpose: it is the same mechanism a
federated query will use. To combine sources (say BigQuery and Postgres), each source's result is
materialised into the organisation's ScramDB through its connector, then joined there with plain
SQL. Connectors therefore report neutral column types (`materialize::ColType`) along with rows.

## Usage metrics

Sign-ins, queries (source, model, rows, time, cache hit, failures), dashboard views and saves,
and admin actions are recorded in the primary database (`usage_events`). Recording never slows
or fails a request: events are buffered and written in batches. They are kept
`MAGNUM_USAGE_RETENTION_DAYS` (default 400). **Usage** shows Org admins their organisation and
system owners the whole platform (or one organisation, from inside it).

## Sign-in

Two methods, both ending in the same server-side session (an opaque token in an HttpOnly cookie;
ScramDB stores only its SHA-256):

- **Password** (on by default; `MAGNUM_PASSWORD_LOGIN=0` turns it off). Argon2id hashes, at
  least 10 characters, lockout for 15 minutes after 5 failures. Accounts get a password from:
  - the server: `magnum-bi user set-password you@example.com` (system owners, recovery);
  - an Org admin: **Members › One-time password…** creates a one-time password to hand over. The
    person must choose their own at the next sign-in;
  - the person themselves: **Password** in the top bar.
- **Google** (optional): set `MAGNUM_GOOGLE_CLIENT_ID` / `MAGNUM_GOOGLE_CLIENT_SECRET` (see
  `.env.local.example`). PKCE, a browser-bound state, and an ID token verified against Google's
  keys; only a Google-verified email is accepted, and the email is bound to that Google account.

An account whose password was issued by an Org admin is **bound to that organisation**. With
that password it can never act as a system owner or in another organisation, because the admin
could have created it for an email they don't own. Signing in with Google, or a password set by
a system owner or on the server, isn't bound.

Disable an account with `magnum-bi user disable <email>`; this also ends its sessions.

## Organisations and roles

| role         | can                                                                   |
|--------------|-----------------------------------------------------------------------|
| Viewer       | view dashboards, use their filters and the period picker              |
| User         | + drill down, embed                                                   |
| Power user   | + create / edit / delete dashboards, Explore, see SQL and YAML        |
| Org admin    | + manage the organisation's members, roles and personal filters       |
| System owner | everything, in every organisation (switch with the organisation picker) |

- Each organisation has its own members, dashboards and models, and none of them are visible
  across organisations.
- People join as listed members, or automatically by email domain with a default role (never an
  admin role).
- **Row filters** only ever narrow what someone sees. Organisation filters are set by system
  owners and personal filters by Org admins. A filter also covers every model reading the same
  data (`extends` family, or the same connection and table). Anything misconfigured fails closed.
- Before the first save on **Organisations**, the system owners are `MAGNUM_SYSTEM_OWNERS`.
- Anyone can preview a lower role ("view as"), never a higher one.
- Dashboards in the platform workspace are templates: **Publish…** copies one into organisations.

## Models and dashboards

```yaml
# project/models/sales.yaml
name: sales
connection: scramdb
table: sample_sales            # or sql: SELECT ...
dimensions:
  - {name: sold_at, type: time}           # string | number | boolean | date | time | datetime
  - {name: region}
  - {name: store, aggregation: count_distinct}
measures:
  - {name: revenue, type: sum, sql: amount, format: currency}
  - {name: transactions, type: count}
  - {name: revenue_per_unit, type: number, sql: SUM(amount) / NULLIF(SUM(qty), 0)}
hierarchies:
  location: [region, store]    # drill path
```

A model can `extends:` another and add fields. Dashboards (see
`project/dashboards/sales_overview.yaml`) declare tiles with a query each, dashboard filters, and
an optional dashboard period with comparison (`previous_period`, `previous_year`, or
`{from: <date>}`). Queries support time grains (`sold_at:month`), ad-hoc aggregations of any
column (`sum(amount)`, `count_distinct(store)`), filters, Top/Bottom N and sorting by change
(`-revenue__change`).

Viewers never send queries: they ask for a saved tile with values for the dashboard's own
filters, a period window and drill clicks, and the server rebuilds the query from the saved tile.

## Embedding

```html
<script src="https://bi.example.com/embed.js"></script>
<magnum-dashboard dashboard="sales_overview" filters="category=Footwear"></magnum-dashboard>
```

Or an iframe on `/embed/<id>`. Allowed host pages are set with `MAGNUM_FRAME_ANCESTORS`. For
embeds on other sites, use `MAGNUM_COOKIE_SAMESITE=none` over https.

## Configuration

| variable | default | |
|---|---|---|
| `MAGNUM_DATABASE_URL` | `postgresql://scramdb@127.0.0.1:5432/scramdb` | the primary ScramDB |
| `MAGNUM_SECRET_KEY` | | seals organisations' connection strings (`magnum-bi gen-key`); required in production |
| `MAGNUM_USAGE_RETENTION_DAYS` | 400 | how long usage events are kept |
| `MAGNUM_DATA_DSN` | (own database) | data for the `scramdb` connection (`connections.yaml`) |
| `MAGNUM_SYSTEM_OWNERS` | | comma-separated emails, until organisations are first saved |
| `MAGNUM_PUBLIC_URL` | | public https address (cookies `Secure`, OAuth callbacks, origin checks) |
| `MAGNUM_ENV` | development | `production`/`staging` refuse to start with unsafe settings |
| `MAGNUM_PASSWORD_LOGIN` | 1 | password sign-in on/off |
| `MAGNUM_GOOGLE_CLIENT_ID` / `_SECRET` / `_HD` | | Google sign-in |
| `MAGNUM_SESSION_HOURS` | 12 | session lifetime |
| `MAGNUM_COOKIE_SAMESITE` | lax | `none` for cross-site embeds (https only) |
| `MAGNUM_TRUSTED_ORIGINS` | | extra origins allowed to send POST/PUT/DELETE |
| `MAGNUM_FRAME_ANCESTORS` | `'self'` | CSP frame-ancestors for embeds |
| `MAGNUM_HOST` / `MAGNUM_PORT` | 127.0.0.1 / 8000 | listen address |
| `MAGNUM_PROJECT_DIR` / `MAGNUM_FRONTEND_DIST` | `project` / `frontend/dist` | paths |
| `MAGNUM_DB_AUTO_MIGRATE` | 1 | migrate ScramDB at startup |
| `MAGNUM_LOG` | `magnum_bi=info` | log filter |

## Security

- Every rule is enforced by the server; the UI only hides what a role can't use.
- CSRF: state-changing requests must come from Magnum BI's own origin (Origin / Sec-Fetch-Site),
  which covers sibling subdomains too. That's on top of `SameSite=Lax` HttpOnly cookies.
- Headers: a strict CSP, `nosniff`, a referrer policy, HSTS on https, and `no-store` on the API.
- Request bodies are limited to 1 MB; YAML files to 512 KB, with alias expansion capped.
- Query errors show database details only to roles that may see SQL. Everyone else gets a
  reference to look up in the server log.

## Roadmap

Everything new follows the [organisation isolation rules](docs/org-isolation.md), so organisations
can become fully independent (own keys, credentials, servers) without rewrites.


Next, in rough order:

- **Connectors** to other databases. The `Connector` trait (`backend/src/connectors/`), a
  `Connection` variant (`schema.rs`) and, where SQL differs, a `Dialect` (`compiler.rs`).
- **Federated queries** across connections, joined in the organisation's ScramDB (on top of the
  cache's materialisation).
- Formulas (a calculated-field language), shared per organisation (from the earlier Python version, like the next two).
- Share links / QR codes, guest links for people without an account.
- Configuration as code (`plan` / `apply` of organisations from git), automation tokens.
- ThoughtSpot liveboard import (from the earlier Python version).
- TLS to ScramDB (the connection is plain TCP today, so keep ScramDB on a private network).
