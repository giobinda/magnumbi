import type { Permission } from './api'

/** One balloon of a tour: next to the control marked `data-tour="<target>"`, after going to
 *  `path` if it's given. A step whose control isn't on screen (the role can't use it, the page
 *  has none) is skipped. */
export interface TourStep { target: string; title: string; text: string; path?: string }

/** The first-time tour of a dashboard, for what this person's role can do. */
export function welcomeTour(perms: Permission[], chat: boolean): TourStep[] {
  const can = (p: Permission) => perms.includes(p)
  const steps: TourStep[] = [
    { target: 'period', title: 'The period', text: 'Pick the days to look at. The numbers are compared with the period before.' },
    { target: 'filters', title: 'Filters', text: 'Narrow every chart on the dashboard at once, for example to one region.' },
    can('drill')
      ? { target: 'tiles', title: 'Click to drill down', text: 'Click a bar or a row to drill down, for example from a region to its stores.' }
      : { target: 'tiles', title: 'The charts', text: 'Each chart answers one question. Point at a bar or a line for its exact number.' },
  ]
  if (can('download')) {
    steps.push(
      { target: 'tile-menu', title: 'A chart’s data', text: 'Its ⤓ menu downloads the data as CSV (it opens in Excel) or the chart as a picture.' },
      { target: 'download', title: 'The whole dashboard', text: 'Every chart’s data at once, as a zip.' },
    )
  }
  if (can('share')) steps.push({ target: 'schedule', title: 'By email', text: 'Get this dashboard every week, or an alert when a number crosses a line.' })
  if (can('edit')) steps.push({ target: 'edit', title: 'Make it yours', text: 'Move and resize the charts, add new ones, notes and tabs. Every saved version is kept.' })
  steps.push({
    target: 'nav-discover', title: 'Discover',
    text: can('explore')
      ? 'Every dashboard, Explore to build a chart by dragging fields, and the Catalog, which says what each number means.'
      : 'Every dashboard you can open, and the Catalog, which says what each number means.',
  })
  if (chat) steps.push({ target: 'nav-chat', title: 'Chat', text: 'Ask a question in plain words. The answer comes with the chart it’s based on.' })
  if (can('manage_members')) {
    steps.push(
      { target: 'nav-data', title: 'Data', text: 'Where your data comes from (Connections), what it means (Models), and data joined across sources (Blends).' },
      { target: 'nav-settings', title: 'Settings', text: 'Members, your colours, email and webhooks, usage, and your setup as code.' },
    )
  }
  steps.push({ target: 'tour-again', title: 'That’s it', text: 'Take this tour again any time from here.' })
  return steps
}

/** "Show me" on the Get started steps: where each one is done (steps about server settings
 *  have no control to point at). */
export const SHOW_ME: Record<string, TourStep[]> = {
  organisation: [{ path: '/organisations', target: 'add-org', title: 'A new organisation', text: 'Give it an id (lowercase, permanent, e.g. acme) and a name, then Save at the bottom of the page.' }],
  org_database: [{ path: '/organisations', target: 'org-database', title: 'Its own database', text: 'Set… takes the address of the ScramDB that will hold its dashboards and cache. It’s tested before it’s saved.' }],
  org_admin: [{ path: '/organisations', target: 'org-open', title: 'Its Org admin', text: 'Open the organisation, then add its admin on Members with the Org admin role.' }],
  connection: [
    { target: 'nav-data', title: 'Data', text: 'Connections, under Data, is where your data comes from.' },
    { path: '/connections', target: 'add-connection', title: 'Connect your data', text: 'Choose the type, fill in where it is and its password or key, Test it, then Save. Name your main one warehouse.' },
  ],
  model: [{ path: '/models', target: 'new-model', title: 'Describe your data', text: 'Pick a connection and a table: Magnum BI proposes the model from its columns, for you to adjust and save.' }],
  dashboard: [{ path: '/models', target: 'starter', title: 'A first dashboard', text: 'Starter dashboard makes one from a model in one click: its main numbers, their trend and the top categories.' }],
  members: [{ path: '/members', target: 'add-person', title: 'Your team', text: 'Add each person’s email and role (and, if needed, what data they may see), then Save.' }],
  theme: [{ path: '/themes', target: 'theme-start', title: 'Your colours', text: 'Start from any theme, change its colours and font, then make it your organisation’s default.' }],
  sign_in: [{ path: '/members', target: 'sign-in', title: 'Sign-in rules', text: 'Choose how your people sign in, require authenticator codes, or connect your company’s single sign-on.' }],
  delivery: [{ path: '/delivery', target: 'mail', title: 'Email and webhooks', text: 'Your mail server (Send me a test checks it), and Slack or Teams webhooks, for reports and alerts.' }],
}

interface Rect { top: number; left: number; width: number; height: number }

const GAP = 12, GUTTER = 16

/** Where a balloon of `size` goes next to a control at `rect`: below it when there's room,
 *  else above, inside the screen; `arrow` is where it points, from the balloon's left edge. */
export function placeBalloon(rect: Rect, size: { width: number; height: number }, view: { width: number; height: number }) {
  const width = Math.min(size.width, view.width - 2 * GUTTER)
  const below = rect.top + rect.height + GAP + size.height <= view.height - GUTTER || rect.top - GAP - size.height < GUTTER
  const top = below ? rect.top + rect.height + GAP : rect.top - GAP - size.height
  const centre = rect.left + rect.width / 2
  const left = Math.max(GUTTER, Math.min(centre - width / 2, view.width - GUTTER - width))
  return { top, left, width, side: below ? 'below' as const : 'above' as const, arrow: Math.max(12, Math.min(centre - left, width - 12)) }
}
