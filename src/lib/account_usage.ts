// The pure usage model behind every usage surface (the Hosts view's usage
// block, New-session host chips, the footer). It turns an
// `AccountUsageSnapshot` into what a decision can rest on: % left, how old the
// number is, how severe the binding window is, when it resets, and the exact
// wording for every status. Binding spec: "Showing usage" and "Staleness and
// failure" in docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md.
//
// Everything takes `now` (unix seconds) as an argument so tests are
// deterministic; the frontend's wall clock is for display only, the backend
// owns scheduling. Formatters take an optional `locale` and `timeZone`
// (defaults: the system's).
import type { AccountUsage, AccountUsageSnapshot, UsageStatus, UsageWindow } from './account_usage_store';
import type { AccountRow } from './accounts';

export type UsageWindowKind = '5h' | 'weekly';
export type Freshness = 'fresh' | 'stale' | 'expired';
export type UsageSeverity = 'ok' | 'caution' | 'low' | 'limit';

/** A value is fresh for this long after its fetch (both windows). */
export const FRESH_SECS = 6 * 60;
/** Beyond this age a value is expired (number withheld). */
export const STALE_LIMIT_SECS: Record<UsageWindowKind, number> = {
  '5h': 30 * 60,
  weekly: 3 * 3600,
};
/** ok ≥ this % left. */
export const CAUTION_BELOW_PCT = 50;
/** low < this % left. */
export const LOW_BELOW_PCT = 20;
/** The 5-hour window drops one severity level when its reset is closer. */
export const RESET_SOON_SECS = 15 * 60;
export const WEEK_SECS = 7 * 86400;
/** The app's own machine; on macOS its token lives in the Keychain. */
export const LOCAL_HOST = 'local';

// ── numbers ──

/** % left of a window, rounded for display and clamped to 0..100. */
export function leftPct(w: UsageWindow): number {
  const u = Number.isFinite(w.utilization) ? w.utilization : 100;
  return Math.min(100, Math.max(0, Math.round(100 - u)));
}

/** % used as displayed: always `100 - leftPct`, so the two never disagree. */
export function usedPct(w: UsageWindow): number {
  return 100 - leftPct(w);
}

/**
 * Whether a window's last-known value may still be shown. Fresh ≤ 6 min;
 * stale up to 30 min (5-hour) or 3 h (weekly); expired beyond, when never
 * fetched, or once `now` is past the window's reset.
 */
export function freshness(
  window: UsageWindowKind,
  fetchedAt: number | null,
  resetsAt: number | null,
  now: number,
): Freshness {
  if (fetchedAt === null) return 'expired';
  if (resetsAt !== null && now > resetsAt) return 'expired';
  const age = now - fetchedAt;
  if (age <= FRESH_SECS) return 'fresh';
  if (age <= STALE_LIMIT_SECS[window]) return 'stale';
  return 'expired';
}

const LEVELS: UsageSeverity[] = ['ok', 'caution', 'low', 'limit'];

/**
 * ok ≥ 50% left, caution 20–50, low < 20, limit at 0. The 5-hour window drops
 * one level while its reset is under 15 minutes away. `hasExtraUsage` never
 * changes the level, only its wording (`limitWording`).
 */
export function severity(
  window: UsageWindowKind,
  left: number,
  resetsAt: number | null,
  now: number,
  _hasExtraUsage = false,
): UsageSeverity {
  let i: number;
  if (left <= 0) i = 3;
  else if (left < LOW_BELOW_PCT) i = 2;
  else if (left < CAUTION_BELOW_PCT) i = 1;
  else i = 0;
  if (window === '5h' && resetsAt !== null && resetsAt > now && resetsAt - now < RESET_SOON_SECS) {
    i = Math.max(0, i - 1);
  }
  return LEVELS[i];
}

export function limitWording(hasExtraUsage: boolean): 'LIMIT' | 'EXTRA USAGE' {
  return hasExtraUsage ? 'EXTRA USAGE' : 'LIMIT';
}

/** The word and glyph that carry a severity besides colour; `null` at ok. */
export function severityBadge(
  level: UsageSeverity,
  hasExtraUsage: boolean,
): { glyph: string; word: string } | null {
  switch (level) {
    case 'caution':
      return { glyph: '△', word: 'low soon' };
    case 'low':
      return { glyph: '▲', word: 'LOW' };
    case 'limit':
      return { glyph: '■', word: limitWording(hasExtraUsage) };
    default:
      return null;
  }
}

