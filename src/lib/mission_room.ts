// Work → Missions as a Thread room (2026-10-15; boards W1Missions and
// W2Plan, the look Control got in the Thread room of 2026-10-14):
//
// - left, the sidebar's Missions tab (WorkMissions): the list grouped
//   Needs you / Working / Draft and paused / Done this week (and a folded
//   Earlier), each row with its state, why it is there and its progress;
// - centre, the room (MissionRoom, the `missions` destination): the
//   mission's header, its task graph card, its log as a chat-like timeline
//   with the cards that need a person inline, and "Steer <mission>…",
//   which hands the text to Control's agent;
// - right, its panel: Session · Plan · Runs · Log · Repos, resizable like
//   Control's (`control_room.rightPanelWidth`), its width kept apart.
//
// This module is the state the list and the room share, and the pure
// rules they ask: how the list groups and words its rows, what the
// composer prefixes, and why it may not send.
import { get, writable } from 'svelte/store';
import { destinationFlag, goTo } from './destination';
import { readPref, writePref } from './prefs';
import { rightPanelWidth } from './control_room';
import { workState } from './handoffs';
import { outbox } from './outbox';
import { controlJournal } from './control_journal';
import { steerPrefix } from './agent_context';
import { sessionActionBlocked } from './share';
import { readErrorText } from './work_view';
import type { OfState } from './kit/status';
import type { SessionRow } from './sessions';
import {
  isFinal,
  listMissions,
  missionReason,
  openProposals,
  waitsOnPerson,
  WAIT_WORDS,
  withoutConfigKeys,
  type Mission,
  type MissionDetail,
  type MissionEvent,
} from './missions';

// ── Where the room is ──

/** The room shows in the right column (a destination, like the board). */
export const workMissionsOpen = destinationFlag('missions');

/** The mission the room shows; null shows the starter. */
export const roomMissionId = writable<number | null>(null);

/** The New mission form shows in the room's centre. */
export const roomCreating = writable<boolean>(false);

/** Open the room on `id` (null: the starter, or the list's first look). */
export function openMissionRoom(id: number | null): void {
  roomCreating.set(false);
  roomMissionId.set(id);
  goTo('missions');
}

/** "+ New" and the starter's New mission: the form, in the room. */
export function startNewMission(): void {
  roomCreating.set(true);
  goTo('missions');
}

// ── The list both columns read ──

export interface MissionListState {
  loaded: boolean;
  missions: Mission[];
  error: string | null;
  /** Review w17-7: a read has answered at least once, so `missions` is a
   *  real list and a later failure is a failed refresh (the list stays). */
  read?: boolean;
}

export const missionList = writable<MissionListState>({ loaded: false, missions: [], error: null });

let listSeq = 0;

/** Read the list again. A load overtaken by a newer one is dropped. */
export async function loadMissionList(): Promise<void> {
  const mine = ++listSeq;
  const r = await listMissions();
  if (mine !== listSeq) return;
  if (r.ok) {
    missionList.set({ loaded: true, missions: Array.isArray(r.value) ? r.value : [], error: null, read: true });
  } else {
    missionList.update((s) => ({ ...s, loaded: true, error: withoutConfigKeys(readErrorText(r.error)) }));
  }
}

// ── The right panel ──

export type MissionPanelTab = 'session' | 'plan' | 'runs' | 'log' | 'repos';
export const MISSION_PANEL_TABS: readonly { id: MissionPanelTab; label: string }[] = [
  { id: 'session', label: 'Session' },
  { id: 'plan', label: 'Plan' },
  { id: 'runs', label: 'Runs' },
  { id: 'log', label: 'Log' },
  { id: 'repos', label: 'Repos' },
];

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/** The panel shown or hidden (its edge button, or a drag to zero). */
export const missionPanelOpen = writable<boolean>(readPref('layout.missions-right-open', true, isBool));
missionPanelOpen.subscribe((v) => writePref('layout.missions-right-open', v));

const panelWidth = rightPanelWidth('layout.missions-right', () => missionPanelOpen.set(false));
/** The panel's width, px (320–640), remembered under its own key. */
export const missionRightPx = panelWidth.px;
/** One drag step on the panel's handle; a drag to zero hides it. */
export const dragMissionRight = panelWidth.drag;

export const missionPanelTab = writable<MissionPanelTab>('plan');

