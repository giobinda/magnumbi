// Questions and messages in the app's own dialogs (components/Dialogs.tsx shows them), not the
// browser's confirm/prompt/alert: the desktop window (WKWebView on macOS) shows none of those,
// so a confirmation there was a silent "no".

export type Dialog = {
  /** each its own: a new dialog starts afresh */
  id?: number
  kind: 'confirm' | 'prompt' | 'alert'
  message: string
  /** a starting value (prompt) */
  value?: string
  /** the button that goes ahead (default: OK) */
  ok?: string
  /** going ahead deletes or removes something */
  danger?: boolean
}

type Waiting = Dialog & { resolve: (answer: unknown) => void }

const queue: Waiting[] = []
let made = 0
const listeners = new Set<() => void>()
const changed = () => listeners.forEach(l => l())

function open<T>(d: Dialog): Promise<T> {
  return new Promise<T>(resolve => {
    queue.push({ ...d, id: ++made, resolve: resolve as (answer: unknown) => void })
    if (queue.length === 1) changed()
  })
}

export const ask = {
  confirm: (message: string, o: { ok?: string; danger?: boolean } = {}) => open<boolean>({ kind: 'confirm', message, ...o }),
  /** the text given, trimmed; null when cancelled or empty */
  prompt: (message: string, value = '') =>
    open<string | null>({ kind: 'prompt', message, value }).then(v => v?.trim() || null),
  alert: (message: string) => open<void>({ kind: 'alert', message }),
}

/** The dialog to show now (one at a time, in order). */
export function current(): Dialog | null {
  return queue[0] ?? null
}

/** Answer the dialog shown: true/false (confirm), the text or null (prompt), anything (alert). */
export function answer(value: boolean | string | null | undefined) {
  const d = queue.shift()
  d?.resolve(d.kind === 'confirm' ? Boolean(value) : value ?? null)
  changed()
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}
