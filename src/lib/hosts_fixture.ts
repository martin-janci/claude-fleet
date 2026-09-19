// Test fixture (imported only by tests): the user's real fleet shape — five
// hosts, four Claude accounts, two hosts sharing an account twice over, one
// account linked to no host, ~14 sessions on claude-fleet-trn and ~6 on
// mefistos.
import type { HostRow } from './hosts';
import type { AccountRow } from './accounts';
import type { SessionRow } from './sessions';
import type { AccountUsageSnapshot } from './account_usage_store';
import type { HostTokenInfo } from './mcp';

/** Mon 2026-09-14 14:32:00 UTC. */
export const NOW = 1789396320;
export const MIN = 60;
export const HOUR = 3600;
/** 15:10 UTC. */
export const RESET_5H = NOW + 38 * MIN;
/** Thu 09:00 UTC. */
export const RESET_WEEK = NOW + 2 * 86400 + 18 * HOUR + 28 * MIN;

export function account(uuid: string, email: string, over: Partial<AccountRow> = {}): AccountRow {
  return {
    uuid,
    email,
    display_name: null,
    organization_name: null,
    organization_uuid: null,
    seat_tier: null,
    last_seen_at: null,
    nickname: null,
    has_extra_usage: false,
    ...over,
  };
}

export const ADMIN = account('acc-admin', 'admin@32bit.sk');
export const WORK = account('acc-work', 'm.janci@32bit.sk');
export const GMAIL = account('acc-gmail', 'mj.janci@gmail.com');
export const SPARE = account('acc-spare', 'spare@32bit.sk');

export function host(alias: string, over: Partial<HostRow> = {}): HostRow {
  return {
    alias,
    ssh_alias: alias === 'local' ? null : alias,
    reachable: true,
    claude_version: '2.1.145',
    tmux_version: '3.5a',
    hidden: false,
    last_pinged_at: NOW - 2 * MIN,
    account_uuid: null,
    provisioned: true,
    transport: 'ssh',
    ...over,
  };
}

export function fleetHosts(): HostRow[] {
  return [
    host('local', { account_uuid: GMAIL.uuid }),
    host('mefistos', { account_uuid: ADMIN.uuid }),
    host('claude-fleet-htz', { account_uuid: WORK.uuid, reachable: false }),
    host('claude-fleet-trn', { account_uuid: GMAIL.uuid }),
    host('claude-fleet-oci', { account_uuid: ADMIN.uuid }),
  ];
}

export const fleetAccounts = (): AccountRow[] => [
  { ...ADMIN },
  { ...WORK },
  { ...GMAIL },
  { ...SPARE },
];

let nextId = 1;
export function session(hostAlias: string, name: string, over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    tmux_name: name,
    host_alias: hostAlias,
    project_id: null,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: null,
    lost_at: null,
    claude_session_id: null,
    claude_status: 'idle',
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null,
    turn_seq: 1,
    // A delivered Stop hook, so installed hooks read as healthy.
    last_stop_at: NOW - 5 * MIN,
    parent_session_id: null,
    tags: [],
    model: null,
    context_tokens: null,
    context_window: null,
    context_source: null,
    context_at: null,
    context_stale: false,
    tmux_pane_id: null,
    ...over,
  };
}

/** 14 on claude-fleet-trn, 6 on mefistos (`6 ⚡2 ⏸1`). */
export function fleetSessions(): SessionRow[] {
  const rows: SessionRow[] = [];
  const add = (alias: string, count: number) => {
    for (let i = 0; i < count; i++) {
      const claude_status = i < 2 ? 'working' : i === 2 ? 'blocked' : 'idle';
      rows.push(session(alias, `${alias}-s${String(i + 1).padStart(2, '0')}`, { claude_status }));
    }
  };
  add('claude-fleet-trn', 14);
  add('mefistos', 6);
  return rows;
}

export const fleetTokens = (): HostTokenInfo[] =>
  ['local', 'mefistos', 'claude-fleet-htz', 'claude-fleet-trn', 'claude-fleet-oci'].map((host_alias) => ({
    host_alias,
    mode: 'full',
    created_at: 1,
  }));

export function snapshot(uuid: string, over: Partial<AccountUsageSnapshot> = {}): AccountUsageSnapshot {
  return {
    account_uuid: uuid,
    usage: {
      five_hour: { utilization: 9, resets_at: RESET_5H },
      seven_day: { utilization: 42, resets_at: RESET_WEEK },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
    subscription: 'max',
    fetched_at: NOW - 2 * MIN,
    source_host: null,
    status: 'ok',
    detail: null,
    next_try_at: 0,
    ...over,
  };
}

export function fleetUsage(): Record<string, AccountUsageSnapshot> {
  return {
    [ADMIN.uuid]: snapshot(ADMIN.uuid, { source_host: 'mefistos' }),
    [WORK.uuid]: snapshot(WORK.uuid, { source_host: 'claude-fleet-htz', fetched_at: NOW - 14 * MIN }),
    [GMAIL.uuid]: snapshot(GMAIL.uuid, { source_host: 'claude-fleet-trn' }),
    [SPARE.uuid]: snapshot(SPARE.uuid, { usage: null, fetched_at: null, status: 'no_online_host' }),
  };
}

/** Every account the endpoint answered for returned `unavailable`. */
export function outageUsage(): Record<string, AccountUsageSnapshot> {
  const dead = (uuid: string, source: string) =>
    snapshot(uuid, {
      source_host: source,
      fetched_at: NOW - 82 * MIN, // last good check 13:10
      status: 'unavailable',
      detail: 'HTTP 404: <html>Not Found</html>',
      next_try_at: NOW + 8 * MIN, // 14:40
    });
  return {
    [ADMIN.uuid]: dead(ADMIN.uuid, 'mefistos'),
    [WORK.uuid]: dead(WORK.uuid, 'claude-fleet-htz'),
    [GMAIL.uuid]: dead(GMAIL.uuid, 'claude-fleet-trn'),
    [SPARE.uuid]: snapshot(SPARE.uuid, { usage: null, fetched_at: null, status: 'no_online_host' }),
  };
}
