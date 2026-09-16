// The pure model behind the Hosts view (spec: "The Hosts view", "Staleness
// and failure", "Ergonomic risks to guard" in
// docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md).
// Grouping and its stable order, session counts, the one attention mark per
// host, the compact usage text for a group header, the endpoint-outage
// banner, and the plain-language consequence copy for the destructive
// confirms. Everything takes `now` (unix seconds) so tests are deterministic.
import type { HostRow } from './hosts';
import { accountLabel, type AccountRow } from './accounts';
import type { SessionRow } from './sessions';
import type { AccountUsageSnapshot, UsageStatus, UsageWindow } from './account_usage_store';
import type { HookHealth } from './hook_health';
import { formatAge } from './hook_health';
import {
  checkedAgo,
  clock,
  formatResetShort,
  freshness,
  httpHint,
  leftPct,
  type Freshness,
  type UsageWindowKind,
} from './account_usage';

// ── grouping ──

export const NO_ACCOUNT_LABEL = 'No Claude account';

export interface HostGroup {
  /** The account uuid, or `''` for the "No Claude account" group. */
  key: string;
  accountUuid: string | null;
  /** `null` for the no-account group, or a uuid the accounts store lacks. */
  account: AccountRow | null;
  label: string;
  hosts: HostRow[];
}

const byText = (a: string, b: string) =>
  a.localeCompare(b, 'en', { sensitivity: 'base' }) || (a < b ? -1 : a > b ? 1 : 0);

/**
 * Hosts grouped by Claude account in a STABLE order: accounts alphabetically
 * by `accountLabel`, the "No Claude account" group last, hosts alphabetically
 * within a group. Never by headroom. An account no host is logged in to has
 * nothing to select, so it gets no group.
 */
export function groupHostsByAccount(
  hosts: readonly HostRow[],
  accounts: readonly AccountRow[],
): HostGroup[] {
  const accountByUuid = new Map(accounts.map((a) => [a.uuid, a]));
  const groups = new Map<string, HostGroup>();
  for (const h of hosts) {
    const uuid = h.account_uuid || null;
    const key = uuid ?? '';
    let g = groups.get(key);
    if (!g) {
      const account = uuid ? (accountByUuid.get(uuid) ?? null) : null;
      const label = uuid ? (account ? accountLabel(account) : uuid.slice(0, 8)) : NO_ACCOUNT_LABEL;
      g = { key, accountUuid: uuid, account, label, hosts: [] };
      groups.set(key, g);
    }
    g.hosts.push(h);
  }
  const out = [...groups.values()];
  for (const g of out) g.hosts.sort((a, b) => byText(a.alias, b.alias));
  out.sort((a, b) => {
    if (a.key === '' || b.key === '') return a.key === '' ? 1 : -1;
    return byText(a.label, b.label) || byText(a.key, b.key);
  });
  return out;
}

/** Groups narrowed to hosts matching `query` (alias, ssh alias, account label or email). */
export function filterGroups(groups: readonly HostGroup[], query: string): HostGroup[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...groups];
  const out: HostGroup[] = [];
  for (const g of groups) {
    const groupHit =
      g.label.toLowerCase().includes(q) || (g.account?.email ?? '').toLowerCase().includes(q);
    const hosts = groupHit
      ? g.hosts
      : g.hosts.filter(
          (h) => h.alias.toLowerCase().includes(q) || (h.ssh_alias ?? '').toLowerCase().includes(q),
        );
    if (hosts.length > 0) out.push({ ...g, hosts });
  }
  return out;
}

/** The other hosts logged in to `host`'s account, alphabetically. */
export function sharedWith(host: HostRow, hosts: readonly HostRow[]): string[] {
  if (!host.account_uuid) return [];
  return hosts
    .filter((h) => h.alias !== host.alias && h.account_uuid === host.account_uuid)
    .map((h) => h.alias)
    .sort(byText);
}

// ── session counts ──

export interface SessionCounts {
  total: number;
  working: number;
  blocked: number;
  /** `6 ⚡2 ⏸1` (zero parts omitted). */
  text: string;
  title: string;
}

export function sessionCounts(alias: string, rows: readonly SessionRow[]): SessionCounts {
  let total = 0;
  let working = 0;
  let blocked = 0;
  for (const s of rows) {
    // External rows are Claude sessions running outside fleet: not counted.
    if (s.host_alias !== alias || s.kind === 'external') continue;
    total++;
    if (s.claude_status === 'working') working++;
    else if (s.claude_status === 'blocked') blocked++;
  }
  const parts = [String(total)];
  if (working) parts.push(`⚡${working}`);
  if (blocked) parts.push(`⏸${blocked}`);
  const title = `${total} session${total === 1 ? '' : 's'}, ${working} working, ${blocked} blocked`;
  return { total, working, blocked, text: parts.join(' '), title };
}

// ── attention ──

/** Numeric segments of a version string (`2.1.145 (Claude Code)` → [2,1,145]). */
function versionParts(v: string | null): number[] | null {
  const m = /\d+(?:\.\d+)*/.exec(v ?? '');
  return m ? m[0].split('.').map(Number) : null;
}

