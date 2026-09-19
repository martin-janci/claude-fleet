// Which fleet this window is onto, and what that costs.
//
// Standalone the desktop *is* the fleet: its own database, its own SSH
// connections, its own reconcile tick, its own control API. Paired with a
// `fleet-hub` it is a client of someone else's fleet, and three families of
// thing stop working from here:
//
//   1. **Fleet administration.** The desktop pairs as an ordinary client, and
//      a client is never the fleet's administrator: adding, removing and
//      hiding hosts, provisioning, asset sync and secrets are the hub's.
//   2. **Things about THIS machine.** The terminal's PTY, the local
//      prerequisites check, SSH tunnels, the asset-catalog git checkout.
//   3. **PARITY OR REFUSAL.** A mutation routes to the hub only where the
//      desktop's arguments map one-to-one onto the tool's parameters. Where
//      they do not — `new_session` and `repair_session` — the command refuses
//      rather than routing, because routing would have SUCCEEDED while
//      silently dropping what the user typed. A refusal is visible; a dropped
//      field is not. Nobody should later "fix" one of these refusals by
//      wiring a lossy mapping.
//
// The backend returns `E_LOCAL_ONLY` for all three, with a message naming
// where the operation does work. This module is the *other* half: the reason
// in front of the control, before the click, so that a paired desktop looks
// like a paired desktop rather than like a broken one.
import { get, writable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';

/** Mirrors the backend `HubStatus` (`src-tauri/src/commands/hub.rs`). */
export interface HubStatus {
  /** What this process is doing right now. */
  remote: boolean;
  /** The hub it is a window onto; null when standalone. */
  url: string | null;
  client_name: string | null;
  /** `full` | `readonly`, known only in the session that paired. */
  client_mode: string | null;
  /** `hub.remote_url` as stored — differs from `url` until a restart. */
  configured_url: string | null;
  configured_client_name: string | null;
  allow_plaintext: boolean;
  /** Why a configured hub is not in use, or why the one in use is risky. */
  warning: string | null;
  /** The stored configuration no longer matches the running mode. */
  restart_required: boolean;
  /**
   * Set when a hub is configured but THIS launch could not use it (no stored
   * token, a keychain that would not open, plain http without the opt-in, a
   * URL that does not parse), with the reason. The backend then owns nothing
   * — no reconcile tick, no usage poll, no control API — and refuses every
   * fleet command, rather than quietly becoming a second brain for the hub's
   * fleet. `remote` is false in this state: nothing is talking to a hub.
   */
  unavailable: string | null;
}

export const STANDALONE: HubStatus = {
  remote: false,
  url: null,
  client_name: null,
  client_mode: null,
  configured_url: null,
  configured_client_name: null,
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

/**
 * The last status the backend gave us. Standalone until told otherwise, which
 * is the safe default in one direction only — it enables things — so
 * `loadHubStatus` runs before the first render that can act on it, and a
 * FAILED reload deliberately keeps the previous value rather than falling
 * back to standalone.
 */
export const hubStatus = writable<HubStatus>({ ...STANDALONE });

export async function loadHubStatus(): Promise<Result<HubStatus>> {
  const r = await invokeCmd<HubStatus>('hub_status');
  if (r.ok && r.value) hubStatus.set(r.value);
  return r;
}

/** Redeem a pairing code. Takes effect at the next launch — the backend mode
 *  is resolved once, at startup, on purpose. */
export async function hubPair(
  url: string,
  code: string,
  allowPlaintext: boolean,
): Promise<Result<HubStatus>> {
  const r = await invokeCmd<HubStatus>('hub_pair', {
    args: { url: url.trim(), code: code.trim(), allow_plaintext: allowPlaintext },
  });
  if (r.ok && r.value) hubStatus.set(r.value);
  return r;
}

/**
 * Forget the pairing on this machine.
 *
 * This **revokes nothing**. The client row stays on the hub and the token
 * stays valid there until an operator revokes it (`fleet-hub client revoke`),
 * and a paired client is refused `revoke_client` by design — so the app could
 * not revoke even if it wanted to. Every string this module and the Settings
 * dialog show says so; anything softer would leave someone believing a stolen
 * laptop had been locked out when it had not.
 */
export async function hubDisconnect(): Promise<Result<HubStatus>> {
  const r = await invokeCmd<HubStatus>('hub_disconnect');
  if (r.ok && r.value) hubStatus.set(r.value);
  return r;
}

/**
 * Whether a client token is sitting on this machine with **no hub
 * configured** — a fleet-wide credential no launch reads.
 *
 * Pairing used to store the token before the URL, so a crash between the two
 * writes stranded one. That order is fixed (the token goes last now), but an
 * install that took the crash carries the token across the upgrade, and
 * `hub_status` never finds it: a blank URL is standalone, and the backend
 * deliberately does not query the OS keychain on every launch just in case.
 *
 * So Settings asks once, on open — the one moment a keychain prompt is
 * explicable — and offers {@link hubDisconnect}, which clears it. Deliberately
 * **not** a field of {@link HubStatus}: it is a one-off question about a legacy
 * state, not part of which fleet this window is onto, and nothing branches on
 * it but the Hub section.
 */
export async function hubStrandedToken(): Promise<Result<boolean>> {
  return invokeCmd<boolean>('hub_stranded_token');
}

// ---- what a hub client cannot do from here ---------------------------------

/**
 * The half of each reason that is specific to the action. `hubBlock` adds the
 * hub's name, so every sentence ends up naming both what and where.
 *
 * The keys are the command names wherever there is one, so a reader can find
 * the matching `backend.local_only(...)` in `src-tauri/src/commands/`.
 */
const REASONS = {
  // --- fleet administration: refused to a client by design -----------------
  add_host:
    'this desktop is a paired client, and a client is never the fleet’s administrator',
  remove_host:
    'this desktop is a paired client, and a client is never the fleet’s administrator',
  hide_host:
    'this desktop is a paired client, and a client is never the fleet’s administrator',
  discover_hosts:
    'host discovery reads this machine’s SSH config, and a paired client does not administer the fleet anyway',
  provision_hosts:
    'provisioning rewrites every host’s hook block to report to whichever app ran it, and this desktop is a paired client',
  apply_sync:
    'asset sync writes to the hosts over SSH, which a paired client does not do',
  set_secret:
    'sync secrets belong to the machine that runs the sync, and this desktop is a paired client',
  install_fleet_hook:
    'the hook points at a control API this app is not running, because a paired client runs none',
  host_tokens:
    'these are a fleet owner’s per-host tokens; this desktop is a paired client and has none',
  mcp_configure:
    'starting a second control API against a fleet that already has one is the failure remote mode exists to prevent',

  // --- things about THIS machine ------------------------------------------
  terminal:
    'the terminal attaches a local ssh/tmux process, and the hub streams no pane — open a shell and run `ssh <host>` then `tmux attach -t <session>`',
  check_local_prereqs:
    'the setup checklist is about running a fleet from this machine, which the hub is doing instead',
  tunnel_status: 'the tunnels belong to whichever process owns the fleet',
  mcp_status: 'this app runs no embedded control API while a hub owns the fleet',
  catalog_config:
    'the asset catalog is a git checkout on the machine that owns the fleet; the hub serves its asset list to any paired client (list_assets), but not the configuration and checkout this panel is built on',
  get_fleet_settings:
    'these settings drive the reconcile tick, the GC sweeper and the playbooks, which the hub runs and this app does not',
  list_account_usage:
    'this app does not poll account usage while a hub owns the fleet, so its cache stays empty',

  // --- parity or refusal ---------------------------------------------------
  new_session:
    'the hub’s new_session tool takes no kind, start command or friendly name, so routing this would have quietly dropped the label you typed — a refusal is visible, a dropped field is not',
  repair_session:
    'the hub’s repair tool always runs the explicit repair, and the desktop’s automatic pre-attach check has no counterpart, so routing this would not mean the same thing',
} as const;

export type HubAction = keyof typeof REASONS;

/** Every action this module has a reason for — the list the tests sweep. */
export const HUB_ACTIONS = Object.keys(REASONS) as HubAction[];

/**
 * Whether this process owns its fleet — standalone, and not pointed at a hub,
 * working or not. Everything that fetches or shows a fleet-owner-only panel
 * asks this rather than `!status.remote`, because a configured hub this launch
 * cannot use is not a hub client and is not standalone either: it owns
 * nothing.
 */
export function ownsTheFleet(status: HubStatus = get(hubStatus)): boolean {
  return !status.remote && !status.unavailable;
}

/**
 * Why `action` is unavailable from this window, or `null` when it is
 * available. `null` in standalone mode, always: nothing here may change what
 * a standalone app does.
 */
export function hubBlock(action: HubAction, status: HubStatus = get(hubStatus)): string | null {
  if (status.unavailable) {
    // Not "do it on the hub": the hub is the problem, and every action is
    // refused until it is fixed.
    return `Not available: ${status.unavailable}. This app is set to use that hub, so it manages no fleet of its own until that is fixed — pair again, or Disconnect, in Settings → Hub.`;
  }
  if (!status.remote) return null;
  const where = status.url ?? 'the hub';
  return `${REASONS[action]}. Do it on the hub (${where}).`;
}

/**
 * The next step a hub client needs for an error that does not carry one.
 *
 * `E_CONFIRM_REQUIRED` is the trap this exists for. With
 * `mcp.confirm_destructive` on, the hub refuses `kill_session`,
 * `delete_worktree`, `move_session` and `cancel_task` until someone approves
 * them — and the desktop's own confirmation dialog answers **this process's**
 * queue, which in remote mode is always empty. So the click is refused, the
 * dialog never appears, and without this sentence there is nothing anywhere
 * saying that the approval has to happen on the hub.
 *
 * "This window will follow" is a consequence of the event bridge, not a hope:
 * once the operator approves, the hub emits `session:killed` /
 * `worktree:removed` / `task:updated` like any other change and the bridge
 * re-emits it here. So the sentence must not say "then refresh".
 */
export function hubNextStep(
  error: IpcError,
  status: HubStatus = get(hubStatus),
): string | null {
  if (!status.remote) return null;
  const where = status.url ?? 'the hub';
  switch (error.code) {
    case 'E_CONFIRM_REQUIRED':
      return `${where} is holding this until someone confirms it there, and this desktop's confirmation dialog answers only its own queue, which is empty. Approve it on the hub — this window will follow.`;
    case 'E_UNAUTHORIZED':
      return `${where} no longer accepts this desktop's client token. Pair again in Settings → Hub.`;
    default:
      return null;
  }
}