/** The window with fewer % left (5-hour on a tie); `null` with neither. */
export function bindingWindow(usage: AccountUsage | null): UsageWindowKind | null {
  const five = usage?.five_hour ?? null;
  const week = usage?.seven_day ?? null;
  if (five && week) return leftPct(week) < leftPct(five) ? 'weekly' : '5h';
  if (five) return '5h';
  if (week) return 'weekly';
  return null;
}

export function windowOf(usage: AccountUsage | null, window: UsageWindowKind): UsageWindow | null {
  if (!usage) return null;
  return window === '5h' ? usage.five_hour : usage.seven_day;
}

export type ModelName = 'Opus' | 'Sonnet';

export interface ModelBucket {
  model: ModelName;
  window: UsageWindow;
  left: number;
  /** Fewer % left than the overall weekly figure. */
  binds: boolean;
}

/** The present per-model weekly buckets, Opus first. */
export function modelBuckets(usage: AccountUsage | null): ModelBucket[] {
  if (!usage) return [];
  const weekLeft = usage.seven_day ? leftPct(usage.seven_day) : null;
  const out: ModelBucket[] = [];
  const add = (model: ModelName, w: UsageWindow | null) => {
    if (!w) return;
    const left = leftPct(w);
    out.push({ model, window: w, left, binds: weekLeft !== null && left < weekLeft });
  };
  add('Opus', usage.seven_day_opus);
  add('Sonnet', usage.seven_day_sonnet);
  return out;
}

/** The model bucket with the fewest % left, when it binds the weekly figure. */
export function bindingModelBucket(usage: AccountUsage | null): ModelBucket | null {
  const binding = modelBuckets(usage).filter((b) => b.binds);
  if (binding.length === 0) return null;
  return binding.reduce((a, b) => (b.left < a.left ? b : a));
}

/** Buckets that earn a visible line: they bind, or are below 50% left. */
export function notableBuckets(usage: AccountUsage | null): ModelBucket[] {
  return modelBuckets(usage).filter((b) => b.binds || b.left < CAUTION_BELOW_PCT);
}

// ── time ──

function clockParts(unix: number, locale: string | undefined, timeZone: string | undefined) {
  const parts = new Intl.DateTimeFormat(locale, {
    weekday: 'short',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
    timeZone,
  }).formatToParts(new Date(unix * 1000));
  const get = (t: string) => parts.find((p) => p.type === t)?.value ?? '';
  return { weekday: get('weekday'), time: `${get('hour')}:${get('minute')}` };
}

/** `15:10` in the given locale/time zone. */
export function clock(unix: number, locale?: string, timeZone?: string): string {
  return clockParts(unix, locale, timeZone).time;
}

/** `Thu 09:00`. */
export function weekdayClock(unix: number, locale?: string, timeZone?: string): string {
  const p = clockParts(unix, locale, timeZone);
  return `${p.weekday} ${p.time}`;
}

