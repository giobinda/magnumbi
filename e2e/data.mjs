// The organisation's sales as CSV (read through DuckDB), with hostile regions, stores and
// categories next to ordinary ones.
import { PAYLOADS } from './payloads.mjs'

const csv = v => `"${String(v).replaceAll('"', '""')}"`
const values = ['EMEA', ...PAYLOADS]
const rows = ['id,sold_at,region,store,category,amount,qty']
let id = 1
for (let day = 0; day < 30; day++) {
  for (const region of values) {
    const date = new Date(Date.now() - day * 86400000).toISOString()
    const store = values[(id * 7) % values.length]
    const category = values[(id * 3) % values.length]
    rows.push([id++, date, csv(region), csv(store), csv(category), (10 + id % 90).toFixed(2), 1 + id % 4].join(','))
  }
}
console.log(rows.join('\n'))
