# Changelog

What changed in each release of Magnum BI. Versions follow the Docker image and Helm chart
(`ghcr.io/giobinda/magnum-bi`), released from tags `vX.Y.Z`.

## 0.1.9 (2026-10-28)

### Added

- **Spreadsheets, CSV files and Google Sheets as data** (Data › Connections, desktop and server):
  **+ Add a file** takes an Excel workbook (.xlsx, .xls), an OpenDocument one (.ods) or a CSV,
  copied into the workspace, a table per sheet (Replace puts in a new version); **+ Add a Google
  Sheet** takes a sheet shared with anyone with the link, a table per tab, read again every hour or
  with Refresh. Either is a DuckDB connection, so the model wizard proposes a model from its columns
  (dates, numbers and text recognised). Up to `MAGNUM_UPLOAD_MAX_MB` (100 MB; 2 GB in lite). As
  code: `POST /api/connections/{name}/file|sheet|refresh`.
- **The chat fills the data catalog**: ask it to document a model and it looks at the model and a
  few of its values, then proposes descriptions, other names, example values and tags for it and its
  fields, and glossary terms, made once confirmed (MCP tools `save_docs` and `save_term`).
- **The first start shows its download**: lite downloads its PostgreSQL (about 37 MB) with a
  progress bar in the desktop window ("Downloading PostgreSQL: 14 of 37 MB", then "Setting up its
  database…") and a line that keeps up in the terminal. The archive is checked against its
  published SHA-256, and extracted aside first, so a cut-off download never looks installed.
- **Connect your AI agent, in Get started and under API tokens**: pick Claude Code, Claude Desktop,
  Codex or Cursor and copy its MCP settings, with this app's address filled in (and the API token,
  where one is needed: on a server, or in lite with a PIN). The step ticks once an agent has used
  a tool; it can be skipped.

### Changed

- **Field lists show whole names**, grouped under their model in a blend ("NYC taxi trips",
  then "Pickup time"...), and the list can be widened by dragging its edge.
- **Split by** in the tile editor: one measure as a series per value of a dimension (trips by
  borough, a line each); it says why when the chart has more than one measure.
- **Edit one tile from the dashboard**: its ✎ (for those who may edit) opens the tile editor;
  Save saves the dashboard with it, without the dashboard's Edit mode.
- **Quieter dashboards** (from a tester's first look): a tile's rows and timings are in its title's
  tooltip (under it while editing); the comparison dates are on hover, not in the filter bar;
  legends sit under the chart and name each measure once (a year earlier is its dashed twin), or
  where the tile says (Legend: above, right, none).
- **A KPI with nothing in its comparison period says so**, instead of "– vs 0" or "– vs –".
- **Get started, back anytime**: a link in the top bar opens the checklist, hidden or done.
  A step already done keeps its **Show me**, to see the way there again.
- **Magnum light and Magnum dark are the default themes** (a tester found Aloha heavy); Aloha
  stays a choice away.

### Security

- **Uploads are the one route past the 1 MB request limit**, up to their own limit; a workbook's
  unpacked size is capped before it's read (a small file can unpack to gigabytes), and a Google Sheet
  is fetched only from Google, by its id, never from an address given.
- **Lite's folder is its owner's only.** The PostgreSQL library wrote the database's password to
  `.pgpass` readable by everyone, in a folder others could enter: another account on the same
  computer could have opened lite's database. Lite now makes its folder `0700` and `.pgpass`
  `0600` on every start, so folders made before (or with `--dir`) are fixed too.

### Fixed

- Magnum light, Paper and Slate: three chart colours (green, amber, pink) were under 3:1 on
  white; they're a shade deeper now (same hue), as the default theme must be readable.
- The chat's instructions to the AI provider had stray `\n` and `\` characters in them (a doubled
  escape): plain text now.
- **Lite starts offline.** It asked GitHub for the newest PostgreSQL 16 on every start, so it
  couldn't start without a network (or once GitHub's limit for unsigned requests was reached); it
  uses the one installed now, and asks only the first time.
- **After the sample data is set up, its models show at once** (Models, Explore, starter
  dashboards): the app kept the list of models it had read before, until a reload.
- The website's Terminal command for opening the macOS app is `xattr -d` (`-dr` failed).

## 0.1.8 (2026-10-28)

### Changed

- **Get started offers one month of sample data first** (about 60 MB, quick to set up); more
  months, for the comparisons with a year earlier, are one choice away. Settings › Organisations ›
  Demo data still offers 13 months first.
- **Get started ticks steps as they're done**: when the sample data is set up (its step and the
  connection, model and dashboard it made turn green at once, and the list stays open on its
  "Open" link), and when the window comes back from another tab.
- **Downloading the macOS app on the website says how to open it**: it isn't signed by Apple, so a
  note explains macOS's warning (*Privacy & Security › Open Anyway*, or `xattr` in Terminal).

