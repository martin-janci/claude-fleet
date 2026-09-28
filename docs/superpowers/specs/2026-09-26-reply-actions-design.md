# Reply actions: Copy, Quote, Retry, Fork here, Rewind here

Date: 2026-09-26
Status: built in `claude-fleet` (PR #338 and follow-up fixes): Copy, Quote,
Retry, Fork here into the SAME worktree, and Rewind here, over one
`rewind_conversation` tool. **Open:** Fork into a NEW worktree (§5.2) — the
engine refuses it `E_UNSUPPORTED` (a new worktree's physical path exists
only after `new_session` creates it, too late for the transcript rewrite),
and the fork sheet shows it disabled. `fleet-mobile` follows separately.
Repos: `claude-fleet` (fleet first), `fleet-mobile` (follows)

## 1. What this adds

A row of actions under every reply in the Conversation view, on the desktop
app and on the phone:

| Button | What it does | Destructive |
| --- | --- | --- |
| `⧉` Copy | the reply's text to the clipboard | no |
| `❝` Quote | `> `-quoted reply into the composer | no |
| `↻` Retry | rewind to before this turn, then re-send the same prompt | yes |
| `⑂` Fork here | a new session continuing from this reply | no |
| `⏪` Rewind here | this session restarted from before this turn | yes |

Copy already exists on the desktop (`CopyButton` on the prompt and on each
reply text group, `ConversationPanel.svelte:1784`). It does not exist on the
phone at all — `Turn()` (`SessionScreen.kt:848`) draws a prompt and its items
and nothing else. The other four are new on both.

## 2. The one idea

Fork, Rewind and Retry are **the same operation**: write a truncated copy of
the transcript under a new conversation id. They differ only in what happens
afterwards.

```
                  ┌──────────────────────────────────┐
  anchor uuid ──> │  truncate transcript, host-side  │ ──> new claude id
                  │  new id, new file, same history  │     + new .jsonl
                  └──────────────────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────┐
            ▼                      ▼                      ▼
     mode = "fork"         mode = "rewind"        mode = "rewind"
     new_session           rebind + restart       + send_prompt
     (worktree sheet)      (this session)         (= Retry)
```

So there is one host-side script, one service function, one MCP tool. Retry
is Rewind with a `send_prompt` after it and needs no engine of its own.

### Why a new conversation id instead of truncating in place

The user's real transcript file is never mutated. A rewind is therefore
undoable, the pre-rewind conversation stays listed in the Conversations
panel, and nothing can corrupt a file Claude Code is holding open. It costs
nothing: Fork has to write a new file regardless.

It also comes with its label for free. `StartSource::Fork` already exists
(`store/conversations.rs:20`) and `'fork'` is already in both clients'
`start_source` unions, so a rewound or forked conversation is labelled
correctly with **no** UI work in the Conversations list.

## 3. Wire change: one anchor per turn

`ConvTurn` carries no identity today — `transcript.rs:330` has no id, and
mobile keys turns by list index with a comment saying exactly why that is
safe. Anything that acts *at one reply* needs an anchor.

Add **one** field to `ConvTurn`: `prompt_uuid`, the JSONL `uuid` of the entry
that opened the turn. `Option<String>` with `#[serde(default)]` in Rust,
`String? = null` in Kotlin. An older hub sends nothing; a client that gets no
anchor falls back to Copy and Quote. Same tolerance rule `items_tolerant`
already sets for `ConvItem` (`transcript.rs:363`).

One field is enough because every truncation is expressible as *"keep
strictly before some prompt"*:

| Action at turn `i` | Anchor |
| --- | --- |
| Rewind here / Retry | `turns[i].prompt_uuid` |
| Fork here | the `prompt_uuid` of the next later turn that has one; **none** ⇒ keep the whole file |

Forking the last turn therefore keeps everything, which is exactly what
"continue from this reply" means. And neither direction depends on a turn
outside the loaded window: Rewind reads its own turn, Fork reads a *later*
turn, and turns later than the one on screen are always loaded (the window
grows backwards — `CONV_TURNS_STEP`, "Load older").

This costs one line in the parser. `parse_conversation` (`transcript.rs:833`)
builds a `ConvTurn` at eight sites, and **exactly one** of them sets a prompt
(`transcript.rs:1120`); the other seven get `prompt_uuid: None`. Verified
against a live transcript: `user` and `assistant` entries carry `uuid`,
`parentUuid`, `sessionId` and `cwd`.

Two consequences worth stating rather than discovering:

- **Turns with no prompt** — a compact boundary, a notification-only turn,
  assistant output whose prompt is before the read tail — have no anchor, so
  they offer no Rewind and no Retry. That is correct on its own terms: there
  is no prompt to put back in the composer. Fork still works on them,
  through the forward scan.
- **Fork's forward scan can keep more than the turn on screen.** If the turns
  after it are prompt-less, they are inside the kept range. Fork keeping
  *more* history is safe; keeping less would silently discard work.

## 4. The engine

New module `crates/fleet-core/src/service/rewind.rs`.

### 4.1 The host-side script

Reuses `transcript::locate_script` (`transcript.rs:131`) — the existing
prefix that resolves the transcript into `$f` through stored path → pane cwd
→ `~/.claude/projects/*/<id>.jsonl`, and exits 4 with a sentinel when there
is none. `locate_script` becomes `pub(crate)`.

After the prefix, one `awk` pass copies the head of the file into the new
transcript:

- copy every line from the start up to but **excluding** the line whose
  `uuid` is the anchor; with no anchor, copy the whole file;
- rewrite `sessionId` to the new id on every copied line;
- rewrite `cwd` when the fork targets a different worktree (§5.2);
- write to `$HOME/.claude/projects/<encoded new cwd>/<new-id>.jsonl`,
  creating the directory;
- exit with a distinct sentinel when the anchor uuid is not in the file.

Copying the head rather than filtering by turn keeps the leading metadata
entries (`custom-title`, `mode`, `agent-name`, `bridge-session`) and the
`parentUuid` chain intact, because a prefix of a chain is still a chain.
`file-history-snapshot` entries in the retained range are preserved too, so
Claude Code's own `/rewind` may still work inside the new conversation over
that range. That is a side effect worth keeping, not a promise this feature
makes.

Every interpolated value goes through `crate::shell::quote` per the repo's
one-quoting-implementation rule. The anchor is validated as a uuid before it
reaches the script.

### 4.2 The service function

```rust
pub async fn rewind_conversation(
    args: RewindArgs,   // session_id, anchor_uuid, mode, worktree
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError>
```

- mints the new conversation uuid (as `claude_id_and_pane_cmd` already does),
- runs the script on the session's host,
- then, by mode:
  - **`rewind`** → `rebind_conversation(session, new_id, StartSource::Fork,
    Some(new_path), model)` then `restart_session`. The pane comes back on
    `cl --resume <new id>` through the existing `pane_command_for`.
  - **`fork`** → ensure the target worktree (§5.2), then `new_session {
    project_id, worktree_id, resume_claude_session_id: new_id }`.

### 4.3 Refusals

| Condition | Code | Says |
| --- | --- | --- |
| anchor not in the transcript (rewound past, compacted away, rotated) | `E_NOTFOUND` | the conversation was rewound (or compacted) past this turn; reload it |
| `rewind` with no anchor (would copy the whole file: a no-op) | `E_INVALID` | rewind needs `anchor_uuid` |
| no transcript at all | existing `read_script` sentinel path | unchanged |
| mode is `rewind` and the session is not quiet (`working`, `blocked`, or unknown — the live pane probe first, the stored status as fallback) | `E_INVALID` | interrupt the session first |
| session is the fleet controller | `guard_not_controller` | as `restart_session` already does |
| `rewind` on the **first** turn | `E_INVALID` | nothing before this turn to rewind to; use `/clear` |

The first-turn case is called out because the engine would otherwise succeed
at it: keeping strictly before turn 0's `prompt_uuid` leaves the metadata
header and no conversation, which is `/clear` by a longer route and under a
misleading label. Rewind is offered from the second turn on.

The working-session refusal matters: `restart_session` respawns the pane, so
doing it mid-turn throws the turn away. Fork is exempt — it touches nothing
live.

## 5. UI

### 5.1 The action row

Under each reply text group, not under the whole turn: a turn can hold
several text groups separated by tool runs, and Copy is already per-group.
The row is always visible (no hover-reveal) for the reason `CopyButton`'s own
comment gives — a control you must hover to find is not a control a keyboard
or touch user has.

Which buttons a reply shows:

| Turn | Copy | Quote | Fork here | Rewind here | Retry |
| --- | --- | --- | --- | --- | --- |
| normal, prompted | ✓ | ✓ | ✓ | ✓ | ✓ |
| the conversation's **first** turn | ✓ | ✓ | ✓ | — | — |
| prompt-less (compact boundary, notification-only) | ✓ | ✓ | ✓ | — | — |
| from a hub too old to send anchors | ✓ | ✓ | — | — | — |

Two gating details an implementer will otherwise get wrong:

- **"First turn" means first of the conversation, not first of the window.**
  The window is truncated by `CONV_TURNS_STEP` / "Load older", so index 0 is
  the conversation's first turn only when `truncated` is false. Hide Rewind
  and Retry on index 0 **only** when `!conv.truncated`; §4.3's backend
  refusal is the real guard and stays the one source of truth.
- **The old-hub row is gated on the hub version, not on a missing anchor.**
  A missing anchor legitimately means "keep the whole file" for Fork on the
  last turn, so absence cannot double as "unsupported". Use the
  `HubContract.kt:87` version pattern, as `send_prompt { keys }` does.

- **Retry needs the whole prompt.** `ConvTurn.prompt_partial` (serde
  default `false`) is set when the prompt was cut to fit the read budget or
  carried an image / document block the text drops; Retry is then shown
  disabled with the reason, since re-sending `prompt` would send something
  else. Rewind stays.

Desktop: a new `ReplyActions.svelte` beside `CopyButton.svelte`, dropped into
the `.text` block at `ConversationPanel.svelte:1832`. Quote goes through the
existing `insertIntoComposer` (`conversation.ts:902`). Retry and Rewind
confirm through `ConfirmDialog.svelte`.

Mobile: a new `ReplyActions` composable in `SessionScreen.kt`, under the
`ConvItem.Text` branch of `Item()`. Clipboard through
`LocalClipboardManager`, as `MiniMarkdown.kt:283` already does. The
destructive two are gated on the client token being `full`, following the
pattern `App.kt:550` already uses for `send_prompt`, and on the hub version,
following `HubContract.kt:87`.

### 5.2 The fork sheet

Fork opens a small sheet before doing anything:

```
┌─ Fork this conversation ───────────────┐
│ Files for the new session:             │
│  (•) New worktree   fork-of-canopus    │
│  ( ) Same worktree  ⚠ both sessions    │
│                       edit these files │
│                        [Cancel] [Fork] │
└────────────────────────────────────────┘
```

Default is a new worktree, because two live Claude sessions editing one
checkout is the standard way to lose work. The sheet **is** Fork's
confirmation; there is no second dialog.

A new worktree branches off the source session's current branch through the
existing worktree machinery, and the truncated transcript is written under
**that** cwd's encoded project dir with `cwd` rewritten to match — which is
what lets `cl --resume` find it, per the constraint documented on
`NewSessionArgs::resume_claude_session_id` (`lifecycle.rs:36`).

### 5.3 Confirmation copy

Rewind here and Retry confirm with this sentence, verbatim:

> The conversation is rewound to before this turn. **Your files are left as
> they are.**

This is the one place fleet diverges from Claude Code's own `/rewind`, which
restores files from its checkpoints. Fleet cannot replicate that, and burying
the difference would be the bug. Rewind also puts the turn's prompt back in
the composer, so the next thing the user sees is the prompt they are about to
change.

## 6. MCP and hub

One new tool, `rewind_conversation { session_id, anchor_uuid, mode,
worktree }` — one tool with a `mode`, not two tools, to keep the definition
budget raise to a single addition.

`mode` is `"fork"` or `"rewind"`. There is deliberately no `"retry"` mode:
Retry is the client calling `rewind` and then `send_prompt` with the turn's
own prompt, which means it inherits every refusal in §4.3 — including the
working-session one — without a second code path to keep in step.

Per `CLAUDE.md`'s checklist for a new command:

1. a row in `src-tauri/src/backend/verdicts.rs` — `Routed { tool:
   "rewind_conversation" }`, since the hub has the transcript and the session
   both, so there is nothing local-only about it;
2. `route` by command name in the handler, never a second tool literal;
3. `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`;
4. `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`;
5. a **measured** raise of `BUDGET_BYTES` in
   `the_served_definition_budget_stays_bounded` (`tests.rs:3291`, currently
   `71_658`), with the before/after numbers written into the doc comment as
   every previous raise there does. Trim first; raise only with the numbers.

`work`/`work_link`/`work_admin` are untouched, so no isolation-matrix row is
needed.

## 7. Testing

**Rust.** The script builder against a fixture JSONL: inclusive vs exclusive
truncation, `sessionId` rewritten on every copied line, `cwd` rewritten only
for a cross-worktree fork, leading metadata lines retained, the missing-anchor
sentinel, and no anchor ⇒ the whole file copied. The parser: `prompt_uuid`
populated for a prompted turn, `None` for a compact boundary and for a
notification-only turn. The service: each mode's
follow-up called once, and each refusal in §4.3.

**Desktop.** Vitest on the action row — anchorless turns show two buttons,
anchored turns five; Quote's composer text; the confirm gate on the
destructive two.

**Mobile.** `commonTest` that `ConvTurn` decodes with both new fields, and
without them (an older hub), mirroring `ConvItemTest`'s tolerance tests.

**Wire compatibility**, both directions: a new client against a hub that
sends no anchors, and an old client against a hub that sends them.

## 8. Out of scope

- **Restoring files on rewind.** Claude Code owns the checkpoints; fleet
  would be guessing. §5.3 says so instead of pretending.
- **Editing the prompt before Retry.** Rewind puts the prompt in the
  composer, which covers it — Retry is the no-edit shortcut.
- **Rewinding to a point inside a turn** (between two tool calls). The turn
  is the unit both UIs already draw and both anchors describe.

## 9. Order of work

1. `claude-fleet`: anchors on the wire → engine → MCP tool + verdict row +
   regens + budget raise → desktop UI. One PR.
2. `fleet-mobile`: the two `ConvTurn` fields → `ReplyActions` → hub-version
   and token gating. A second PR, after the hub release that sends anchors.

`fleet-mobile`'s main checkout is shared with live sessions, so its branch is
cut in a worktree.
