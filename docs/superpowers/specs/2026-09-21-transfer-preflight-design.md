# Transfer slice 3b — preflight

**Date:** 2026-09-21
**Status:** design, approved in brainstorm
**Follows:** slices 1, 2, 3a and 3d (`specs/2026-09-19-move-carry-engine-design.md`,
`specs/2026-09-20-move-carry-claude-state-design.md`,
`specs/2026-09-20-transfer-sheet-design.md`,
`specs/2026-09-20-transfer-retry-design.md`)
**Roadmap:** `docs/superpowers/2026-09-20-transfer-roadmap.md` → "3b"

## 1. What this slice adds

Today the Transfer sheet's setup view offers a host list, a `keepSource`
checkbox and a button. Everything about what will actually happen — which
commits travel, which uncommitted files, which ignored files are too big, how
large the transcript is, whether the target is even provisioned — is discovered
by pressing Transfer and reading what came back.

3b puts that in front of the button: **what would travel, and what would be
refused, before anything is written.**

## Revisions, made while planning

Planning against the real code found five places, and review found two more (6, 7), this spec asked for more than
the move can give. The plan (`plans/2026-09-21-transfer-preflight.md`, "Corrections
to the spec") argues each in full; the sections below have been updated to match.

1. **No accumulation.** `gather()` short-circuits exactly as the move does, so a
   preview reports the one refusal the move would raise (§5).
2. **"Nothing writes" means nothing writes to a host.** The source reconcile
   (`refresh_host`) updates the local store, as the background tick does (§4).
3. **`TargetState` describes the path the move would aim at**, which
   `ensure_target_workspace` may resolve differently on repair (§3).
4. **A refusal is the move's own error.** `refusals`, `Refusal` and
   `transcript_over_cap` are gone: under revision 1 a successful preview can hold
   no refusal, so a dry run returns `Err` with the same code and message the real
   move would (§3, §5).
5. **`unpushed_commits` and `target_path` are added** — the first because
   `commits_ahead` is `None` on every first transfer to a host (§3).

6. **A dry run never fetches** (found by the whole-branch review). The move's
   source inspection fetches `origin/<branch>` when its tip is not local; a dry
   run skips that fetch and runs its git calls under `GIT_OPTIONAL_LOCKS=0`, so
   it writes nothing on the source either. The cost is the slice's one
   sanctioned drift: when origin has commits the source has not fetched, the
   preview's unpushed count is unknown, and a strict dry run cannot decide the
   unpushed refusal — both say so in `unknowns` rather than guess. The real
   move keeps fetching; its script is byte-identical to before (its `git status`
   index write is load-bearing for the snapshot's racily-clean handling).
7. **The desktop refuses a preview until the hub's contract is confirmed.** An
   older hub would silently run a real move for a `dry_run`, and the contract
   check only engages once a `ready` frame arrives. So in hub-client mode the
   desktop answers a dry run with `E_HUB_CONTRACT` until this launch has seen an
   in-range `ready` frame. Real moves are unaffected.

## 2. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | How is the preflight exposed? | A **`dry_run` flag on `move_session`**, not a new command. |
| D2 | What does a dry run return? | A tagged **`MoveOutcome`** enum: `Moved(MoveReport)` or `Preview(MovePreview)`. |
| D3 | How far does it go? | **Read-only, one call.** Everything it reports is exact; what a read-only pass cannot know is named as unknown, never estimated. |
| D4 | When does it run? | On the setup view having a target and on every target change, debounced. It **never gates Transfer**. |

### Why `dry_run` and not a `preflight_move` command

Three reasons, in the order that decided it:

1. **One code path.** The move's own preflight becomes the function a dry run
   returns (§4), so the preview cannot drift from what the move does. A separate
   service would be a second implementation of the same checks — which is the
   defect 3d's `finalise_source` extraction exists to prevent, and a preflight
   that can disagree with the move is worse than no preflight.
2. **A routed command needs an MCP tool, and the surface is full.** 3d
   established that `Verdict::Routed` resolves its tool through the MCP table, so
   a routed `preflight_move` would need a tool of that name. The served surface
   measured **56,831 bytes of a 57,000 budget — 169 bytes of slack**; a two-field
   tool costs several hundred, so this would force a third deliberate raise.
   One boolean parameter fits.
3. **No new machinery to get wrong.** No verdicts row, no routing row, no
   `REGEN_HUB_VERDICTS`. 3d shipped a routed command whose new argument silently
   never reached the hub, because the argument was added to the service and the
   command but not to the tool's parameter struct; every row that does not exist
   is a row that cannot have that bug.

The cost, recorded rather than hidden: `move_session` is `readonly: false`, so a
**readonly client token cannot preview a move**. Acceptable — such a token
cannot perform one either.

### Why a tagged enum and not a `MoveReport` with a preview field

Keeping the return type and leaving `target_session_id`, `transcript_bytes`,
`source_killed` and `carried` at their defaults for a dry run is precisely the
lie 3d's whole-branch review caught in Finish: a default `CarryReport` that
claimed nothing travelled. Here it would assert a session id of `0` and
`source_killed: false` as facts about a move that never ran. The enum makes it
impossible to read a preview as a completed move.

The wire break is contained: every caller is in this repo (the Tauri command,
the MCP tool, the hub route, `moveSession.ts`), the hub contract golden holds 32
types and **none** of them move-related — so `REGEN_HUB_CONTRACT` is not
involved — and `docs/control-api-reference.md` regenerates itself.

## 3. The wire

`MoveSessionArgs` gains:

```rust
/// Report what this move WOULD do and change nothing on either host: no
/// snapshot, no clone, no worktree, no transcript copy, no fetch, no tmux, no
/// timeline event. `strict` is honoured (it is a read-only verdict); a
/// `clean_target` is named in `unknowns` and not applied. Default false.
#[serde(default)]
pub dry_run: bool,
```

The return type becomes:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MoveOutcome {
    Moved(MoveReport),
    Preview(MovePreview),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePreview {
    pub session_id: i64,
    pub from_host: String,
    pub to_host: String,
    pub branch: String,
    pub source_cwd: String,
    /// Commits the source has that origin lacks; `None` when git could not say.
    /// Always available, unlike `commits_ahead`.
    pub unpushed_commits: Option<u32>,
    /// Commits the target's clone lacks. `None` when it has no clone yet, or
    /// when its branch tip is a commit the source has never seen — not zero,
    /// which would read as "the target is up to date".
    pub commits_ahead: Option<u32>,
    /// `git status --porcelain` rows, exactly as the carry would replay them.
    pub dirty: Vec<DirtyFile>,
    pub ignored_carried: Vec<carry::IgnoredEntry>,
    pub ignored_left_behind: Vec<carry::LeftBehind>,
    pub transcript_bytes: u64,
    /// Present on the source; the move applies its own caps to these.
    pub session_state_files: u32,
    pub session_state_bytes: u64,
    pub memory_files: u32,
    pub memory_bytes: u64,
    /// The path the move would aim at (revision 3).
    pub target_path: String,
    pub target: TargetState,
    /// What this preview cannot tell you, in words a person can read: both
    /// what a read-only pass cannot establish (§5) and anything about the
    /// request a dry run ignores (§6).
    pub unknowns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TargetState {
    /// No worktree there yet; the move would create it.
    Absent,
    /// The probe could not answer; the reason is in `unknowns`. Never a
    /// refusal — the move itself does not probe.
    Unknown,
    Clean { head: String },
    /// Its porcelain. Deliberately NOT classified as 3d's `ours` / `theirs`:
    /// that verdict comes from `carry::verify_replayed_script`, which compares
    /// against `refs/fleet/transfer/<id>/*` — refs a dry run never creates. A
    /// preview cannot know whose work a dirty target holds, and must not
    /// imply it can.
    Dirty { head: String, entries: Vec<DirtyFile> },
}

```

Every one of these derives `Serialize + Deserialize` with **no
`#[serde(default)]`**, per the repo's wire rule.

## 4. The engine: one function, two callers

The move's preflight becomes:

```rust
pub(super) async fn preflight(
    args: &MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
) -> Result<MovePreview, IpcError>;
```

`move_session` calls it as its first step and carries on from what it returns;
a dry run returns it and stops. It reuses, unchanged:

- **`snapshot()`** — the pure synchronous store pass: not-found, same host,
  wrong `kind`, missing `claude_session_id`, no repo, no worktree row, the
  `main` worktree, an unknown or offline host, an unprovisioned target, the
  3d twin refusal, and the source-idle check.
- **`# cf-move:inspect`** — worktree path, porcelain, HEAD, current branch,
  remote sha, ahead count, mid-operation marker.
- **`# cf-move:locate`** — the transcript's size, mtime and path.
- **`# cf-carry:ignored-list`** plus **`carry::select_ignored`** — the exact
  carry / leave-behind split, with `LeftReason` for each entry, under the same
  `move.ignored_entry_kb` and `move.ignored_total_mb` settings the move uses.
- **`claude_state::session_list_script`** + `parse_file_list`, and
  **`claude_state::memory_list_script`** + `parse_memory_list` — the counts and
  byte totals for the session directory and the project memory.
- A read-only probe of the target worktree for `TargetState`.

Roughly five round trips on the source and two on the target. Nothing in that
list writes to either host; the source reconcile updates the local store's rows,
as the background tick does (revision 2), and the target probes run git under
`GIT_OPTIONAL_LOCKS=0` so that even `git status` leaves the index untouched.

## 5. What is exact, and what is not

**Exact:** every refusal above, the dirty set, the unpushed commit count, the
ignored split with its reasons, the transcript size against its cap, the session
state and memory counts, and the target's HEAD and porcelain when its worktree
exists (though not whose work a dirty one holds — see `TargetState::Dirty`).

**Named as unknown, never estimated:**

- **The bundle size**, and therefore `E_MOVE_TOO_LARGE`. Knowing it means
  snapshotting — writing refs and a bundle file. A guess from commit counts and
  dirty byte totals is wrong in both directions because of packing, and "about
  400 MB, cap is 500" followed by a refusal at 520 is worse than saying nothing.
- **A target worktree that does not exist yet.** `ensure_target_workspace` is
  what creates it, so its cleanliness is unknowable until the move runs.
  `TargetState::Absent` says exactly that.

**A refusal is the move's own error, and there is only ever one.** The preview
runs the move's opening sequence (`gather()`) unchanged, and that sequence
short-circuits on its first problem — the store checks, the source reconcile and
idle check, the source's git state, the transcript's size. So a dry run that
meets a refusal returns `Err` carrying **the same code and message** the real
move would return: not a description of the refusal, the refusal itself. A
source that is both mid-merge and over the transcript cap therefore shows one of
the two, exactly as the move would.

What runs *after* `gather()` — the target's state, the ignored-file split, the
session-state and memory counts — is independent of it and only runs when it
succeeds. None of it can refuse: the ignored carry and the Claude-state carry
are warn-only in the move, so a failed listing lands in `unknowns`.

## 6. `dry_run`'s guard rails

- The MCP tool branches on `dry_run` **before** `confirm_gate`, so a dry run
  needs no confirmation (it writes nothing) and a real move can never reach that
  branch. `move_session` stays `confirm: true`, `readonly: false`.
- No `move:progress` events and no timeline event for a dry run: the sheet's
  nine-step view belongs to real moves, and a preview in the timeline would be
  noise in the durable record 3d leans on.
- **`strict` is honoured; `clean_target` is not, and the preview says why.**
  (Revised during Task 3's review, which found the first version saying strict
  was ignored while `gather()` enforced it.) `strict` is a read-only verdict
  evaluated inside the move's opening checks, so a dry run with `strict: true`
  reports exactly what a strict move would do — on a dirty or unpushed source,
  that is the refusal, with the move's own code and message. Ignoring it would
  make the preview disagree with a strict move: drift. `clean_target` only acts
  in the write phase, which a dry run never reaches, and whether it would fire
  depends on a target classification that needs transfer refs a dry run never
  creates — so `unknowns` names it and explains that.
- A dry run takes no move claim, so it can run while a real move of the same
  session is in flight and two of them cannot collide.

## 7. Frontend

- `moveSession.ts` returns `MoveOutcome`; a new `previewMove(sessionId, toHost)`
  wraps the `dry_run: true` call and narrows to `Preview`, returning an error if
  the backend ever answers `Moved` to a dry run.
- **A preview cannot reach the run store, by construction.** `moveSession()`
  narrows its answer to `moved` and a new `previewMove()` narrows to `preview`,
  each refusing the other kind — so `moves.ts`, which only ever calls
  `moveSession()`, needs no change and can never be handed a preview. (Stronger
  than `moves.ts` checking `kind`, which is what this bullet first asked for.)
- A small `preflight.ts` store holds the newest preview per `(sessionId,
  toHost)`, its `at` timestamp, and its in-flight state.
- The setup view requests a preflight when it has a target and on every target
  change, debounced (~250 ms) so flicking through a host list fires one call.
  It renders: what would travel (commits, dirty entries, ignored files with
  sizes, session-state and memory counts), what would be left behind and why,
  a refusal — in the same words the failure view would use — and the unknowns.
- **Transfer stays enabled throughout** — while a preflight is in flight and
  when one returned a refusal. A refusal is text beside the button, never a
  disabled button: the engine re-checks everything authoritatively, and a stale
  preview must not be the thing that stops you. `moveEligibility`'s existing
  `blocked` gate is untouched.
- A preview older than 30 s shows its age rather than presenting itself as
  current.

## 8. Testing

Engine, over `FakeSsh`:

1. A dry run **writes nothing** — assert no `# cf-carry:snapshot`,
   `# cf-carry:apply`, `# cf-move:prep`, upload or tmux call, and that
   `MoveHooks::ensure_target_workspace` / `start_target` / `kill_tmux_session`
   were never invoked.
2. The preview's `dirty`, `ignored_carried` and `ignored_left_behind` equal the
   same fixture's real move in `MoveReport.carried.dirty_entries`,
   `.ignored_carried` and `.ignored_left_behind` respectively — the test that
   pins "the preview cannot drift from the move". It runs the dry run and the
   real move over one fixture and compares, rather than asserting two
   hand-written expectations that could drift together.
3. A refusal from a dry run equals the real move's error — same code, same
   message — over one fixture. (By construction under revision 4; one
   representative plus the alias-validation case pins the mechanism.)
4. A malformed `target_host_alias` gets the same error from a dry run as from
   the move, since the move validates it before `gather()`.
5. `commits_ahead` is `None` when the target has no clone, not `0`.
6. `TargetState::Absent` when the target worktree does not exist.
7. The bundle size appears in `unknowns` and nowhere else — no numeric field
   for it exists.
8. `strict: true, dry_run: true` on a dirty source returns the same `Err` as a
   real strict move over the same fixture; `clean_target: true, dry_run: true`
   names the flag in `unknowns` and changes nothing else in the preview.
9. A dry run emits no `move:progress` event and inserts no timeline row.
10. A real move still returns `Moved` and behaves exactly as before — every
    pre-existing `move_session` test passes unedited apart from the mechanical
    `MoveOutcome` unwrap.

Frontend: `previewMove`'s narrowing including the `Moved`-to-a-dry-run error
path; `moves.ts` refusing a preview; the debounce firing once across rapid
target changes; Transfer enabled during flight and after a refusal; the 30 s age
display; and the setup view's rendering of each list including the
leave-behind reasons.

## 9. Non-goals

- No bundle-size estimate, and no numeric field that could be mistaken for one.
- No target-worktree creation, so no answer about an absent worktree's state.
- No new MCP tool, no `preflight_move` command, no verdicts or routing row.
- No change to what the move itself does — `preflight` is an extraction, and
  the existing tests are the proof.
- Not a gate: nothing here can prevent a Transfer.

## 10. Risks

- **The extraction touches the move's first step.** Behaviour preservation is
  the acceptance criterion, exactly as it was for 3d's `finalise_source`: the
  pre-existing `move_session` tests must pass unedited apart from unwrapping
  `MoveOutcome`, and that is stated as a task requirement rather than left for a
  reviewer to notice.
- **The return-type change is a breaking wire change.** Contained to this repo,
  with a regenerated reference and no contract-golden involvement — but every
  caller must be found. The compiler finds the Rust ones; `svelte-check` finds
  the TypeScript ones.
- **Five round trips on a slow link.** The setup view shows a spinner and stays
  usable; Transfer never waits. If this proves annoying in practice, the answer
  is a cheaper first pass (store-level only, instantly) rather than a cached
  preview that can be wrong.
- **The preview can still be out of date by the time Transfer is pressed.**
  That is by design and is why it never gates. The move re-checks everything.
