# Changelog

What changed in each release of Magnum BI. Versions follow the Docker image and Helm chart
(`ghcr.io/giobinda/magnum-bi`), released from tags `vX.Y.Z`.

## 0.1.6 (2026-10-22)

### Added

- **Data catalog: owners, stewards and status.** Each model has, per organisation, an owner (any
  member: ownership isn't a role), stewards, and a status (certified, draft, deprecated) with a
  note, on the new **Catalog** page. Org admins choose owners; owners decide the rest. The status
  shows in Explore's model picker, deprecated models warn on their dashboards, and agents see it
  through MCP. An organisation's own model belongs to its maker. API: `/api/governance`.
- **Documentation and a glossary.** Stewards document a model and its fields: descriptions,
  other names, examples, classification tags (information only), and the glossary terms they
  implement; the Catalog's glossary holds the organisation's business terms. Explore shows it in
  tooltips, finds fields by their other names, and marks sensitive fields; agents get it through
  MCP (`describe_model`, `get_glossary`). Platform YAML takes `synonyms` and `tags` too.
- **Catalog search and lineage.** Search finds models, fields, terms and dashboards by name,
  other words and description (agents: MCP `search_catalog`). Each model's lineage shows what it
  reads, the dashboards, tiles, reports, blends and shared formulas using it, each field's uses
  (and the fields no dashboard uses), and its queries in the last 30 days.
- **Chat.** People ask questions in plain words and get the answer with its chart, using the
  organisation's own AI provider (OpenAI-compatible APIs or Anthropic; key sealed per
  organisation). It runs MCP's tools as the person, offers no admin tools, and shows every change
  as a proposal to confirm. Org admins choose what leaves (rows per result, sensitive fields) and a
  monthly limit, and whether people start on the chat; each person can choose for themselves.
- **More for agents (MCP):** scheduled reports and alerts (`list_reports`, `create_report` with
  `dry_run`, `pause_report`, `resume_report`, `send_report_now`, `delete_report`), formulas
  (`check_formula`, `save_formula` with `dry_run`, `delete_formula`) and the catalog
  (`get_catalog`, `get_lineage`). Creating a report takes `?dry_run=1` in the API too.
- **What a field means, in Explore:** a **?** next to every field opens its catalog entry
  (description, other names, examples, tags, and its glossary terms with their definitions).

## 0.1.5 (2026-10-20)

### Added

- **Formulas.** Calculations on a model's measures and columns, written like a spreadsheet:
  `(revenue - cost) / revenue`, `sum(amount) / count_distinct(store)`, `if(…)`, `round(…)` and
  more. In Explore, **ƒ New formula** offers the fields and functions to click and checks as you
  type, showing where a mistake is. A formula is a measure like any other, period comparisons
  included. In YAML as a model's measure (`type: formula`) or a query's `formulas:`; for agents
  in MCP's `run_query`. Formulas carry no SQL: they're parsed, and only model fields, numbers
  and a few functions reach the database, written for each engine.
- **Per-row formulas:** fields of each row from its columns (`amount * qty`,
  `if(amount * qty > 500, 'large', 'small')`, `concat(region, ' / ', category)`), to group by,
  filter on or add up (`sum(line_total)`). In Explore, **ƒ New field**; in YAML a dimension with
  `formula:`, or a query formula with `per_row: true`. Formulas now take text (`'EMEA'`) and
  `lower`, `upper`, `concat`.
- **Calculations on a chart's results:** `running_sum`, `moving_avg(x, rows)`, `previous`, `rank`,
  `percent_of_total`, offered in Explore's formula editor. They run on ScramDB over the chart's
  result (which stays cached), along the first date dimension or across the others.
- **Shared formulas.** Org admins add a formula to a model for the whole organisation, from
  Explore or Models › Shared formulas (`/api/formulas`, with `?dry_run=1`). Saved in the
  organisation's own database.
- The sample model's "Revenue per unit" is now a formula.
- **The image for ARM too** (`linux/arm64`, built natively), next to x86: the same tags.
- **Docs with "In the app" and "In code" tabs** on every section that has a screen (models,
  dashboards, connections, blends, cache, embedding, links, delivery, reports, themes, tokens,
  members, sign-in), with screenshots. `make screenshots` retakes them from a demo company.

### Fixed

- An organisation couldn't save a blend joining one of its own models ("unknown model").
- The Connections page listed a blend among the connections still to set up, as an empty name.

## 0.1.4 (2026-10-20)

### Added

