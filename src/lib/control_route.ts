import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { ProposalLike } from './ai_proposal';

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
 * The message has already reached Control's agent: a receipt never moves,
 * holds or re-sends it. "Change" records the person's pick, which is what
 * the use case learns from.
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
}

/** Receipts shown in Control, newest last; only the latest few are kept. */
export const receipts = writable<Receipt[]>([]);
export const MAX_RECEIPTS = 3;

/** Ask where `text` (outbox message `key`) goes; keeps a receipt to show. */
export async function routeSent(key: string, text: string): Promise<void> {
  const r = await proposeRoute(text);
  if (!r.ok || r.value.outcome === 'none') return;
  const route = r.value;
  receipts.update((rs) => [...rs.filter((x) => x.key !== key), { key, route, chosen: null, followed: false }].slice(-MAX_RECEIPTS));
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

/** The option a receipt shows now: the person's pick, else the proposal. */
export function shownOption(r: Receipt): string | null {
  return r.chosen ?? (r.route.outcome === 'proposed' ? (r.route.target ?? null) : null);
}

/** Test seam. */
export function resetReceiptsForTests(): void {
  receipts.set([]);
}
