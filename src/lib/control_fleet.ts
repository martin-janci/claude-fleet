// Control's Needs you view (board MissionControl): the whole fleet in four
// groups, worst first. Needs you is the Inbox's own query (`inboxQueue`),
// so the panel, the Inbox and the rail's badge never disagree; Running,
// Idle and Done today are the rest of the same visible rows, by attention
// state (step 0.4). Each row carries one reason line in the board's words:
// "Waiting for you: approve push to main", "Failed: tests crashed",
// "Paused · weekly limit on tech.silvester".
import { derived } from 'svelte/store';
import {
  attentionState,
  classify,
  promptPreview,
  stuckKindLabel,
  type AttentionFacts,
  type AttentionOptions,
  type AttentionState,
  type TriageBucket,
} from './attention';
import { blockedLine, attentionFacts } from './attention_facts';
import { effectiveHostFilter } from './hosts';
import { inboxQueue } from './inbox';
import { attentionIdleMinutes } from './notify';
import { effectiveScope, scopeOf } from './orgs';
import { pendingInputFor } from './pending_input';
import { sessions, showBgAgents, type SessionRow } from './sessions';
import { sessionVisible } from './sidebar_index';
import { localMidnight } from './today';

export interface FleetGroups {
  needsYou: SessionRow[];
  running: SessionRow[];
  idle: SessionRow[];
  /** Finished since local midnight. */
  done: SessionRow[];
}

const newest = (a: SessionRow, b: SessionRow) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0);

/**
 * The rows that are not in Needs you, by state: Working is Running; Done
 * (a finished turn not yet read) or anything that stopped since `midnight`
 * is Done today; the rest is Idle. A ghost row is in none: it is the
 * Restore row's business, not the fleet's.
 */
export function fleetGroups(
  visible: readonly SessionRow[],
  needsYou: readonly SessionRow[],
  opts: AttentionOptions,
  midnight: number,
): FleetGroups {
  const asked = new Set(needsYou.map((s) => s.id));
  const out: FleetGroups = { needsYou: [...needsYou], running: [], idle: [], done: [] };
  for (const s of visible) {
    if (asked.has(s.id) || s.status === 'ghost') continue;
    const st: AttentionState = attentionState(s, opts);
    if (st === 'working') out.running.push(s);
    else if (st === 'done' || (s.last_stop_at ?? 0) >= midnight) out.done.push(s);
    else out.idle.push(s);
  }
  out.running.sort(newest);
  out.idle.sort(newest);
  out.done.sort(newest);
  return out;
}

/** The four groups under the Inbox's filters (host, background agents,
 *  organisation), so Needs you here is exactly the Inbox. */
export const fleetGroupsStore = derived(
  [sessions, effectiveHostFilter, showBgAgents, effectiveScope, scopeOf, attentionIdleMinutes, attentionFacts, inboxQueue],
  ([$sessions, $host, $bg, $scope, $of, $idle, $facts, $queue]) => {
    const scope = $scope === 'all' ? null : { id: $scope, of: $of };
    const visible = $sessions.filter((s) => sessionVisible(s, $host, $bg, null, scope));
    const now = Math.floor(Date.now() / 1000);
    return fleetGroups(visible, $queue, { idleSecs: $idle * 60, now, facts: $facts }, localMidnight());
  },
);

/** "4 need you · 6 running · 9 idle": the welcome line under the greeting. */
export function welcomeLine(g: FleetGroups): string {
  return `${g.needsYou.length} need you · ${g.running.length} running · ${g.idle.length} idle`;
}

/** A row's reason, split so the state word can take its colour. */
export interface Reason {
  /** "Waiting for you:", "Failed:", "Paused · weekly limit" … */
  word: string;
  /** What follows it; may be empty. */
  detail: string;
  /** The attention state the colour comes from. */
  state: AttentionState;
}

function split(text: string, state: AttentionState): Reason {
  // "Paused · weekly limit on X": the word is everything up to " on ".
  const on = text.indexOf(' on ');
  if (state === 'blocked' && on > 0) return { word: text.slice(0, on), detail: text.slice(on + 1), state };
  return { word: text, detail: '', state };
}

/** What a row is waiting on, or doing, in one line. */
export function rowReason(
  s: SessionRow,
  opts: AttentionOptions,
  facts: AttentionFacts | undefined,
  accountName: (uuid: string) => string,
): Reason {
  const bucket: TriageBucket = classify(s, opts);
  const state = attentionState(s, opts);
  const blocked = blockedLine(bucket, s, facts, accountName);
  if (blocked) return split(blocked, state);
  switch (bucket) {
    case 'waiting': {
      const v = pendingInputFor({ rowStatus: s.claude_status, rowStuck: s.stuck_kind, rowPending: s.pending_input ?? null, probe: null });
      const what = v?.question || v?.detail || '';
      return { word: what ? 'Waiting for you:' : 'Waiting for you', detail: what, state };
    }
    case 'stuck':
      return { word: 'Stuck:', detail: stuckKindLabel(s.stuck_kind) || 'needs a look', state };
    case 'failed':
    case 'stop_failed':
      return { word: 'Failed:', detail: s.current_activity || 'the last turn failed', state };
    case 'ci_failing':
      return { word: 'Failed:', detail: 'CI is failing', state };
    case 'context_full':
      return { word: 'Needs you:', detail: 'the context is full', state };
    case 'stale_working':
      return { word: 'Needs you:', detail: 'no output for a while', state };
    case 'done_unread':
      return { word: 'Done:', detail: promptPreview(s.last_prompt, 80), state };
    case 'working':
      return { word: '', detail: s.current_activity || promptPreview(s.last_prompt, 80), state };
    default:
      return { word: '', detail: promptPreview(s.last_prompt, 80), state };
  }
}

/** A reason as one string: the Today briefing's lines, a row's title. */
export function reasonText(r: Reason): string {
  return [r.word, r.detail].filter(Boolean).join(' ');
}

/** A Needs you line of the briefing (board MCViews): "Waiting for you: Fix
 *  hub-e2e flake, approve push to main", "Paused · weekly limit: PD-2988". */
export function briefingLine(name: string, r: Reason): string {
  const word = r.word.replace(/:$/, '');
  if (!word) return r.detail ? `${name}, ${r.detail}` : name;
  if (r.state === 'blocked') return `${word}: ${name}`;
  return r.detail ? `${word}: ${name}, ${r.detail}` : `${word}: ${name}`;
}
