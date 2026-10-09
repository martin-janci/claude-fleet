import { writable, derived } from 'svelte/store';
import { createRowStore } from './row_store';
import { invokeCmd, type Result } from './result';
import { readPref, writePref } from './prefs';

export interface HostRow {
  alias: string;
  ssh_alias: string | null;
  reachable: boolean;
  claude_version: string | null;
  tmux_version: string | null;
  hidden: boolean;
  last_pinged_at: number | null;
  account_uuid: string | null;
  provisioned: boolean;
  /** `"ssh"` | `"agent"` — see `HostRow::transport` in `crates/fleet-core/src/store/rows.rs`. */
  transport: 'ssh' | 'agent';
  /** The host's org (work graph M5): its per-host token's boundary. */
  org_id?: number | null;
  /** When claude/tmux versions were last read from the host; null = never. Absent from an older hub. */
  claude_version_at?: number | null;
  /** Health sample from the last reachable probe; absent from an older hub. */
  disk_home_free_kb?: number | null;
  disk_home_total_kb?: number | null;
  disk_tmp_free_kb?: number | null;
  load_1m?: number | null;
  mem_avail_kb?: number | null;
  uptime_secs?: number | null;
  health_at?: number | null;
  /** Last hook accepted from this host's own token. */
  last_hook_at?: number | null;
  /** The fleet-agent version its last hello reported (agent hosts). */
  agent_version?: string | null;
  /** When provision_hosts last completed on this host; absent from an older hub. */
  provisioned_at?: number | null;
  /** Provisioned, but with content older than this build ships (or unknown). */
  provision_stale?: boolean;
  /**
   * How many sessions on this host nobody has claimed (multi-user M1).
   *
   * An `unclaimed` session is one fleet did not start — a tmux session the
   * reconcile pass found — and spec §4.3 says the only thing an out-of-scope
   * caller may learn about one is a **count**. So this is the count, and there
   * are deliberately no rows behind it anywhere: rendering them would be the
   * metadata leak the number exists to avoid (rule 6).
   *
   * **Frequently absent, and `null` is not zero** (R5-d): on a hub with more
   * than one person the backend serves `null` rather than a number, because a
   * zero is itself a claim about the host that would let a second person infer
   * one. A surface must render nothing at all for `null` — not `0`, not a dash
   * — and `fleet-hub session unclaimed` is then the only way a human sees the
   * counts. Absent, too, from a hub older than M1.
   */
  unclaimed_sessions?: number | null;
  /** Harnesses the asset catalog syncs here (multi-harness F3a): null or absent
   *  = auto (Claude, plus Codex where a scan finds it); a list always holds claude. */
  harnesses?: string[] | null;
  /** What the last provisioning warned about, when it delivered the content but
   *  degraded part way (the ag launcher did not install, say). Cleared by the
   *  next clean run; absent from an older hub. */
  provision_warning?: string | null;
  /** Credential variables set on the host that outrank its /login, by name only
   *  (ANTHROPIC_API_KEY, CLAUDE_CODE_USE_BEDROCK, …), in Claude Code's precedence
   *  order. null or absent = unknown (never sampled, or an older hub); [] = none. */
  auth_overrides?: string[] | null;
  /** The host's Claude login profiles (`~/.claude-profiles/<name>`, docs/accounts.md),
   *  each with the account it is logged into; account_uuid null = not logged in yet.
   *  null or absent = never read (or an older hub). */
  claude_profiles?: HostProfile[] | null;
  /** Probe facts for the Hosts page (Orbit Fleet 4.6); all absent from an older hub. */
  cpu_count?: number | null;
  mem_total_kb?: number | null;
  /** Boot time the host states, unix seconds. */
  boot_at?: number | null;
  /** Round trip of an empty command over SSH; null for `local` or when not timed. */
  latency_ms?: number | null;
  /** Disk fleet's worktrees hold on the host, and when that was last asked. */
  worktree_kb?: number | null;
  worktree_at?: number | null;
  /** Which agent CLIs the host has on its PATH (Orbit Fleet 12.4), in the
   *  health checklist's order; null when never sampled. Absent from an older hub. */
  agents_on_path?: string[] | null;
  /** When the host last answered a probe (review r13). Unlike `last_pinged_at`,
   *  which a failed probe stamps too, it stays put while the host is offline.
   *  null = never answered; absent from an older hub. */
  last_reachable_at?: number | null;
  /** Why the last probe failed: its IpcError code and a short message. Cleared
   *  by the next probe the host answers; absent from an older hub. */
  last_probe_error_code?: string | null;
  last_probe_error?: string | null;
}