/** A duration for a countdown or an age: `<1 min`, `38 min`, `2h 5m`, `2d 18h`. */
export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return '<1 min';
  const mins = Math.floor(s / 60);
  if (mins < 60) return `${mins} min`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ${mins % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/**
 * The full reset line. 5-hour: `resets in 38 min (15:10)`; weekly:
 * `resets Thu 09:00 (in 2d 18h)`. A reset already past reads `reset at …`.
 */
export function formatReset(
  window: UsageWindowKind,
  resetsAt: number | null,
  now: number,
  locale?: string,
  timeZone?: string,
): string {
  if (resetsAt === null) return 'reset time unknown';
  const at = window === '5h' ? clock(resetsAt, locale, timeZone) : weekdayClock(resetsAt, locale, timeZone);
  if (resetsAt <= now) return `reset at ${at}`;
  const inText = formatDuration(resetsAt - now);
  return window === '5h' ? `resets in ${inText} (${at})` : `resets ${at} (in ${inText})`;
}

/** The compact reset: `resets 15:10` / `resets Thu 09:00`. */
export function formatResetShort(
  window: UsageWindowKind,
  resetsAt: number | null,
  now: number,
  locale?: string,
  timeZone?: string,
): string {
  if (resetsAt === null) return 'reset time unknown';
  const at = window === '5h' ? clock(resetsAt, locale, timeZone) : weekdayClock(resetsAt, locale, timeZone);
  return resetsAt <= now ? `reset at ${at}` : `resets ${at}`;
}

/** Fraction (0..1) of the 7-day window elapsed, for the weekly pace tick. */
export function paceFraction(resetsAt: number | null, now: number): number | null {
  if (resetsAt === null) return null;
  const f = (now - (resetsAt - WEEK_SECS)) / WEEK_SECS;
  return Math.min(1, Math.max(0, f));
}

/** `on pace` while used ≤ elapsed, else `ahead of pace`. */
export function paceLabel(
  usedFraction: number,
  pace: number | null,
): 'on pace' | 'ahead of pace' | null {
  if (pace === null) return null;
  return usedFraction > pace ? 'ahead of pace' : 'on pace';
}

/** `checked 2 min ago` / `checked just now`. */
export function checkedAgo(fetchedAt: number, now: number): string {
  const age = now - fetchedAt;
  return age < 60 ? 'checked just now' : `checked ${formatDuration(age)} ago`;
}

/** `2:10` until `nextTryAt`, or `null` once a refresh is allowed. */
export function refreshCountdown(nextTryAt: number, now: number): string | null {
  const left = Math.ceil(nextTryAt - now);
  if (left <= 0) return null;
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`;
}

// ── compact label ──

/**
 * The compact chip text: the binding window's % left AND its reset, the
 * window named only when weekly binds. Stale → `~62% left ◷`; expired →
 * `? left`; nothing fetched yet → `checking…`.
 */
export function chipLabel(
  snapshot: AccountUsageSnapshot | null,
  now: number,
  hasExtraUsage = false,
  locale?: string,
  timeZone?: string,
): string {
  if (!snapshot || (snapshot.status === 'never_fetched' && !snapshot.usage)) return 'checking…';
  const kind = bindingWindow(snapshot.usage);
  const w = windowOf(snapshot.usage, kind ?? '5h');
  if (!kind || !w) return '? left';
  const fr = freshness(kind, snapshot.fetched_at, w.resets_at, now);
  if (fr === 'expired') return '? left';
  const left = leftPct(w);
  const level = severity(kind, left, w.resets_at, now, hasExtraUsage);
  const name = kind === 'weekly' ? 'weekly ' : '';
  const reset = w.resets_at === null ? '' : ` · ${formatResetShort(kind, w.resets_at, now, locale, timeZone)}`;
  if (fr === 'stale') {
    const glyph = level === 'low' || level === 'limit' ? '▲ ' : '';
    return `${glyph}${name}~${left}% left ◷${reset}`;
  }
  if (level === 'limit') return `■ ${name}${limitWording(hasExtraUsage)}${reset}`;
  if (level === 'low') return `▲ ${name}${left}% left${reset}`;
  return `${name}${left}% left${reset}`;
}

// ── status messages ──

export type MessageTone = 'muted' | 'warn' | 'alarm';

export type MessageKind =
  | 'no_account'
  | 'first_load'
  | 'age'
  | 'past_reset'
  | 'rate_limited'
  | 'unavailable'
  | 'access_token_expired'
  | 'login_expired'
  | 'token_rejected'
  | 'host_unsupported'
  | 'no_credentials'
  | 'no_online_host'
  | 'unreachable'
  | 'host_note';

export interface UsageMessageLine {
  kind: MessageKind;
  tone: MessageTone;
  /** Leading glyph (`◷`, `⏸`, `⚠`, `🔑`, `○`), or `''`. */
  glyph: string;
  /** Plain text without the glyph. Never raw `detail`. */
  text: string;
  /** Set on a line about one window (`past_reset`). */
  window?: UsageWindowKind;
}

export interface UsageStatusMessage {
  lines: UsageMessageLine[];
  /** Nothing fetched yet: render `checking…` with dashed tracks. */
  checking: boolean;
  /** The backend's raw `detail`, for a Copy-details affordance only. */
  copyDetail: string | null;
}

type HostProblem =
  | 'no_credentials'
  | 'access_token_expired'
  | 'login_expired'
  | 'token_rejected'
  | 'host_unsupported'
  | 'unreachable';

interface HostNote {
  host: string;
  problem: HostProblem;
  text: string;
}

/**
 * Parse the backend's per-host notes (`"<host>: <text>; <host>: <text>"`),
 * which accompany host-specific failures and a success after skipped hosts.
 */
export function parseHostNotes(detail: string | null): HostNote[] {
  if (!detail) return [];
  const out: HostNote[] = [];
  for (const part of detail.split('; ')) {
    const m = /^([^\s:;]+): (.+)$/.exec(part.trim());
    if (!m) continue;
    const [, host, text] = m;
    let problem: HostProblem;
    if (text === 'no credentials file') problem = 'no_credentials';
    else if (text === 'access token expired') problem = 'access_token_expired';
    else if (text === 'login expired') problem = 'login_expired';
    else if (text === 'token rejected') problem = 'token_rejected';
    else if (
      text.startsWith('missing ') ||
      text.startsWith('curl is older than') ||
      text.startsWith('no usage markers')
    )
      problem = 'host_unsupported';
    else problem = 'unreachable';
    out.push({ host, problem, text });
  }
  return out;
}

/** What a host lacks, from a `host_unsupported` note: `curl 7.55+ needed on mefistos`. */
export function missingToolText(noteText: string | null, host: string): string {
  const t = noteText ?? '';
  if (t === 'missing curl' || t.startsWith('curl is older than')) return `curl 7.55+ needed on ${host}`;
  if (t === 'missing python3_or_jq') return `python3 or jq needed on ${host}`;
  const tool = /^missing ([A-Za-z0-9._+-]+)$/.exec(t)?.[1];
  if (tool) return `${tool} needed on ${host}`;
  return `curl 7.55+ or python3/jq needed on ${host}`;
}

function httpHint(detail: string | null): string {
  const code = /^HTTP (\d{3})/.exec(detail ?? '')?.[1];
  if (code) return `HTTP ${code}`;
  if ((detail ?? '').startsWith('no HTTP response')) return 'no response';
  return '';
}

function hostFor(status: UsageStatus, notes: HostNote[], snapshot: AccountUsageSnapshot): string {
  return notes.find((n) => n.problem === status)?.host ?? snapshot.source_host ?? 'its host';
}

/**
 * The exact wording for an account's usage state, as structured lines (no
 * HTML). `sharedWith` = the other hosts logged in to the account.
 *
 * `access_token_expired` is benign — Claude Code refreshes the token the next
 * time it runs there — so it never tells the user to log in; only
 * `login_expired` says `Run claude /login there.`
 */
export function statusMessage(
  snapshot: AccountUsageSnapshot | null,
  account: AccountRow | null,
  sharedWith: readonly string[],
  now: number,
  locale?: string,
  timeZone?: string,
): UsageStatusMessage {
  if (!account) {
    return {
      lines: [
        { kind: 'no_account', tone: 'muted', glyph: '', text: 'Not logged in to Claude on this host — no usage to show.' },
      ],
      checking: false,
      copyDetail: null,
    };
  }
  if (!snapshot || (snapshot.status === 'never_fetched' && !snapshot.usage)) {
    const host = snapshot?.source_host;
    return {
      lines: [
        {
          kind: 'first_load',
          tone: 'muted',
          glyph: '',
          text: host ? `Asking ${host} for usage…` : "Asking this account's hosts for usage…",
        },
      ],
      checking: true,
      copyDetail: null,
    };
  }

  const lines: UsageMessageLine[] = [];
  const notes = parseHostNotes(snapshot.detail);
  const status = snapshot.status;
  const nextTry = snapshot.next_try_at > now ? ` Next try ${clock(snapshot.next_try_at, locale, timeZone)}.` : '';
  const host = hostFor(status, notes, snapshot);
  const via = snapshot.source_host;
  const failed = status !== 'ok' && status !== 'never_fetched';

  // The status line (failures), and what the age line calls the reason.
  let reason = '';
  let statusCarriesNextTry = false;
  switch (status) {
    case 'rate_limited':
      reason = 'rate-limited';
      statusCarriesNextTry = true;
      lines.push({ kind: 'rate_limited', tone: 'warn', glyph: '⏸', text: `Anthropic is rate-limiting usage checks.${nextTry}` });
      break;
    case 'unavailable': {
      const hint = httpHint(snapshot.detail);
      reason = `usage endpoint unavailable${hint ? ` (${hint})` : ''}`;
      statusCarriesNextTry = true;
      lines.push({
        kind: 'unavailable',
        tone: 'warn',
        glyph: '⚠',
        text: `Usage unavailable. Anthropic's usage endpoint returned an unexpected response${hint ? ` (${hint})` : ''}. It's undocumented and may have changed. Sessions are unaffected.${nextTry}`,
      });
      break;
    }
    case 'access_token_expired':
      reason = `token refresh pending on ${host}`;
      lines.push({
        kind: 'access_token_expired',
        tone: 'muted',
        glyph: '🔑',
        text: `Usage checks are paused until Claude Code refreshes its token on ${host}; it does so the next time it runs there.`,
      });
      break;
    case 'login_expired':
      reason = `Claude login expired on ${host}`;
      lines.push({
        kind: 'login_expired',
        tone: 'alarm',
        glyph: '🔑',
        text: `Claude login expired on ${host} — usage can't be checked from it. Run claude /login there.`,
      });
      break;
    case 'token_rejected':
      reason = `token rejected on ${host}`;
      lines.push({
        kind: 'token_rejected',
        tone: 'alarm',
        glyph: '🔑',
        text: `Anthropic rejected the Claude token on ${host} — usage can't be checked from it.`,
      });
      break;
    case 'host_unsupported': {
      const note = notes.find((n) => n.problem === 'host_unsupported')?.text ?? null;
      reason = missingToolText(note, host);
      lines.push({
        kind: 'host_unsupported',
        tone: 'warn',
        glyph: '⚠',
        text: `${missingToolText(note, host)} — usage can't be checked from it.`,
      });
      break;
    }
    case 'no_credentials':
      if (host === LOCAL_HOST) {
        reason = 'no other host on this account answered';
        lines.push({
          kind: 'no_credentials',
          tone: 'muted',
          glyph: '○',
          text:
            'Usage is read through another host on this account.' +
            (sharedWith.length === 0
              ? ' No other host is logged in to it.'
              : ` None of ${sharedWith.join(', ')} could be asked.`),
        });
      } else {
        reason = `no Claude credentials on ${host}`;
        lines.push({
          kind: 'no_credentials',
          tone: 'warn',
          glyph: '🔑',
          text: `No Claude credentials on ${host} — usage can't be checked from it.`,
        });
      }
      break;
    case 'no_online_host': {
      const unreachable = notes.filter((n) => n.problem === 'unreachable').map((n) => n.host);
      if (unreachable.length > 0) {
        reason = `couldn't reach ${unreachable.join(', ')}`;
        lines.push({ kind: 'unreachable', tone: 'warn', glyph: '○', text: `Couldn't reach ${unreachable.join(', ')} to check usage.` });
      } else {
        reason = 'no online host';
        lines.push({
          kind: 'no_online_host',
          tone: 'warn',
          glyph: '○',
          text: `No online host is logged in to this account${via ? ` (${via} offline)` : ''}.`,
        });
      }
      break;
    }
    case 'ok':
      // Skipped hosts before the one that answered: a muted note, not an alarm.
      for (const n of notes) {
        const tail = via ? ` — usage is read via ${via}.` : '.';
        let text: string | null;
        switch (n.problem) {
          case 'login_expired':
            text = `Claude login expired on ${n.host}${tail} Run claude /login there.`;
            break;
          case 'token_rejected':
            text = `Anthropic rejected the Claude token on ${n.host}${tail}`;
            break;
          case 'access_token_expired':
            text = `${n.host} is waiting for Claude Code to refresh its token${tail}`;
            break;
          case 'host_unsupported':
            text = `${missingToolText(n.text, n.host)}${tail}`;
            break;
          case 'no_credentials':
            text = n.host === LOCAL_HOST ? null : `No Claude credentials on ${n.host}${tail}`;
            break;
          default:
            text = `Couldn't reach ${n.host}${tail}`;
        }
        if (text) lines.push({ kind: 'host_note', tone: 'muted', glyph: '', text });
      }
      break;
  }

  const usage = snapshot.usage;
  const fetchedAt = snapshot.fetched_at;
  if (usage && fetchedAt !== null) {
    // Past-reset lines, one per window whose reset came after the last check.
    for (const kind of ['5h', 'weekly'] as const) {
      const w = windowOf(usage, kind);
      if (!w || w.resets_at === null || now <= w.resets_at) continue;
      const at = kind === '5h' ? clock(w.resets_at, locale, timeZone) : weekdayClock(w.resets_at, locale, timeZone);
      lines.push({
        kind: 'past_reset',
        tone: 'muted',
        glyph: '',
        window: kind,
        text: `Window reset at ${at} after the last check. Last known: ${leftPct(w)}% left at ${clock(fetchedAt, locale, timeZone)}.`,
      });
    }
    const age = now - fetchedAt;
    if (age > FRESH_SECS) {
      const ageText = `${formatDuration(age)} old`;
      const tailNext = statusCarriesNextTry ? '' : nextTry;
      const text = failed ? `${ageText} — last check failed: ${reason}.${tailNext}` : `${ageText}.${tailNext}`;
      lines.unshift({ kind: 'age', tone: 'muted', glyph: '◷', text });
    }
  }

  return { lines, checking: false, copyDetail: snapshot.detail };
}
