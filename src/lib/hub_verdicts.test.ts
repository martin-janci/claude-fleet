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
});

// REASONS keys that name something other than a single Tauri command: each
// covers a cluster of commands under one shared, more general sentence. A
// key not on this list and not itself a command name would be a bug in
// REASONS (a typo, or a command that got renamed on one side and not the
// other) — see the "every other REASONS key is a command name" test below.
//
//   terminal    -> pty_open (TerminalView.svelte gates the whole pane on
//                  `ownsTheFleet`, before either pty_open or the
//                  upload-to-session drop handler can fire)
//   repo_write  -> the ten git-write commands FilesPanel.svelte fans
//                  `hubBlock('repo_write', …)` out to (FileList,
//                  RemoteToolbar, BranchList, CommitGraph)
//   host_tokens -> list_host_tokens / set_host_token_mode / rotate_host_token
//                  (HostDetail.svelte's per-host token controls)
//   apply_sync  -> catalog_apply_sync — no component calls
//                  hubBlock('apply_sync', …) today; AssetsPanel hides the
//                  whole Sync section instead (transitively gated via
//                  catalog_config, group B below)
//   set_secret  -> catalog_set_secret — same as apply_sync
const REASONS_KEYS_THAT_ARE_NOT_COMMANDS: ReadonlySet<string> = new Set([
  'terminal',
  'repo_write',
  'host_tokens',
  'apply_sync',
  'set_secret',
]);

describe('REASONS against the generated local_only commands', () => {
  it('every other REASONS key that is a command name is local_only in the generated file', () => {
    for (const action of HUB_ACTIONS) {
      if (REASONS_KEYS_THAT_ARE_NOT_COMMANDS.has(action)) continue;
      expect(allCommands.has(action), `${action} is not a known command at all`).toBe(true);
      expect(localOnlyCommands.has(action), `${action} is a command but not local_only`).toBe(true);
    }
  });

  it('the five non-command REASONS keys really are not command names', () => {
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
// click then dies with a raw `E_LOCAL_ONLY`). Every one of the 54 commands
// below is `local_only` today, is not a `REASONS` key (checked below), and
// falls into one of these groups. Seeded from today's truth
// (generated `local_only` minus the `REASONS` keys that are command names);
// each group below names the component whose own gate covers it, which is
// how to re-verify a group.
const LOCAL_ONLY_WITH_NO_DIRECT_REASONS_ENTRY = {
  // No frontend UI calls these at all — the Layers feature
  // (list/resolve/propose/set-host-layers/template/write/delete) has no
  // Svelte component yet.
  noUiControl: [
    'catalog_list_layers',
    'catalog_resolve_preview',
    'catalog_propose_layers',
    'catalog_set_host_layers',
    'catalog_layer_template',
    'catalog_write_layer',
    'catalog_delete_layer',
  ],
  // pick_attachments (the OS file picker that authorises its own result,
  // SEC-9) and attachment_preview (the inline thumbnail data URL) have no
  // caller yet either — the composer's attach button and preview strip are
  // later tasks of the same plan (`.superpowers/sdd/2026-09-20-conversation-attachments/`).
  noUiControlYet: ['pick_attachments', 'attachment_preview'],
  // Gated by AssetsPanel.svelte's own `catalog_config` gate: `{#if
  // catalogBlocked}` swaps the ENTIRE panel body (Sync, Secrets, asset
  // list/detail, import, lint, layers-adjacent…) for the remote note, so
  // none of these ever fires — `catalog_config` already carries the
  // REASONS entry that disables the panel.
  gatedByAssetsPanel: [
    'catalog_configure',
    'catalog_load',
    'catalog_list_assets',
    'catalog_get_asset',
    'catalog_import_host',
    'assets_scan_hosts',
    'assets_inventory',
    'catalog_plan_sync',
    'catalog_apply_sync',
    'catalog_last_sync',
    'catalog_list_secrets',
    'catalog_set_secret',
    'catalog_delete_secret',
    'catalog_create_asset',
    'catalog_update_asset',
    'catalog_delete_asset',
    'catalog_add_resource',
    'catalog_remove_resource',
    'catalog_lint_asset',
    'catalog_lint_all',
    'catalog_commit_pending',
    'catalog_push',
    'catalog_repo_status',
    'catalog_template',
    'catalog_spawn_author_session',
  ],
  // Gated by SettingsDialog.svelte's own `get_fleet_settings` gate: `{#if
  // !ownsFleet}` swaps the whole Projects section for the remote note.
  gatedBySettingsDialog: ['set_fleet_setting'],
  // Gated by TerminalView.svelte's own `ownsTheFleet` gate: `{#if
  // !ownsTheFleet(...)}` swaps the whole pane for the remote note, so
  // neither the attach nor the file-drop handler (which requires
  // `ptyOpen`) ever runs.
  gatedByTerminalView: ['pty_open', 'upload_to_session'],
  // Gated by FilesPanel.svelte's own `repo_write` gate (see the
  // REASONS_KEYS_THAT_ARE_NOT_COMMANDS comment above).
  gatedByFilesPanel: [
    'repo_checkout',
    'repo_checkout_commit',
    'repo_create_branch',
    'repo_delete_branch',
    'repo_stage',
    'repo_unstage',
    'repo_commit_create',
    'repo_fetch',
    'repo_pull',
    'repo_push',
  ],
  // Gated by HostDetail.svelte's own `host_tokens` gate (see the
  // REASONS_KEYS_THAT_ARE_NOT_COMMANDS comment above).
  gatedByHostDetail: ['list_host_tokens', 'set_host_token_mode', 'rotate_host_token'],
  // GithubRepoBrowser.svelte only mounts inside AddProjectDialog, whose
  // opener (`+ Add project…`) is disabled via `hubBlock('add_project', …)`.
  gatedByAddProjectDialog: ['list_github_repos'],
  // AddHostPicker.svelte only mounts inside the Add-host dialog, whose
  // opener (`+ Add host`) is disabled via `hubBlock('add_host', …)`.
  gatedByAddHostDialog: ['probe_ssh_alias'],
  // Guarded directly with `ownsTheFleet($hubStatus)` at the call site
  // instead of `hubBlock`/`REASONS`: the call is skipped and a safe
  // substitute used in its place (ConversationPanel.svelte's `probeLive`
  // simply never becomes true, so the pane is never polled).
  guardedDirectlyWithOwnsTheFleet: ['session_activity'],
  // Reachable and attempted even on a hub client — ToolLine.svelte fetches
  // it when a tool row is expanded — but handled per-click with an inline,
  // non-retryable `E_LOCAL_ONLY` message (`loadRetryable = r.error.code !==
  // 'E_LOCAL_ONLY'`) rather than a pre-emptive disable. Already a
  // deliberate, documented choice (see the comment above `loadRetryable` in
  // ToolLine.svelte), so left as is here.
  handledInlinePerClickNotPreGated: ['session_tool_detail'],
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
