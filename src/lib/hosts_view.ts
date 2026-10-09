// The pure model behind the Hosts view (spec: "The Hosts view", "Staleness
// and failure", "Ergonomic risks to guard" in
// docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md).
// Grouping and its stable order, session counts, the one attention mark per
// host, the compact usage text for a group header, the endpoint-outage
// banner, and the plain-language consequence copy for the destructive
// confirms. Everything takes `now` (unix seconds) so tests are deterministic.
import type { HostRow } from './hosts';
import type { OfIconName } from './kit/icons';
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
  /** `6 · 2 working · 1 needs you` (zero parts omitted): the manual's
   *  status words, no glyphs. A blocked Claude asks for a permission. */
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
  if (working) parts.push(`${working} working`);
  if (blocked) parts.push(`${blocked} needs you`);
  const title = `${total} session${total === 1 ? '' : 's'}, ${working} working, ${blocked} need${blocked === 1 ? 's' : ''} you`;
  return { total, working, blocked, text: parts.join(' · '), title };
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

/** A version stamp the badge may trust: read from the host within `maxAgeSecs`. */
export function versionFresh(host: HostRow, now: number, maxAgeSecs: number): boolean {
  return host.claude_version_at != null && now - host.claude_version_at <= maxAgeSecs;
}

/** The newest FRESH `claude_version` in the fleet, or null. */
export function newestClaudeVersion(hosts: readonly HostRow[], now: number, maxAgeSecs: number): string | null {
  let best: string | null = null;
  for (const h of hosts) {
    if (!versionFresh(h, now, maxAgeSecs) || !versionParts(h.claude_version)) continue;
    if (best === null || compareVersions(h.claude_version, best) > 0) best = h.claude_version;
  }
  return best;
}

export type AttentionKind =
  | 'token_missing'
  | 'hooks_missing'
  | 'hooks_stale'
  | 'provision_warning'
  | 'provision_stale'
  | 'auth_override'
  | 'disk_low'
  | 'agent_old'
  | 'claude_old';

export interface HostAttention {
  kind: AttentionKind;
  /** The mark, from the manual's icon set. */
  icon: OfIconName;
  /** The tooltip explaining the mark. */
  title: string;
}

/**
 * At most ONE attention mark per host, strongest first: no control-API token
 * (so no hooks either), hooks installed but silent, a credential variable
 * outranking the host's /login, a home filesystem almost full, a fleet-agent behind the hub, then a Claude Code older than the
 * newest in the fleet. Token-derived marks wait for `tokensLoaded` so a slow
 * token fetch never flashes a false alarm.
 */
export function hostAttention(args: {
  host: HostRow;
  hasToken: boolean;
  tokensLoaded: boolean;
  hook: HookHealth;
  sessionCount: number;
  newestClaude: string | null;
  /** Unix seconds; the version stamp is judged against it. */
  now: number;
  /** `health.version_max_age_secs`: a stamp older than this earns no `claude_old` mark. */
  versionMaxAgeSecs: number;
  /** `health.disk_low_pct`: used percent of `$HOME` at which `disk_low` fires. */
  diskLowPct: number;
  /** The hub's (or this app's) version; null until known. */
  hubVersion: string | null;
}): HostAttention | null {
  const { host, hasToken, tokensLoaded, hook, sessionCount, newestClaude, now, versionMaxAgeSecs, diskLowPct, hubVersion } =
    args;
  if (tokensLoaded && !hasToken) {
    if (hook.state === 'seen') {
      return {
        kind: 'token_missing',
        icon: 'key',
        title: `${host.alias} has no control-API token (it may have been revoked), so its fleet hooks can no longer report. Provision hosts to mint one.`,
      };
    }
    return {
      kind: 'hooks_missing',
      icon: 'key',
      title: `Fleet hooks are not installed on ${host.alias}: it has no control-API token. Provision hosts to install them.`,
    };
  }
  if (tokensLoaded && hook.state === 'never_seen' && sessionCount > 0) {
    return {
      kind: 'hooks_stale',
      icon: 'warning',
      title: `Fleet hooks are installed on ${host.alias}, but none of its sessions has reported a finished turn. The hooks may be stale — re-provision the host.`,
    };
  }
  // A provisioning that DELIVERED the content but degraded part way. It also
  // reads provision_stale (its fingerprint was cleared so it is retried), so
  // this has to outrank that branch — "provisioned with an older fleet" would
  // be the wrong reason, and the real one was previously only a tracing::warn!
  // nobody saw.
  if (host.provision_warning) {
    return {
      kind: 'provision_warning',
      icon: 'warning',
      title: `${host.alias}: the last provisioning did not finish cleanly — ${host.provision_warning}. Fleet will retry it; to retry now, fleet-hub provision --host ${host.alias} --content-only.`,
    };
  }
  // hosts F1: every host ran skills from 15 hub upgrades ago and nothing said so.
  if (host.provision_stale) {
    return {
      kind: 'provision_stale',
      icon: 'recreate',
      title: `${host.alias} was provisioned with an older fleet (content differs from this build): re-provision it — fleet-hub provision --host ${host.alias} --content-only.`,
    };
  }
  // Multi-account groundwork: a credential variable in the host's shell or tmux
  // environment outranks its /login, so sessions there bill that credential
  // while the Hosts view shows the logged-in account.
  const overrides = host.auth_overrides ?? [];
  if (overrides.length > 0) {
    return {
      kind: 'auth_override',
      icon: 'key',
      title: `${host.alias}: ${overrides.join(', ')} ${overrides.length === 1 ? 'is' : 'are'} set in its shell or tmux environment and outrank${overrides.length === 1 ? 's' : ''} the /login account, so new Claude sessions there use that credential instead. Unset it (and restart tmux) to use the login.`,
    };
  }
  // hosts F4 / ux F-14: two hosts sat at 98 % disk with no signal anywhere.
  // A stale sample (the host stopped answering) earns no mark: the disk may
  // have been cleaned up since.
  const disk = diskMeter(host);
  if (disk && healthSampleFresh(host, now) && disk.pct >= diskLowPct) {
    return {
      kind: 'disk_low',
      icon: 'disk',
      title: `${host.alias} is at ${disk.pct}% disk in $HOME (${gb(host.disk_home_free_kb ?? 0)} free): transcripts, worktrees and moves onto it will fail with ENOSPC.`,
    };
  }
  // hosts F5: an agent older than the hub silently lacks features. One ahead
  // (after a hub rollback) or unparseable earns no mark.
  if (
    host.transport === 'agent' &&
    hubVersion &&
    host.agent_version &&
    compareVersions(host.agent_version, hubVersion) < 0
  ) {
    return {
      kind: 'agent_old',
      icon: 'upgrade',
      title: `fleet-agent ${host.agent_version} on ${host.alias}, hub ${hubVersion}: upgrade the agent (fleet-agent install with the existing token, then systemctl restart fleet-agent).`,
    };
  }
  // ux F-13: only a version read from the host recently earns the mark; a
  // provisioning-day cache stamped with today's ping was wrong on 3 of 4.
  if (
    newestClaude &&
    host.claude_version &&
    versionFresh(host, now, versionMaxAgeSecs) &&
    compareVersions(host.claude_version, newestClaude) < 0
  ) {
    return {
      kind: 'claude_old',
      icon: 'upgrade',
      title: `Claude Code ${host.claude_version} on ${host.alias} is older than ${newestClaude}, the newest in the fleet (checked ${formatAge(now - (host.claude_version_at ?? now))} ago).`,
    };
  }
  return null;
}