/** The session the Session tab shows; null: the mission's first live one. */
export const missionSessionId = writable<number | null>(null);

/** Put `tab` in front, showing the panel if it was hidden. */
export function showMissionPanel(tab: MissionPanelTab): void {
  missionPanelTab.set(tab);
  missionPanelOpen.set(true);
}

/** Open one of the mission's sessions in the Session tab, in place. */
export function openMissionSession(sessionId: number): void {
  missionSessionId.set(sessionId);
  showMissionPanel('session');
}

/** The tab a mission opens on: Session while it has live sessions and
 *  is still going (board W1), else Plan (board W2). */
export function defaultPanelTab(detail: Pick<MissionDetail, 'mission' | 'sessions'>): MissionPanelTab {
  return (detail.sessions ?? []).length > 0 && !isFinal(detail.mission.state) ? 'session' : 'plan';
}

// ── The list's groups and rows ──

export type RoomGroupKey = 'needs' | 'running' | 'paused' | 'done' | 'earlier';

export const ROOM_GROUPS: readonly { key: RoomGroupKey; label: string }[] = [
  { key: 'needs', label: 'Needs you' },
  { key: 'running', label: 'Working' },
  { key: 'paused', label: 'Draft and paused' },
  { key: 'done', label: 'Done this week' },
  { key: 'earlier', label: 'Earlier' },
];

const WEEK = 7 * 86_400;

/** Which group a mission sits in. A mission that waits on a person (its
 *  `waiting_on`, or an open card or ask step in its detail) needs you
 *  whatever its state; one that ended is Done this week or Earlier. */
export function roomGroupOf(m: Mission, detail: MissionDetail | null | undefined, now: number): RoomGroupKey {
  if (isFinal(m.state)) return (m.finished_at ?? m.updated_at) >= now - WEEK ? 'done' : 'earlier';
  if (m.waiting_on || (detail && waitsOnPerson(detail))) return 'needs';
  if (m.state === 'active') return 'running';
  return 'paused';
}

export interface RoomGroup {
  key: RoomGroupKey;
  label: string;
  missions: Mission[];
}

/** The list by group (board W1 says Running; the manual's status word is
 *  Working), each newest change first, empty groups left out. */
export function roomGroups(
  missions: readonly Mission[],
  details: ReadonlyMap<number, MissionDetail>,
  now: number,
): RoomGroup[] {
  const sorted = [...missions].sort((a, b) => b.updated_at - a.updated_at || b.id - a.id);
  return ROOM_GROUPS.map((g) => ({
    ...g,
    missions: sorted.filter((m) => roomGroupOf(m, details.get(m.id), now) === g.key),
  })).filter((g) => g.missions.length > 0);
}

/** A row's dot. */
export function missionTone(m: Mission, detail?: MissionDetail | null): OfState {
  if (!isFinal(m.state) && (m.waiting_on || (detail && waitsOnPerson(detail)))) return 'waiting';
  if (m.state === 'paused' || m.state === 'draft') return 'idle';
  return workState(m.state);
}

const money = (micros: number) => `$${(micros / 1e6).toFixed(2)}`;

/** A row's second line: why it is where it is (`missionReason`), else why
 *  it waits, else what it is: "Draft · not planned yet", "Finished · 6 tasks
 *  · $3.20". */
export function missionRowLine(m: Mission, detail?: MissionDetail | null): string {
  const why = missionReason(m, detail);
  if (why) return why;
  if (m.waiting_on) return WAIT_WORDS[m.waiting_on.reason] ?? 'Waits on you';
  const total = m.total ?? 0;
  switch (m.state) {
    case 'draft':
      return total > 1 ? `Draft · ${total} tasks` : 'Draft · not planned yet';
    case 'paused':
      return 'Paused';
    case 'active':
      return 'Working';
    case 'completed':
      return [`Finished · ${total} task${total === 1 ? '' : 's'}`, m.cost_micros ? money(m.cost_micros) : null].filter(Boolean).join(' · ');
    default:
      return m.state;
  }
}

/** A row's meter: done of total, or null with no tasks. */
export function rowProgress(m: Pick<Mission, 'done' | 'total'>): { value: number; text: string; label: string } | null {
  const total = m.total ?? 0;
  if (total <= 0) return null;
  const done = Math.min(m.done ?? 0, total);
  return { value: done / total, text: `${done}/${total}`, label: `${done} of ${total} tasks done` };
}

