# Changelog

What changed in each release of Magnum BI. Versions follow the Docker image and Helm chart
(`ghcr.io/giobinda/magnum-bi`), released from tags `vX.Y.Z`.

## Unreleased

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
