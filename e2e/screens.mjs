// The website's screenshots: `node screens.mjs organisation` makes ACME; `node screens.mjs`
// furnishes it (as its system owner, then its Org admin) and photographs each screen the docs
// show, light theme, into website/assets/screens/. Run through screens.sh (make screenshots).
import { chromium } from 'playwright'
import { mkdirSync } from 'fs'

const BASE = process.env.SCREENS_BASE
const OUT = new URL('../website/assets/screens/', import.meta.url).pathname
const ORG = 'acme'
mkdirSync(OUT, { recursive: true })

const browser = await chromium.launch()
async function session(email, org) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 820 }, colorScheme: 'light', deviceScaleFactor: 1 })
  const page = await context.newPage()
  page.on('dialog', d => d.accept())
  const r = await page.request.post(`${BASE}/api/auth/login`, { data: { email, password: process.env.SCREENS_PASSWORD }, headers: { origin: BASE } })
  if (!r.ok()) throw new Error(`signing in ${email}: ${r.status()} ${await r.text()}`)
  const api = async (method, path, data) => {
    const headers = { origin: BASE, ...(org ? { 'x-magnum-org': org } : {}) }
    const res = await page.request.fetch(`${BASE}${path}`, typeof data === 'string'
      ? { method, headers: { ...headers, 'content-type': 'text/plain' }, data }
      : { method, headers, ...(data ? { data } : {}) })
    if (!res.ok()) throw new Error(`${method} ${path}: ${res.status()} ${await res.text()}`)
    return res.json()
  }
  return { page, api }
}

const owner = await session(process.env.SCREENS_OWNER, null)
if (process.argv[2] === 'organisation') {
  await owner.api('PUT', '/api/platform', {
    system_owners: [process.env.SCREENS_OWNER],
    organisations: { [ORG]: { name: 'ACME', domains: ['acme.example'], models: ['sales'] } },
  })
  await browser.close()
  process.exit(0)
}

// as the platform: ACME's people, and the sales template copied in
const inAcme = (await session(process.env.SCREENS_OWNER, ORG)).api
await inAcme('PUT', '/api/members', {
  default_role: 'viewer',
  users: {
    [process.env.SCREENS_ADMIN]: 'org_admin', 'ben@acme.example': 'power_user', 'chloe@acme.example': 'user',
    'dev@acme.example': { role: 'viewer', filters: [{ model: 'sales', dimension: 'region', values: ['EMEA'] }] },
  },
})
await owner.api('POST', '/api/platform/dashboards/sales_overview/publish', { orgs: [ORG] })

// as ACME's Org admin: its warehouse, delivery, a report, an alert, links, a token
const { page, api } = await session(process.env.SCREENS_ADMIN, null)
await api('PUT', '/api/connections/warehouse', {
  config: { type: 'scramdb', host: '127.0.0.1', port: 55493, database: 'scramdb', user: 'scramdb', schema: 'public', tls: false },
})
// a model of its own (no SQL), and a blend of it with the platform's sales model
await api('PUT', '/api/org-models/stores', { name: 'stores', label: 'Stores', connection: 'warehouse', table: 'sample_sales',
  dimensions: [{ name: 'store', label: 'Store' }, { name: 'region', label: 'Region' }],
  measures: [{ name: 'stores', label: 'Stores', type: 'count_distinct', sql: 'store' }] })
await api('PUT', '/api/blends/sales_by_store', { name: 'sales_by_store', label: 'Sales × stores', base: 'sales',
  joins: [{ model: 'stores', on: ['store = store'] }] })
await api('PUT', '/api/delivery/mail', { host: 'smtp.acme.example', port: 587, security: 'starttls', username: 'reports',
  password: 'not-a-real-password', from: 'ACME reports <reports@acme.example>', allowed_domains: ['partner.example'] })
