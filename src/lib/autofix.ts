// Autofix findings (crates/fleet-core/src/service/autofix, spec
// docs/superpowers/specs/2026-10-17-autofix-design.md): known broken fleet
// states, each with a plain-language explanation and, where one exists, a
// fix. Safe fixes run on the hub by themselves; the rest wait here for a
// person's Fix. The panel's "Problems & fixes", the `fix` chat card, the
// host chip's count and Control's own finding all read this one store.
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';

export type Safety = 'safe' | 'needs_approval';

export interface FixInfo {
  summary: string;
  /** What the fix writes, and where: the confirm lists these. */
  changes: string[];
  safety: Safety;
  reversible: boolean;
  /** Whether the hub may run it by itself when it is safe. */
  auto: boolean;
}

/** A step the app takes when fleet has no fix to run (a login on a host). */
export interface ClientAction {
  action: 'login';
  host: string;
}

export type FindingState = 'open' | 'verifying' | 'apply_failed' | 'dismissed' | 'cleared';

export interface Finding {
  id: number;
  kind: string;
  /** `host:<alias>`, `session:<id>`, `token:<name>` or `hub`. */
  subject: string;
  host_alias?: string | null;
  severity: 'blocking' | 'degraded' | 'info';
  state: FindingState;
  first_seen_at: number;
  last_seen_at: number;
  attempts: number;
  last_attempt_at?: number | null;
  last_error?: string | null;
  dismissed_until?: number | null;
  title?: string | null;
  explanation?: string | null;
  evidence: string[];
  fix?: FixInfo | null;
  action?: ClientAction | null;
}

export interface Findings {
  findings: Finding[];
  fixed: Finding[];
  can_apply: boolean;
}

/** Live findings, oldest first. */
export const findings = writable<Finding[]>([]);
/** Cleared in the last day after a fix ran. */
export const fixedRecently = writable<Finding[]>([]);
/** This device may press Fix and Later (the desktop, or a trusted device). */
export const canApplyFixes = writable<boolean>(false);
/** The list was read at least once. */
export const findingsLoaded = writable<boolean>(false);
/** The last read failed (the hub unreachable, timed out, or without the
 *  tool): what the store holds is not the hub's answer, so a finding missing
 *  from it is unknown, not resolved. Null once a read succeeds. */
export const findingsError = writable<IpcError | null>(null);

/** The findings a person is asked about: everything live but the dismissed. */
export const openFindings = derived(findings, ($f) => $f.filter((x) => x.state !== 'dismissed'));

/** Hidden with Later, until the hub shows them again: the panel's own fold. */
export const hiddenFindings = derived(findings, ($f) => $f.filter((x) => x.state === 'dismissed'));

/** What a fixed row says ran: the fix's summary, and how many tries it took. */
export function fixedWords(f: Finding): string {
  const what = f.fix?.summary || 'Fixed';
  return f.attempts > 1 ? `${what} · ${f.attempts} tries` : what;
}

/** Open findings per host alias, for the host chip. */
export const findingsByHost = derived(openFindings, ($f) => {
  const m = new Map<string, number>();
  for (const x of $f) if (x.host_alias) m.set(x.host_alias, (m.get(x.host_alias) ?? 0) + 1);
  return m;
});

/** Control's own finding (its token belongs to nobody), when open. */
export const operatorFinding = derived(openFindings, ($f) => $f.find((x) => x.kind === 'operator_unbound') ?? null);

export async function loadFindings(): Promise<Result<Findings>> {
  const r = await invokeCmd<Findings>('autofix_findings', {});
  // A hub older than autofix answers nothing usable: no findings, read-only.
  if (r.ok && r.value) {
    findings.set(r.value.findings ?? []);
    fixedRecently.set(r.value.fixed ?? []);
    canApplyFixes.set(r.value.can_apply === true);
  }
  findingsError.set(r.ok ? null : r.error);
  findingsLoaded.set(true);
  return r;
}

function replace(row: Finding) {
  const live = row.state !== 'cleared';
  findings.set([...get(findings).filter((x) => x.id !== row.id), ...(live ? [row] : [])].sort((a, b) => a.first_seen_at - b.first_seen_at || a.id - b.id));
  if (!live && row.attempts > 0) fixedRecently.set([row, ...get(fixedRecently).filter((x) => x.id !== row.id)]);
}

/** Fix: the hub runs the fix and re-checks. The row comes back as it now
 *  stands: `cleared`, `verifying` (still showing; the hub re-checks), or
 *  `apply_failed` with the reason. */
export async function applyFinding(id: number): Promise<Result<Finding>> {
  const r = await invokeCmd<Finding>('autofix_apply', { id });
  if (r.ok && r.value) replace(r.value);
  return r;
}

/** Later: hide it for a day. */
export async function dismissFinding(id: number): Promise<Result<Finding>> {
  const r = await invokeCmd<Finding>('autofix_dismiss', { id, until: null });
  if (r.ok && r.value) replace(r.value);
  return r;
}

const POLL_MS = 60_000;
let users = 0;
let timer: ReturnType<typeof setInterval> | undefined;

/** Keep the store fresh while a view shows it; returns the release. The
 *  hub's own pass is once a minute, so polling faster shows nothing new. */
export function watchFindings(): () => void {
  users += 1;
  if (users === 1) {
    void loadFindings();
    timer = setInterval(() => void loadFindings(), POLL_MS);
  }
  return () => {
    users = Math.max(0, users - 1);
    if (users === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

/** What the subject names, in words: "host mercury", "session #12". */
export function subjectWords(subject: string): string {
  const [k, ...rest] = subject.split(':');
  const v = rest.join(':');
  if (k === 'host') return `host ${v}`;
  if (k === 'session') return `session #${v}`;
  if (k === 'token') return `token ${v}`;
  return subject;
}

/** The state of a finding in a few words, for the card's status line. */
export function stateWords(f: Finding): string | null {
  switch (f.state) {
    case 'verifying':
      return 'Fix applied — checking that it worked…';
    case 'apply_failed':
      return f.last_error ? `The fix did not work: ${f.last_error}` : 'The fix did not work.';
    case 'dismissed':
      return 'Hidden until later.';
    case 'cleared':
      return 'Resolved.';
    default:
      return null;
  }
}
