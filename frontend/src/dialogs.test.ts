import { describe, expect, it } from 'vitest'
import { answer, ask, current, subscribe } from './dialogs'

describe('dialogs in the app (not the browser\'s: a desktop window may not show those)', () => {
  it('asks to confirm, and answers', async () => {
    const seen: (string | undefined)[] = []
    const stop = subscribe(() => seen.push(current()?.message))
    const yes = ask.confirm('Delete "Sales"?', { danger: true, ok: 'Delete' })
    expect(current()).toMatchObject({ kind: 'confirm', message: 'Delete "Sales"?', danger: true, ok: 'Delete' })
    answer(true)
    expect(await yes).toBe(true)
    expect(current()).toBeNull()
    const no = ask.confirm('Discard changes?')
    answer(false)
    expect(await no).toBe(false)
    expect(seen).toEqual(['Delete "Sales"?', undefined, 'Discard changes?', undefined])
    stop()
  })

  it('asks for a name, with a starting value; cancelled is null', async () => {
    const named = ask.prompt('Dashboard title', 'Overview')
    expect(current()).toMatchObject({ kind: 'prompt', value: 'Overview' })
    answer('  Sales  ')
    expect(await named).toBe('Sales')
    const cancelled = ask.prompt('Dashboard title')
    answer(null)
    expect(await cancelled).toBeNull()
    const empty = ask.prompt('Dashboard title')
    answer('   ')
    expect(await empty).toBeNull()
  })

  it('says something, one dialog at a time, in order', async () => {
    const first = ask.alert('Save failed')
    const second = ask.confirm('Delete?')
    expect(current()?.message).toBe('Save failed')
    answer(undefined)
    await first
    expect(current()?.message).toBe('Delete?')
    answer(true)
    expect(await second).toBe(true)
    expect(current()).toBeNull()
  })
})
