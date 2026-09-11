# Session-management pre-check

> **Point-in-time document.** These are read-only checks against main as it
> stood at the commit cited below, not a description of the code today. The
> owner's decisions on the questions this fed are recorded in the addendum
> (sections A.1 to A.8) of the companion analysis.

- **Host:** mefistos (local)
- **Claude Code:** 2.1.267
- **Date:** 2026-09-11
- **Code base:** origin/main `5ee07ee`
- **Scope:** read-only checks. `st/` = `src-tauri/src/`, `fe/` = `src/lib/`.

This file is for sm-q-heur (Q1, Q6) and sm-q-ui (Q8, Q9, double-click edits the label). The full plan is in [`2026-09-11-session-management-analysis.md`](2026-09-11-session-management-analysis.md).

---

## (a) `claude agents --json` on mefistos

The output is verbatim. It contains no secrets: only session ids, working directories and names.

```json
[
  { "id": "d89375a1", "cwd": "/home/mjanci", "kind": "background", "startedAt": 1782115539756,
    "sessionId": "d89375a1-614c-4851-bbd9-a8c51723c0a7", "name": "pap1371-pomodoro-rosso-retrain", "state": "blocked" },
  { "id": "70f61965", "cwd": "/home/mjanci", "kind": "background", "startedAt": 1782115629381,
    "sessionId": "70f61965-a0bc-47f0-93f1-4e0cd2c37ca3", "name": "pap1384-klaudius-feed-diag", "state": "blocked" },
  { "pid": 96281, "cwd": "/mnt/sda4/projects/github.com/FrantisekSefcik/sales-twins-app/.worktrees/eeeee", "kind": "interactive",
    "startedAt": 1785595436663, "sessionId": "56571c99-3559-4cba-b4eb-c26134bffd58", "name": "eeeee-22", "status": "idle" },
  { "pid": 3949506, "cwd": "/mnt/sda4/projects/github.com/papayapos/papayapos-backend", "kind": "interactive",
    "startedAt": 1786975140788, "sessionId": "dbe96ba0-efb9-4a21-8290-7b6b4d55416e", "name": "papayapos-backend-d2", "status": "idle" },
  { "pid": 451468, "cwd": "/home/mjanci", "kind": "interactive", "startedAt": 1787036317958,
    "sessionId": "f5b59713-c923-4beb-960a-a821fbf1f786", "name": "mjanci-78", "status": "idle" },
  { "id": "dbfe2ddd", "cwd": "/home/mjanci", "kind": "background", "startedAt": 1789038794085,
    "sessionId": "dbfe2ddd-4607-4e35-917d-853b9ef856ba", "name": "pd2276-int-test", "state": "blocked" },
  { "pid": 2495934, "id": "841ae322", "cwd": "/home/mjanci", "kind": "background", "startedAt": 1789072055522,
    "sessionId": "4505909e-b2c8-4e4b-8594-9c6700820a0b", "name": "841ae322", "status": "idle", "state": "blocked" },
  { "pid": 3173883, "cwd": "/mnt/sda4/projects/github.com/martin-janci/claude-fleet/.worktrees/jhkljh", "kind": "interactive",
    "startedAt": 1789072590555, "sessionId": "ca94f8a4-ee54-47b2-8f14-6d83a34af3c6", "name": "jhkljh-f2", "status": "busy" }
]
```

### Values observed

| Field | Values seen | Present on |
|---|---|---|
| `kind` | `interactive`, `background` | every row |
| `status` | `idle`, `busy` | every interactive row; one background row that has a `pid` (`841ae322`) |
| `state` | `blocked` | background rows only |
| `waitingFor` | not observed (nothing was waiting at capture time) | none |
| other fields | `id` (short, background only), `pid` (live process), `cwd`, `startedAt` (**milliseconds**), `sessionId`, `name` | |

