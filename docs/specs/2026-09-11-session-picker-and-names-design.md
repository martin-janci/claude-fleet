# Session picker & generated names — design

**Date:** 2026-09-11
**Status:** Implemented (PR `feat/session-picker-and-names`)
**Problem:** With ~60 sessions across 5 hosts, (1) the create/select surfaces
overflow the viewport so the wanted row is off-screen, (2) every new session
demands a typed name ("work 3"), and (3) there is no keyboard path from "I am in
a terminal" to "I am in the other session".

## What we looked at

| Source | Takeaway | Copied? |
| --- | --- | --- |
| Docker `namesgenerator` (moby `internal/namesgenerator/legacy`) | 103 adjectives × 250 surnames, `adjective_surname`; the daemon calls `GetRandomName(retry)` again on conflict and only then appends a random digit. One hard-coded exclusion (`boring_wozniak`). | Two-list shape, retry-then-suffix policy. **Not** surnames (the owner asked for "blue sirius"), not `_`, not a *random* digit — we count `-2`, `-3` so the suffix reads as "the second one". |
| Heroku / haikunator | `adjective-noun-NNNN`; uniqueness by a 4-digit token every time. | Separator `-`. **Not** the always-on number: it defeats the "memorable" goal and is only needed because Heroku's namespace is global; ours is per project. |
| GitHub Codespaces | Two/three random words as a *display* name ("literate space parakeet") over a permanent id; introduced because users could not tell codespaces on the same branch apart; renamable. | Display name ≠ identity: the generated pair feeds `friendly_name`, the worktree slug and the tmux-name suffix, but the DB row id stays the identity. Renaming already exists. |
| Conductor | Workspaces named after cities, branch shown alongside. | The idea that the auto name is the *primary* label and the branch is secondary. Not cities — too few, and they collide with host names. |
| Vibe Kanban | `vk/<4-char id>-<slug>` branches; open request for AI-generated titles. | Nothing. IDs in names is what the owner wants to stop typing; AI naming needs a model round-trip on the create path. |
| sesh / tmux-sessionizer / BartSte tmux-session | One fzf list of *all* sources, recency (zoxide frecency) first, live capture-pane preview, "connect creates when missing", vim keys, one chord from anywhere in tmux. | Single list, recent-first, Enter = connect, Cmd/Ctrl+Enter = "create with this query" (sessionizer's create-if-missing). Not the preview pane (the terminal is right there once attached) and not vim keys (Ctrl+N/Down only). |
| VS Code quick pick | label / description / detail rows, separators for groups, "recently opened" section, always a "create new" item, chord works from the integrated terminal (`commandsToSkipShell`). | All of it: rows carry name + `project · host · branch` + status; grouped Sessions / Projects; project rows are the "create new" items; the chord is taken in the capture phase so it works while the terminal has focus. |

## Design

### Names (`src/lib/names.json`, `names.ts`, `service/names.rs`)

One JSON file holds 67 adjectives (colours, short evocative qualities) and 124
nouns (stars, constellations, planets, moons, celestial phenomena). Every word
is lowercase ASCII, 3–8 letters, unique across both lists, so `adjective-noun`
is a legal git ref *and* a legal tmux name with no cleaning. The TS module
imports the JSON; the Rust module `include_str!`s the same file, and both test
suites assert the same list sizes so a list edit that forgets one side fails CI.

Pairs whose words share a 4-letter root ("lunar luna", "cosmic cosmos") are
never drawn. `generateName(existing, rng)` draws up to 24 random pairs not in `existing`,
then falls back to `<last-pair>-2`, `-3`, … (Docker's retry policy, counting
suffix). `existing` is every slug already in use on the project: worktree names,
the suffix of every tmux name, and slugified friendly names.

### Backend: empty `name` → fleet picks

`new_session` with `name: ""` now calls `fill_session_name`: the deterministic
`dev-<owner>-<repo>[--<worktree>][-term]` when free on the host, else
`dev-<owner>-<repo>[--<worktree>]--<adjective>-<noun>[-term]`. The MCP
`new_session` tool signature is untouched; a caller that passes `""` just gets
the same behaviour (previously `E_INVALID`).

### Dialog (`NewSessionDialog.svelte`)

- **Name** field is prefilled: the humanised branch for an existing worktree
  unless it is empty (main) or already used by a session on this project —
  then a generated pair. `+ new worktree` always starts from a fresh pair; the
  slug follows the name until the user edits the slug. 🎲 and Cmd/Ctrl+R re-roll.
- **Enter** in any field creates. The tmux name is derived (worktree-based; a
  second session on the same worktree gets `--<name-slug>` appended instead of a
  collision) and stays editable; clearing it sends `""` and the backend mints.
- **Preview** line shows the cwd the pane will start in (local from the DB,
  remote per the `~/projects/github.com/<owner>/<repo>` convention).
- **Memory:** `newsession.project.<id>` = `{host, worktree: id|'new', kind}`
  written on a successful create; `last-host` stays as the global fallback.
- **Viewport:** the field stack scrolls inside `max-height: calc(85vh - 2rem)`,
  the Cancel/Create row is pinned below it, the worktree list is a `PickerList`
  capped at 9rem with the active row scrolled into view, the host chips cap at
  ~2 rows and scroll.

### Quick switcher (`QuickSwitcher.svelte`, `quick_switcher.ts`, `fuzzy.ts`)

⌘K / ⌘P on macOS, Ctrl+Shift+K / Ctrl+Shift+P elsewhere (TerminalView's
copy/paste convention), captured before the terminal; plain Ctrl+K / Ctrl+P
stay with the pty (readline kill-line, previous history). Ignored while
another modal is open. One input (`role=combobox`, `aria-activedescendant`
on the highlighted option); rows =
every session (label = friendly name, description = `project · host · branch`,
meta = status) grouped before "New session in <project>" rows. Empty query:
MRU order (`quick-switcher.recent`, 20 keys, fed by *every* selection), then
last activity. Query: dependency-free subsequence fuzzy over friendly name,
tmux name, project, host, branch, status, kind; multi-token AND across fields.
↑/↓ (Ctrl+N) move with wrap and `scrollIntoView({block:'nearest'})`; Enter
attaches (or opens the dialog for a project row); Cmd/Ctrl+Enter opens the
dialog for the context project (selected session's → top row's → MRU project)
with the query as the name. The dialog is mounted from App.svelte through a
`newSessionRequest` store so the switcher never reaches into the Sidebar.
Whatever selects a session, the Sidebar expands its project and scrolls the
row (`data-session-id`) into view.

### Shared list (`PickerList.svelte`)

Bounded, scrollable, `role=listbox`, active row kept visible; keyboard handling
stays with the owner so the same list works under a search box (switcher) or
inside a form (dialog). The Sidebar can adopt it after #46.

## Deliberately not done

- Only a small Sidebar change (reveal the selected row); the row list itself
  is not yet a `PickerList`.
- No live pane preview in the switcher, no vim keys, no per-word emoji.
- No AI-generated names; no numeric token on every name.
- Word lists are curated, not exhaustive: nothing ambiguous (no `cancer`,
  `lupus`, `norma`), nothing that is also a host or status word.

## Follow-ups (post-#46)

- Sidebar: use `PickerList` for session rows so arrow-key selection scrolls;
  switch its footer button to `requestNewSession` and drop its own dialog
  mount.
- Expose the generator to the MCP surface explicitly (`name` optional in the
  tool schema) once #45/#46 settle `tools.rs`.
