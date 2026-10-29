import type { ChartType, Column, QueryResult, Value } from './api'

/** The tile editor's chart gallery: every chart, what it's for, and a preview on made-up data
 *  (never the person's: the gallery opens before the query has run). */

const dim = (name: string, label: string, type = 'string', grain: string | null = null): Column =>
  ({ name, field: name, label, kind: 'dimension', type, grain })
const mea = (name: string, label: string, format = 'number'): Column =>
  ({ name, field: name, label, kind: 'measure', type: 'number', format, additive: true })
const result = (columns: Column[], rows: Value[][]): QueryResult => ({ columns, rows, stats: {}, sql: null })

const MONTHS = ['2026-01-01', '2026-02-01', '2026-03-01', '2026-04-01', '2026-05-01', '2026-06-01']
const REGIONS = ['North', 'South', 'East', 'West']
const month = dim('month', 'Month', 'date', 'month')
const region = dim('region', 'Region')
const revenue = mea('revenue', 'Revenue', 'currency')
const orders = mea('orders', 'Orders')

const byMonth = () => result([month, revenue], MONTHS.map((m, i) => [m, [42, 48, 45, 56, 61, 67][i] * 1000]))
const byRegion = () => result([region, revenue], REGIONS.map((r, i) => [r, [52, 38, 27, 19][i] * 1000]))

export const GALLERY: { type: ChartType; label: string; icon: string; use: string; sample: () => QueryResult }[] = [
  { type: 'table', label: 'Table', icon: '▦', use: 'Rows to read, sort and download.', sample: byRegion },
  { type: 'bar', label: 'Bar', icon: '▮', use: 'Compare categories, or follow time.', sample: byRegion },
  { type: 'hbar', label: 'Horizontal bar', icon: '▬', use: 'A ranking, with long names.', sample: byRegion },
  { type: 'line', label: 'Line', icon: '⟋', use: 'A trend over time.', sample: byMonth },
  { type: 'area', label: 'Area', icon: '◭', use: 'A trend over time, and its volume.', sample: byMonth },
  { type: 'pie', label: 'Donut', icon: '◔', use: 'Shares of a whole, for a few parts.', sample: byRegion },
  {
    type: 'scatter', label: 'Scatter', icon: '⁘', use: 'How two numbers go together.',
    sample: () => result([dim('store', 'Store'), orders, revenue],
      [['A', 120, 9000], ['B', 260, 17000], ['C', 180, 14000], ['D', 340, 25000], ['E', 90, 8000], ['F', 300, 19000]]),
  },
  { type: 'kpi', label: 'KPI', icon: '#', use: 'One big number to watch.', sample: () => result([revenue], [[318000]]) },
  { type: 'trend', label: 'KPI with trend', icon: '↗', use: 'The latest number and its recent path.', sample: byMonth },
  {
    type: 'combo', label: 'Bar + line', icon: '⫼', use: 'Two numbers of different sizes, over time.',
    sample: () => result([month, revenue, orders], MONTHS.map((m, i) => [m, [42, 48, 45, 56, 61, 67][i] * 1000, [310, 330, 300, 380, 410, 450][i]])),
  },
  {
    type: 'pivot', label: 'Pivot table', icon: '⊞', use: 'One field across, another down, with totals.',
    sample: () => result([region, dim('quarter', 'Quarter'), revenue],
      REGIONS.flatMap((r, i) => ['Q1', 'Q2'].map((q, j) => [r, q, (20 - i * 3 + j * 4) * 1000]))),
  },
  {
    type: 'heatmap', label: 'Heatmap', icon: '▩', use: 'Where the busy spots are: two fields, colour by value.',
    sample: () => result([dim('day', 'Day'), dim('hour', 'Hour'), orders],
      ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'].flatMap((d, i) => ['9', '12', '15', '18'].map((h, j) => [d, h, (i + 2) * (j + 1) * 7 % 40 + 5]))),
  },
  { type: 'treemap', label: 'Treemap', icon: '◫', use: 'Shares of a whole, with many parts.', sample: byRegion },
  {
    type: 'funnel', label: 'Funnel', icon: '⏷', use: 'Steps people go through, and who stays.',
    sample: () => result([dim('step', 'Step'), mea('people', 'People')],
      [['Visits', 1000], ['Sign-ups', 420], ['Trials', 210], ['Orders', 90]]),
  },
  {
    type: 'waterfall', label: 'Waterfall', icon: '▟', use: 'How a total builds up, step by step.',
    sample: () => result([dim('item', 'Item'), mea('amount', 'Amount', 'currency')],
      [['Sales', 50000], ['Returns', -6000], ['Costs', -21000], ['Other', 4000]]),
  },
  {
    type: 'histogram', label: 'Histogram', icon: '▂▅', use: 'How values spread: how many of each size.',
    sample: () => result([dim('amount', 'Amount', 'number'), mea('count', 'Count')],
      [12, 18, 22, 25, 25, 28, 31, 33, 36, 40, 44, 52, 58, 70, 85].map(v => [v, 1])),
  },
  { type: 'gauge', label: 'Gauge', icon: '◠', use: 'Progress towards a target.', sample: () => result([revenue], [[72000]]) },
]
