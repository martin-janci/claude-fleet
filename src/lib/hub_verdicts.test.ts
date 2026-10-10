// Cross-checks `hub.ts`'s hand-written `ROUTED_ACTIONS`/`REASONS` against the
// generated `hub_verdicts.generated.json` — the one table
// (`src-tauri/src/backend/verdicts.rs`) the backend actually enforces from.
//
// `ROUTED_ACTIONS` and `REASONS` stay hand-written on purpose (see the
// comment above `ROUTED_ACTIONS` in `hub.ts`): `ROUTED_ACTIONS` is the
// SUBSET of routed commands the UI actually gates the connection state on,
// not every routed command, and `REASONS` carries hand-written user-facing
// wording, not the backend's `instead` sentence. A generated-file-driven
// type would have to be exactly as precise as those two to be worth
// swapping in, and it is not — so this file holds the literals accountable
// to the generated JSON instead of replacing them.
import { describe, it, expect } from 'vitest';
import verdicts from './hub_verdicts.generated.json';
import { HUB_ACTIONS, ROUTED_ACTIONS, type HubAction } from './hub';

// The one place the frontend's action vocabulary and the backend's command
// vocabulary differ: `ROUTED_ACTIONS`' `set_friendly_name` is the command
// `set_session_friendly_name` (see the comment above `ROUTED_ACTIONS` in
// `hub.ts`, and `tests_routing.rs`'s `check`, which cross-checks the same
// exception on the Rust side). Every other action name equals its command
// name.
const ACTION_TO_COMMAND: Readonly<Record<string, string>> = {
  set_friendly_name: 'set_session_friendly_name',
};
function toCommand(action: string): string {
  return ACTION_TO_COMMAND[action] ?? action;
}

const routedCommands = new Set<string>([
  ...verdicts.routed,
  ...verdicts.routed_unless.map((e) => e.command),
]);
const localOnlyCommands = new Set<string>(verdicts.local_only);
// Every name VERDICTS has a row for, regardless of verdict kind — used only
// to tell "not a command name at all" apart from "a command name whose
// verdict isn't local_only" below.
const allCommands = new Set<string>([
  ...verdicts.local_only,
  ...verdicts.routed,
  ...verdicts.routed_unless.map((e) => e.command),
  ...verdicts.same_in_both,
]);

describe('ROUTED_ACTIONS against the generated routed/routed_unless commands', () => {
  it('every ROUTED_ACTIONS entry, mapped through the name exception, is routed or routed_unless', () => {
    for (const action of ROUTED_ACTIONS) {
      const command = toCommand(action);
      expect(routedCommands.has(command), `${action} -> ${command}`).toBe(true);
    }
  });

  it('the name exception itself is real: set_friendly_name is not a command, set_session_friendly_name is', () => {
    expect(allCommands.has('set_friendly_name')).toBe(false);
    expect(routedCommands.has('set_session_friendly_name')).toBe(true);
  });

  // Multi-user M1 (F3): the sharing commands this file had to wait for.
  // `session_share` / `session_unshare` / `session_narrow` are the three
  // WRITES and are `ROUTED_ACTIONS` entries (checked above, like every other
  // one). The three routed READS are not, by `ROUTED_ACTIONS`' own rule — a
  // read has no control to disable — so this pins that they really are
  // routed commands that were deliberately left out rather than names
  // nobody noticed: `my_grants`'s failure is `access.ts`'s fail-closed arm,
  // and `session_access` / `capture_session` are a list and a snapshot that
  // simply show their error.
  it('the routed sharing READS are real commands, left out of ROUTED_ACTIONS on purpose', () => {
    const routedSet = new Set<string>(ROUTED_ACTIONS);
    for (const command of ['session_access', 'my_grants', 'capture_session']) {
      expect(routedCommands.has(command), `${command} is routed`).toBe(true);
      expect(routedSet.has(command), `${command} is not a ROUTED_ACTIONS entry`).toBe(false);
    }
  });

  // `claim_session` is the one M1 command the desktop does NOT have. The hub
  // has the tool (an operator claims an unowned session), but T13 added no
  // `#[tauri::command]` for it on purpose: F2 made the unclaimed surface a
  // per-host COUNT with no rows and no expand, so there is no session id for
  // a desktop command to pass, and spec §4.3 forbids the button that would
  // supply one. The plan's revision 4 called it "UI-reachable" and would have
  // had it allowlisted below as a `local_only` command the UI can reach —
  // which would fail, because a command with no handler has no verdict row at
  // all. This is the pin on that: if a `claim_session` command is ever added,
  // this line fails, and whoever added it has to decide how it is gated — a
  // `REASONS` entry, an allowlist line, or a `ROUTED_ACTIONS` entry if it
  // routes — instead of inheriting a stale note either way.
  it('claim_session has no desktop command at all, so there is nothing to gate', () => {
    expect(allCommands.has('claim_session')).toBe(false);
  });
});