- **Text tiles and tabs.** A tile can be words in markdown (`type: text`): headings, notes, how
  to read the dashboard. A dashboard can have tabs (`tabs:`, and `tab:` on a tile); the address
  keeps the open tab (`?tab=…`). HTML in text stays text; only web, mail and in-app links work.
- **Downloads.** A tile's data as CSV and a chart as a PNG (the tile's ⤓ menu), the whole
  dashboard as a zip of CSVs, one folder per tab (**Download**). Same data as on screen, under the
  same restrictions. Users and up; guests with the User role; never on public links.
- **Email and webhooks, per tenant.** Each organisation, and the platform workspace, has its own
  mail server and Slack, Microsoft Teams or JSON webhooks (**Delivery**), with test buttons.
  Nothing is shared or inherited between tenants. Groundwork for scheduled reports and alerts.
- **Scheduled reports.** **Schedule…** on a dashboard sends it daily, weekly or monthly at a
  local time: the data (a zip of CSVs), a fresh guest link with a password, or a public link,
  by email and to Slack/Teams webhooks, through the tenant's own delivery settings. Recipients
  are members and domains Org admins allow; reports run as their maker and pause when the maker
  can't send them; every email can be unsubscribed; Org admins see and pause them all.
- **Alerts.** On the same Schedule… dialog: a number on the dashboard (a KPI) above or below a
  value, or rising or falling by a percentage on the comparison period, checked hourly, daily,
  weekly or monthly. People are told once when it crosses, and again only after it came back.
- **An attack suite** that misuses the API as every kind of caller, in `make test-db` and CI:
  every route in an access matrix, planted secrets, row filters on every path to data, hostile
  input, brute force, cross-site requests. `make e2e-attack` opens hostile dashboards in a real
  browser (local only).

### Security

- A read-only API token could change anything by adding `?dry_run=1` to a request (delete
  dashboards, links and connections; reset a member's password and receive it). Dry runs are now
  allowed only where the handler really checks without saving.
- Through MCP, a read-only token could run a writing tool by passing `dry_run: true` to a tool
  that ignores it (`save_dashboard`, `delete_dashboard`, `save_connection`, `delete_connection`).
- A request naming an organisation the caller can't work in ran in another one (the caller's
  own, or the platform workspace for a misspelt id). It is now refused.
- The request log at debug level held public and guest link tokens and sign-in callback codes. It
  now logs the path only, with link tokens masked.
- YAML nested very deep took up to 40 seconds of CPU to reject, so a few requests could freeze
  the server. YAML nested more than 100 levels is refused before parsing.
- Requests on a session cookie must send JSON (a browser can post `text/plain` to another site
  without asking first; the Origin checks already stopped this).
- A mail server's host must be a DNS name or an IP address (no spaces or line breaks).

### Fixed

- ScramDB 1.0 returns made-up values when grouping by a decimal (`NUMERIC`) column after the
  first query. Number dimensions are now grouped in a way it handles correctly.
- A huge number in a filter (`1e400`) reached the warehouse and failed there (502); numbers must
  now be finite and below 1e30.
- Top N accepted a made-up grain after a dimension's name.

### Removed

- The offer of a commercial license: there is none for now (NOTICE, README, website).

## 0.1.3 (2026-10-17)

- **Organisations' own models:** Org admins make models in the app from their tables (no SQL:
  columns and aggregations only), with the same data restrictions as the platform's.
- **Nine more chart types:** pivot table, heatmap, combo, gauge, trend, treemap, funnel,
  waterfall, histogram; 100% stacking; Explore suggests a chart.
- **Get started:** a checklist for platform owners and Org admins, and a starter dashboard from
  any model in one click.
- **How-to guides** on the website: infrastructure, platform owner, Org admin.

## 0.1.2 (2026-10-17)

- **Public links:** share a dashboard with anyone, served from a cache so visitors can't
  overload the data warehouse; no cache only after an explicit warning. Org admins only.
- **Guest links:** share a dashboard by link and password, for up to 14 days, at the maker's role
  or lower, with extra limits on what guests see. Power users and up.
- Pie slices take colours by value; migrations are safe on a new database.

## 0.1.1 (2026-10-16)

- Fixes from an end-to-end test of the setup; CI.
- The public website (GitHub Pages); README for everyone; called an early alpha.

## 0.1.0 (2026-10-16)

- First release: semantic models in YAML, dashboards, Explore, blends across engines,
  organisations with their own ScramDB, roles and row filters, sign-in with passwords,
  authenticator codes, passkeys and single sign-on, API tokens and MCP for agents, themes,
  embedding; the Docker image, Compose and the Helm chart.
