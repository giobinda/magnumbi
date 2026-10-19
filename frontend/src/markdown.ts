/** The small piece of markdown text tiles use: headings, paragraphs, lists, rules, bold, italics,
 *  code and links. Parsed to plain data that React renders as elements, so HTML in the text stays
 *  text and only web, mail and in-app links become links. */

export type Inline =
  | { t: 'text'; v: string }
  | { t: 'code'; v: string }
  | { t: 'strong' | 'em'; c: Inline[] }
  | { t: 'link'; href: string; c: Inline[] }
export type Block =
  | { t: 'h'; level: 1 | 2 | 3; c: Inline[] }
  | { t: 'p'; c: Inline[] }
  | { t: 'ul' | 'ol'; items: Inline[][] }
  | { t: 'hr' }

/** A link target that can't run script: http(s), mailto, or a path in this app. */
export function safeHref(href: string): string | null {
  return /^(https?:|mailto:)/i.test(href) || /^\/(?!\/)/.test(href) || href.startsWith('#') ? href : null
}

// earliest match wins; at the same place, the first alternative (** before *)
const INLINE = /`([^`]+)`|\*\*(.+?)\*\*|\[([^\]]+)\]\(((?:[^()\s]|\([^()\s]*\))+)\)|\*([^*]+)\*|_([^_]+)_/

function inline(s: string): Inline[] {
  const out: Inline[] = []
  let rest = s
  for (let m = INLINE.exec(rest); m; m = INLINE.exec(rest)) {
    if (m.index > 0) out.push({ t: 'text', v: rest.slice(0, m.index) })
    const [, code, strong, label, href, em, em2] = m
    if (code !== undefined) out.push({ t: 'code', v: code })
    else if (strong !== undefined) out.push({ t: 'strong', c: inline(strong) })
    else if (label !== undefined) {
      const safe = safeHref(href)
      if (safe) out.push({ t: 'link', href: safe, c: inline(label) })
      else out.push(...inline(label))
    } else out.push({ t: 'em', c: inline(em ?? em2) })
    rest = rest.slice(m.index + m[0].length)
  }
  if (rest) out.push({ t: 'text', v: rest })
  return out
}

export function parse(md: string): Block[] {
  const blocks: Block[] = []
  let para: string[] = []
  let list: { t: 'ul' | 'ol'; items: Inline[][] } | null = null
  const end = () => {
    if (para.length) blocks.push({ t: 'p', c: inline(para.join(' ')) })
    if (list) blocks.push(list)
    para = []; list = null
  }
  const item = (t: 'ul' | 'ol', text: string) => {
    if (list?.t !== t) { end(); list = { t, items: [] } }
    list.items.push(inline(text))
  }
  for (const raw of md.split('\n')) {
    const line = raw.trim()
    let m: RegExpMatchArray | null
    if (!line) end()
    else if ((m = line.match(/^(#{1,6})\s+(.*)$/))) { end(); blocks.push({ t: 'h', level: Math.min(m[1].length, 3) as 1 | 2 | 3, c: inline(m[2]) }) }
    else if (/^(-{3,}|\*{3,})$/.test(line)) { end(); blocks.push({ t: 'hr' }) }
    else if ((m = line.match(/^[-*]\s+(.*)$/))) item('ul', m[1])
    else if ((m = line.match(/^\d+[.)]\s+(.*)$/))) item('ol', m[1])
    else { if (list) end(); para.push(line) }
  }
  end()
  return blocks
}