// REASONS keys that name something other than a single Tauri command: each
// covers a cluster of commands under one shared, more general sentence. A
// key not on this list and not itself a command name would be a bug in
// REASONS (a typo, or a command that got renamed on one side and not the
// other) — see the "every other REASONS key is a command name" test below.
//
//   repo_write  -> the eleven git-write commands FilesPanel.svelte fans
//                  `hubBlock('repo_write', …)` out to (FileList,
//                  RemoteToolbar, BranchList, CommitGraph)
//   catalog_registry -> catalog_add_catalog / catalog_remove_catalog, which
//                  ROUTE (master-only on the hub): Settings → Catalogs shows
//                  this reason to a paired desktop through `resourceBlock`
//   host_tokens -> list_host_tokens / set_host_token_mode / rotate_host_token
//                  (HostDetail.svelte's per-host token controls)
const REASONS_KEYS_THAT_ARE_NOT_COMMANDS: ReadonlySet<string> = new Set([
  'repo_write',
  'fleet_settings',
  'host_tokens',
  'catalog_registry',
]);

describe('REASONS against the generated local_only commands', () => {
  it('every other REASONS key that is a command name is local_only in the generated file', () => {
    for (const action of HUB_ACTIONS) {
      if (REASONS_KEYS_THAT_ARE_NOT_COMMANDS.has(action)) continue;
      expect(allCommands.has(action), `${action} is not a known command at all`).toBe(true);
      expect(localOnlyCommands.has(action), `${action} is a command but not local_only`).toBe(true);
    }
  });

  it('the non-command REASONS keys really are not command names', () => {
    for (const key of REASONS_KEYS_THAT_ARE_NOT_COMMANDS) {
      expect(allCommands.has(key), key).toBe(false);
    }
  });

  it('no name is in both REASONS and ROUTED_ACTIONS', () => {
    const routedSet = new Set<string>(ROUTED_ACTIONS);
    for (const action of HUB_ACTIONS) {
      expect(routedSet.has(action as HubAction & string), action).toBe(false);
    }
  });
});

