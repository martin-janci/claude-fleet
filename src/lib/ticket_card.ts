/**
 * The ticket context card (work graph M9.2). Types mirror
 * `crates/fleet-core/src/service/work/card.rs`; every field a newer hub may
 * add is optional. The card comes from the hub's tracker cache (no fetch).
 *
 * `composer_text` is built — and its tracker text fenced as untrusted — by
 * the hub. It is inserted verbatim: this side never assembles text for an
 * agent from tracker fields, so the fence has one implementation.
 */

import { invokeCmd, type Result } from './result';
import { hasNoPane, type SessionRow } from './sessions';

export interface TicketCard {
  key: string;
  title?: string;
  url?: string | null;
  status_name?: string | null;
  status_category?: string | null;
  org_id?: number | null;
  /** A tracker item is cached for the key. */
  cached?: boolean;
  /** Plain text, for a person's screen. */
  acceptance?: string[];
  /** The description's start, when it names no criteria. */
  excerpt?: string | null;
  /** What "Insert into composer" inserts (fenced by the hub). */
  composer_text: string;
}

export function loadTicketCard(key: string): Promise<Result<TicketCard>> {
  return invokeCmd<TicketCard>('work_ticket_card', { args: { key } });
}

/** Whether a session has a composer to insert into: a pane, and a Claude
 *  conversation for the Conversation view to show. */
export function canInsertInto(s: Pick<SessionRow, 'kind' | 'claude_session_id'>): boolean {
  return !hasNoPane(s) && !!s.claude_session_id;
}
