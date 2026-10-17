# Organisation isolation: design rules

Goal: organisations can become **completely independent** later without rewrites. The primary
ScramDB is the control plane and keeps platform-level data (identity routing, organisation
registry, templates, usage metrics). Everything an organisation owns should be able to live
with that organisation, under its own keys, and eventually behind its own server ("cell").

Not everything below is built yet (see "Planned"), but **new code must follow these rules** so
the planned steps stay cheap.

## Rules for new code

1. **Org-owned data goes in the org's ScramDB**, through `state.orgs.store(org)`, never the
   primary `state.store`. Ask for each new table: whose is it? Only control-plane data
   (identity, routing, registry, templates, platform metrics) belongs in the primary.
2. **Every org-owned row carries `org`**, and every query filters on it, even in the org's own
   instance (development instances can be shared).
3. **Secrets are sealed per organisation**: `crypto::SecretKey::seal(value, org)` binds the
   organisation as associated data. New secrets (connector credentials, tokens) must be scoped
   the same way, so a per-organisation key (envelope encryption, KMS) can replace the
   platform key later.
4. **Connectors and connections are organisation-scoped.** New connectors must accept
   credentials that belong to one organisation. Don't add platform-wide credentials that read
   several organisations' data and rely on row filters to separate them.
5. **No cross-organisation reads in one request.** A request runs inside one scope
   (`Caller::org()`); code paths needing several organisations (publishing templates, platform
   usage) are system-owner actions and loop over organisations explicitly.
6. **Fail closed per organisation.** A missing or unreachable org database is a 503 for that
   organisation only (`OrgDbError`), never a fallback to the primary, and never an outage for
   others.
7. **Least privilege on org databases.** Runtime code should need only DML on its own tables;
   keep schema changes in `migrations/org/` so a separate migration role can run them.
8. **Audit what touches an organisation**: admin actions and platform access are recorded
   (today in `usage_events`; later also in the org's own audit log).
9. **Assume a separate server per organisation later**: no in-process state shared across
   organisations that couldn't be rebuilt from the primary plus that organisation's database.

## Built

- Organisation-owned data connections (connections.rs): stored in the organisation's ScramDB,
  secrets sealed per organisation and connection; models only name a connection. DuckDB runs in
  a per-query sandbox confined to the organisation's directory and object storage.

- Blends: an organisation's own blends are kept in its ScramDB; every blend query joins its parts
  on the organisation's own ScramDB, each part run with that model's data restrictions (a
  blend never has restrictions of its own, so it can't widen access).

- Sign-in policies per organisation (within the platform's), and the organisation's own OpenID
  Connect provider, which can only vouch for the organisation's own domains. Its client secret is
  sealed for "<org>/oidc".

- Public links: kept in the organisation's ScramDB with the results they show (the token sealed
  for "<org>/share" and hashed); the address names the organisation, so a visit never needs
  the primary to find it. Visitors act as the link's maker, a Viewer at most, with their row
  filters.

- Guest links: kept in the organisation's ScramDB (token sealed for "<org>/guest" and hashed,
  password hashed); guests act as the link's maker with the link's role or lower, the maker's row
  filters plus the link's own; their unlock cookie is sealed for that organisation and link.

- Themes: an organisation's own themes and default themes are kept in its ScramDB and only join
  that organisation's view.

## Planned (not built yet)

- Organisation-specific models (today models are platform YAML shared by all organisations).
- A data key per organisation (envelope encryption, KMS-ready); crypto-shredding on offboarding.
- Separate migration and runtime roles on org instances; refuse `--pg-no-auth`, non-TLS and
  shared instances in production.
- Platform access to an organisation by time-limited, approved, audited grants.
- An audit log in each org DB; export and offboarding.
- Roles, row filters and member lists in the org DB, with only an email → organisation index
  in the primary (sign-in settings, today with the members in the primary, would move along).
- Cells: a Magnum BI server per organisation (or per pool), the primary as control plane.

ScramDB notes (1.0): `SUM` of an unconstrained `NUMERIC` after a join loses its scale
(materialized columns are `NUMERIC(38, 10)`); `COPY ... FROM STDIN` only finds tables in the
default schema (blend work tables live there); roles and GRANT/REVOKE are enforced; the default container image runs
with `--pg-no-auth` (no authentication at all); TLS and encryption at rest are configured in
`scramdb-config.toml` (still to verify).
