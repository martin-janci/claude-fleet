import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { ProposalLike } from './ai_proposal';
import { outbox } from './outbox';
import { sessions } from './sessions';
import { decideMissionCard, getMission } from './missions';

/**
 * Jev K2, redesign step 9.9: where a message just sent in Control goes.
 * The backend (`service::decide::control_route`, the hub's `control_route`
 * tool on a hub-backed desktop) answers one receipt per message:
 *
 * - `proposed`: a mission or session, pre-selected, "Proposed by Jev".
 * - `ask`: the message was too short or unclear to route; Control asks.
 * - `none`: show nothing (feature off, shadow, a slash command, or the
 *   message is for Control itself).
 *
 * The message has already reached Control's agent: a receipt never moves
 * or holds it. "Change" records the person's pick, which is what the use
 * case learns from. Gap plan G3.9: a receipt about a session can hand the
 * message on — "Send to <session>" puts the same text in that session's
 * outbox, on the person's press, never on the proposal alone; the receipt
 * then reads "Sent to session …". G7.8: a mission takes the message as the
 * answer to its open question (its oldest open `ask` card, through
 * `decide_mission_card`), and the receipt then reads "Sent to mission …";
 * a mission with no open question has nothing to hand text to, so its
 * receipt says so and opens it.
 */

export type TargetKind = 'mission' | 'session';

export interface RouteTarget {
  kind: TargetKind;
  id: number;
  name: string;
}

export interface ControlRoute {
  outcome: 'proposed' | 'ask' | 'none';
  /** The proposed target's option word (`m<id>` / `s<id>`). */
  target?: string;
  proposal?: ProposalLike & { run_id?: number | null };
  targets: RouteTarget[];
  run_id?: number | null;
}

/** The option word for "the message is for Control itself". */
export const CONTROL = 'control';

export function optionOf(t: Pick<RouteTarget, 'kind' | 'id'>): string {
  return `${t.kind === 'mission' ? 'm' : 's'}${t.id}`;
}

export function targetOf(route: ControlRoute, option: string | null | undefined): RouteTarget | null {
  return route.targets.find((t) => optionOf(t) === option) ?? null;
}

export function proposeRoute(text: string): Promise<Result<ControlRoute>> {
  return invokeCmd<ControlRoute>('control_route_propose', { text });
}

export function followRoute(runId: number, chosen: string): Promise<Result<boolean>> {
  return invokeCmd<boolean>('control_route_follow', { runId, chosen });
}

/** One sent message's receipt, and what the person made of it. */
export interface Receipt {
  /** The outbox message it belongs to. */
  key: string;
  route: ControlRoute;
  /** The option the person picked, overriding the proposal. */
  chosen: string | null;
  /** Whether a follow-up was recorded (once per receipt). */
  followed: boolean;
  /** What the person typed (no context prefix): what Send hands on. */
  text: string;
  /** The session it was handed on to, once it was. */
  handed: RouteTarget | null;
}

/** Receipts shown in Control, newest last; only the latest few are kept. */
export const receipts = writable<Receipt[]>([]);
export const MAX_RECEIPTS = 3;

/** Ask where `text` (outbox message `key`) goes; keeps a receipt to show. */
export async function routeSent(key: string, text: string): Promise<void> {
  const r = await proposeRoute(text);
  // `!r.value`: a command answering nothing (an older hub, a test's bare
  // mock) keeps no receipt rather than throwing out of the send.
  if (!r.ok || !r.value || r.value.outcome === 'none') return;
  const route = r.value;
  receipts.update((rs) => [...rs.filter((x) => x.key !== key), { key, route, chosen: null, followed: false, text, handed: null }].slice(-MAX_RECEIPTS));
}

/**
 * The person kept or changed receipt `key`. The first decision is
 * recorded against the run; a later change only updates the receipt.
 */
export async function choose(key: string, option: string): Promise<void> {
  let runId: number | null = null;
  receipts.update((rs) =>
    rs.map((x) => {
      if (x.key !== key) return x;
      if (!x.followed && x.route.run_id != null) runId = x.route.run_id;
      return { ...x, chosen: option, followed: x.followed || x.route.run_id != null };
    }),
  );
  if (runId != null) await followRoute(runId, option);
}

/**
 * Hand receipt `key`'s message on to the session it is about (the person's
 * pick, else the proposal). Recorded as the person's choice when they had
 * not made one. `null` when it went into the session's outbox, else why not.
 */
export async function handOn(key: string): Promise<string | null> {
  const r = get(receipts).find((x) => x.key === key);
  if (!r) return 'That receipt is gone.';
  if (r.handed) return null;
  const t = targetOf(r.route, shownOption(r));
  if (!t) return 'Pick a mission or session first.';
  if (t.kind === 'mission') return handOnToMission(key, r, t);
  const row = get(sessions).find((s) => s.id === t.id);
  if (!row || row.lost_at != null) return `${t.name} is no longer running.`;
  outbox.enqueue({ id: row.id, host_alias: row.host_alias, tmux_name: row.tmux_name }, { kind: 'prompt', text: r.text });
  receipts.update((rs) => rs.map((x) => (x.key === key ? { ...x, handed: t } : x)));
  if (r.chosen === null) await choose(key, optionOf(t));
  return null;
}

/** A mission takes the message as the answer to its oldest open question. */
async function handOnToMission(key: string, r: Receipt, t: RouteTarget): Promise<string | null> {
  const m = await getMission(t.id);
  if (!m.ok) return m.error.message ?? `${t.name} could not be read.`;
  const ask = (m.value.plan?.cards ?? [])
    .filter((c) => c.state === 'open' && c.kind === 'ask')
    .sort((a, b) => a.created_at - b.created_at)[0];
  if (!ask) return `${t.name} has no open question to answer; open it to steer it.`;
  const d = await decideMissionCard(ask.id, true, r.text);
  if (!d.ok) return d.error.message ?? `${t.name} did not take the answer.`;
  receipts.update((rs) => rs.map((x) => (x.key === key ? { ...x, handed: t } : x)));
  if (r.chosen === null) await choose(key, optionOf(t));
  return null;
}

/** The option a receipt shows now: the person's pick, else the proposal. */
export function shownOption(r: Receipt): string | null {
  return r.chosen ?? (r.route.outcome === 'proposed' ? (r.route.target ?? null) : null);
}

/** Test seam. */
export function resetReceiptsForTests(): void {
  receipts.set([]);
}