**Values documented but not observed** ([agent-view](https://code.claude.com/docs/en/agent-view)):
- `state`: `working`, `done`, `failed`, `stopped`
- `status`: `waiting`
- `waitingFor`: `permission prompt`, `input needed`, `sandbox request`, `worker request`, `dialog open`

**What fleet does with this today:**
- `ClaudeAgentRow` reads only `sessionId`, `name`, `status` and `cwd` (`st/claude_agents.rs:7`).
- `known_agent_status` keeps only fleet's own words, `working|blocked|completed|failed|stopped|idle` (`st/service/sessions.rs:3049`).

**Consequences:**
- **`busy` is dropped** (it is not in the vocabulary), so every working interactive session falls back to the pane guess.
- **`state` is never read**, so a blocked background agent is invisible to fleet.

### What Q6 should implement (for sm-q-heur)

1. **Parse the extra fields.** Add `kind`, `state` and `waitingFor` to `ClaudeAgentRow`. All of them are `#[serde(default)]` and optional, and unknown fields stay ignored.
2. **Map `status` for interactive rows:**

   | `status` | fleet value |
   |---|---|
   | `busy` | `working` |
   | `waiting` | `blocked` |
   | `idle` | `idle` |

3. **Map `state` for background rows.** Prefer `state` over `status`:

   | `state` | fleet value |
   |---|---|
   | `working` | `working` |
   | `blocked` | `blocked` |
   | `done` | `completed` |
   | `failed` | `failed` |
   | `stopped` | `stopped` |

4. **Keep logging and dropping anything else.** Do not invent new `ClaudeStatus` values in Q6.
5. **Fixtures.** Use the JSON above verbatim, plus one synthetic row carrying `status:"waiting"` and `waitingFor:"permission prompt"`, marked as documented-not-observed.
6. **Pane cue.** The permission and question dialog cue in `pane_intel` still needs a real pane capture of a permission prompt. No session was showing one at capture time, so none could be captured here.
7. **Q1.** Change `st/service/health.rs:70` from `claude_status == "ghost"` to the `status` field. Add a regression test.

---

## (b) Does fleet pass `--name` when it launches Claude?

**Interactive (work and review) sessions: no.**
- **Launch command.** `pane_command_for` (`st/tmux.rs:430-437`) builds: `cl --resume '<id>' 2>/dev/null || cl --session-id '<id>' || cl; exec ${SHELL:-/bin/zsh} -l`. Rows without an id get `cl --continue || cl; …`.
- **The `cl` wrapper.** On this box `cl` is a user shell alias for `claude --allow-dangerously-skip-permissions`. It comes from the user's dotfiles, not from fleet, and it adds no `--name`.

**Background sessions: yes.** `claude --bg --name <name> -- <prompt>` (`st/claude_cli.rs:73`).

**Live pane start commands** (`tmux list-panes -a -F '#{session_name} #{pane_start_command} #{pane_current_command}'`). Non-Claude shell and script panes are omitted here because their commands contain host addresses:

| tmux session | pane start command (id) | current command |
|---|---|---|
| `dev-FrantisekSefcik-sales-twins-app--eeeee` | `cl --resume '56571c99-…'` | zsh |
| `dev-martin-janci-claude-fleet--jhkljh` | `cl --resume 'f581e458-…'` | claude |
| `dev-papayapos-pos-frontend--asasas` | `cl --resume 'f5b59713-…'` | zsh |
| `pd2758-e2e` | `cl --resume 'dbe96ba0-…'` | zsh |

None of them carry `--name`.

**Consequences:**
- **Agent names are Claude's defaults.** Interactive agent `name`s are Claude's default `<dir>-<2 chars>` names (`jhkljh-f2`, `eeeee-22`). They never equal the tmux name, so `find_by_name` never matches an interactive session. Binding relies on a **unique cwd** (`st/claude_agents.rs:47`).
- **No AI title to unlock.** The agents output carries no AI-generated title. Dropping `--name` would unlock nothing, because fleet does not pass it for interactive sessions. Report decision 4 is moot.
- **The "Claude's title" label source is not in agents output.** Only a status-line wrapper (X2) or the transcript can supply it.
- **Live evidence for D4 (stale binding):**
  - The `jhkljh` pane was started with id `f581e458-…`, but the live agent in that directory now reports `ca94f8a4-…`. The conversation changed after `/clear` or `/resume`.
  - Hooks from `ca94f8a4` match no row until a reconcile pass rebinds by cwd, so hooks in that window are dropped.
  - The `asasas` and `pd2758-e2e` panes resumed ids that `claude agents` now reports at *other* directories (`/home/mjanci` and `papayapos-backend`). Those panes' current command is `zsh`, meaning Claude exited there.
  - So the stored id and the live agent diverge in both directions. This supports P3 (SessionStart plus pane-id binding) in Wave 1.

---

## (c) Is there already a Tauri command that sets `friendly_name`?

> **Line numbers moved after the #74 sidebar split.** Main is now `62a9d25`. Use the locations in section "(c) refreshed" at the end of this file; the frontend line numbers below are from `5ee07ee`.

**Yes. It is registered but not wired into the frontend.**

- **Command:** `commands::sessions::set_session_friendly_name` (`st/commands/sessions.rs:110`), registered in `st/lib.rs:625`.
- **Arguments:** `args: SetFriendlyNameArgs { host_alias: String, tmux_name: String, friendly_name: String }` (`st/service/sessions.rs:2169`).
  - An empty or whitespace-only `friendly_name` clears the label, so the row falls back to `tmux_name`.
  - `tmux_name` goes through the lookup validator, which accepts `bg:<uuid>` rows.
- **Returns:** the updated `SessionRow`, and it emits `session_updated` so other views patch live.
- **Errors:** `E_NOTFOUND` when the row doesn't exist; validation errors for bad input.

**Frontend status:** `fe/sessions.ts` has no wrapper. Add `setFriendlyName(hostAlias, tmuxName, friendlyName)` mirroring `renameSession` (`fe/sessions.ts:236`), using the same `invokeCmd` / `{ args: … }` shape that function uses.

**Notes for sm-q-ui:**
1. **User labels are not protected yet.** Label ownership (P10: `label_source`, pinning, precedence) is deferred to Track N after the F4 splits. Until then the in-session fleet-friendly-name skill can overwrite a label the user typed. `record_prompt_outcome` will **not** overwrite it, because it only replaces still-default labels (`st/service/sessions.rs:2400`).
2. **Double-click behaviour.** Double-click should call `setFriendlyName`, targeting the pinned `(host_alias, tmux_name)` of the row that was clicked, the same way `commitRename` does (`fe/Sidebar.svelte:438`).
   - The input starts with the current label, falling back to `tmux_name`.
   - Enter saves, Esc cancels.
   - An empty value clears the label.
   - Move tmux rename (`renameSession`) to a context-menu item. Keep its `migrateSessionUi` step.
3. **Where labels are edited today.** No frontend surface edits labels except `NewSessionDialog.svelte:356` at create time. SessionDetails shows the label (`fe/SessionDetails.svelte:383`) and could offer the same inline edit.
4. **Q8.** The local hook install is `install_fleet_hook` (`st/commands/mcp.rs:508`). It supports `local` only, and today it is triggered only from `fe/SettingsDialog.svelte:389`.

---

## Copy of report addendum A.7 (verbatim from session-management-analysis.md)
### A.7 Pre-check results (local host, Claude Code 2.1.267, 2026-09-11)

`claude agents --json`, run on the local host:

**Interactive rows** carry `pid`, `cwd`, `kind: "interactive"`, `startedAt`, `sessionId`, `name` and `status`.
- Values observed for `status`: `idle` and `busy`.
- No `state` field.

**Background rows** carry `id`, `cwd`, `kind: "background"`, `startedAt`, `sessionId`, `name` and `state`.
- Value observed for `state`: `blocked`.
- Some background rows also carry `status: "idle"`.

No `waitingFor` appeared, because no session was waiting at the time.

**Consequences (D12 confirmed):**
- `busy` is not in fleet's `ClaudeStatus` vocabulary, so `known_agent_status` drops it (`st/service/sessions.rs:3049`). Every working interactive session therefore falls back to the pane guess.
- Background `state` is never parsed (`st/claude_agents.rs:7`), so a blocked background agent is invisible.
- Q6 must:
  - parse `kind`, `state` and `waitingFor`;
  - map `busy`→working, `waiting`→blocked, `done`→completed;
  - prefer `state` over `status` for background rows.

**`--name` (§5 decision 4) is moot:**
- Interactive panes launch with `cl --resume '<id>' || cl --session-id '<id>' || cl` and no `--name` (`st/tmux.rs:435`). Only `claude --bg` passes `--name` (`st/claude_cli.rs:73`).
- The interactive `name` in agents output is Claude's default `<dir>-<2 chars>` display name (e.g. `jhkljh-f2`). It is not an AI title and never equals the tmux name.
- So `find_by_name` never matches interactive sessions; they bind by unique cwd only. That strengthens the case for P3 (pane-id binding).
- Claude's AI title, where one exists, is not in `claude agents` output. The "Claude's title" source in A.2 therefore needs the status-line wrapper (X2) or the transcript, or should be dropped.

**Not yet checked:** a remote host's output, which may run another CLI version.

---

## (c) refreshed for origin/main `62a9d25` (after the #74 sidebar split)

**Answer: yes, a Tauri command already sets `friendly_name`.**
- **Command:** `commands::sessions::set_session_friendly_name` (`src-tauri/src/commands/sessions.rs:110`).
- **Registered:** inside `tauri::generate_handler![…]` at `src-tauri/src/lib.rs:629`. The list spans `lib.rs:608-683`.
- **Signature:** `args: SetFriendlyNameArgs { host_alias, tmux_name, friendly_name }`, returning `SessionRow`.
  - An empty value clears the label.
  - It emits `session_updated`.
  - It returns `E_NOTFOUND` when no row matches.
- **No frontend wrapper exists yet.** Nothing under `src/` calls `set_session_friendly_name`. Add `setFriendlyName(hostAlias, tmuxName, friendlyName)` to `src/lib/sessions.ts` next to `renameSession` (`:236`), using the same `invokeCmd` / `{ args }` shape.

**Rename surfaces that call `renameSession`, the tmux rename.** Converting double-click to edit the label means changing all of them:

| Surface | Where |
|---|---|
| Sidebar row double-click | `src/lib/SessionRowItem.svelte:118` (`ondblclick` → `beginRename(sess, e)`), passed in as a prop from `src/lib/Sidebar.svelte:563-564` |
| Sidebar row pencil button | `src/lib/SessionRowItem.svelte:282` |
| Sidebar commit | `src/lib/Sidebar.svelte:370` (`beginRename`), `:386` (`commitRename`), `:399` (the `renameSession` call), `:407` (`migrateSessionUi`, needed only for tmux rename), `:419` (`onRenameKey`) |
| SessionDetails double-click | `src/lib/SessionDetails.svelte:378` (on the label, whose display is at `:383`) |
| SessionDetails "Rename" button | `src/lib/SessionDetails.svelte:547` (`data-testid="rename-from-details"`) |
| SessionDetails commit | `src/lib/SessionDetails.svelte:112` (`beginRename`), `:121` (`commitRename`), `:128` (the `renameSession` call), `:141` (`onRenameKey`) |

**Suggested split for sm-q-ui:**
- Double-click, and the pencil or "Rename" buttons, edit the **label** through `setFriendlyName`.
  - The input starts with `friendly_name ?? tmux_name`.
  - Enter saves, Esc cancels.
  - Empty clears the label.
  - Skip the call when nothing changed.
- "Rename tmux session…" becomes a separate, explicit action, from a context menu or a secondary button in SessionDetails. It keeps the existing `renameSession` + `migrateSessionUi` path.
- Existing tests that assert double-click triggers `rename_session` must change. Search `Sidebar.test.ts` and `SessionDetails.test.ts` for `rename-input` and `rename-from-details`.

**Other moved references:**
- The local hook install button (Q8) is now `src/lib/McpSettings.svelte:88` (`installFleetHook('local')`), no longer SettingsDialog.
- `NewSessionDialog.svelte:356` is unchanged: it sets the label at create time.

**Caveat, unchanged:** until P10 (label ownership, Track N after F4), the in-session fleet-friendly-name skill can overwrite a label the user typed. Prompt-derived labels cannot, because they only replace still-default labels.
