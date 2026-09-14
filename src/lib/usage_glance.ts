// The glanceable usage surfaces (spec: "Glanceable surfaces"): the
// New-session host chips, the selected chip's full line with its low-headroom
// warning, and the window footer's one segment. Pure, on top of the usage
// model in `account_usage.ts`; every function takes `now` (unix seconds).
//
// Usage belongs to an ACCOUNT, so every surface here speaks about the host's
// account and names the other hosts that share it.
import type { AccountRow } from './accounts';
import { accountLabel } from './accounts';
import type { AccountUsageSnapshot, UsageWindow } from './account_usage_store';
import type { HostRow } from './hosts';
import {
  LOCAL_HOST,
  bindingWindow,
  chipLabel,
  clock,
  formatDuration,
  formatReset,
  formatResetShort,
  freshness,
  leftPct,
  limitWording,
  severity,
  weekdayClock,
  windowOf,
  type Freshness,
  type UsageSeverity,
  type UsageWindowKind,
} from './account_usage';

/** The footer collapses to `usage off` after this long unavailable. */
export const FOOTER_OFF_AFTER_SECS = 24 * 3600;

const LEVEL_RANK: Record<UsageSeverity, number> = { ok: 0, caution: 1, low: 2, limit: 3 };

function isChecking(s: AccountUsageSnapshot | null): boolean {
  return !s || (s.status === 'never_fetched' && !s.usage);
}

/** A host counts as offline for a chip unless it is `local` (always pickable). */
function hostOffline(h: HostRow): boolean {
  return !h.reachable && h.alias !== LOCAL_HOST;
}

/** Other hosts logged in to `host`'s account, alphabetically. */
export function otherHostsOnAccount(host: HostRow, hosts: readonly HostRow[]): string[] {
  if (!host.account_uuid) return [];
  return hosts
    .filter((h) => h.alias !== host.alias && h.account_uuid === host.account_uuid)
    .map((h) => h.alias)
    .sort((a, b) => a.localeCompare(b));
}

// ── one account's binding number ──

export interface BindingNumber {
  window: UsageWindowKind;
  w: UsageWindow;
  left: number;
  freshness: Freshness;
  level: UsageSeverity;
}

/** The binding window's number while it may still be shown (not expired). */
export function bindingNumber(snapshot: AccountUsageSnapshot | null, now: number): BindingNumber | null {
  if (!snapshot?.usage) return null;
  const kind = bindingWindow(snapshot.usage);
  const w = kind ? windowOf(snapshot.usage, kind) : null;
  if (!kind || !w) return null;
  const fr = freshness(kind, snapshot.fetched_at, w.resets_at, now);
  if (fr === 'expired') return null;
  const left = leftPct(w);
  return { window: kind, w, left, freshness: fr, level: severity(kind, left, w.resets_at, now) };
}

// ── New-session host chips ──

/**
 * A chip's second line: `offline`, `no account`, or the account's compact
 * usage — % left AND the reset time (`91% left · resets 17:05`), `~` + ◷
 * when stale, `? left` when expired, `checking…` before the first fetch.
 */
export function hostChipUsage(
  host: HostRow,
  account: AccountRow | null,
  snapshot: AccountUsageSnapshot | null,
  now: number,
  locale?: string,
  timeZone?: string,
): string {
  if (hostOffline(host)) return 'offline';
  if (!host.account_uuid) return 'no account';
  return chipLabel(snapshot, now, account?.has_extra_usage ?? false, locale, timeZone);
}

function pctText(kind: UsageWindowKind, w: UsageWindow, fetchedAt: number | null, now: number): string {
  const fr = freshness(kind, fetchedAt, w.resets_at, now);
  if (fr === 'expired') return '?';
  return `${fr === 'stale' ? '~' : ''}${leftPct(w)}%`;
}

function ageText(fetchedAt: number, now: number): string {
  const age = now - fetchedAt;
  return age < 60 ? 'just now' : `${formatDuration(age)} ago`;
}

/**
 * The selected chip's full line:
 * `<account> · 5h 91% left, resets 17:05 · weekly 86% left · 2 min ago`.
 */
export function selectedUsageLine(
  host: HostRow,
  account: AccountRow | null,
  snapshot: AccountUsageSnapshot | null,
  now: number,
  locale?: string,
  timeZone?: string,
): string {
  if (!host.account_uuid) return 'Not logged in to Claude on this host — no usage to show.';
  const label = accountLabel(account);
  if (isChecking(snapshot)) return `${label} · checking usage…`;
  const usage = snapshot!.usage;
  const fetchedAt = snapshot!.fetched_at;
  const parts = [label];
  const five = usage?.five_hour ?? null;
  const week = usage?.seven_day ?? null;
  if (five) {
    const pct = pctText('5h', five, fetchedAt, now);
    const reset = pct === '?' || five.resets_at === null ? '' : `, ${formatResetShort('5h', five.resets_at, now, locale, timeZone)}`;
    parts.push(`5h ${pct} left${reset}`);
  }
  if (week) parts.push(`weekly ${pctText('weekly', week, fetchedAt, now)} left`);
  if (!five && !week) parts.push('usage unavailable');
  if (fetchedAt !== null) parts.push(ageText(fetchedAt, now));
  return parts.join(' · ');
}