await api('PUT', '/api/delivery/webhooks/sales_team', { kind: 'slack', url: 'https://hooks.slack.com/services/T000/B000/xxxx' })
await api('PUT', '/api/delivery/webhooks/ops', { kind: 'teams', url: 'https://acme.webhook.office.com/webhookb2/xxxx' })
await api('POST', '/api/dashboards/sales_overview/schedules', {
  name: 'Monday numbers', format: 'csv', recipients: [process.env.SCREENS_ADMIN, 'ben@acme.example'], webhooks: ['sales_team'],
  when: { every: 'weekly', day: 1, at: '08:00', timezone: 'Europe/Rome' },
})
await api('POST', '/api/dashboards/sales_overview/schedules', {
  name: 'Revenue watch', format: 'alert', recipients: [process.env.SCREENS_ADMIN], webhooks: ['ops'],
  when: { every: 'hourly', at: '00:05', timezone: 'Europe/Rome' }, alert: { tile: 'revenue', op: 'falls_by', value: 10 },
})
await api('POST', '/api/dashboards/sales_overview/share-links', { cache_seconds: 3600 })
await api('POST', '/api/dashboards/sales_overview/guest-links', { expires_in_hours: 168, password: 'Partner-Preview-1', role: 'viewer',
  filters: [{ model: 'sales', dimension: 'region', values: ['EMEA'] }] })
await api('POST', '/api/tokens', { name: 'Claude Code', scope: 'read' })
await api('PUT', '/api/governance/sales', { owner: 'ben@acme.example', stewards: ['chloe@acme.example'], status: 'certified',
  note: 'Reconciled with the finance close every month.' })
await api('PUT', '/api/glossary/Net%20revenue', { name: 'Net revenue', definition: 'What customers paid after discounts and returns, before tax.',
  synonyms: ['net sales'] })
await api('PUT', '/api/glossary/Store', { name: 'Store', definition: 'A shop of ours with its own till, counted once however many sales it makes.' })
await api('PUT', '/api/governance/sales/docs', { description: 'Every sale at our tills over the last 18 months, after returns.',
  tags: ['finance'], fields: {
    revenue: { description: 'What customers paid, after discounts.', synonyms: ['takings'], examples: ['1,234.50'], terms: ['Net revenue'] },
    store: { description: 'The shop where the sale happened.', tags: ['location'], terms: ['Store'] },
    amount: { tags: ['confidential'] },
  } })
await api('PUT', '/api/governance/stores', { owner: process.env.SCREENS_ADMIN, stewards: [], status: 'draft',
  note: 'New: store attributes are being checked.' })
await api('PUT', '/api/governance/sales_by_store', { owner: 'ben@acme.example', stewards: [], status: 'deprecated',
  note: 'Use Stores directly: it has the same fields.' })
await api('PUT', '/api/formulas/sales/revenue_per_store', { name: 'revenue_per_store', label: 'Revenue per store',
  expr: 'revenue / count_distinct(store)', format: 'currency' })

/** Open a page, wait for it to settle, photograph it (or one element of it). */
async function shot(name, path, { el, before, height } = {}) {
  if (path) await page.goto(`${BASE}${path}`)
  await page.waitForLoadState('networkidle')
  await page.waitForTimeout(1200)
  if (before) { await before(); await page.waitForTimeout(900) }
  const file = `${OUT}${name}.jpg`
  const opts = { path: file, type: 'jpeg', quality: 82 }
  // the app's header stays at the top of the window: not over a tall element being photographed
  if (el) await page.addStyleTag({ content: '.topnav { position: relative !important; }' })
  if (el) await page.locator(el).first().screenshot(opts)
  else await page.screenshot({ ...opts, clip: { x: 0, y: 0, width: 1280, height: height ?? 820 } })
  console.log('photographed', name)
}

