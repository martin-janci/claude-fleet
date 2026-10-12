// Asking for a wider share level, and answering such an ask (gap plan G4.2,
// the Watch board: "Ask Martin for Answer").
//
// The recipient's side is `askForAccess`: a note to the owner that confers
// nothing. The owner's side is the list of open asks on their sessions
// (`accessRequests`), read at startup and again whenever a `grant:changed`
// frame arrives that is not about this person's own grants, and the two
// answers: Grant (the hub revokes the asker's share and shares again at the
// asked level, in one step) and Decline.
//
// The hub decides who may ask and who may answer, per request; nothing here
// is the rule.
import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { applyGrantChanges, myPersonId, type GrantChanged, type GrantLevel } from './access';
import { push } from './toasts';
import { sessions } from './sessions';
import { shareSheetFor } from './share';
import { LEVEL_NAMES } from './shared_view';

/** One open ask on a session this person owns, as the hub answers it. */
export interface AccessRequest {
  id: number;
  session_id: number;
  session_name?: string | null;
  person_id: number;
  person_name?: string | null;
  person_display_name?: string | null;
  /** The level asked for. */
  level: string;
  requested_at: number;
  resolution?: string | null;
}

/** The owner's open asks, oldest first. Empty until the first read, and on a
 *  hub that predates the feature. */
export const accessRequests = writable<readonly AccessRequest[]>([]);

/** The asker, in words. */
export function askerName(r: Pick<AccessRequest, 'person_display_name' | 'person_name' | 'person_id'>): string {
  return r.person_display_name || r.person_name || `person ${r.person_id}`;
}

let loadSeq = 0;
/** Re-read the owner's open asks. A failure keeps the list it had. */
export async function loadAccessRequests(): Promise<Result<AccessRequest[]>> {
  const seq = ++loadSeq;
  const r = await invokeCmd<AccessRequest[]>('access_requests', { args: { action: 'list' } });
  if (seq === loadSeq && r.ok && Array.isArray(r.value)) accessRequests.set(r.value);
  return r;
}

/** Ask the owner of a session shared with you for `level`. */
export async function askForAccess(sessionId: number, level: GrantLevel): Promise<Result<AccessRequest>> {
  return invokeCmd<AccessRequest>('session_ask_access', { args: { session_id: sessionId, level } });
}

/** The owner grants or declines ask `id`; the list is re-read either way. */
export async function answerAccessRequest(id: number, grant: boolean): Promise<Result<AccessRequest>> {
  const r = await invokeCmd<AccessRequest>('access_requests', {
    args: { action: grant ? 'grant' : 'decline', id },
  });
  void loadAccessRequests();
  return r;
}

let refreshTimer: ReturnType<typeof setTimeout> | null = null;
let announce = false;

/**
 * The app's `grant:changed` handler: the recipient's own grant set first
 * (`applyGrantChanges`), then the owner's side. A frame about someone else's
 * grant reaches this client only when it owns the session (the hub fences it
 * to the two people a share is between), so it re-reads the open asks; when a
 * frame opened an ask, each ask the re-read finds new gets a toast whose
 * button opens the Share sheet, where it is answered.
 */
export function onGrantFrames(changes: readonly GrantChanged[]): void {
  applyGrantChanges(changes);
  const me = get(myPersonId);
  const others = changes.filter((c) => c.person_id !== me);
  if (others.length === 0) return;
  if (others.some((c) => c.request !== undefined)) announce = true;
  if (refreshTimer !== null) return;
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    const before = new Set(get(accessRequests).map((r) => r.id));
    const toast = announce;
    announce = false;
    void loadAccessRequests().then((r) => {
      if (!toast || !r.ok || !Array.isArray(r.value)) return;
      for (const a of r.value) if (!before.has(a.id)) announceAsk(a);
    });
  }, 300);
}

function announceAsk(a: AccessRequest): void {
  const row = get(sessions).find((s) => s.id === a.session_id);
  const name = row?.friendly_name || row?.tmux_name || a.session_name || 'a session of yours';
  const level = LEVEL_NAMES[a.level as GrantLevel] ?? a.level;
  push({
    kind: 'info',
    message: `${askerName(a)} asks for ${level} on ${name}`,
    sub: 'Grant or decline it in the Share sheet.',
    action: { label: 'Review', run: () => shareSheetFor.set(a.session_id) },
  });
}