/** One login profile on a host. */
export interface HostProfile {
  name: string;
  account_uuid?: string | null;
  email?: string | null;
}

/** The volatile half of a host row, as `host:pinged` carries it. */
export interface HostHealth {
  disk_home_free_kb: number | null;
  disk_home_total_kb: number | null;
  disk_tmp_free_kb: number | null;
  load_1m: number | null;
  mem_avail_kb: number | null;
  uptime_secs: number | null;
  health_at: number | null;
  /** Absent from an older hub's ping (the row keeps what it had). */
  auth_overrides?: string[] | null;
  /** Orbit Fleet 4.6; absent from an older hub's ping. */
  cpu_count?: number | null;
  mem_total_kb?: number | null;
  boot_at?: number | null;
  latency_ms?: number | null;
  worktree_kb?: number | null;
  /** Orbit Fleet 12.4; absent from an older hub's ping. */
  agents_on_path?: string[] | null;
}

export interface SshHost {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
}

const rows = createRowStore<HostRow, string>({
  key: (h) => h.alias,
  // `removeHost()` (optimistic) and the `host:removed` event both delete a
  // row; a `host:probed` event still in flight for that alias would otherwise
  // re-insert the dead host. Entries expire so re-adding a host with the
  // same alias isn't blocked for long.
  tombstoneMs: 5000,
});
export const hosts = rows.store;
export const resetTombstonesForTests = rows.resetTombstonesForTests;

/** O(1) alias -> host lookup, derived once per `hosts` change. Consumers
 *  that previously did a linear `$hosts.find` should read this instead. */
export const hostByAlias = derived(hosts, ($h) => new Map($h.map((h) => [h.alias, h])));

/** Whether a new session / project may target `alias`: visible, and
 *  reachable unless it is `local`. The rule behind HostChips' `disabled`
 *  and every new-thing dialog's host choice. */
export function isPickableHost(list: readonly HostRow[], alias: string | null | undefined): alias is string {
  return !!alias && list.some((h) => h.alias === alias && !h.hidden && (h.reachable || h.alias === 'local'));
}

/** The host a dialog starts on when nothing remembered is pickable: `local`
 *  when it is, else the first pickable host. A hub started with
 *  `hub.local_host=false` hides its `local` row, and defaulting to it there
 *  only earns "host local is disabled on this hub" on submit. `'local'` when
 *  nothing at all is pickable (the submit then says why). */
export function defaultHost(list: readonly HostRow[]): string {
  if (isPickableHost(list, 'local')) return 'local';
  return list.find((h) => isPickableHost(list, h.alias))?.alias ?? 'local';
}

// Sidebar host filter — `'all'` shows sessions from every host, otherwise
// the value is a specific `alias`. Persisted across restarts.
const isString = (v: unknown): v is string => typeof v === 'string';
export const hostFilter = writable<string>(readPref('host-filter', 'all', isString));
hostFilter.subscribe((v) => writePref('host-filter', v));

/** The host filter as it applies: a remembered alias that is no longer a
 *  visible host (removed, or hidden) reads as `'all'` — otherwise no host
 *  pill is active and the list is silently empty. Before the hosts load
 *  (an empty list) the remembered value stands. */
export function effectiveHostOf(filter: string, list: readonly Pick<HostRow, 'alias' | 'hidden'>[]): string {
  if (filter === 'all' || list.length === 0) return filter;
  return list.some((h) => h.alias === filter && !h.hidden) ? filter : 'all';
}
export const effectiveHostFilter = derived([hostFilter, hosts], ([f, list]) => effectiveHostOf(f, list));

