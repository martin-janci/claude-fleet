# Code review round 20 — the post-v0.2.35 UX/agent/conversation wave

Date: 2026-09-22 · Reviewed at `332284c9` (`main`, in sync with `origin/main`)

Scope: everything merged since the `v0.2.35` release commit (`310a03d3..HEAD`) —
1 783 added lines across 49 files, landed as three PRs:

- [#240](https://github.com/martin-janci/claude-fleet/pull/240) `feature/ten-ai-agent-debug-0d1563` — the agent sheet sends through the composer that owns its live state
- [#238](https://github.com/martin-janci/claude-fleet/pull/238) `feature/ui-incomplete-functionality-2fb8e8`
- [#239](https://github.com/martin-janci/claude-fleet/pull/239) `feature/ux-expert-review-fix-95377f` — the UX-124…UX-139 fix wave

Method: three parallel read-only expert lenses (Rust/backend, Svelte frontend,
tests + generated-artifact consistency), each asked to falsify the authors'
own claims in `docs/ux/2026-09-21-audit/iterations/11-expert-review-and-fixes.md`
rather than restate them. Every finding below is tied to lines in the merged code.

## Baseline

Verified locally at `332284c9`:

| Check | Result |
|---|---|
| `npx vitest run` | **2459 passed**, 132 files, 0 failed |
| `npx svelte-check` | 0 errors, 0 warnings |
| `docs/control-api-reference.md` regen drift | none |
| `hub_verdicts.generated.json` / `docs/hub.md` / goldens | reconcile; no drift |
| GitHub CI on `main` | **RED** — see F1 |

`cargo test` was not run locally (shared target dir across worktrees); all Rust
findings are from source, and CI's own Rust results are used for F1.

## Findings

Ordered by the cost of leaving them alone.

### F1 — `main` is red, and was merged through twice · **H**

Two of the three merges produced a failing CI run on `main`. Both branches were
green on their own PR runs; only the post-merge runs failed, and each failed in a
*different* job than the other:

| Run | Commit | Failing job | Test |
|---|---|---|---|
| PR #240 merge | `467bbc12` | `rust (ubuntu-24.04)` | `ssh::tests::a_mux_failure_is_retried_only_once` |
| PR #239 merge | `332284c9` (HEAD) | `hub-headless` | `tmux::tests::pane_command_prefers_the_users_cl_when_present` |

One root cause, two symptoms: **`ETXTBSY`**. The ssh failure names it outright —
`ssh spawn h-twice: Text file busy (os error 26)`.

`crates/fleet-core/src/ssh.rs:1927` writes a fake executable and immediately
execs it:

```rust
std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
```

In a multi-threaded test binary this races: another test's `fork()` between the
`open(O_WRONLY)` and the `close()` copies the write fd into the child, and that
child's `exec` of the file fails with `ETXTBSY`. `O_CLOEXEC` does not help — the
fd is closed *at* exec, which is exactly when the kernel checks for writers.

It explains the tmux failure too, which looks unrelated.
`crates/fleet-core/src/tmux.rs:1532` uses the same write-then-exec helper, and the
pane command swallows the error:

```
cl --resume ... 2>/dev/null || cl --session-id ... || cl ...; exec ${SHELL:-/bin/zsh} -l
```

An `ETXTBSY` exec returns 126 on each of the three `cl` attempts, then `;` runs
`exec fake-shell`, which exits 0 — so the harness's `status.success()` assert
passes and the argv log is simply **empty**. That is precisely the reported
`left: []`. The test cannot distinguish "the fallback logic is wrong" from "the
fake binary was unexecutable", because `tmux.rs:1578` turns a missing log into an
empty vec with `.unwrap_or_default()`.

Seven files use write-then-exec; `service/catalog/repo.rs` **added one in this
diff**. Rate: 2 failures in the last 30 CI runs (~7%), both on `main`.

Fix: open the file, write, `sync_all`, and **drop the handle before spawning**
(or write to a sibling path and `rename` into place, which is atomic and cannot
be held open by a forked child). Separately, `run_pane_command` should assert the
log is non-empty rather than `unwrap_or_default`, so this failure mode names
itself instead of masquerading as a logic bug.

Round 19 (`docs/specs/2026-09-20-post-merge-review.md` F1) reported the *same
class* — "a flake here silently skips a whole verification layer". That specific
test was fixed (the bound is now 5 s at `agent/registry.rs:490`), but the class
was not, and two days later `main` is red twice.

### F2 — "⏎ Press Enter" sends the whole context paragraph into the REPL · **H**

`src/lib/ConversationPanel.svelte:1904` and `:1096`

The chip's own tooltip says *"The session is waiting on a key press. Sends a bare
Enter."* It calls `sendText('')`. With a context chip active, `promptPrefix` is
truthy and `''` does not start with `/`, so:

```js
const prefixed = promptPrefix && !text.startsWith('/') ? `${promptPrefix}\n\n${text}` : text;
```

…builds the entire context paragraph and `sendPrompt` types it into the pane.
Worse, the empty-text guard sits **after** the send (`:1128`):

```js
const r = await sendPrompt(session.host_alias, session.tmux_name, body);
...
if (text === '') return;
```

so no `pending` turn is recorded — the user sees nothing sent, and the paragraph
surfaces as a phantom prompt on the next poll. This is the recovery path for a
`press_enter`-stuck session, so it fires exactly when the session is already
wedged. The same path affects Shift+click on a non-slash preset (`:1015`).

### F3 — the agent sheet lost its "won't send while working" gate · **H**

Found independently by two lenses. `src/lib/ConversationPanel.svelte:956`

Before, the sheet's own composer gated on agent status:

```js
// AgentPanel.svelte at v0.2.35
const busy = $derived(statusNote !== null);
if (!session || busy || sending || !draft.trim()) return;
disabled={busy || sending || !session}
```

The refactor deleted that composer and delegated to `ConversationPanel`'s, whose
gate is **byte-identical before and after** (was `:942`, now `:956`) and has no
status term:

```js
const canSend = $derived(draft.trim().length > 0 && !sending && viewing === null);
```

`liveStatus`/`liveStuck` feed only the advisory note. The deleted test named the
consequence — *"two pastes into one REPL is one mangled prompt"* — and its
replacement (`AgentPanel.integration.test.ts:196`) was renamed "busy **gate**" →
"busy **note**" and asserts only that the note text appears.

How it slipped through: the old code's comment claimed the two composers shared
this signal *"so the two composers can never disagree about it"*. They only ever
shared `composerStatus`, the note — never the gate. The comment was already wrong
at v0.2.35, which made delegating look like a no-op.

### F4 — UX-132 is not fixed: row actions are revealed but cannot be activated · **H**

`src/lib/SessionRowItem.svelte:164` + `src/lib/Sidebar.svelte:533-538`

`:focus-within` now reveals the row actions and puts them in the tab order, but
the row's own handler cancels their activation:

```js
function onKeySession(e: KeyboardEvent, sess: SessionRow) {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault();
    onSelectSession(sess);
  }
}
```

`keydown` bubbles from the focused `<button>` to the row, and a button's
activation is the *default action* of that keydown — cancelled by an ancestor's
`preventDefault()` during bubbling. Net effect: the fix adds 5–6 dead tab stops
per row and reaches none of Restart / Edit label / Rename / Recreate / Kill. The
claimed coverage for UX-131/132 is `svelte-check`, which cannot see this.

Fix: guard on `e.target === e.currentTarget` before handling.

### F5 — `ConvTurn.reminders` escapes the conversation char budget · **H**

`crates/fleet-core/src/service/transcript.rs:600`, `:1259-1262`

`split_reminders` pushes the body with no cap, unlike every other carried text in
the file (`COMPACT_SUMMARY_MAX_CHARS`, `NOTIFICATION_RESULT_MAX_CHARS`,
`COMMAND_OUTPUT_MAX_CHARS`, `SUBAGENT_RESULT_MAX_CHARS`):

```rust
if !body.is_empty() { found.push(body.to_string()); }
```

and `trim_conversation` never sums it:

```rust
.map(|t| prompt_chars(t) + t.items.iter().map(item_chars).sum::<usize>())
```

Before this change the reminder rode inside `turn.prompt`, so it was counted and
truncatable. Now a session that has `/clear`-cycled a dozen times carries a dozen
uncapped copies of a multi-kilobyte project-instructions block past the
64 KB/512 KB budget, on the hub wire, in the MCP `session_conversation` answer,
and in the panel. `truncated` is also wrong: the loop can empty a turn of every
item, report `truncated: true`, and still serialise that turn's full reminder.

### F6 — two new `ConvItem` variants break an older desktop's parse · **H**

`crates/fleet-core/src/service/transcript.rs:319-320`; `crates/fleet-core/src/wire_contract.rs:62`

`ConvItem` is an internally-tagged enum with no `#[serde(other)]` and no
catch-all variant, and `CONTRACT_REVISION` is still `3` on both sides. A v0.2.35
desktop against a HEAD hub receives `{"kind":"bash",...}` → `unknown variant
'bash'` → the **entire `Conversation`** fails to deserialise, so the tab shows a
parse error instead of one degraded line. The skew banner never fires because
both sides report revision 3.

The asymmetry is documented for the *other* client — `transcript.rs:1816-1838`
says fleet-mobile "falls back to an `Unsupported` placeholder … so that an older
phone against a newer hub degrades instead of throwing" — but the desktop's own
copy of the enum has no such fallback.

### F7 — `session_activity` is routed to a tool older hubs do not serve · **H (for this deployment)**

`src-tauri/src/backend/verdicts.rs:259-264`; `src/lib/ConversationPanel.svelte:876-885`

`session_activity` is **brand new** in this diff (0 occurrences at v0.2.35), the
desktop flipped it `LocalOnly` → `Routed`, and `CONTRACT_REVISION` was not bumped.
A HEAD desktop paired with a hub pinned to an earlier release tag — which is how
`fleet.rlt.sk` is deployed — calls a tool the hub's router does not know.

`probeNow` swallows it (`:858`):

```js
if (!r.ok) return;
```

So the user gets **exactly the "no live indicator" state the change was meant to
remove**, while the app issues a failing round-trip every `ACTIVITY_POLL_MS`
(2 000 ms) per open panel and fills the hub's log. `wire_contract.rs`'s "when to
bump" list covers row fields and event kinds but says nothing about routing a
command to a tool that did not previously exist, so no invariant catches this.

### F8 — the UX-05 self-heal does not fire for real rows · **M**

`crates/fleet-core/src/service/sessions/prompt.rs:377-381`, `:443`

The heal keys on `row.last_prompt`, but `set_last_prompt` runs **unconditionally
on every prompt** (`:421`), before the `replaceable` computation reads the
previously-stored value. So the window is one prompt wide.

A session named `yes` heals only if the very next fleet prompt is a labelling
one. Send `yes`, then `ok`, then a real prompt: `legacy_label_from_prompt("ok")`
is `Some("ok")`, which is not `"yes"` → `replaceable = false` → the row stays
`yes` forever. That is the exact population the heal was written for. The test
(`sessions/tests.rs:3703-3712`) sets `last_prompt` and the name back-to-back with
nothing in between, so it passes while the field case does not.

Also here: a stale copy of the pre-change comment was left directly above its
replacement (`:431-436`) — two contradictory paragraphs describing `replaceable`.

### F9 — `AssetsPanel`'s error is a shared bucket, so UX-130 is reachable again · **M**

`src/lib/AssetsPanel.svelte:175-181` (also `:117`, `:163`)

After a failed catalog load the new Retry block renders. One click on **Sync**
(always enabled) sets `error = null` on entry and leaves it null on success,
while `$catalog` is still null — so `{#if $catalog}{:else if error}{:else}` falls
through to `<p class="muted">Loading…</p>`, permanently, with Retry gone. `doPush`
and `scan` open the same window. The fix keyed its recovery UI off a variable
four unrelated handlers reset.

### F10 — `:focus-within` re-inverts the UX-133 trash-button fix for keyboard users · **M**

`src/lib/Sidebar.svelte:1080` vs `:1181`

```css
.proj-row:focus-within .purge-btn { opacity: 0.6; }   /* (0,3,0) */
.purge-btn:disabled               { opacity: 0; }     /* (0,2,0) */
```

Specificity, not order, decides: the *disabled* destructive trash renders at 0.6
on keyboard focus — more prominent than the 0.35 the same commit deliberately
chose for the working-on-hover case. The hover path got a `:disabled` companion
rule; the focus path did not.

### F11 — the toast stack is still uncapped · **M**

`src/lib/toasts.ts:80` — `toasts.update((arr) => [...arr, {...}])`, no slice, no
max. UX-135 called out "unbounded **and** no dismiss-all"; only the dismiss-all
half shipped. N failing sessions still push N sticky errors with distinct
`code+message` keys (so dedup does nothing), and because `.toasts` is a
bottom-anchored column with *Dismiss all* as its first child, the button is
pushed off-screen exactly when it is needed.

### F12 — `agent_context.ts` is still a third session-name policy · **M**

`src/lib/agent_context.ts:23` — `friendly_name || tmux_name`, unconditional.
UX-129's diagnosis named this file; the fix covered only the terminal header. With
`$showFriendlyNames` off, the sidebar row and terminal header show the tmux name
while the agent chip and prompt prefix name the session by the friendly name the
user has told the app not to show.

### F13 — `onRepair` is still a one-click destructive action · **M**

`src/lib/SessionDetails.svelte:659` — `🩹 Repair workspace`, `onclick={onRepair}`,
no dialog. `repairSession(..., { explicit: true })` is documented three lines
above as able to "respawn a live pane", i.e. kill the running claude — the same
consequence Restart now confirms for. UX-126 says *"To isté `onRepair`"*; the fix
row lists only Restart.

### F14 — `split_reminders` strips reminders from anywhere in an entry · **M**

`crates/fleet-core/src/service/transcript.rs:588-604`

The file states the opposite rule three times (e.g. `:810-812`: *"a human prompt
that merely contains one (a pasted transcript) stays a prompt"*), but
`split_reminders` has no starts-with guard. A user pasting a transcript excerpt to
ask about it has the quoted block silently excised from mid-sentence and moved
into a collapsed chip.

### F15 — `lone_block` demotes and truncates a legitimate prompt · **M**

`crates/fleet-core/src/service/transcript.rs:644-665`, `:925-935`

A prompt that is one hyphen/underscore-tagged element becomes a `Harness` item
with `prompt: None` and a body cut to 4 000 chars. `tag.contains('-') ||
tag.contains('_')` is the only discriminator, so `<div>` is rejected but
`<my_config>` is accepted — pasting a config fragment to be analysed loses the
prompt. Minor: `open_len` takes the *first* `>`, so `<a-b title="x>y">` leaks
`y">` into the body.

### F16 — the two new `ConvItem` variants are not pinned by the contract golden · **M**

`src-tauri/src/backend/tests_contract.rs:377-431` pins every variant except
`::Bash` and `::Harness`. The map is hand-built, so a later rename of
`Harness.body` or `Bash.stdout` passes `the_hubs_field_names_are_the_ones_the_desktop_reads`
and ships a silently-dropped field — the exact hole that test exists to close.

### F17 — `isolate_git_config` mutates process-global env from a test thread · **M**

`crates/fleet-core/src/service/catalog/repo.rs:1050-1071` calls
`std::env::set_var` / `remove_var` inside a `Once`. `Once` guarantees it runs
once, not that no other thread is in `getenv` — and the `fleet-core` test binary
has 158 `Command::new` sites (each `fork`/`exec` reads `environ`) and 35
`env::var` reads running in parallel. The crate documents the rule it breaks, at
`service/projects.rs:421`: *"The env var is process-global; tests never set it
(set_var races other tests)."* It also sets `GIT_CONFIG_GLOBAL=/dev/null`
permanently for every later test in the binary that shells out to git.

### F18 — the MCP description budget was raised to 14 bytes of slack · **M**

`crates/fleet-core/src/mcp/tools/tests.rs:2625` raises the cap by 343 bytes for a
tool whose served cost is ~429. Against the constant's own documented baseline
(58 957), the surface lands at ~59 386 under a 59 400 cap — where every previous
raise documented 100 bytes of headroom. Not papering over creep (the tool is
genuinely new), but the next one-line description change fails CI.

### Low

- `src/lib/CommitGraph.svelte:100`, `src/lib/Sidebar.svelte:779` — row `onkeydown` has no `e.target` guard, so Enter on a nested button both activates it and selects the row. Milder than F4 only because these do not `preventDefault`.
- `src/app.css:52` vs `src/App.svelte:901`, `src/lib/AgentPanel.svelte:140` — "one source of measurements" is aspirational: `.status` still hardcodes `height: 24px` (plus a 1px border and no `box-sizing: border-box`, so the token is already 1px wrong) and the sheet hardcodes `bottom: 80px`.
- `crates/fleet-core/src/service/sessions/prompt.rs:381` — `is_legacy_derived_junk` can declare a *human-chosen* name replaceable when that name equals the legacy reduction of the last prompt, contradicting the comment above it.
- `src/lib/McpConfirmDialog.test.ts:91` — the UX-124 test asserts the `data-autofocus` attribute, not `document.activeElement`; it passes even if `Modal.svelte` stops honouring it.
- `src/lib/McpConfirmDialog.test.ts:100` — titled "keeps the request queued **and reports**", asserts only the queue; deleting the `pushError` line still passes.
- `src/lib/ConversationPanel.test.ts:477,506,531` — three "not raw XML" assertions are vacuous: the fixtures are already-parsed items.
- `crates/fleet-core/src/service/transcript.rs:4111,4122` — two new tests pass identically before and after the change (both inputs are rejected by `lone_block` on other grounds, so neither covers the accepting case).
- Untested claimed fixes: `confirm-restart-details` (`SessionDetails.svelte:783`), `toast-dismiss-all` and the `role="alert"` removal (no `Toasts.svelte` component test exists), the UX-08 zero-count hide (`SidebarFilters.svelte:148`).
- Stale comment `src/lib/AgentPanel.svelte:46` — "the composer's busy gate … hang off this"; the sheet has neither a composer nor a busy gate now.

## Claims audit

Against `iterations/11-expert-review-and-fixes.md` §3.

**Hold:** UX-124, UX-125, UX-127, UX-131, UX-134, UX-08 (remainder).

**Partly hold:** UX-126 (`onRepair` unguarded → F13) · UX-128 (three of four layers ignore the tokens) · UX-129 (`agent_context.ts` → F12) · UX-130 (reachable again → F9) · UX-133 (keyboard path re-inverts → F10) · UX-135 (still uncapped → F11).

**Does not hold:** UX-132 (F4).

**Correctly recorded as deferred:** UX-136, UX-137, UX-138, UX-139 — no code change, consistent with §4.

Additionally, the composer refactor (#240, commit `ac403084`) is not described in
the iteration doc at all, which is how F3 shipped without anyone weighing the
trade it makes.

## Clean

- **Shell quoting.** The diff adds no SSH/bash command construction; `session_activity` reuses the existing `capture_pane_scrollback` builder and `send_system_prompt` reuses `send_prompt_inner`'s `build_send_script`.
- **No `Store` guard held across `.await`** in `record_prompt_outcome`, `send_prompt_inner` or `service::sessions::session_activity`.
- **No blocking I/O on a sync Tauri command** — both new command paths are `async fn`.
- **Hub parity bookkeeping for `session_activity` is complete** — verdict row, dispatch by command name, regenerated json, `docs/hub.md` counts (39→40 routed / 70→69 refuse), allowlist removal, routing test with non-default args. Only the *compatibility* question (F7) is unanswered.
- **`ConvTurn.reminders` carries `#[serde(default)]`**, so an older hub that omits it still decodes.
- **No panics on untrusted transcript input** — every index comes from a `find` offset plus an ASCII-constant length; the one risky slice uses `.get(open_len..)?`. All three loops advance strictly monotonically and are single-pass.
- **Svelte 5 runes** — no `$state` mutated from a `$derived`, no `$effect` that writes state it also reads, and every `$effect` in `ConversationPanel.svelte` returns a teardown.
- **Generated artifacts** — `control-api-reference.md`, `hub_verdicts.generated.json`, `docs/hub.md` and both goldens reconcile with their generators. No regen drift.

---

## Resolution — 2026-09-23

Every finding above is fixed on `feature/agent-expert-codereview-20-7aa88d`.
Five implementation lanes with disjoint file ownership; each wrote a failing
test first where the defect was testable.

### Verification of the combined tree

| Check | Result |
|---|---|
| `npx vitest run` | **2497 passed**, 133 files, 0 failed |
| `npx svelte-check` | 0 errors, 0 warnings |
| `cargo test --workspace` | **3110 passed**, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (local clippy 0.1.94; CI runs newer) |
| `cargo fmt --all --check` | clean |

### Notes that outlived their finding

**F1 — the prescribed fix was wrong, and the lane caught it.** Writing to a
temp path and `rename`-ing into place does *not* close the `ETXTBSY` race:
`rename` preserves the inode, so a child forked mid-write still holds a write
fd on exactly the inode the final name now points at. The landed fix adds a
probe: after the rename, exec the file once with a guard argument and retry
while that exec reports `ETXTBSY`. A successful exec is the only available
proof that no writer remains, and nothing reopens the file afterwards.

**This fix cannot be proven on macOS.** Darwin does not enforce `ETXTBSY` on
exec at all — verified directly: a file held open for writing, renamed, then
exec'd returns 0 here. The 25× local stress run (25/25 green, 2 457 tests each)
proves no regression and nothing more. Both red `main` runs were on Linux
runners, which is where the inode writer-count check lives. **Only CI on
`ubuntu-24.04` confirms this one.**

**F6 — serde could not express the obvious fix.** `#[serde(other)]` on an
internally-tagged enum accepts only a unit variant, which loses the tag name
and renders as a silent blank (the panel's `{#if}` chain has no `else`). A
second variant carrying `rename = "harness"` compiles but triggers
`unreachable_patterns`, which CI's `-D warnings` rejects. The landed shape is a
`deserialize_with` on `ConvTurn::items`: an unreadable item degrades to a
labelled `Harness` line — *"This build cannot read a `bash` conversation item.
It came from a newer hub; update claude-fleet"* — which every existing renderer,
including the phone, already folds. **There is deliberately no
`ConvItem::Unsupported` variant**: a new variant with its own `kind` would be
skipped by every renderer older than it, which is the failure F6 exists to
prevent.

**F6/F7 — the revision bump had a second half.** `CONTRACT_REVISION` 3→4 alone
is a shipped outage *against this build's own hub*: `MIN_HUB_CONTRACT` /
`MAX_HUB_CONTRACT` in `src-tauri/src/backend/contract.rs` gate the accepted
range and were still `3..=3`, so the desktop refused every row from the hub it
ships with (26 tests red). Both constants are now `4`. That is also what makes
F7 work as intended: a revision-3 hub — how `fleet.rlt.sk` is pinned — is now
`TooOld` and raises an honest skew banner instead of silently failing
`session_activity` every two seconds.

**F8 — the fix changed shape.** Keying the self-heal on `last_prompt` cannot
work, because `set_last_prompt` runs on every prompt. The landed rule keys on
the *name itself*: a name is legacy-derived junk iff it is its own canonical
reduction **and** its shape is one the old reducer only produced from junk (a
single word, or a stop-list ack as the first word). Intervening acks are now
irrelevant, and `code review` — two ordinary words — is no longer mistaken for
junk, which also closes the Low about human-chosen names. Documented residue: a
lowercase single word a human chose (`reviewer`) is indistinguishable from
`push`. Accepted, since there is no "who named this" bit and migration 040
stays rejected.

**F3 — the gate is scoped, not global.** Restoring "won't send while working"
to the *shared* composer would have imposed a new restriction on the
Conversation tab, which never had it and where typing ahead of a running turn
is deliberate. The gate is opt-in via `blockWhileBusy`, set only by the agent
sheet, with a test that goes red if it ever leaks to the tab. One consequence
had to be handled: the busy signal is also non-null when the session is
*stuck*, including `press_enter` — gating there would have disabled the very
chip that recovers the session, so a bare key press is never gated.

**F11 — eviction prefers the oldest transient.** A non-sticky toast was going
to vanish in four seconds anyway; only when none remain does the oldest sticky
error go, and the newest entry is never a candidate. Silently dropping a sticky
error on arrival means the user never learns the thing failed, which is worse
than the overflow bug. Every eviction increments a counter the UI renders as
`+N older not shown`, so the visible count is never presented as the whole
story.

**F12 — `friendly` is a required field.** It was implemented as optional first,
because making it required broke another lane's in-flight file. It is now
required: an optional field with a default is precisely how this divergence
would return silently, and a required one makes the compiler ask.

### Carried forward, deliberately not done

- **`CommitGraph.svelte` has no test file.** The `e.target === e.currentTarget`
  guard there ships on the strength of the identical, tested mechanism in
  `Sidebar.svelte`. Proving it needs a new test file.
- **F10 has no test.** Component CSS never reaches jsdom (the codebase says so
  itself at `SessionRowItem.svelte:89-91`) and the bug is purely a specificity
  outcome. An attribute-shaped assertion that passes whatever the CSS says is
  the exact failure mode this round is correcting, so none was written.
- **The five other write-then-exec sites** the review counted. `fake_exec` is
  `pub(crate)`; adopting it elsewhere is a one-line change per site.
- **No global `box-sizing: border-box`.** Scoped to `.status`, which is what
  the token-honesty fix needs; a global change would move every component in
  the app.
- **UX-136/137/138/139** remain deferred as `iterations/11` §4 records.

### Filesystem hazard worth remembering

This checkout is on a case-insensitive volume (APFS): `src/lib/Toasts.test.ts`
and `src/lib/toasts.test.ts` are **the same path**. Writing the new component
test at the capitalised name silently overwrote the existing store test. It was
restored byte-exact (`git diff` on it is empty) and the new tests live at
`src/lib/Toasts.svelte.test.ts`, following the repo's existing
`Attention.svelte.test.ts` convention.
