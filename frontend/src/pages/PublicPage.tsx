import { useEffect, useState } from 'react'
import { useParams } from 'react-router-dom'
import GridLayout, { WidthProvider } from 'react-grid-layout'
import { api } from '../api'
import type { PublicDashboard, PublicResult, Tile } from '../api'
import Chart from '../components/Chart'
import { tileTitle } from '../components/TileView'
import { usePhone } from '../hooks'
import { phoneLayout } from '../phoneLayout'
import { cacheLabel } from '../share'
import { useTheme, useThemeOverride } from '../theme'

const Grid = WidthProvider(GridLayout)

/** A dashboard behind a public link (/s/<org>/<token>): read-only, no sign-in, from the
 *  results the link keeps. */
export default function PublicPage() {
  const { org = '', token = '' } = useParams()
  const [dash, setDash] = useState<PublicDashboard | null>(null)
  const [error, setError] = useState<string | null>(null)
  const phone = usePhone()
  const { adopt } = useTheme()

  useEffect(() => {
    api.publicDashboard(org, token).then(d => { if (d.theme) adopt(d.theme); setDash(d) }, e => setError(e.message))
  }, [org, token, adopt])
  useThemeOverride(dash?.theme?.id)
  useEffect(() => {  // not for search engines (the server says so too)
    const meta = Object.assign(document.createElement('meta'), { name: 'robots', content: 'noindex' })
    document.head.appendChild(meta)
    return () => meta.remove()
  }, [])
  useEffect(() => { if (dash) document.title = `${dash.title} · Magnum BI` }, [dash])

  if (error) return <div className="public-gone"><h1>This link doesn't work</h1><p className="muted">{error}</p></div>
  if (!dash) return <div className="page"><div className="empty">Loading…</div></div>

  const layout = phone ? phoneLayout(dash.tiles) : dash.tiles.map(t => ({ i: t.id, ...t.layout }))
  return (
    <div className="page public">
      <div className="public-head">
        <h1>{dash.title}</h1>
        {dash.description && <p className="muted">{dash.description}</p>}
        <p className="muted small">
          {dash.cache_seconds > 0 ? `Shared publicly · the data is refreshed at most every ${cacheLabel(dash.cache_seconds)}` : 'Shared publicly · live data'}
        </p>
      </div>
      <Grid className="grid" layout={layout} cols={12} rowHeight={64} margin={[12, 12]} isDraggable={false} isResizable={false} compactType="vertical">
        {dash.tiles.map(t => <div key={t.id}><PublicTile org={org} token={token} tile={t} /></div>)}
      </Grid>
      <p className="center muted small">Made with <a href="https://giobinda.github.io/magnumbi/" target="_blank" rel="noreferrer">Magnum BI</a></p>
    </div>
  )
}

function PublicTile({ org, token, tile }: { org: string; token: string; tile: PublicDashboard['tiles'][number] }) {
  const [data, setData] = useState<PublicResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    const ctrl = new AbortController()
    api.publicTile(org, token, tile.id, ctrl.signal).then(setData, e => { if (e.name !== 'AbortError') setError(e.message) })
    return () => ctrl.abort()
  }, [org, token, tile.id])
  const title = tileTitle({ ...tile, query: { model: '' } } as unknown as Tile, undefined, data?.columns)
  return (
    <div className="tile">
      <div className="tile-head"><div className="tile-title" title={title}>{title}</div></div>
      <div className="tile-body">
        {error ? <div className="error">{error}</div>
          : data ? <Chart result={data} type={tile.type} options={tile.options} />
          : <div className="empty">Loading…</div>}
      </div>
      {data && !['kpi', 'trend', 'gauge'].includes(tile.type) && <div className="tile-foot">as of {new Date(data.as_of).toLocaleString()}</div>}
    </div>
  )
}
