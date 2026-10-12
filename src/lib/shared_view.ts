// What a person a session is shared with sees of the share (gap plan G4.2,
// the Watch board): the header's "Shared with you · Read" and "Shared by
// Martin · level Read · since 13:20", the state "Waiting for Martin", the
// Inbox row "Martin · Read · waiting for Martin" or "Martin · via 32bit ·
// Steer", and the read-only answer card's line. Pure: the components feed it
// `access.ts`'s level and `myGrantInfo`.
import type { GrantInfo, GrantLevel } from './access';
import type { AttentionState } from './attention';
import type { OrgDetail } from './orgs';
import { personName } from './session_scope';
import type { SessionRow } from './sessions';

/** The board's names for the three levels. */
export const LEVEL_NAMES: Record<GrantLevel, string> = {
  watch: 'Read',
  answer: 'Answer',
  drive: 'Steer',
};

/** The level an ask asks for: the next one up, or null at the top. */
export function nextLevel(level: GrantLevel): GrantLevel | null {
  if (level === 'watch') return 'answer';
  if (level === 'answer') return 'drive';
  return null;
}

/** Who shared the session: the name `my_grants` gave, else the owner's name
 *  from an org this client can read, else null. */
export function sharerName(
  row: Pick<SessionRow, 'owner_person_id'>,
  info: GrantInfo | undefined,
  orgs: readonly OrgDetail[],
): string | null {
  return info?.sharedByName ?? personName(info?.sharedBy ?? row.owner_person_id, orgs);
}

/** The session waits on a person, which for a recipient is the owner. */
export function waitsOnOwner(state: AttentionState | null): boolean {
  return state === 'action_required' || state === 'blocked';
}

/** The header's state word for a recipient: "Waiting for Martin" while the
 *  session waits on its owner, else null (the usual word stands). */
export function recipientStateLabel(state: AttentionState | null, sharer: string | null): string | null {
  return waitsOnOwner(state) ? `Waiting for ${sharer ?? 'the owner'}` : null;
}

/** The header chip: "Shared with you · Read". */
export function sharedWithYouChip(level: GrantLevel): string {
  return `Shared with you · ${LEVEL_NAMES[level]}`;
}

/** "13:20" today, else "12 Oct". */
function since(unixSecs: number, now: Date): string {
  const d = new Date(unixSecs * 1000);
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
    : d.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

/** The header's meta lead: "Shared by Martin Janči · level Read · since
 *  13:20", with "· via 32bit" for an org share. Parts the hub did not send
 *  are left out. */
export function sharedByMeta(
  level: GrantLevel,
  info: GrantInfo | undefined,
  sharer: string | null,
  now: Date = new Date(),
): string {
  const parts = [sharer ? `Shared by ${sharer}` : 'Shared with you', `level ${LEVEL_NAMES[level]}`];
  if (info?.grantedAt != null) parts.push(`since ${since(info.grantedAt, now)}`);
  if (info?.viaOrg) parts.push(`via ${info.viaOrg}`);
  return parts.join(' · ');
}

/** The Inbox's Shared with me row: "Martin · Read · waiting for Martin",
 *  "Martin · via 32bit · Steer". */
export function sharedRowLine(
  level: GrantLevel,
  info: GrantInfo | undefined,
  sharer: string | null,
  state: AttentionState | null,
): string {
  const who = sharer ?? 'Shared with you';
  const parts = [who];
  if (info?.viaOrg) parts.push(`via ${info.viaOrg}`);
  parts.push(LEVEL_NAMES[level]);
  if (waitsOnOwner(state)) parts.push(`waiting for ${sharer ?? 'the owner'}`);
  return parts.join(' · ');
}

/** The answer card's line at Read: the question is the owner's to answer. */
export function readOnlyAnswerLine(sharer: string | null): string {
  const who = sharer ? `${sharer}’s` : 'the owner’s';
  return `You can read this session. The question above is ${who} to answer; with Answer you could reply here.`;
}

/** The Ask button: "Ask Martin for Answer". */
export function askLabel(sharer: string | null, level: GrantLevel): string {
  return `Ask ${sharer ?? 'the owner'} for ${LEVEL_NAMES[level]}`;
}

/** After an ask: "Asked for Answer · waiting for Martin". */
export function askedLabel(sharer: string | null, level: GrantLevel): string {
  return `Asked for ${LEVEL_NAMES[level]} · waiting for ${sharer ?? 'the owner'}`;
}