/** What a list row shows besides the host itself. */
export interface HostRowInfo {
  counts: SessionCounts;
  attention: HostAttention | null;
}

// ── host health (host identity & health, task 3) ──

export interface DiskMeter {
  pct: number;
  /** `98% · 3.4 GB free` */
  text: string;
  level: 'ok' | 'warn' | 'crit';
}

function gb(kb: number): string {
  const g = kb / (1024 * 1024);
  return g >= 10 ? `${Math.round(g)} GB` : `${g.toFixed(1)} GB`;
}

/** Used percent of `$HOME`'s filesystem; null until sampled. */
export function diskMeter(h: HostRow): DiskMeter | null {
  const free = h.disk_home_free_kb ?? null;
  const total = h.disk_home_total_kb ?? null;
  if (free === null || total === null || total <= 0) return null;
  const pct = Math.min(100, Math.max(0, Math.round(((total - free) * 100) / total)));
  const level = pct >= 95 ? 'crit' : pct >= 90 ? 'warn' : 'ok';
  return { pct, text: `${pct}% · ${gb(free)} free`, level };
}

/**
 * A health sample older than this is stale: its disk reading earns no
 * `disk_low` mark. The same value as the Rust `health::HEALTH_SAMPLE_FRESH_SECS`.
 */
export const HEALTH_SAMPLE_FRESH_SECS = 3600;

/** The host's health sample was taken within `HEALTH_SAMPLE_FRESH_SECS`. */
export function healthSampleFresh(h: HostRow, now: number): boolean {
  return h.health_at != null && now - h.health_at <= HEALTH_SAMPLE_FRESH_SECS;
}

function days(secs: number): string {
  return secs >= 86400 ? `${Math.floor(secs / 86400)}d` : formatAge(secs);
}

/** One line for the Health block; only the parts the host answered. */
export function healthLine(h: HostRow, now: number): string {
  if (h.health_at == null) return 'not sampled yet';
  const parts: string[] = [];
  const disk = diskMeter(h);
  if (disk) parts.push(`disk ${disk.text}`);
  if (h.load_1m != null) parts.push(`load ${h.load_1m.toFixed(1)}`);
  if (h.uptime_secs != null) parts.push(`up ${days(h.uptime_secs)}`);
  if (h.transport === 'agent' && h.agent_version) parts.push(`agent ${h.agent_version}`);
  if (!parts.length) parts.push('nothing readable');
  const stale = healthSampleFresh(h, now) ? '' : ' (stale)';
  parts.push(`sampled ${formatAge(now - h.health_at)} ago${stale}`);
  return parts.join(' · ');
}

/** `checked 2h ago` for the claude/tmux version stamp. */
export function versionAge(h: HostRow, now: number): string {
  return h.claude_version_at == null ? 'never checked' : `checked ${formatAge(now - h.claude_version_at)} ago`;
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
  const reset = win.resets_at == null ? null : formatResetShort(kind, win.resets_at, now, locale, timeZone);
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
