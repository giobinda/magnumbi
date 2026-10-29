import { describe, expect, it } from 'vitest'
import type { Permission } from './api'
import { SHOW_ME, placeBalloon, welcomeTour } from './tour'

const targets = (perms: Permission[], chat = false) => welcomeTour(perms, chat).map(s => s.target)

describe('the welcome tour', () => {
  it('shows a viewer only what a viewer can do', () => {
    const t = targets(['view', 'filter'])
    expect(t).toContain('period')
    expect(t).toContain('filters')
    expect(t).not.toContain('tile-menu')
    expect(t).not.toContain('edit')
    expect(t).not.toContain('nav-data')
    expect(t[t.length - 1]).toBe('tour-again')
  })

  it('shows everyone the search and their settings (theme, API tokens)', () => {
    const t = targets(['view'])
    expect(t).toContain('search')
    expect(t).toContain('nav-settings')
    const settings = (perms: Permission[]) => welcomeTour(perms, false).find(s => s.target === 'nav-settings')!.text
    expect(settings(['view'])).toMatch(/theme/i)
    expect(settings(['view'])).not.toMatch(/members/i)
    expect(settings(['view', 'manage_members'])).toMatch(/members/i)
  })

  it('tells people who may drill to click, and others to hover', () => {
    const tiles = (perms: Permission[]) => welcomeTour(perms, false).find(s => s.target === 'tiles')!.text
    expect(tiles(['view', 'drill'])).toMatch(/drill/i)
    expect(tiles(['view'])).not.toMatch(/drill/i)
  })

  it('adds downloads, schedules, editing, the chat and the admin menus by permission', () => {
    const t = targets(['view', 'filter', 'drill', 'download', 'share', 'explore', 'create', 'edit', 'manage_members'], true)
    for (const x of ['tile-menu', 'download', 'schedule', 'edit', 'nav-chat', 'nav-data', 'nav-settings']) expect(t).toContain(x)
  })
})

describe('show me', () => {
  it('has a way for each step that has a control, and starts on its page', () => {
    for (const id of ['organisation', 'org_database', 'org_admin', 'connection', 'model', 'dashboard', 'members', 'theme', 'sign_in', 'delivery']) {
      expect(SHOW_ME[id]?.length, id).toBeGreaterThan(0)
      expect(SHOW_ME[id].some(s => s.path), id).toBe(true)
    }
    expect(SHOW_ME.secret_key).toBeUndefined() // a server setting: nothing to point at
  })
})

describe('placing a balloon', () => {
  const view = { width: 1200, height: 800 }
  const size = { width: 300, height: 120 }

  it('goes below the control when there is room, pointing at it', () => {
    const p = placeBalloon({ top: 100, left: 500, width: 80, height: 30 }, size, view)
    expect(p.side).toBe('below')
    expect(p.top).toBe(100 + 30 + 12)
    expect(p.arrow).toBe(540 - p.left)
  })

  it('goes above a control near the bottom', () => {
    const p = placeBalloon({ top: 740, left: 500, width: 80, height: 30 }, size, view)
    expect(p.side).toBe('above')
    expect(p.top).toBe(740 - 120 - 12)
  })

  it('stays on screen, 16px from the edges', () => {
    expect(placeBalloon({ top: 100, left: 1180, width: 20, height: 20 }, size, view).left).toBe(1200 - 300 - 16)
    expect(placeBalloon({ top: 100, left: 0, width: 20, height: 20 }, size, view).left).toBe(16)
    // narrower than the balloon (a phone): as wide as fits
    const phone = placeBalloon({ top: 100, left: 10, width: 20, height: 20 }, size, { width: 320, height: 600 })
    expect([phone.left, phone.width]).toEqual([16, 288])
  })
})
