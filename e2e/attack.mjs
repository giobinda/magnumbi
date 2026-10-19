// The browser attack: hostile names, labels, markdown and data on every page that shows them.
// `node attack.mjs setup` makes the organisation (named with a payload); `node attack.mjs`
// fills it and opens the pages. Fails if a payload runs: window.__xss, a dialog, an inline
// handler or a javascript: link in the page.
import { chromium } from 'playwright'
import { PAYLOADS } from './payloads.mjs'

const BASE = process.env.E2E_BASE
const ORG = 'xss'
const [P_IMG, P_SCRIPT, P_SVG, P_HREF, P_TEMPLATE, P_LINK] = PAYLOADS

const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
const page = await context.newPage()
const found = []
page.on('dialog', d => { found.push(`a dialog: ${d.message()}`); d.dismiss() })

const api = async (method, path, body, org = ORG) => {
  const headers = { origin: BASE, ...(org ? { 'x-magnum-org': org } : {}) }
  const r = await page.request.fetch(`${BASE}${path}`, typeof body === 'string'
    ? { method, headers: { ...headers, 'content-type': 'text/plain' }, data: body }
    : { method, headers, ...(body ? { data: body } : {}) })
  if (!r.ok()) throw new Error(`${method} ${path}: ${r.status()} ${await r.text()}`)
  return r.json()
}
await api('POST', '/api/auth/login', { email: process.env.E2E_EMAIL, password: process.env.E2E_PASSWORD }, null)

if (process.argv[2] === 'setup') {
  await api('PUT', '/api/platform', { system_owners: [process.env.E2E_EMAIL], organisations: { [ORG]: { name: P_IMG, models: ['*'] } } }, null)
  await browser.close()
  process.exit(0)
}

// the organisation's data (DuckDB over the CSV), a dashboard named and labelled with payloads
await api('PUT', '/api/connections/warehouse', { config: { type: 'duckdb', tables: { sample_sales: 'sales.csv' } } })
const tile = (id, type, title, extra = {}) => ({ id, type, title, ...extra })
const q = { model: 'sales', dimensions: ['region'], measures: ['transactions'] }
const dashboard = {
  title: P_IMG, description: P_SCRIPT, tabs: ['Main', P_SVG],
  filters: [{ model: 'sales', field: 'region', label: P_SVG }, { model: 'sales', field: 'store', label: P_TEMPLATE }],
  tiles: [
    tile('table', 'table', P_SVG, { query: q, drill: 'location' }),
    tile('bar', 'bar', P_SCRIPT, { query: q }),
    tile('pie', 'pie', P_TEMPLATE, { query: q }),
    tile('heat', 'heatmap', P_LINK, { query: { ...q, dimensions: ['region', 'category'] } }),
    tile('pivot', 'pivot', P_IMG, { query: { ...q, dimensions: ['region', 'store'] } }),
    tile('kpi', 'kpi', P_HREF, { query: { model: 'sales', measures: ['transactions'] } }),
    tile('note', 'text', P_LINK, { text: `# ${P_IMG}\n\n${P_SCRIPT}\n\n[click me](${P_HREF}) ${P_LINK}\n\n- ${P_SVG}` }),
    tile('other', 'text', 'Other tab', { tab: P_SVG, text: P_TEMPLATE }),
  ],
}
await api('PUT', '/api/dashboards/xss/yaml', JSON.stringify(dashboard))
const share = await api('POST', '/api/dashboards/xss/share-links', { cache_seconds: 300 })
const guest = await api('POST', '/api/dashboards/xss/guest-links', { expires_in_hours: 1, password: 'Attack-Password-1', role: 'user' })

// the pages, as the system owner working in the organisation
await page.addInitScript(org => localStorage.setItem('magnum.org', org), ORG)

async function check(where) {
  await page.waitForTimeout(1500)
  // hover every chart: tooltips are HTML
  for (const canvas of await page.$$('canvas')) {
    const box = await canvas.boundingBox()
    if (!box) continue
    for (const [fx, fy] of [[0.3, 0.5], [0.5, 0.5], [0.7, 0.4]]) {
      await page.mouse.move(box.x + box.width * fx, box.y + box.height * fy)
      await page.waitForTimeout(150)
    }
  }
  const state = await page.evaluate(() => ({
    xss: window.__xss,
    handlers: [...document.querySelectorAll('*')].flatMap(e => [...e.attributes].filter(a => /^on/i.test(a.name)).map(a => `${e.tagName} ${a.name}`)),
    links: [...document.querySelectorAll('a[href]')].map(a => a.getAttribute('href')).filter(h => /^\s*javascript:/i.test(h)),
  }))
  if (state.xss) found.push(`${where}: a payload ran (${state.xss})`)
  for (const h of state.handlers) found.push(`${where}: an inline handler in the page (${h})`)
  for (const l of state.links) found.push(`${where}: a javascript: link (${l})`)
  console.log(`checked ${where}`)
}

// the detector works: a payload planted on purpose is caught
await page.goto(`${BASE}/`)
await page.evaluate(html => { document.body.insertAdjacentHTML('beforeend', html) }, P_IMG)
await check('control')
if (!found.some(f => f.startsWith('control'))) throw new Error('the detector missed a planted payload')
found.length = 0

/** The hostile text is on the page, as text: the data and labels did reach it. */
async function shown(where, ...texts) {
  const text = await page.evaluate(() => document.body.innerText)
  for (const t of texts) if (!text.includes(t)) throw new Error(`${where}: ${JSON.stringify(t)} isn't shown, so it wasn't tested`)
}

for (const path of ['/', '/dashboards/xss', `/dashboards/xss?tab=${encodeURIComponent(P_SVG)}`, '/dashboards/xss?edit', '/organisations', '/connections', '/usage']) {
  await page.goto(`${BASE}${path}`)
  await check(path)
}
// pick lists and a drill-down on the dashboard
await page.goto(`${BASE}/dashboards/xss`)
await page.waitForTimeout(1500)
for (const control of await page.$$('.filter-bar select, .filter-bar input, .filter-bar button')) {
  await control.click().catch(() => {})
  await page.waitForTimeout(300)
}
await shown('dashboard', P_IMG, P_SCRIPT, P_SVG, 'EMEA', `window.__xss='href'`)
await page.click('.react-grid-item:has(table) td', { timeout: 3000 }).catch(() => {})
await check('dashboard filters and drill-down')
// Explore with the hostile values in its pick list
await page.goto(`${BASE}/explore`)
await check('/explore')

// the links: public, and guest (after the password)
const tokenOf = url => url.split('/').pop()
await page.goto(`${BASE}/s/${ORG}/${tokenOf(share.url)}`)
await check('public link')
await shown('public link', P_SCRIPT, 'EMEA')
await page.goto(`${BASE}/g/${ORG}/${tokenOf(guest.url)}`)
await page.fill('input[type=password]', 'Attack-Password-1')
await page.keyboard.press('Enter')
await page.waitForTimeout(1500)
await check('guest link')
await shown('guest link', P_SCRIPT, 'EMEA')

await browser.close()
if (found.length) {
  console.error(`\nScripts ran or could run:\n${[...new Set(found)].join('\n')}`)
  process.exit(1)
}
console.log('\nNo payload ran on any page.')