/**
 * The inline warning when the chosen host's account is at low or limit
 * severity, naming the account and the other hosts that share it:
 * `▲ admin@32bit.sk has 8% of its 5-hour window left (resets 15:10). Also
 * used by claude-fleet-oci.` `null` otherwise. It never changes the host.
 */
export function lowHeadroomWarning(
  host: HostRow,
  hosts: readonly HostRow[],
  account: AccountRow | null,
  snapshot: AccountUsageSnapshot | null,
  now: number,
  locale?: string,
  timeZone?: string,
): string | null {
  if (!host.account_uuid) return null;
  const b = bindingNumber(snapshot, now);
  if (!b || (b.level !== 'low' && b.level !== 'limit')) return null;
  const label = accountLabel(account);
  const win = b.window === '5h' ? '5-hour' : 'weekly';
  const at =
    b.w.resets_at === null
      ? null
      : b.window === '5h'
        ? clock(b.w.resets_at, locale, timeZone)
        : weekdayClock(b.w.resets_at, locale, timeZone);
  const reset = at ? ` (resets ${at})` : '';
  const approx = b.freshness === 'stale' ? '~' : '';
  let text: string;
  if (b.level === 'limit') {
    const extra = account?.has_extra_usage ? ' Further use spends extra usage.' : '';
    text = `■ ${label} is at its ${win} limit${reset}.${extra}`;
  } else {
    text = `▲ ${label} has ${approx}${b.left}% of its ${win} window left${reset}.`;
  }
  const others = otherHostsOnAccount(host, hosts);
  if (others.length > 0) text += ` Also used by ${others.join(', ')}.`;
  return text;
}

// ── footer ──

export type FooterState = 'ok' | 'attention' | 'unavailable' | 'off' | 'checking';

export interface FooterUsage {
  state: FooterState;
  /** The whole segment, starting `usage `. */
  text: string;
  /** How loud: severity drives `warn`/`alarm`; `muted` for off/checking. */
  tone: 'normal' | 'warn' | 'alarm' | 'muted';
  /** The full state in words, for the button's `aria-label`. */
  ariaLabel: string;
  /** Host to preselect when the segment opens the Hosts view. */
  host: string | null;
}

interface AccountEval {
  uuid: string;
  label: string;
  hosts: HostRow[];
  snapshot: AccountUsageSnapshot | null;
  account: AccountRow | null;
  number: BindingNumber | null;
}

