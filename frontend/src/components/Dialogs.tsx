import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { answer, current, subscribe } from '../dialogs'
import type { Dialog } from '../dialogs'

/** The app's questions and messages (dialogs.ts), one at a time. Mounted once, in App. */
export default function Dialogs() {
  const d = useSyncExternalStore(subscribe, current)
  return d ? <Box key={d.id} d={d} /> : null
}

function Box({ d }: { d: Dialog }) {
  const [text, setText] = useState(d.value ?? '')
  const box = useRef<HTMLInputElement>(null)
  const ok = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (d.kind === 'prompt') box.current?.select()
    else ok.current?.focus()
  }, [d.kind])

  const cancel = () => answer(d.kind === 'confirm' ? false : null)
  const go = () => answer(d.kind === 'prompt' ? text : d.kind === 'confirm' ? true : undefined)
  return (
    <div className="modal-bg dialog-bg" onMouseDown={e => { if (e.target === e.currentTarget) cancel() }}
      onKeyDown={e => { if (e.key === 'Escape') cancel() }}>
      <div className="modal dialog" role={d.kind === 'alert' ? 'alertdialog' : 'dialog'} aria-modal="true">
        <p className="dialog-message">{d.message}</p>
        {d.kind === 'prompt' && (
          <input ref={box} value={text} onChange={e => setText(e.target.value)} onKeyDown={e => { if (e.key === 'Enter') go() }} />
        )}
        <div className="row dialog-buttons">
          {d.kind !== 'alert' && <button onClick={cancel}>Cancel</button>}
          <button ref={ok} className={d.danger ? 'danger primary' : 'primary'} onClick={go}>{d.ok ?? 'OK'}</button>
        </div>
      </div>
    </div>
  )
}