## 0.1.7 (2026-10-28)

### Added

- **Magnum BI lite** (`magnum-bi lite`, `make lite`): for one person on their own computer, with no
  container. It starts its own PostgreSQL in `~/.magnum-bi` (downloaded the first time), has no
  organisations, accounts or sign-in, and answers only on this computer (`127.0.0.1`, requests
  addressed to `localhost` only). The query cache works in its one workspace; an optional PIN
  (`--set-pin`) locks it. Its port is picked the first time and kept. The online version keeps
  running on ScramDB.
- **The desktop window** (`magnum-bi desktop`, `make desktop`, built with `--features desktop`):
  lite in a window of its own, through the system's web engine (WebView2, WebKit, WebKitGTK). It
  shows the local app only (other links open in the browser), puts downloads in Downloads, and
  stops lite and its PostgreSQL when it closes. The server's build never includes it.
- **Sample data in Get started**: the checklist on the home page offers the NYC taxi × weather demo
  (*Try it with sample data*, optional) and sets it up right there, with its progress: for system
  owners in the platform workspace, and first in lite, where it can be skipped (*Skip*: it leaves
  the list for good on that computer).
- **Downloads for lite**: README and website (a "Magnum BI lite" section on the landing page, the
  desktop app in the docs) link to the latest release's app for macOS, Windows and Linux. The
  packages are named without the version (`magnum-bi-macos.dmg`, `magnum-bi-windows-x64.zip`,
  `magnum-bi-linux-amd64.deb`, `magnum-bi-linux-x86_64.rpm`) so those links stay the same.
- **The desktop app, packaged** (`.github/workflows/desktop.yml`, `packaging/`): each release gets a
  `.dmg` for macOS (both chips), a `.zip` for Windows and a `.deb` / `.rpm` for Linux, unsigned, with
  their `SHA256SUMS`. The UI and OpenSSL are built into the program; opened with no command, it
  shows the window. Opening it twice shows where the first one is.
- **Setup as code.** Org admins export what their organisation set up in the app (connections,
  its own models, shared formulas, blends, themes, glossary, catalog entries, dashboards, the mail
  server, webhooks, chat settings, scheduled reports and alerts) as YAML files, see what applying
  files would change, and apply them (members, roles, row filters and sign-in rules too):
  `GET /api/code`, `POST /api/code` with `?dry_run=1` for the
  plan and `?prune=1` to delete what the files don't have. Applying goes through the app's own
  checks; with any problem nothing changes. Secrets never go in the
  files: new ones travel apart, in the request, and are never sent back.
  In the app: **Code** downloads the files as a zip and applies a folder after showing the plan.
  From a terminal or CI: `magnum-bi code export|plan|apply <folder>` with an API token
  (`MAGNUM_URL`, `MAGNUM_TOKEN`), secrets in `MAGNUM_SECRET_<FILE>` variables.
- Connections, the mail server, webhooks and the sign-in rules take `?dry_run=1`.

### Changed

- Building needs **Rust 1.94** (lite's embedded PostgreSQL): this also fixes the container image,
  which still built with 1.89.
- **Blends are checked against the organisation as the person sees it**, so a blend may join a
  model saved in the same apply.

- **The chat's dashboard proposals say what you'd get:** whether the dashboard is new or changed,
  its title and each tile ("Revenue by month (line chart)"); once confirmed, a link opens it.

### Security

- The desktop window hands only `http`/`https` links to the system, and Windows opens them without
  `cmd` (which would read a `&` in an address as another command).
- **A saved secret no longer follows a changed address.** Changing a connection's host or user,
  the mail server's host or user name, the chat's provider address, or the single
  sign-on issuer or client now needs the password, key or client secret again, instead of
  sending the saved one to the new address.

### Fixed

- Lite: a PostgreSQL left running in its folder (the program killed, or crashed) made the next
  start fail ("lock file postmaster.pid already exists"); it is stopped first now.
- Lite: the NYC demo failed at "Creating connections and dashboards" (lite's project folder had
  none of its models): its models, blend and dashboards are built into the program now.

- **Stability under load.** The cache no longer holds a database connection while asking for
  another (a pool of requests each waiting for a second one could stop the server); scheduled
  reports, the cache clean-up and the usage clean-up wait a full interval after a slow round
  instead of catching up in a burst; a table another server dropped at the same moment no longer
  stops a clean-up; time, number and true/false values are typed for ScramDB everywhere.
- **Applying files that stops partway says what was applied and what wasn't** (`applied`,
  `not_applied`), so it's clear what to fix before applying again.
- **The query cache no longer grows without end.** Expired results were dropped only now and
  then when a new one was cached, so a quiet organisation kept them on disk. The server now drops
  them every 5 minutes, and keeps each organisation within `MAGNUM_CACHE_MAX_ROWS` rows of cached
  results (default 5 million), least recently used going first.

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
