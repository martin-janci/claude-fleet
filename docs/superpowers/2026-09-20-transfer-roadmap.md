# Transfer: where it stands and what comes next

**Date:** 2026-09-20
**Read this first** when picking the Transfer work up in a new session or on
another machine. It carries what the specs do not: the order of the remaining
slices, the open design questions, the debts, and the working rules that were
learned the hard way and live in no other file in this repo.

## The goal

One easy button that successfully transfers a Claude Code session to another
fleet host "with everything" — the worktree, the uncommitted and unpushed
work, the small ignored files, Claude's own state — between any two hosts.

## What is done

| Slice | What | Where |
|---|---|---|
| 1 carry engine | `move_session` carries uncommitted, unpushed and small git-ignored work as it is; `strict: true` restores the old refusals | PR #163 · spec `specs/2026-09-19-move-carry-engine-design.md` · ADR 0002 |
| 2 Claude-side state | the per-session directory (subagents, tool results, title) and the project's Claude memory travel too; they only ever add on the target | PR #194 · spec `specs/2026-09-20-move-carry-claude-state-design.md` |
| 3d retry + return trip | adopt a target that already holds the work; `clean_target`; `resolve_move` Finish/Undo; a preflight refusal when the target already runs this conversation; the sheet's and panel's actions | spec `specs/2026-09-20-transfer-retry-design.md` · plan `plans/2026-09-20-transfer-retry.md` |
| 3a Transfer sheet | a button on the terminal header's host name (and in the details panel); nine live steps from the `move:progress` event; closable while the move runs; a readable result and honest failure text | merge `8356217` · spec `specs/2026-09-20-transfer-sheet-design.md` (read its section 7, "Revisions") · plan `plans/2026-09-20-transfer-sheet.md` |

Slice 3a's code map: `crates/fleet-core/src/events.rs` (`MoveStep`,
`MoveProgress`), `service/move_session/progress.rs` (the emitter),
`src/lib/moveProgress.ts`, `moves.ts` (the run store), `moveEligibility.ts`,
`moveErrors.ts`, `TransferSheet.svelte`, `TransferChip.svelte`.

## What comes next, in order

Each part gets its own brainstorm → spec → plan → subagent-driven execution →
whole-branch review → PR, exactly like the slices above.

### 3d — Retry and the return trip (LANDED)

