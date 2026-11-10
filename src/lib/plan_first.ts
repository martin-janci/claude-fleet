// "Plan first" (docs/plan-first.md, HumanLayer plan v2, Batch 3 item 7): a
// checkbox on New session that tells the agent to write a plan file and stop,
// and a "Plan ready" card in the conversation once that file shows up in the
// session's changes. No phase engine and no auto-advance: the person reads
// the plan and says go.
//
// Which sessions were started with it is remembered on this device only
// (localStorage); another device shows no card and the agent still waits in
// words, as its instructions say.
import type { ChangedFile } from './files';

/** What the agent is told when Plan first is ticked. `{date}` is today. */
export function planFirstBrief(today: string): string {
  return [
    'Plan first. For the task in this session, do not change any code yet:',
    '1. Research how things work today: read the code and docs the task touches. Describe what is there; do not propose anything in this step.',
    `2. Write the plan to docs/plans/${today}-<short-slug>.md: the goal in two sentences, what you found, the steps as small vertical slices (each one ends with a check that shows it works: a test, a command, a screen), and open questions for me at the end. Decide nothing I have to decide; list it as a question.`,
    '3. Stop. Say "Plan ready" with the file\'s path and wait. Start the work only after I say the plan is approved, and follow the plan as approved.',
  ].join('\n');
}

/** The prompt Approve sends. */
export function planApprovedPrompt(path: string): string {
  return `The plan in ${path} is approved. Go ahead and implement it, step by step, running each step's check before the next.`;
}

/** The prompt "Ask for changes" starts the composer with. */
export function planChangesPrefix(path: string): string {
  return `Change the plan in ${path} (do not start the work yet): `;
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

// A plan is a Markdown file directly in docs/plans/ (or the older
// docs/superpowers/plans/), at any depth of the checkout.
const PLAN_PATH = /(^|\/)docs\/(superpowers\/)?plans\/[^/]+\.md$/i;
const NEW_STATUS = new Set(['added', 'untracked', 'modified', 'renamed', 'copied']);

/** The plan the session wrote, from its changes: a new one before an edited
 *  one, then the last by path (the date prefix sorts the newest last). */
export function planFile(changes: readonly ChangedFile[]): string | null {
  const plans = changes.filter((c) => PLAN_PATH.test(c.path) && NEW_STATUS.has(c.status));
  if (plans.length === 0) return null;
  const fresh = plans.filter((c) => c.status === 'added' || c.status === 'untracked');
  const pool = fresh.length > 0 ? fresh : plans;
  return [...pool].sort((a, b) => a.path.localeCompare(b.path)).at(-1)!.path;
}

const KEY = 'fleet.planFirst.v1';
const KEEP = 200;

/** session key → the plan path already approved or dismissed ('' = waiting). */
type Marks = Record<string, string>;

function read(): Marks {
  try {
    const raw = localStorage.getItem(KEY);
    const v: unknown = raw ? JSON.parse(raw) : {};
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Marks) : {};
  } catch {
    return {};
  }
}

function write(m: Marks): void {
  try {
    // Oldest marks fall off: insertion order is the order they were made.
    const keys = Object.keys(m);
    for (const k of keys.slice(0, Math.max(0, keys.length - KEEP))) delete m[k];
    localStorage.setItem(KEY, JSON.stringify(m));
  } catch {
    // Private window or blocked storage: the card just never shows.
  }
}

/** Remember that this session was started with Plan first. */
export function markPlanFirst(key: number): void {
  const sessionKey = String(key);
  const m = read();
  delete m[sessionKey];
  m[sessionKey] = '';
  write(m);
}

/** Whether a "Plan ready" card is due for `path` on this session. */
export function planPending(key: number, path: string | null): boolean {
  if (!path) return false;
  const sessionKey = String(key);
  const m = read();
  return sessionKey in m && m[sessionKey] !== path;
}

/** Whether this session was started with Plan first on this device. */
export function isPlanFirst(key: number): boolean {
  return String(key) in read();
}

/** The plan at `path` was approved or dismissed: no card for it again. A
 *  later, different plan file in the same session asks once more. */
export function settlePlan(key: number, path: string): void {
  const sessionKey = String(key);
  const m = read();
  if (!(sessionKey in m)) return;
  m[sessionKey] = path;
  write(m);
}
