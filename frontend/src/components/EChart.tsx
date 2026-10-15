import { useEffect, useRef } from 'react'
import * as echarts from 'echarts'

type Props = { option: echarts.EChartsCoreOption; onClick?: (p: echarts.ECElementEvent) => void }

/** Minimal ECharts wrapper: init once, update option, follow container size. */
export default function EChart({ option, onClick }: Props) {
  const el = useRef<HTMLDivElement>(null)
  const chart = useRef<echarts.ECharts | null>(null)
  const clickRef = useRef(onClick)
  clickRef.current = onClick

  useEffect(() => {
    const c = echarts.init(el.current!, undefined, { renderer: 'canvas' })
    chart.current = c
    c.on('click', p => clickRef.current?.(p))
    const ro = new ResizeObserver(() => c.resize())
    ro.observe(el.current!)
    return () => { ro.disconnect(); c.dispose() }
  }, [])

  useEffect(() => { chart.current?.setOption(option, true) }, [option])

  return <div ref={el} style={{ width: '100%', height: '100%', cursor: onClick ? 'pointer' : undefined }} />
}