// ── The timeline ──

/** The log oldest first, as a chat reads (the hub sends newest first). */
export function roomEvents(detail: Pick<MissionDetail, 'events'>): MissionEvent[] {
  return [...(detail.events ?? [])].sort((a, b) => a.at - b.at || a.id - b.id);
}

/** The planner's proposals as one card: how many, over how many waves,
 *  and their titles in wave order. */
export function proposalSummary(detail: MissionDetail): { ids: number[]; waves: number; titles: string[] } {
  const ids = openProposals(detail);
  const waveOf = new Map((detail.graph?.nodes ?? []).map((n) => [n.item_id, n.wave || 1]));
  const titles = new Map((detail.items ?? []).map((i) => [i.id, i.key ? `${i.key} · ${i.title}` : i.title]));
  const ordered = [...ids].sort((a, b) => (waveOf.get(a) ?? 1) - (waveOf.get(b) ?? 1) || a - b);
  return {
    ids: ordered,
    waves: new Set(ordered.map((id) => waveOf.get(id) ?? 1)).size,
    titles: ordered.map((id) => titles.get(id) ?? `Task ${id}`),
  };
}

// ── Steer: the composer hands the text to Control ──

/** What rides in front of a steer, so Control knows which mission. */
export { steerPrefix } from './agent_context';

/** Why the composer may not send now, or null. */
export function steerBlocked(operator: SessionRow | null): string | null {
  if (!operator) return "Control's agent isn't running, so there is no one to steer through. Open Control to start it.";
  if (operator.lost_at != null) return "Control's agent was lost. Open Control to bring it back.";
  return sessionActionBlocked(operator, 'send_prompt');
}

/** Why Send waits now, or null (review w04-12): a steer goes into
 *  Control's REPL as Control's own composer would, so it waits as that
 *  composer's gate does — while Control is mid-turn, while its last message
 *  is not taken yet, and behind lines already queued in Control's composer,
 *  which go first. The box stays open; only Send waits. */
export function steerBusy(operator: SessionRow | null, outboxBusy: boolean, queuedInControl: number): string | null {
  if (queuedInControl > 0) {
    return `Control has ${queuedInControl === 1 ? 'a line' : `${queuedInControl} lines`} queued from its chat; they go first. Open Control to send them.`;
  }
  if (operator?.claude_status === 'working' || outboxBusy) return 'Control is mid-turn. Send opens when it finishes, so this is not typed into its turn.';
  return null;
}

/** One steer sent: the room's receipt line. */
export interface SteerReceipt {
  /** The outbox message it went as. */
  key: string;
  missionId: number;
  operatorId: number;
  text: string;
  /** ms. */
  at: number;
}

/** Steers sent this run, newest last; the room shows its mission's. */
export const steerReceipts = writable<SteerReceipt[]>([]);
const MAX_STEER_RECEIPTS = 20;

/**
 * Send `text` about mission `m` to Control's agent, through the one outbox
 * every composer shares (the path Control's own chat sends by), with
 * `steerPrefix` in front. Answers the receipt.
 */
export function sendSteer(m: Pick<Mission, 'id' | 'name'>, text: string, operator: SessionRow): SteerReceipt {
  const key = outbox.enqueue(
    { id: operator.id, host_alias: operator.host_alias, tmux_name: operator.tmux_name },
    { kind: 'prompt', text, prefix: steerPrefix(m) },
  );
  const receipt: SteerReceipt = { key, missionId: m.id, operatorId: operator.id, text, at: Date.now() };
  steerReceipts.update((rs) => [...rs, receipt].slice(-MAX_STEER_RECEIPTS));
  // Control follows the mission from now on (review w04-11): a `steer`
  // receipt, so the feed tells it what happens next. Best effort: the steer
  // itself went already.
  void controlJournal({ action: 'follow', thread: 'mission', id: m.id, text });
  return receipt;
}

/** Test seam: the room as a fresh run has it. */
export function resetMissionRoomForTests(): void {
  listSeq++;
  missionList.set({ loaded: false, missions: [], error: null });
  roomMissionId.set(null);
  roomCreating.set(false);
  missionPanelTab.set('plan');
  missionSessionId.set(null);
  missionPanelOpen.set(true);
  steerReceipts.set([]);
  if (get(workMissionsOpen)) workMissionsOpen.set(false);
}