Spec `specs/2026-09-20-transfer-retry-design.md`, plan
`plans/2026-09-20-transfer-retry.md`. What shipped: content-exact adopt of a
target that already holds the work (`carry::verify_replayed_script` over a
throwaway index), `clean_target` to replace an unfinished attempt's leftovers
(never the target's own work), `resolve_move { Finish | Undo }` for a partial,
a preflight refusal when the target host already runs this conversation, and the
sheet plus details-panel actions. The sections below are kept for the record of
what the question was.

### 3d — the original sketch

**Why first:** it is the part that makes a failed move recoverable, and 3a's
failure sheet currently ends in a single "Done". It is also where the engine
still contradicts the UI: a failed attempt leaves the clone, the worktree and
copied files on the target, and the NEXT attempt then meets
`E_MOVE_TARGET_DIRTY` ("the target has uncommitted work — possibly an
unfinished earlier attempt's"). A return trip has the same shape: after a
successful move the source worktree "still holds a copy of the uncommitted
work" (a warning 3a already shows), so moving BACK meets a dirty target too.

The one design question underneath both: **when are a target's leftovers
ours?** Candidates to weigh in the brainstorm:
- recognise them by lineage — the transfer id / `refs/fleet/transfer/<id>`
  left in place until the move is confirmed, or a marker file in the
  worktree's git dir naming the Claude session id and source HEAD — and let a
  retry replay over exactly that state;
- verify-then-adopt — if the target's porcelain already equals the source's
  snapshot, skip the replay instead of refusing;
- never guess — keep refusing, but give the sheet a "Clean up {host} and
  retry" action that removes only what the failed attempt created.

Scope sketch: `Retry` on the failure view (same target, same options);
`Move back to {host}` on the result view and on the chip of a session whose
`parent_session_id` points at a moved source; the engine change above; failure
text that names the leftovers precisely. Non-goals: cancelling a running move
(needs cancel-safe cleanup — see debts), bulk moves.

### 3b — Preflight (next)

Before the move starts, the setup view shows what WOULD travel and what would
be refused: commits and dirty entries, ignored files with sizes and the ones
that stay behind (and why), session-state and memory counts, and the refusals
that can be known early (source busy, mid-merge, target unreachable or not
provisioned, target dirty). Design questions: a read-only `preflight_move`
service + Tauri command routed to the hub (a row in `backend/verdicts.rs`, the
routing table, `REGEN_HUB_VERDICTS`) versus a `dry_run` flag on
`move_session`; whether the MCP surface gets it at all (the served tool
surface is byte-budgeted — default to no); how stale a preflight may be when
Transfer is pressed (the move re-checks everything anyway — the preflight is
advice, never a gate).

### 3c — Wait for idle

Today a busy source is refused ("The source Claude is in the middle of a
turn"). 3c turns that into "Transfer when it finishes": the run sits in a
`waiting` state, the chip says so, and the move starts when `claude_status`
goes idle — with a way to cancel the wait. Design questions: where the wait
lives (frontend store vs a backend task that survives the window closing;
hub-client mode says backend), a timeout, what happens if the session asks a
question instead of finishing (`blocked` is not idle).

## Debts and follow-ups (none blocks the slices above)

From slice 3a's reviews — decided, not forgotten:
- A run whose hub goes quiet never settles by itself; "Stop following" is the
  way out. A staleness rule or settle-on-source-row-removed would close it.
- For 5 s after a local run settles, events only patch its steps; a NEW move of
  the same session started inside that window is drawn on the old run. A
  per-move id in `MoveProgress` would remove the need for the window.
- `E_HUB_UNREACHABLE` covers both "no answer" and definite connect-phase
  failures (refused, DNS, TLS, 404). The sheet now shows the message, but
  `src-tauri/src/backend/remote.rs` should mark connect-phase failures so the
  store can call them failed.
- `moveErrors.ts` matches the `partial(…)` step strings by value with a
  neutral fallback; they are not pinned against Rust.
- The red/amber literals (`#e64a4a`, `#d29b4a`) are not theme variables — the
  app has none for danger/warning.
- Nobody has clicked through the sheet in the real app: a dev build was
  deliberately never run (see the rules below). Do it once on a released build.

From slice 3d's reviews — decided, not forgotten:
- A per-move id in `MoveProgress` would retire BOTH the `SETTLE_GRACE_MS`
  window and the `awaitingStart` flag the retry path now needs. Until then one
  hole stays open: `Done` on a failed sheet calls `dismissMove`, so the next
  `startMove` of that session does not count as "replacing" and a straggler can
  still settle an `E_HUB_UNREACHABLE` run as failed. Self-healing, but real.
- `clean_target` is documented on the MCP **parameter**, not in
  `move_session`'s description, because the served-surface budget had 68 bytes
  and the clause cost 113. `docs/control-api-reference.md` lists parameter names
  only, so a reader of that file sees the flag with no explanation, and the
  "never the target's own work" guarantee survives only on the non-served args
  struct. ~31 bytes of budget remain — folding three words into the parameter
  doc would restore it.
- `Ours` cannot separate an unfinished attempt's leftover from the target's own
  edit to a file the snapshot also writes; `clean_target`'s reset to `HEAD`
  discards the latter. Fenced by the flagless refusal and by the sheet naming
  every path, but inherent to path-based classification.
- Sparse-checkout: a `read-tree`-seeded throwaway index can carry
  `skip-worktree` bits, which `update-index --refresh` does not check, so a path
  outside the cone could read clean while differing on disk. `apply_script`'s
  existing `read-tree -u` has the identical exposure.
- `parse_leftovers` drops a path containing a TAB from the advisory list (the
  adopt is still aborted); `verify` leaves an empty
  `~/.cache/claude-fleet/transfer/<id>` when `cleanup_script` never runs;
  `home_guard` rejects an empty `$HOME` but not a relative one.
- `putRunForTest` in `moves.ts` forges arbitrary run state past every status
  guard, protected only by its name.
- Tests worth adding: the `session_moved` detail's `from_session_id` and
  `source_killed` are asserted by nothing, so transposing the two row ids would
  go undetected; the `finalise_source` seam test drives the transcript
  comparison in the inverted direction (a shrinking transcript, which no real
  write causes).
- `finalise.rs`'s `extra_detail` merge skips a colliding key with a
  `debug_assert!`; in release a collision is silently skipped rather than
  reported.

From slices 1–2:
- A real move through the app followed by `cl --resume` on the target — needs
  a released build.
- The harness has only run macOS → Linux (`mefistos`); Linux → macOS is
  untested.
- Cancel-safe cleanup: a dropped move future emits `failed` now, but
  `CarryCleanup` does not run for it.
- Chunked UPLOAD (download is chunked; upload is one `put`).
- Byte bounds in the carry script builders (the whole script is one argv
  word, 128 KiB at most).
- A delete-after-use sweep of `~/.cache/claude-fleet/transfer/` leftovers.
- A per-file-type merge policy for the session directory ("larger is newer"
  is only true for `*.jsonl`).

## Working rules that are not written anywhere else in the repo

Safety:
- **Never run a dev build of the desktop app** (`cargo tauri dev`,
  `pnpm tauri dev`, `cargo run -p claude-fleet`) on a machine that has the
  installed app: the data dir is derived from `HOME`, so it migrates the
  production `state.db` irreversibly, and the singleton guard kills the
  running app. UI is verified by component tests; the carry by the harness.
- The cross-host harness: `cargo run -p fleet-core --example carry_e2e -- <ssh-host>`.
  It runs this build's real scripts locally and over ssh, and creates/removes
  only `/tmp/cf-e2e.*`, `~/.claude/projects/*cf-e2e*` and its own transfer
  dirs. Ask before pointing it at a host; `mefistos` was approved for slices
  1–2 on the original machine.
- Do not print full process command lines (`pgrep -fl`, `ps aux`): MCP servers
  on the dev machine carry API tokens in argv. Use `pgrep -x cargo | wc -l`.

Repo mechanics:
- `main` moves ~50 commits a day. `git fetch` and compare before planning,
  before the final review and before the PR. A clean TEXT merge has hidden
  semantic breaks here twice — re-run the suites on the merged tree. GitHub
  shows CONFLICTING when `main` edits a file this branch renamed and grew:
  merge `origin/main` locally.
- Generated files CI checks: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
  (any `#[tool]` text or ANY new Tauri command); `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`
  (any command's hub verdict); `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`
  (a wire ROW type's keys — events are not in it; the regen run itself reports
  FAILED, re-run to verify).
- The MCP tool surface has a byte budget enforced by a test: add a slim clause,
  put prose in the ADR or the control skill, regenerate the reference — never
  hand-merge it.
- Wire rule: report/event types derive `Serialize + Deserialize` with no
  `#[serde(default)]`; a new argument on a hub-routed command needs
  `Serialize` and a non-default row in `src-tauri/src/backend/tests_routing.rs`.
- `gh pr edit` fails on this account (`read:project` scope): use
  `gh api -X PATCH repos/<o>/<r>/pulls/N -F body=@file`; merge with
  `gh api -X PUT …/pulls/N/merge -f merge_method=merge -f sha=<full sha>`.
  Merge commits, never squash. No attribution lines in commits or PRs.
- If `pnpm test` / `pnpm check` cannot find their binaries, use
  `npx vitest run` / `npx svelte-check`; run `pnpm install --frozen-lockfile`
  after every pull before calling a frontend failure pre-existing.

Subagent-driven development, as practised on these slices:
- One writer in the tree at a time; plan tasks on DIFFERENT files or every
  implementer and fix round serialises. Reviews run in parallel with the next
  implementer; findings that arrive while another writer is active are ruled
  into the final fix wave with `Ruling: … — cost if wrong`.
- Sonnet is the floor for implementers and reviewers: the cheapest tier twice
  presented a green run as the RED step. Reviewers check the RED evidence.
- Subagents never run `pull/push/rebase/checkout/stash/merge/reset` and never
  ssh; every git command is `git -C <worktree>`. After each one, verify the
  branch, HEAD and `git stash list`.
- The whole crate suite per task, unpiped — a filtered per-task run once hid a
  red test for five tasks; never judge CI through `| tail`.
- The final whole-branch review goes to the most capable model with the
  controller's own worries listed, then ONE fix wave, one scoped re-review,
  and the residuals are adjudicated rather than looped on. On 3a that review
  found six cross-task defects seven task reviews could not see — do not skip
  it. Component code drafted in a plan without being run is where they were.
- Ledger at `.superpowers/sdd/<plan>/progress.md` (git-ignored, does not travel
  between machines): trust it and `git log` over memory after a compaction.