/** Negative when `a` is older than `b`; `0` when either is unparseable. */
export function compareVersions(a: string | null, b: string | null): number {
  const pa = versionParts(a);
  const pb = versionParts(b);
  if (!pa || !pb) return 0;
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/** The newest `claude_version` in the fleet, or null. */
export function newestClaudeVersion(hosts: readonly HostRow[]): string | null {
  let best: string | null = null;
  for (const h of hosts) {
    if (!versionParts(h.claude_version)) continue;
    if (best === null || compareVersions(h.claude_version, best) > 0) best = h.claude_version;
  }
  return best;
}

export type AttentionKind = 'token_missing' | 'hooks_missing' | 'hooks_stale' | 'claude_old';

export interface HostAttention {
  kind: AttentionKind;
  glyph: string;
  /** The tooltip explaining the mark. */
  title: string;
}

/**
 * At most ONE attention mark per host, strongest first: no control-API token
 * (so no hooks either), hooks installed but silent, then a Claude Code older
 * than the newest in the fleet. Token-derived marks wait for `tokensLoaded`
 * so a slow token fetch never flashes a false alarm.
 */
export function hostAttention(args: {
  host: HostRow;
  hasToken: boolean;
  tokensLoaded: boolean;
  hook: HookHealth;
  sessionCount: number;
  newestClaude: string | null;
}): HostAttention | null {
  const { host, hasToken, tokensLoaded, hook, sessionCount, newestClaude } = args;
  if (tokensLoaded && !hasToken) {
    if (hook.state === 'seen') {
      return {
        kind: 'token_missing',
        glyph: '🔑',
        title: `${host.alias} has no control-API token (it may have been revoked), so its fleet hooks can no longer report. Provision hosts to mint one.`,
      };
    }
    return {
      kind: 'hooks_missing',
      glyph: '🔑',
      title: `Fleet hooks are not installed on ${host.alias}: it has no control-API token. Provision hosts to install them.`,
    };
  }
  if (tokensLoaded && hook.state === 'never_seen' && sessionCount > 0) {
    return {
      kind: 'hooks_stale',
      glyph: '⚠',
      title: `Fleet hooks are installed on ${host.alias}, but none of its sessions has reported a finished turn. The hooks may be stale — re-provision the host.`,
    };
  }
  if (newestClaude && host.claude_version && compareVersions(host.claude_version, newestClaude) < 0) {
    return {
      kind: 'claude_old',
      glyph: '⬆',
      title: `Claude Code ${host.claude_version} on ${host.alias} is older than ${newestClaude}, the newest in the fleet.`,
    };
  }
  return null;
}

/** What a list row shows besides the host itself. */
export interface HostRowInfo {
  counts: SessionCounts;
  attention: HostAttention | null;
}

// ── compact usage (group header) ──

export interface CompactWindow {
  /** `91% left`, `~62% left`, `? left`, `—`, `checking…`. */
  left: string;
  /** `resets 17:05` / `resets Thu 09:00`; null when the number is withheld. */
  reset: string | null;
  freshness: Freshness | 'unknown';
}

/**
 * One window for a compact surface: % left AND its reset together (the
 * user's equal-weight decision), `~` when stale, `? left` when expired.
 */
export function compactWindow(
  kind: UsageWindowKind,
  snapshot: AccountUsageSnapshot | null,
  now: number,
  locale?: string,
  timeZone?: string,
): CompactWindow {
  if (!snapshot || (snapshot.status === 'never_fetched' && !snapshot.usage)) {
    return { left: 'checking…', reset: null, freshness: 'unknown' };
  }
  const usage = snapshot.usage;
  const win: UsageWindow | null = usage ? (kind === '5h' ? usage.five_hour : usage.seven_day) : null;
  if (!win) return { left: '—', reset: null, freshness: 'unknown' };
  const fr = freshness(kind, snapshot.fetched_at, win.resets_at, now);
  if (fr === 'expired') return { left: '? left', reset: null, freshness: fr };
  const left = `${fr === 'stale' ? '~' : ''}${leftPct(win)}% left`;
  const reset = win.resets_at === null ? null : formatResetShort(kind, win.resets_at, now, locale, timeZone);
  return { left, reset, freshness: fr };
}

export interface FreshnessMark {
  /** `2m`, `◷ 14m`, `?`, `…`. */
  mark: string;
  state: 'fresh' | 'stale' | 'expired' | 'checking';
  title: string;
}

/** How old an account's numbers are, as one short mark for a group header. */
export function freshnessMark(snapshot: AccountUsageSnapshot | null, now: number): FreshnessMark {
  if (!snapshot || (snapshot.status === 'never_fetched' && !snapshot.usage)) {
    return { mark: '…', state: 'checking', title: 'checking…' };
  }
  const fetchedAt = snapshot.fetched_at;
  if (fetchedAt === null || !snapshot.usage) {
    return { mark: '?', state: 'expired', title: 'No usage known for this account yet' };
  }
  const u = snapshot.usage;
  const states = [
    u.five_hour ? freshness('5h', fetchedAt, u.five_hour.resets_at, now) : null,
    u.seven_day ? freshness('weekly', fetchedAt, u.seven_day.resets_at, now) : null,
  ].filter((s): s is Freshness => s !== null);
  const age = formatAge(now - fetchedAt);
  const ago = checkedAgo(fetchedAt, now);
  if (states.includes('expired') || states.length === 0) {
    return { mark: '?', state: 'expired', title: `${ago} — too old to rely on` };
  }
  if (states.includes('stale')) return { mark: `◷ ${age}`, state: 'stale', title: `${ago} — stale` };
  return { mark: age, state: 'fresh', title: ago };
}

// ── endpoint outage banner ──

/**
 * Statuses that say nothing about Anthropic's endpoint: no attempt yet, or
 * the host side failed before a request (offline, no credentials file,
 * missing curl/python3). An account with no host lands in `no_online_host`
 * forever, so these must not keep the banner away.
 */
const ENDPOINT_BLIND: ReadonlySet<UsageStatus> = new Set<UsageStatus>([
  'never_fetched',
  'no_online_host',
  'no_credentials',
  'host_unsupported',
]);

export interface EndpointOutage {
  /** `Usage unavailable since 13:10. Anthropic's usage endpoint … unaffected.` */
  text: string;
  /** The earliest `next_try_at` still ahead, or null. */
  nextTryAt: number | null;
  /** `status` and `detail` lines only — never a token. */
  copyDetails: string;
  accountUuids: string[];
}

/**
 * The ONE banner for a dead usage endpoint: shown when every account whose
 * last attempt reached Anthropic got `unavailable`. "Since" is the newest
 * successful check (the endpoint has failed at least since then); omitted
 * when there was none.
 */
export function endpointOutage(
  snapshots: readonly AccountUsageSnapshot[],
  now: number,
  locale?: string,
  timeZone?: string,
): EndpointOutage | null {
  const considered = snapshots.filter((s) => !ENDPOINT_BLIND.has(s.status));
  if (considered.length === 0 || considered.some((s) => s.status !== 'unavailable')) return null;
  let since: number | null = null;
  let hint = '';
  let nextTryAt: number | null = null;
  for (const s of considered) {
    if (s.fetched_at !== null && (since === null || s.fetched_at > since)) since = s.fetched_at;
    hint ||= httpHint(s.detail);
    if (s.next_try_at > now && (nextTryAt === null || s.next_try_at < nextTryAt)) nextTryAt = s.next_try_at;
  }
  const text =
    `Usage unavailable${since !== null ? ` since ${clock(since, locale, timeZone)}` : ''}. ` +
    `Anthropic's usage endpoint returned an unexpected response${hint ? ` (${hint})` : ''}. ` +
    "It's undocumented and may have changed. Sessions are unaffected.";
  const lines = new Set(
    considered.map((s) => `status: ${s.status}${s.detail ? `\ndetail: ${s.detail}` : ''}`),
  );
  return {
    text,
    nextTryAt,
    copyDetails: [...lines].join('\n'),
    accountUuids: considered.map((s) => s.account_uuid),
  };
}

// ── destructive confirms ──

/**
 * What `Remove host…` does, verified against `Store::delete_host`
 * (src-tauri/src/store/hosts_accounts.rs): in one transaction it deletes the
 * host's session rows with their timeline events and the messages addressed
 * to them, its control-API token, worktree rows, parent fingerprints and
 * estimated-usage history. `remove_host` runs no SSH, so tmux on the host is
 * untouched. `local` is never removed.
 */
export function removeHostMessage(alias: string, sessionRows: number): string {
  const rows =
    sessionRows === 0
      ? 'It has no session rows.'
      : `Fleet deletes its ${sessionRows} session row${sessionRows === 1 ? '' : 's'} from its database, with their timelines and the messages sent to them.`;
  return (
    `${rows} The host's control-API token stops working, and its worktree records and usage history are deleted. ` +
    `The tmux sessions on ${alias} are not touched and keep running; fleet just stops tracking them. ` +
    'Adding the host again later starts fresh — the deleted history does not come back.'
  );
}

/**
 * What `Rotate token…` does, verified against `rotate_host_token` →
 * `provision_host_with_token(rotate = true)`: mint a token, re-provision the
 * host's `~/.claude.json` MCP entry and `~/.claude/settings.json` hooks with
 * it, and only then replace the stored token (a failed provision keeps the
 * old one).
 */
export function rotateTokenMessage(alias: string): string {
  return (
    `Fleet mints a new control-API token for ${alias} and rewrites its entries in ~/.claude.json and ~/.claude/settings.json there. ` +
    'The old token stops working. Claude Code sessions already running on the host may keep the old token — and lose fleet tools and hooks — until they are restarted. ' +
    `If ${alias} can't be reached, nothing changes.`
  );
}
