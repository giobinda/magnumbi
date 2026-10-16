# Working on Magnum BI (for coding agents)

Magnum BI is a multi-tenant BI tool: a Rust (axum) server with a React UI, storing its own data
in ScramDB (a PostgreSQL-wire database). People define semantic models in YAML, query them
through connections (ScramDB, PostgreSQL, BigQuery, DuckDB), and build dashboards. Read
`README.md` for the product, and `docs/org-isolation.md` before touching anything an
organisation owns.

## Rules of the house

- **Test first, keep it simple (TDD + KISS).** Write the failing test, watch it fail, then write
  the smallest code that passes. Prefer the boring design. Don't add abstractions, options or
  dependencies "for later".
- **Organisations are independent.** Follow the rules in `docs/org-isolation.md`:
  - org-owned data goes in the org's own ScramDB (`state.orgs.store(org)`);
  - every org row carries `org` and every query filters on it;
  - secrets are sealed per organisation (`SecretKey::seal(value, "<org>/<purpose>")`);
  - failures fail closed for that organisation only;
  - nothing in process memory is shared across organisations unless it can be rebuilt.
- **Permissions live in the API handlers.** Anything else that acts on data, such as MCP tools
  (`api/mcp.rs`) or the CLI, calls those handlers or store functions. It never re-implements a
  check.
- **Never weaken a security property silently.** That covers: data restrictions (row filters),
  the network guard on organisation-supplied addresses (`netguard.rs`), the read-only DuckDB
  sandbox, CSRF/origin checks, session and token checks, and write-only secrets. If a change
  needs to touch one, say so explicitly.
- **Match the surrounding code:** short doc comments that say why, plain English in
  user-facing messages ("this account signs in with …"), names over abbreviations.

## Layout

```
backend/src/       Rust server + CLI (crate magnum-bi): main.rs, api/ (HTTP handlers, mcp.rs),
                   auth/ (sessions, passwords, TOTP, passkeys, OIDC, policies), compiler.rs
                   (semantic query -> SQL, per dialect), blend.rs, connectors/, store.rs, access.rs
backend/migrations/primary/  the control-plane ScramDB   } embedded, applied at start; add a
backend/migrations/org/      each organisation's ScramDB } new numbered file, never edit an old one
frontend/src/      React + Vite + ECharts; api.ts mirrors the API types
project/           models/, blends/, dashboards/ (YAML; dashboards are seeded on first start)
devtools/          scramdb.sh (local ScramDB containers)
Dockerfile, compose.yaml      the image (UI + server + project) and a one-machine stack
deploy/helm/magnum-bi/        the Helm chart (bundled or external ScramDB; checks in secret.yaml)
```

## Commands

```bash
make db-up                      # dev ScramDB (Podman/Docker) on 127.0.0.1:55432
make dev-backend                # API on :8000 (loads .env.local); make dev-frontend: UI on :5173
make test                       # backend unit tests + frontend typecheck and Vitest
make test-db                    # also the tests that need ScramDB / DuckDB extensions
make check                      # validate project YAML (models, blends, dashboards)
make demo [ORG=x] [MONTHS=13]   # the NYC taxi x weather demo (downloads; backend/src/demo.rs)
make lint                       # cargo clippy --all-targets + oxlint
cargo fmt                       # rustfmt.toml: max_width 120
docker build -t localhost/magnum-bi . && docker compose up -d   # the packaged stack on :8000
helm lint deploy/helm/magnum-bi --set production=false          # after chart changes, also
                                # helm template … | kubeconform -strict
```

Tests that need services are skipped unless their variable is set:

- `MAGNUM_TEST_DATABASE_URL=postgresql://scramdb@127.0.0.1:55432/scramdb` for ScramDB;
- `MAGNUM_TEST_POSTGRES_URL` for a real PostgreSQL;
- `MAGNUM_TEST_DUCKDB_EXTENSIONS=$PWD/data/duckdb/_extensions` for DuckDB extensions.

Run the full suite with all three before calling something done. Clippy must be clean. The
frontend runs `npx tsc -b`, `npx vitest run`, `npx oxlint src` and `npm run build`.

## Writing tests here

- API tests use `api::tests::app()` (real router and state on the dev ScramDB) plus `call(...)`.
  Tests run **in parallel against a shared database**, so:
  - give everything you create a unique name (`uuid`), and delete it at the end;
  - never change shared settings (the `platform` setting, the `warehouse` connection) in the
    database. Pin access settings in memory instead: `state.access.pin(AccessConfig::...)`;
  - each test that writes a `members:<org>` setting uses its own organisation id;
  - a test needing an organisation with a database pins one: `state.orgs.pin(&org, state.store.clone())`
    with a unique `org` (its rows carry that id), and deletes its rows at the end.
- For external services, run a small axum stand-in on `127.0.0.1:0` inside the test, as the OIDC
  provider and BigQuery tests do. Passkeys are tested with `webauthn-authenticator-rs`'
  `SoftPasskey`.

## ScramDB 1.0 quirks (it isn't PostgreSQL)

- Some `$n` parameters are inferred as text: cast them (`$2::BIGINT`, `$1::TEXT`,
  `$3::TIMESTAMPTZ`), or inline a validated `LIMIT`.
- DDL isn't transactional: migrations must be idempotent (`IF NOT EXISTS`).
  `ALTER TABLE … ADD COLUMN IF NOT EXISTS` works.
- `CREATE TABLE schema.t AS SELECT` ignores the schema: create the table, then `INSERT`.
- `COPY … FROM STDIN` only finds tables in the default schema (blend work tables live there).
- `SUM` of an unconstrained `NUMERIC` after a join loses the scale: materialised decimals are
  `NUMERIC(38, 10)`.
- `CREATE DATABASE` doesn't isolate reliably; `COPY TO STDOUT` ignores CSV options; no
  `setseed()`.

Other engines:

- **DuckDB** (bundled) has no ICU, so `time` columns are compared as UTC `TIMESTAMP`s.
- **BigQuery** quotes identifiers with backticks and escapes strings with backslashes.

`compiler::Dialect` handles all of this. Add dialect differences there, with a test per dialect.

## Conventions worth knowing

- **Errors:** `ApiError` (`error.rs`) is JSON `{"detail": …}`. Database outages become 503.
  Errors that quote SQL are shown only to roles that may see SQL.
- **Writes that agents use** take `?dry_run=1`, and dashboards and members take
  `If-Match: <version>`. Keep both working when changing those handlers.
- **Secrets are write-only:** never return them, log them or put them in error messages.
- **New sign-in or auth features** go through `auth::policy` (methods and requirements are
  checked at sign-in and on every request).
- **Dependencies:** prefer what's already in `Cargo.lock` / `package-lock.json`. If one is
  needed, pick a release at least two weeks old and pin it in the frontend.
- **Commits:** only when asked. Use an imperative summary line, then a body listing what changed.

## Decisions already made (don't re-propose)

- No standalone desktop app.
- No LDAP / Active Directory.
- MCP is hand-rolled (`api/mcp.rs`), not the `rmcp` SDK.
- Blends aggregate per engine, then join on ScramDB (never copy raw rows).
