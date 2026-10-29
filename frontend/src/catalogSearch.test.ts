import { describe, expect, it } from 'vitest'
import { chatTarget, exploreFor, isSearchShortcut, resultTarget } from './catalogSearch'

const r = (kind: 'model' | 'field' | 'term' | 'dashboard', name: string, model: string | null = null) => ({ kind, name, model, label: name, text: '' })

describe('the search in the top bar', () => {
  it('takes each result where it is told about', () => {
    expect(resultTarget(r('dashboard', 'nyc taxi'))).toBe('/dashboards/nyc%20taxi')
    expect(resultTarget(r('model', 'sales'))).toBe('/catalog?model=sales')
    expect(resultTarget(r('field', 'revenue', 'sales'))).toBe('/catalog?model=sales&field=revenue')
    expect(resultTarget(r('term', 'Net revenue'))).toBe('/catalog?term=Net%20revenue#glossary')
  })

  it("offers to explore a model's data, for those who explore", () => {
    expect(exploreFor(r('field', 'revenue', 'sales'))).toBe('/explore?model=sales')
    expect(exploreFor(r('model', 'sales'))).toBe('/explore?model=sales')
    expect(exploreFor(r('term', 'Churn'))).toBeNull()
    expect(exploreFor(r('dashboard', 'd'))).toBeNull()
  })

  it('opens with / or Ctrl+K (Cmd+K), but not while typing elsewhere', () => {
    const key = (key: string, extra: Partial<{ ctrlKey: boolean; metaKey: boolean }> = {}, typing = false) =>
      isSearchShortcut({ key, ctrlKey: false, metaKey: false, ...extra }, typing)
    expect(key('/')).toBe(true)
    expect(key('k', { ctrlKey: true })).toBe(true)
    expect(key('K', { metaKey: true })).toBe(true)
    expect(key('k')).toBe(false)
    expect(key('/', {}, true)).toBe(false)
    expect(key('k', { ctrlKey: true }, true)).toBe(true)
  })

  it('asks the chat the question as typed', () => {
    expect(chatTarget('  revenue by region last month? ')).toBe('/chat?ask=revenue%20by%20region%20last%20month%3F')
  })
})