/** Compact age for the footer: `<1m`, `3m`, `2h`, `1d`. */
export function compactAge(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return '<1m';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** The host to open for an account: its polling host, else an online one, else any. */
function hostFor(e: AccountEval): string | null {
  const byName = [...e.hosts].sort((a, b) => a.alias.localeCompare(b.alias));
  const source = e.snapshot?.source_host;
  if (source && byName.some((h) => h.alias === source)) return source;
  return (byName.find((h) => h.reachable) ?? byName[0])?.alias ?? null;
}

/**
 * The footer segment: whether to look, not the numbers.
 *
 * Only accounts some host is logged in to count. Then:
 * - every account fresh and ok → `usage ✓ all accounts · <age of oldest>`;
 * - no account's latest check succeeded (or none has a number that may
 *   still be shown) → `usage checking…` before the first fetch, else
 *   `usage ◷ unavailable since <last good check>`, collapsing to a muted
 *   `usage off` after 24 hours — unless a still-showable number is low or at
 *   its limit, which is named as below;
 * - otherwise the WORST account with a showable number — highest severity,
 *   ties by fewest % left, then label: `usage ▲ <label> 5h 8% left · resets
 *   15:10`. An account without a showable number (no online host, a failed
 *   or expired check) is never picked over one with a number, so it cannot
 *   raise the alarm on its own or outrank a real low-headroom account.
 *
 * `null` when no host is logged in to any account (nothing to say).
 */
export function footerUsage(
  hosts: readonly HostRow[],
  accounts: readonly AccountRow[],
  snapshots: Record<string, AccountUsageSnapshot>,
  now: number,
  locale?: string,
  timeZone?: string,
): FooterUsage | null {
  const byUuid = new Map<string, HostRow[]>();
  for (const h of hosts) {
    if (!h.account_uuid) continue;
    const list = byUuid.get(h.account_uuid) ?? [];
    list.push(h);
    byUuid.set(h.account_uuid, list);
  }
  if (byUuid.size === 0) return null;
  const evals: AccountEval[] = [...byUuid.entries()].map(([uuid, hs]) => {
    const account = accounts.find((a) => a.uuid === uuid) ?? null;
    const snapshot = snapshots[uuid] ?? null;
    return { uuid, label: accountLabel(account ?? { uuid } as AccountRow), hosts: hs, snapshot, account, number: bindingNumber(snapshot, now) };
  });

  const withNumber = evals.filter((e) => e.number !== null);
  const worstOf = (list: AccountEval[]) =>
    [...list].sort(
      (a, b) =>
        LEVEL_RANK[b.number!.level] - LEVEL_RANK[a.number!.level] ||
        a.number!.left - b.number!.left ||
        a.label.localeCompare(b.label),
    )[0];
  // "Unavailable": no account's latest check succeeded. A still-showable low
  // or limit number is named anyway — the alarm is worth more than the outage.
  const noneOk = evals.every((e) => e.snapshot?.status !== 'ok');
  const alarming = withNumber.filter((e) => e.number!.level === 'low' || e.number!.level === 'limit');

  if (withNumber.length === 0 || (noneOk && alarming.length === 0)) {
    const first = [...evals].sort((a, b) => a.label.localeCompare(b.label))[0];
    if (withNumber.length === 0 && evals.some((e) => isChecking(e.snapshot))) {
      return {
        state: 'checking',
        text: 'usage checking…',
        tone: 'muted',
        ariaLabel: 'Account usage: checking. Open Hosts.',
        host: hostFor(first),
      };
    }
    const fetched = evals.map((e) => e.snapshot?.fetched_at ?? null).filter((t): t is number => t !== null);
    const since = fetched.length > 0 ? Math.max(...fetched) : null;
    if (since !== null && now - since > FOOTER_OFF_AFTER_SECS) {
      return {
        state: 'off',
        text: 'usage off',
        tone: 'muted',
        ariaLabel: `Account usage has been unavailable for more than 24 hours, since ${weekdayClock(since, locale, timeZone)}. Open Hosts.`,
        host: hostFor(first),
      };
    }
    const at = since === null ? null : clock(since, locale, timeZone);
    return {
      state: 'unavailable',
      text: at ? `usage ◷ unavailable since ${at}` : 'usage ◷ unavailable',
      tone: 'normal',
      ariaLabel: `Account usage unavailable${at ? ` since ${at}` : ''}. Open Hosts.`,
      host: hostFor(first),
    };
  }

  const worst = worstOf(withNumber);

  const allFreshOk = evals.every((e) => e.number !== null && e.number.freshness === 'fresh' && e.number.level === 'ok');
  if (allFreshOk) {
    const oldest = Math.min(...evals.map((e) => e.snapshot!.fetched_at!));
    const n = evals.length;
    return {
      state: 'ok',
      text: `usage ✓ all accounts · ${compactAge(now - oldest)}`,
      tone: 'normal',
      ariaLabel: `Account usage: all ${n} account${n === 1 ? '' : 's'} have headroom; oldest check ${formatDuration(now - oldest)} ago. Open Hosts.`,
      host: hostFor(worst),
    };
  }

  const b = worst.number!;
  const hasExtra = worst.account?.has_extra_usage ?? false;
  const win = b.window === '5h' ? '5h' : 'weekly';
  const winWords = b.window === '5h' ? '5-hour window' : 'weekly window';
  const stale = b.freshness === 'stale';
  const glyph = b.level === 'limit' ? '■' : b.level === 'low' ? '▲' : b.level === 'caution' ? '△' : stale ? '◷' : '';
  const reset = b.w.resets_at === null ? '' : ` · ${formatResetShort(b.window, b.w.resets_at, now, locale, timeZone)}`;
  const figure = b.level === 'limit' ? limitWording(hasExtra) : `${stale ? '~' : ''}${b.left}% left`;
  const text = `usage ${glyph ? `${glyph} ` : ''}${worst.label} ${win} ${figure}${reset}`;
  const tone = b.level === 'limit' || b.level === 'low' ? 'alarm' : b.level === 'caution' ? 'warn' : 'normal';
  const ageNote = stale && worst.snapshot?.fetched_at != null ? ` (checked ${formatDuration(now - worst.snapshot.fetched_at)} ago)` : '';
  const resetWords = b.w.resets_at === null ? '' : `, ${formatReset(b.window, b.w.resets_at, now, locale, timeZone)}`;
  const levelWords = b.level === 'limit' ? `is at its ${winWords} limit` : `has ${stale ? 'about ' : ''}${b.left}% of its ${winWords} left`;
  return {
    state: 'attention',
    text,
    tone,
    ariaLabel: `Account usage: ${worst.label} ${levelWords}${ageNote}${resetWords}. Open Hosts on ${hostFor(worst) ?? 'its host'}.`,
    host: hostFor(worst),
  };
}
