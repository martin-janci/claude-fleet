# HANDOVER — session environment variables and secrets

Branch: `feat/session-env-secrets` (pushed to origin). Cut from `origin/main` 27e1a2235; main is 13 commits ahead now (no conflicts expected; migration 173 is still free on main).
Status on 2026-10-12: **Tasks 1–5 of 9 implemented and committed; Task 5 not yet reviewed; Tasks 6–9 not started.**

**Update, 2026-10-12 (second client, branch `cf-2-environment-variables-and-secrets-management`):**
Tasks 5–9 are done. Task 5 was reviewed and amended: the env-file record
is `{"<host>": noted_at}`, `prepare` notes a host before and after the
write, the sweep forgets a clean host only when its note predates the
sweep (so a file written while the sweep looked is never orphaned), a host
with no row is forgotten unreached, and an unreachable host backs off with
the forms sweep's delay. Tasks 6–9 followed the plan; ruling 2 became the
split commands `env_var_set` / `env_var_unset` (no constant binding in
`resources.rs`). Main took migration 173 meanwhile, so the branch's
migration is **179** (`scripts/renumber-migrations.sh`). A move now checks
the target's environment among its opening checks (`env::check`), so a
dry run predicts an unresolvable reference before any work is carried
(deferred minor of Task 4). Rust verification ran on mefistos through the
CF-1 git relay (the user's rule), so the commits carry `--no-verify`.

Read in this order:
1. Spec: `docs/superpowers/specs/2026-10-11-session-env-and-secrets-design.md`. Its last section, *Revisions after the code survey*, overrides the sections above it.
2. Plan: `docs/superpowers/plans/2026-10-11-session-env-and-secrets.md` (9 tasks, TDD steps, Global Constraints).
3. This file. It records the decisions taken during execution that the plan does not contain.

## Done

| Task | Commits | Review |
|---|---|---|
| 1 Migration 173 + `store/env_vars.rs` | aca8d9a55 | clean |
| 2 `service/env.rs`: resolve / render / wrap / prepare, `E_ENV_UNRESOLVED` | 73ac8c4c8, c900a92bf | clean |
| 3 Create / restart / recreate wiring, `NewSessionArgs.env` | 3e4d9ae7f, 44c04111b (+ 23cd2fd87, see below) | clean after 1 fix round |
| 4 Repair / review / move wiring, `Store::copy_session_env` | dbc02db86 | clean |
| 5 Tick sweep `sweep_env_files` + diagnostics masking | afe2efcba | **NOT REVIEWED** |

**Extra commit 23cd2fd87, `fix(provision)`.** Pre-existing bug on main: secret files written over SSH by `write_host_file_secret` ended at mode **0644**, not 0600. That covers hook tokens, `~/.claude.json`, form secrets and catalog secrets. `atomic_upload_command` replaced the 0600 tmp file. The fix was reviewed as real and correct. Consider cherry-picking it into its own PR so it ships sooner. A short 0644 window between upload and chmod remains; the follow-up is a umask-077 upload variant.

## Next steps

1. **Review Task 5** (`git diff dbc02db86..afe2efcba`) against the plan's Task 5. The implementer flagged:
   - **LOOPS description shortened.** The brief's text broke `every_loop_that_keeps_running_says_why`, which caps descriptions under 120 chars. The implementer used a 104-char version.
   - **Diagnostics test header.** It now says "last 6 lines".
   - **Narrow race.** If `prepare` re-notes a host that is already listed while the sweep finds that host clean, the host is forgotten. Its new file then lingers until the next `prepare` on that host. Not closed.
   - **Missing env directory.** A host with no env dir is forgotten as clean.
2. **Task 6**: MCP `env_vars` tool, `new_session { env }` mapping (currently `env: None` at `mcp/tools/session_ops.rs`), audit redaction, `REGEN_DOCS`.
3. **Task 7**: form field `env_target`.
4. **Task 8**: Tauri command, routing, contract 18 (`MIN_HUB_CONTRACT` stays 16). **Also fix:** `src-tauri/src/backend/remote.rs::new_session` builds its JSON body by hand, so a hub-client desktop drops `env`.
5. **Task 9**: Settings › Environment resource page + docs.
6. **Final whole-branch review**, then `scripts/verify.sh full` with the Tauri crates. src-tauri has NOT been compiled on this branch: two `env: None` edits are in `src-tauri/src/backend/tests_routing.rs`.

## Environment gotchas (why commits used --no-verify)

- **CargoSD was not mounted** (`/Volumes/CargoSD` missing), and the Mac has about 11 GiB free.
- All Rust checks therefore ran on **mercury** through `mercury-run`, with `-p fleet-core -p fleet-hub`. Mercury has no Tauri system libraries, so it cannot build src-tauri or the `--workspace` aliases.
- The commands used were `cargo fmt --all --check`, `mercury-run 'cargo clippy -p fleet-core -p fleet-hub --all-targets -- -D warnings'` and `mercury-run 'cargo test -p fleet-core --lib -- <filter>'`.
- Commits used `--no-verify` because the pre-commit hook needs a local target dir. With CargoSD back, run `scripts/verify.sh full` before marking the PR ready.

## Rulings made during execution (each with its cost if wrong)

1. **`sweep_env_files` lives in Task 5, not Task 2.** Cost if wrong: none.
2. **Task 9 constant binding.** If `resources.rs` has no `Bind::Const`, Task 9 either adds the smallest constant binding or splits the command into `env_var_set` / `env_var_unset`, whichever touches fewer files. Cost if wrong: one extra routed command row.
3. **Task 1 test assertion.** The vacuous final assertion was replaced: a refused secret value must not appear in the error message. Cost if wrong: none.
4. **Mercury builds and `--no-verify` commits** (see above). Cost if wrong: a workspace-wide lint could slip through until `verify.sh full`.
5. **`prepare` runs before the create workspace step.** The plan placed it after; an unresolved ref used to leave a worktree behind. Cost if wrong: an env file is written for a create that then fails in the workspace step. The sweep removes it.
6. **Repair is wired at the tmux step** (`RepairExec::launch_cmd`), not at `spec_for_session_with`. That function is shared by non-launching paths, which would have written env files over ssh. This covers explicit repair, attach repair and recreate's fallback (which previously launched without env). Cost if wrong: a future non-`HostExec` executor launches without env.

**Behaviour change from Task 3.** A session created against a project that reconcile can't match by cwd (for example a system project on a remote host or custom root) now keeps that `project_id` instead of becoming an orphan row. Reconcile's existing `started_at` rule preserves it. The reviewer checked grouping, move, repair and adoption: no consumer relies on NULL. Mention it in the PR.

## Deferred minors (for the final review to triage)

- Task 1:
  - No 128/129-char name boundary test.
  - Lowercase `path` is accepted (names are case-sensitive by design).
  - The pin count excludes the host-merge SQL in `hosts_accounts.rs`.
- Task 2:
  - `set_var` is used in a multi-threaded test (with a unique name).
  - `note_host` runs before a write that may fail, leaving a harmless record.
- Task 3: env files left by failed creates or relaunches persist until the 10-minute sweep. This is by design (spec rev 3).
- Task 4:
  - Move resolves env only after seed and fetch, so a dry run can't predict the refusal. Add a store-only `resolve` before the seed (~`move_session/mod.rs:2706`).
  - Repair `launch_cmd` writes the file before the adoption guard and the git steps.
  - A `copy_session_env` failure in move only traces a warning; consider a timeline event.

## Resuming with superpowers SDD

The SDD ledger, briefs and reports live in the git-ignored `.superpowers/sdd/2026-10-11-session-env-and-secrets/` inside the original worktree. They are not on the branch, and this file replaces them. To resume:

1. Run `scripts/sdd-workspace` on the plan.
2. Create a new ledger whose first line names the plan.
3. Mark Tasks 1–4 `complete` and Task 5 `implemented, review pending`.
4. Continue from the Task 5 review.