await shot('dashboard', '/dashboards/sales_overview')
await shot('dashboard-edit', '/dashboards/sales_overview?edit', { height: 640 })
await shot('explore', '/explore', {
  before: async () => {
    for (const f of ['Region', 'Revenue']) await page.dblclick(`.field-item:has(.flabel:text-is("${f}"))`)
  },
})
await shot('formula', '/explore', {
  el: '.modal',
  before: async () => {
    for (const f of ['Region', 'Revenue']) await page.dblclick(`.field-item:has(.flabel:text-is("${f}"))`)
    await page.click('button:has-text("ƒ New formula")')
    await page.fill('.formula-name', 'Revenue per sale')
    await page.selectOption('.formula-editor select', 'currency')
    await page.fill('.formula-editor textarea', 'round(revenue / transactions, 2)')
    await page.waitForSelector('.formula-check.fine')
  },
})
await shot('explore-formula', null, {
  before: async () => {
    await page.click('.formula-editor button.primary')
    await page.waitForSelector('.chip.formula')
  },
})
await shot('formula-row', '/explore', {
  el: '.modal',
  before: async () => {
    await page.click('button:has-text("ƒ New field")')
    await page.fill('.formula-name', 'Order size')
    await page.fill('.formula-editor textarea', "if(amount * qty > 500, 'large', 'small')")
    await page.waitForSelector('.formula-check.fine')
  },
})
await shot('explore-row', null, {
  before: async () => {
    await page.click('.formula-editor button.primary')
    await page.waitForSelector('.chip.dimension:has-text("Order size")')
    await page.dblclick('.field-item:has(.flabel:text-is("Revenue"))')
    await page.click('.chart-types button:has-text("Donut")')
  },
})
await shot('formula-results', '/explore', {
  el: '.modal',
  before: async () => {
    for (const f of ['Sale time', 'Revenue']) await page.dblclick(`.field-item:has(.flabel:text-is("${f}"))`)
    await page.click('button:has-text("ƒ New formula")')
    await page.fill('.formula-name', 'Revenue so far')
    await page.selectOption('.formula-editor select', 'currency')
    await page.fill('.formula-editor textarea', 'running_sum(revenue)')
    await page.waitForSelector('.formula-check.fine')
  },
})
await shot('explore-results', null, {
  before: async () => {
    await page.click('.formula-editor button.primary')
    await page.waitForSelector('.chip.formula')
    await page.click('.chart-types button:has-text("Line")')
  },
})
await shot('models', '/models', {
  el: '.page',
  before: async () => {
    await page.click('button:has-text("+ New model")')
    await page.selectOption('.model-wizard select', 'warehouse')
    await page.click('button.link:has-text("sample_sales")')
  },
})
await shot('connections', '/connections', { el: '.page', before: async () => page.click('text=Edit') })
await shot('blends', '/blends', { el: '.page', before: async () => page.click('tr:has-text("sales_by_store") button:has-text("Edit")') })
const dialog = (button) => async () => { await page.click(`button:has-text("${button}")`) }
await shot('embed', '/dashboards/sales_overview', { el: '.modal', before: dialog('Embed') })
await shot('public-link', '/dashboards/sales_overview', { el: '.modal', before: dialog('Share publicly') })
await shot('guest-link', '/dashboards/sales_overview', { el: '.modal', before: dialog('Share with guests') })
await shot('reports', '/dashboards/sales_overview', { el: '.modal', before: dialog('Schedule…') })
await shot('delivery', '/delivery', { el: '.page' })
await shot('themes', '/themes', { height: 820 })
await shot('catalog', '/catalog', {
  el: '.page',
  before: async () => { await page.click('.card.governance:has-text("Store sales") button:has-text("Edit")') },
})
await shot('catalog-docs', '/catalog', {
  el: '.card.governance:has-text("Store sales")',
  before: async () => { await page.click('.card.governance:has-text("Store sales") button:has-text("Documentation")') },
})
await shot('glossary', '/catalog', { el: '.glossary' })
await shot('catalog-search', '/catalog', {
  el: '.catalog-search',
  before: async () => { await page.fill('.catalog-search input', 'takings'); await page.waitForSelector('.search-results li') },
})
await shot('lineage', '/catalog', {
  el: '.card.governance:has-text("Store sales")',
  before: async () => {
    await page.click('.card.governance:has-text("Store sales") button:has-text("Lineage & usage")')
    await page.waitForSelector('.lineage table')
  },
})
await shot('members', '/members', { height: 820 })
await shot('sign-in', '/members', { el: '.sign-in-settings' })
await shot('usage', '/usage', { height: 820 })
await shot('security', '/', { el: '.security-card', before: async () => page.click('button:has-text("Security")') })
await browser.close()