// Coverage the other way: a `local_only` command the UI can reach with no
// `REASONS` entry at all would fail open (or, worse, open a dialog whose
// click then dies with a raw `E_LOCAL_ONLY`). Every command listed below is
// `local_only` today, is not a `REASONS` key (checked below), and
// falls into one of these groups. Seeded from today's truth
// (generated `local_only` minus the `REASONS` keys that are command names);
// each group below names the component whose own gate covers it, which is
// how to re-verify a group.
const LOCAL_ONLY_WITH_NO_DIRECT_REASONS_ENTRY = {
  // (The asset catalog used to be most of this list, gated by AssetsPanel's
  // `catalog_config` check. Every one of its commands routes to the hub's
  // `catalog_admin` now, including `catalog_import_host` (Task 6: import
  // works from any host over SSH) — it is a `ROUTED_ACTIONS` entry below,
  // not a `REASONS` one. `catalog_spawn_author_session` is the one left
  // refusing, and has a REASONS entry of its own.)
  // No frontend UI calls these at all.
  noUiControl: [
    // Host identity & health, task 5: the alias merge is an operator's
    // `fleet-hub host merge` / master-token tool; no Svelte control calls it.
    'merge_host',
  ],
  // Gated by SettingsDialog.svelte's own `get_fleet_settings` gate: `{#if
  // !ownsFleet}` swaps the whole Projects section for the remote note, and
  // the generated pages too. Work retention (work graph M12.3) is a page's
  // data source and its Sweep now a page action since declarative pages P5:
  // a read-only page shows neither.
  // `describe_fleet_settings` (declarative pages P1) and
  // `fetch_page_source` (P3) feed the generated pages, which show the same
  // hub reason instead (`pages-remote`) when the app does not own the fleet.
  // Declarative pages P4b: a resource's create flow. ResourcePage renders a
  // paired desktop's resource read-only, with no Add, so no flow starts.
  gatedByReadonlyResourcePage: ['flow_start', 'flow_submit', 'flow_back', 'flow_cancel'],
  gatedBySettingsDialog: [
    // (The settings themselves, their proposals and history route to the
    // hub since declarative pages P6.) A page's data sources read this
    // app's store: a paired desktop's pages show no data item.
    'fetch_page_source',
    // No view calls it since P5 (the page reads `work.retention` instead);
    // it stays a command for work_admin's status on a standalone desktop.
    'work_retention_status',
    'work_retention_sweep',
    // G4.6: Repair now and Restore lost sessions are page actions, which a
    // read-only (paired) page does not show.
    'repair_workspaces_now',
    'restore_all_lost_sessions',
  ],
  // (`pty_open` and `upload_to_session` used to be listed here, gated by
  // TerminalView's `ownsTheFleet` check. They are `same_in_both` now: both
  // are this machine's own `ssh`, addressed by the alias passed in, reading
  // no state.db — so the pane attaches for a paired client exactly as it does
  // standalone. TerminalView still declines an AGENT host, which has no SSH
  // route from anywhere, but that is not a local-only refusal.)
  // Gated by FilesPanel.svelte's own `repo_write` gate (see the
  // REASONS_KEYS_THAT_ARE_NOT_COMMANDS comment above).
  gatedByFilesPanel: [
    'repo_checkout',
    'repo_checkout_commit',
    'repo_create_branch',
    'repo_delete_branch',
    'repo_delete_merged_branches',
    'repo_stage',
    'repo_unstage',
    'repo_commit_create',
    'draft_commit_message',
    'repo_fetch',
    'repo_pull',
    'repo_push',
  ],
  // Gated by HostDetail.svelte's own `host_tokens` gate (see the
  // REASONS_KEYS_THAT_ARE_NOT_COMMANDS comment above).
  gatedByHostDetail: ['list_host_tokens', 'set_host_token_mode', 'rotate_host_token'],
  // AddHostWizard.svelte only mounts inside the Add-host dialog, whose
  // opener (`+ Add host`) is disabled via `hubBlock('add_host', …)`.
  gatedByAddHostDialog: ['probe_ssh_alias'],
} as const;

const ALLOWLISTED_LOCAL_ONLY_COMMANDS: readonly string[] = Object.values(
  LOCAL_ONLY_WITH_NO_DIRECT_REASONS_ENTRY,
).flat();

describe('local_only commands the UI can reach without a REASONS entry', () => {
  it('every allowlisted command really is local_only today', () => {
    for (const command of ALLOWLISTED_LOCAL_ONLY_COMMANDS) {
      expect(localOnlyCommands.has(command), command).toBe(true);
    }
  });

  it('no allowlisted command is also a REASONS key', () => {
    const reasonsSet = new Set<string>(HUB_ACTIONS);
    for (const command of ALLOWLISTED_LOCAL_ONLY_COMMANDS) {
      expect(reasonsSet.has(command as HubAction), command).toBe(false);
    }
  });

  it('the allowlist has no duplicate entries', () => {
    expect(new Set(ALLOWLISTED_LOCAL_ONLY_COMMANDS).size).toBe(ALLOWLISTED_LOCAL_ONLY_COMMANDS.length);
  });

  it('every local_only command is covered: either a REASONS key, or on the allowlist', () => {
    const reasonsCommandNames = new Set<string>(
      HUB_ACTIONS.filter((a) => !REASONS_KEYS_THAT_ARE_NOT_COMMANDS.has(a)),
    );
    const allowlisted = new Set(ALLOWLISTED_LOCAL_ONLY_COMMANDS);
    const uncovered = verdicts.local_only.filter(
      (command) => !reasonsCommandNames.has(command) && !allowlisted.has(command),
    );
    expect(uncovered, `uncovered local_only commands: ${uncovered.join(', ')}`).toEqual([]);
  });
});
