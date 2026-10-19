import { describe, expect, it } from 'vitest'
import { parse, safeHref } from './markdown'

describe('markdown', () => {
  it('reads headings, paragraphs, lists and rules', () => {
    expect(parse('# Title\n\nFirst line\nsame paragraph\n\n- a\n- b\n\n1. one\n2. two\n\n---')).toEqual([
      { t: 'h', level: 1, c: [{ t: 'text', v: 'Title' }] },
      { t: 'p', c: [{ t: 'text', v: 'First line same paragraph' }] },
      { t: 'ul', items: [[{ t: 'text', v: 'a' }], [{ t: 'text', v: 'b' }]] },
      { t: 'ol', items: [[{ t: 'text', v: 'one' }], [{ t: 'text', v: 'two' }]] },
      { t: 'hr' },
    ])
  })

  it('reads bold, italics, code and links inside a line', () => {
    expect(parse('**Revenue** is *up* in `EMEA`, see [the report](https://x.io/r)')).toEqual([{
      t: 'p', c: [
        { t: 'strong', c: [{ t: 'text', v: 'Revenue' }] },
        { t: 'text', v: ' is ' },
        { t: 'em', c: [{ t: 'text', v: 'up' }] },
        { t: 'text', v: ' in ' },
        { t: 'code', v: 'EMEA' },
        { t: 'text', v: ', see ' },
        { t: 'link', href: 'https://x.io/r', c: [{ t: 'text', v: 'the report' }] },
      ],
    }])
  })

  it('keeps HTML as plain text and drops unsafe links', () => {
    expect(parse('<b>hi</b>')).toEqual([{ t: 'p', c: [{ t: 'text', v: '<b>hi</b>' }] }])
    expect(parse('[click](javascript:alert(1))')).toEqual([{ t: 'p', c: [{ t: 'text', v: 'click' }] }])
  })

  it('allows web, mail and in-app links only', () => {
    for (const ok of ['https://a.b', 'http://a.b', 'mailto:x@y.z', '/dashboards/sales', '#top']) expect(safeHref(ok)).toBe(ok)
    for (const bad of ['javascript:alert(1)', 'data:text/html,x', '//evil.com', ' JavaScript:x', 'vbscript:x']) expect(safeHref(bad)).toBeNull()
  })

  it('caps headings at three levels', () => {
    expect(parse('#### Small')).toEqual([{ t: 'h', level: 3, c: [{ t: 'text', v: 'Small' }] }])
  })
})
