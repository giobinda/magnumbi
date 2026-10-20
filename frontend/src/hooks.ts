import { useEffect, useState } from 'react'
import { api, ApiError } from './api'
import type { ModelInfo, Query, QueryResult, TileRequest } from './api'

let modelsPromise: Promise<{ models: ModelInfo[]; errors: Record<string, string> }> | null = null

/** Models changed (a blend was saved): fetch them again next time. */
export function forgetModels() {
  modelsPromise = null
}

export function useModels() {
  const [state, setState] = useState<{ models: ModelInfo[]; errors: Record<string, string> } | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    modelsPromise ??= api.models()
    modelsPromise.then(setState, e => { modelsPromise = null; setError(String(e.message ?? e)) })
  }, [])
  // after a change to the models (e.g. a shared formula): read them again
  const reload = () => { modelsPromise = api.models(); modelsPromise.then(setState, e => setError(String(e.message ?? e))) }
  return { models: state?.models ?? [], modelErrors: state?.errors ?? {}, error, loaded: state !== null, reload }
}

/** True on a phone-sized screen (the same width as the CSS's phone rules), following resizes. */
export function usePhone() {
  const query = '(max-width: 760px)'
  const [phone, setPhone] = useState(() => matchMedia(query).matches)
  useEffect(() => {
    const m = matchMedia(query)
    const onChange = () => setPhone(m.matches)
    m.addEventListener('change', onChange)
    return () => m.removeEventListener('change', onChange)
  }, [])
  return phone
}

/** Runs a free-form query whenever its JSON changes (debounced, previous request aborted). */
export function useQuery(q: Query | null, debounceMs = 250) {
  return useLoader(q ? JSON.stringify(q) : '', (k, signal) => api.query(JSON.parse(k), signal), debounceMs)
}

/** Runs a SAVED tile with viewer inputs (filters, period window, drill clicks). */
export function useTileQuery(dashboard: string | null, tile: string, req: TileRequest, debounceMs = 250) {
  return useLoader(dashboard ? JSON.stringify([dashboard, tile, req]) : '', (k, signal) => {
    const [d, t, r] = JSON.parse(k)
    return api.tileQuery(d, t, r, signal)
  }, debounceMs)
}

function useLoader(key: string, load: (key: string, signal: AbortSignal) => Promise<QueryResult>, debounceMs: number) {''
  const [state, setState] = useState<{ data?: QueryResult; error?: string; status?: number; loading: boolean }>({ loading: false })
  useEffect(() => {
    if (!key) { setState({ loading: false }); return }
    const ctrl = new AbortController()
    setState(s => ({ ...s, loading: true, error: undefined }))
    const t = setTimeout(() => {
      load(key, ctrl.signal).then(
        data => setState({ data, loading: false }),
        e => {
          if (e.name === 'AbortError') return
          setState({ error: e.message, status: e instanceof ApiError ? e.status : undefined, loading: false })
        })
    }, debounceMs)
    return () => { clearTimeout(t); ctrl.abort() }
  }, [key, debounceMs])
  return state
}
