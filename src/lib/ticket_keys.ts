// Fleet's own ticket keys (`crates/fleet-core/src/store/ticket_keys.rs`):
// every project, GitHub repository and Asana tracker numbers its tickets
// under a short prefix (`CF-12`). Settings → Ticket keys lists them and
// renames a prefix — the hub's master-only `work_admin { ticket_prefixes |
// set_ticket_prefix }`, so both commands are `LocalOnly` on a paired desktop.
import { invokeCmd, type Result } from './result';

/** One row of `ticket_prefixes`: a ticket space, or a project with none yet. */
export interface TicketPrefixRow {
  /** Absent for a project whose prefix is only suggested. */
  space_id?: number | null;
  project_id?: number | null;
  /** `project:<id>` | `github:<owner/repo>` | `tracker:<id>`. */
  scope: string;
  label: string;
  prefix: string;
  /** Nothing has drawn on the prefix yet; saving it reserves it. */
  suggested?: boolean;
  tickets?: number;
  /** Prefixes a rename replaced; their keys still resolve. */
  former_prefixes?: string[];
}

/** The shape the backend accepts (`valid_ticket_prefix`), after upper-casing. */
export const TICKET_PREFIX_RE = /^[A-Z][A-Z0-9_]{1,9}$/;

/** What a person typed, as the prefix it would be; `null` when it is not one. */
export function normalizePrefix(raw: string): string | null {
  const p = raw.trim().toUpperCase();
  return TICKET_PREFIX_RE.test(p) ? p : null;
}

export function ticketPrefixes(): Promise<Result<TicketPrefixRow[]>> {
  return invokeCmd<TicketPrefixRow[]>('ticket_prefixes');
}

/** Set a row's prefix: by its space, else (a suggested row) by its project. */
export function setTicketPrefix(row: TicketPrefixRow, prefix: string): Promise<Result<TicketPrefixRow[]>> {
  const target = row.space_id != null ? { space_id: row.space_id } : { project_id: row.project_id };
  return invokeCmd<TicketPrefixRow[]>('set_ticket_prefix', { args: { ...target, prefix } });
}

/** What a row's key looks like, for the "e.g." hint: `CF-12`, `PAY-1`. */
export function exampleKey(row: TicketPrefixRow, prefix = row.prefix): string {
  return `${prefix}-${Math.max(1, row.tickets ?? 0)}`;
}
