// The members list: who may add data. Org admins can with their role; a power user only when an
// Org admin ticks it for them; nobody below (the server decides, this only edits the list).
import type { PersonAccess, Role, RowRule } from './api'

export const canBeAllowedData = (role: Role) => role === 'power_user'

/** What's saved for a person: the flag only where it means something. */
export function accessOf(p: { role: Role; filters: RowRule[]; add_data?: boolean }): PersonAccess {
  return { role: p.role, filters: p.filters, ...(p.add_data && canBeAllowedData(p.role) ? { add_data: true } : {}) }
}
