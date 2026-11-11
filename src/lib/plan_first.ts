// "Plan first" (docs/plan-first.md, HumanLayer plan v2, Batch 3 item 7): a
// checkbox on New session that tells the agent to write a plan file and stop,
// and a "Plan ready" card in the conversation once the agent says so and the
// file it names is among the session's changes. No phase engine and no
// auto-advance: the person reads the plan and says go.
//
// Nothing records that a session was started this way. The agent's own
// words are the signal ("Plan ready" and the file's name, since the last
// prompt), so the card shows on every device, for a session started
// anywhere, and goes away with the next prompt whoever sent it. Only "this
// one was answered here" is kept, on this device (localStorage).
import type { ConvTurn } from './conversation';
import type { ChangedFile } from './files';

/** What the agent is told when Plan first is ticked. `{date}` is today. */
export function planFirstBrief(today: string): string {
  return [
    'Plan first. For the task in this session, do not change any code yet:',
    '1. Research how things work today: read the code and docs the task touches. Describe what is there; do not propose anything in this step.',
    `2. Write the plan to docs/plans/${today}-<short-slug>.md and do not commit it: the goal in two sentences, what you found, the steps as small vertical slices (each one ends with a check that shows it works: a test, a command, a screen), and open questions for me at the end. Decide nothing I have to decide; list it as a question.`,
    '3. Stop. Say "Plan ready" with the file\'s path and wait; say it again each time you change the plan. Start the work only after I say the plan is approved, and follow the plan as approved.',
  ].join('\n');
}

/** The prompt Approve sends. */
export function planApprovedPrompt(path: string): string {
  return `The plan in ${path} is approved. Go ahead and implement it, step by step, running each step's check before the next.`;
}

/** The prompt "Ask for changes" starts the composer with. */
export function planChangesPrefix(path: string): string {
  return `Change the plan in ${path}, then say "Plan ready" again (do not start the work yet): `;
}

/** Today as the plan file's prefix (local date, YYYY-MM-DD). */
export function planDate(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

/** A ticket brief with the Plan first paragraph under it. */
export function withPlanFirst(brief: string, today: string): string {
  const b = brief.trimEnd();
  return b ? `${b}\n\n${planFirstBrief(today)}` : planFirstBrief(today);
}

/** The brief half of a start's arguments. Without Plan first, `brief` only
 *  when the person edited it: an untouched preview lets the backend build
 *  the canonical one (comments, attachments, parent and origin context, the
 *  org fence). With Plan first, `plan_first` carries the paragraph and the
 *  backend puts it after the brief it builds, or after the edit
 *  (`brief_edited`); `brief` still carries the composed text for a hub that
 *  predates `plan_first`, which reads only it. */
export function startBriefArgs(o: {
  briefOn: boolean;
  planFirstOn: boolean;
  edited: boolean;
  /** The text the dialog shows: the edit, or the preview. */
  text: string;
  today: string;
}): { brief?: string; plan_first?: string; brief_edited?: boolean } {
  if (!o.briefOn) return {};
  if (!o.planFirstOn) return o.edited ? { brief: o.text } : {};
  return {
    brief: withPlanFirst(o.text, o.today),
    plan_first: planFirstBrief(o.today),
    brief_edited: o.edited,
  };
}

/** "Plan ready", as the instructions ask for it ("the plan is ready" too). */
const SAYS_READY = /\bplan\s+(is\s+)?ready\b/i;

/** The agent's "Plan ready": what it said since the last prompt, and when
 *  that prompt was (which tells one announcement from the next). */
export interface PlanSaid {
  text: string;
  at: string;
}

/** What the agent said since the last prompt, when that says its plan is
 *  ready; null otherwise. Turns no prompt opened (a notification, a
 *  compaction) belong to the prompt before them. */
export function planSaid(turns: readonly ConvTurn[]): PlanSaid | null {
  if (turns.length === 0) return null;
  let from = turns.length - 1;
  while (from > 0 && !turns[from].prompt) from--;
  const said: string[] = [];
  for (const turn of turns.slice(from)) {
    for (const item of turn.items) if (item.kind === 'text') said.push(item.text);
  }
  const text = said.join('\n');
  return SAYS_READY.test(text) ? { text, at: turns[from].at ?? '' } : null;
}

// A plan is a Markdown file directly in docs/plans/ (or the older
// docs/superpowers/plans/), at any depth of the checkout.
const PLAN_PATH = /(^|\/)docs\/(superpowers\/)?plans\/[^/]+\.md$/i;
const NEW_STATUS = new Set(['added', 'untracked', 'modified', 'renamed', 'copied']);

/** The plan `reply` announces, from the session's changes: a new or edited
 *  plan file whose name the reply says. With several, a new one before an
 *  edited one, then the last by path (the date prefix sorts the newest last). */
export function planFile(changes: readonly ChangedFile[], reply: string): string | null {
  const plans = changes.filter(
    (c) => PLAN_PATH.test(c.path) && NEW_STATUS.has(c.status) && reply.includes(c.path.slice(c.path.lastIndexOf('/') + 1)),
  );
  if (plans.length === 0) return null;
  const fresh = plans.filter((c) => c.status === 'added' || c.status === 'untracked');
  const pool = fresh.length > 0 ? fresh : plans;
  return [...pool].sort((a, b) => a.path.localeCompare(b.path)).at(-1)!.path;
}

const KEY = 'fleet.planSettled.v1';
const KEEP = 200;

/** [session key, the announcement answered there], oldest first. */
type Settled = [number, string][];

function read(): Settled {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(KEY) ?? '[]');
    if (!Array.isArray(v)) return [];
    return v.filter((e): e is [number, string] => Array.isArray(e) && typeof e[0] === 'number' && typeof e[1] === 'string');
  } catch {
    return [];
  }
}

/** One announcement: the plan file and the prompt it was said after. */
const announcement = (path: string, at: string) => `${at} ${path}`;

/** Whether this "Plan ready" was already approved or hidden on this device. */
export function planSettled(key: number, path: string, at: string): boolean {
  const id = announcement(path, at);
  return read().some(([k, a]) => k === key && a === id);
}

/** This "Plan ready" was approved or hidden: no card for it again on this
 *  device. The next one the agent says, after another prompt, asks again. */
export function settlePlan(key: number, path: string, at: string): void {
  const kept = read().filter(([k]) => k !== key);
  kept.push([key, announcement(path, at)]);
  try {
    localStorage.setItem(KEY, JSON.stringify(kept.slice(-KEEP)));
  } catch {
    // Private window or blocked storage: the card comes back on the next look.
  }
}