export async function loadHosts(): Promise<Result<HostRow[]>> {
  const token = rows.beginList();
  const r = await invokeCmd<HostRow[]>('list_hosts');
  // A probe or a remove that landed while the list was in flight wins.
  if (r.ok) rows.applyList(r.value, token);
  return r;
}

export async function discoverHosts(): Promise<Result<SshHost[]>> {
  return invokeCmd<SshHost[]>('discover_hosts');
}

export async function addHost(
  alias: string,
  sshAlias: string,
): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('add_host', {
    args: { alias, ssh_alias: sshAlias },
  });
  if (r.ok) {
    // An explicit re-add overrides any lingering tombstone from a recent
    // removeHost() of the same alias.
    rows.accept(r.value);
  }
  return r;
}

export async function probeHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('probe_host', { args: { alias } });
  // A command result is authoritative — clear any stale tombstone first.
  if (r.ok) {
    rows.accept(r.value);
  }
  return r;
}

export async function deleteHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('remove_host', { args: { alias } });
  if (r.ok) removeHost(r.value.alias);
  return r;
}

export async function hideHost(
  alias: string,
  hidden: boolean,
): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('hide_host', { args: { alias, hidden } });
  if (r.ok) {
    rows.accept(r.value);
  }
  return r;
}

/** Codex's place in a host's harness set: `auto` = `harnesses` null (Codex
 *  where the scan finds it), `on` / `off` = an explicit list. */
export type HarnessMode = 'auto' | 'on' | 'off';

export function codexModeOf(h: Pick<HostRow, 'harnesses'>): HarnessMode {
  if (h.harnesses == null) return 'auto';
  return h.harnesses.includes('codex') ? 'on' : 'off';
}

/** The `harnesses` value a mode stores; Claude is always in an explicit list. */
export function harnessesFor(mode: HarnessMode): string[] | null {
  if (mode === 'auto') return null;
  return mode === 'on' ? ['claude', 'codex'] : ['claude'];
}

export async function setHostHarnesses(
  alias: string,
  harnesses: string[] | null,
): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('catalog_set_host_harnesses', {
    args: { host_alias: alias, harnesses },
  });
  if (r.ok) rows.accept(r.value);
  return r;
}

function removeHost(alias: string): void {
  rows.remove(alias);
}

/** One backend host event, as delivered by `events.ts`. */
export type HostEvent =
  | { type: 'added' | 'probed'; row: HostRow }
  /** A probe that changed nothing but the stamp — patched onto the row we hold. */
  | {
      type: 'pinged';
      alias: string;
      last_pinged_at: number;
      reachable: boolean;
      claude_version_at?: number | null;
      health?: HostHealth | null;
    }
  | { type: 'removed'; alias: string };

/** Apply a burst of host events in ONE store update, in order (see
 *  `applySessionEvents` for the rationale). */
export function applyHostEvents(events: readonly HostEvent[]): void {
  if (events.length === 0) return;
  hosts.update((arr) => {
    let next = arr;
    for (const ev of events) {
      if (ev.type === 'removed') {
        next = rows.removeFrom(next, ev.alias);
      } else if (ev.type === 'pinged') {
        // A partial event, so it patches rather than replaces — and only a
        // row we already hold: a heartbeat is never the first we hear of a
        // host, and inventing one from three fields would show a host with
        // no versions and no transport.
        const have = next.find((h) => h.alias === ev.alias);
        if (have) {
          next = rows.mergeInto(next, {
            ...have,
            last_pinged_at: ev.last_pinged_at,
            // A ping is a probe that changed nothing else: answered, it moved
            // `last_reachable_at` with the stamp (store/reconcile.rs).
            last_reachable_at: ev.reachable ? ev.last_pinged_at : have.last_reachable_at,
            claude_version_at: ev.claude_version_at ?? have.claude_version_at,
            reachable: ev.reachable,
            // The health sample rides the ping (it moves every pass).
            ...(ev.health ?? {}),
          });
        }
      } else {
        next = rows.mergeInto(next, ev.row);
      }
    }
    return next;
  });
}
