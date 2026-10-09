# CLAUDE.md

Orientation for Claude Code working in this repository.

## What this is

`claude-fleet` — a Tauri 2 desktop app (Rust backend + Svelte 5 frontend) for
managing long-lived Claude Code sessions running in tmux across multiple
machines over SSH.

The Rust side is a workspace: `crates/fleet-core` (Tauri-free service/store/SSH/MCP),
`crates/fleet-hub` (headless daemon, see `docs/hub.md`), `crates/fleet-proto` (the
hub/agent frame types, shared by both ends), `crates/fleet-agent` (the agent binary
for hosts the hub cannot reach — depends on `fleet-proto` only, never `fleet-core`),
`crates/fleet-agent-e2e` (tests only: the hub against the real agent over a socket,
through fleet-core's `testkit` feature), `crates/fleet-update` (the Tauri-free update
engine shared by every component — manifests, signatures, the update state machine —
with no `fleet-core` dependency; also the `fleet-release` binary), `src-tauri` (the
desktop app).

## Build & test

### Validation ladder (use this, in this order)

One canonical way to validate Rust changes. Every command below selects the
whole workspace. All but the first build test targets, so they share one set
of compiled dependencies in `target/debug/`; `fleet-fast-check` builds none
and keeps its own set in `target/fast-check/` (a cargo profile), so the two
never evict each other. Mixing in other selections (`-p <crate>`, plain
`cargo build`, `cargo check` without `--all-targets`, `pnpm tauri …`) makes
cargo compile another copy of fleet-core and its dependency graph: 1.5–3 min
the first time, then every edit paid once per copy
(RUST-BUILD-PERFORMANCE-AUDIT.md §10.3). The aliases live in
`.cargo/config.toml`. Times are for a fleet-core edit on 4 cores.

```bash
# 1. while you work, after every edit (≈ 15 s; libraries and binaries only)
cargo fleet-fast-check                  # check --workspace --profile fast-check
pnpm check                              # frontend edits: svelte-check
# 2. before committing (≈ 65 s: fmt 3 s, lint 27 s, tests 33 s)
scripts/verify.sh                       # fmt, lint and the tests of the modules
                                        # the change touches, each once; --dry-run
                                        # prints the plan
# 3. before pushing / marking a PR ready (≈ 3 min warm)
scripts/verify.sh full                  # = scripts/ci-local.sh, narrowed to the jobs
                                        # the change touches; the suite runs once
#    or, with BUILDKITE_API_TOKEN / BUILDKITE_ORG set, after pushing the branch:
scripts/verify.sh remote                # the same `full` on the persistent Buildkite
                                        # builder; waits, exit 0 iff it passed
                                        # (docs/buildkite.md)
```

Enable the hooks once per clone: `git config core.hooksPath .githooks`. The
pre-commit hook runs fmt + clippy for Rust and the frontend job for frontend
changes (its `pnpm audit` only warns; CI's fails); the pre-push hook checks
migration numbers against a freshly fetched `origin/main`. Fix what they
report rather than committing with `--no-verify`.

What `verify.sh` runs, for running a piece of it by hand:

```bash
cargo fmt --all --check
cargo fleet-lint                        # clippy --workspace --all-targets -- -D warnings
cargo fleet-test -- service::health     # test --workspace --lib --bins -- <filter>
pnpm exec vitest related --run src/lib/foo.ts
cargo fleet-check                       # check --workspace --all-targets (= rust-analyzer's check)
cargo test --workspace                  # the full suite, what CI runs
scripts/ci-local.sh                     # everything in CI order; --rust-only / --frontend-only / --hub-e2e
```

`fleet-fast-check` does not type-check test code: a signature change that
breaks a test passes it and fails `fleet-lint` / `fleet-check`. rust-analyzer
stays on the full check. `target/fast-check/` costs ~2 GB once and is never
cleaned automatically.

`fleet-lint` reports everything `fleet-check` does (clippy is the compiler
plus lints, test code included), but the two compile separately: running
both after an edit costs ~22 s more than lint alone. `verify.sh` lints and
does not check, and the pre-commit hook's clippy is then a no-op. Likewise
`scripts/ci-local.sh` runs `cargo test --workspace` itself, so running both
pays the ~2 min suite twice.

Rules:

- Do not run `cargo build` to see whether something compiles; `cargo
  fleet-fast-check` / `cargo fleet-check` answer that 2–6× faster. Build
  only when you need a binary.
- Do not narrow with `-p <crate>` in the inner loop; narrow with a test filter.
  Keep `-p` for the cases below that need a binary or a different feature set.
- A test that checks a file outside its crate (a `src/lib/*.ts` mirror,
  `src-tauri/src/lib.rs`, a `docs/*.md` guide) reads it when it runs
  (`repo_files::read` in fleet-core), never with `include_str!`: a compiled-in
  copy makes every edit to that file recompile the whole test target (~26 s
  for fleet-core's, against ~0.4 s). Likewise fleet-core takes no
  dev-dependency on a workspace crate it does not already depend on; a test
  that needs one lives in a crate of its own, as `crates/fleet-agent-e2e`
  does. `src/lib/names.json`, `tools/ag/**`, `tools/voice/arecord` and
  three `skills/*/SKILL.md` are embedded in fleet-core itself, so editing
  them does recompile it.
- `pnpm tauri dev` / `pnpm tauri build` and `cargo build -p fleet-hub` use other
  feature sets. Run them when you need them; in a cloud session (no display,
  ~30 GB disk) do not run the Tauri ones at all.

Other commands:

```bash
pnpm install --frozen-lockfile
pnpm test                       # frontend (Vitest), all of it
cargo deny check                # licenses + advisories (cargo install cargo-deny --locked)
cargo build -p fleet-hub --locked   # headless hub (no Tauri libs needed)
scripts/hub-e2e.sh               # real fleet-hub/-agent e2e; needs tmux, opt-in via ci-local.sh --hub-e2e
```

The Rust toolchain is pinned in `rust-toolchain.toml` (bump it in its own PR,
with the CI and Dockerfile pins; `scripts/check-version-consistency.sh`
enforces it).

`devtools` is an off-by-default cargo feature: `cargo tauri build --features
devtools` enables the Web Inspector in a release bundle; dev builds have it
automatically.

**Caveat:** `cargo` builds need the Tauri system libraries (dbus, gtk/atk,
pkg-config). On a headless box without them, `cargo build`/`cargo test` fail in
a build script — that is an environment gap, not a code error. Frontend
(`pnpm test`) builds anywhere.

After pulling, run `pnpm install --frozen-lockfile` before testing. Stale
`node_modules` cause `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"`
in `App.test.ts` and `clipboard_native.test.ts` — that is a dependency gap, not a
code error. (`localStorage` is polyfilled in `vitest.setup.ts`; there are no
known pre-existing frontend test failures.)

**`src-tauri`'s test target parks on a loaded box, and `TMPDIR` fixes it.**
`claude_fleet_lib`'s ~700 tests each build a temp SQLite store through
`tempfile::tempdir()`, so on the root ext4 filesystem they serialise behind one
journal: threads sit in `jbd2_log_wait_commit` with `/proc/pressure/io` at
35–83% and `cargo test --workspace` never finishes. Point `TMPDIR` at tmpfs and
the same binary runs the whole target in ~80–120 s:

```bash
mkdir -p /dev/shm/fleet-tests
TMPDIR=/dev/shm/fleet-tests cargo fleet-test -- backend::
```

Two traps when running a test binary directly rather than through cargo:
`ls -t target/debug/deps/<crate>-*` can hand you a STALE binary (several hashes
live there and the newest-written is not always first) — use
`find target/debug/deps -name '<crate>-*' ! -name '*.d' -printf '%T@ %p\n' | sort -rn | head -1`;
and a stale `claude_fleet_lib` fails `verdict_gen` for the right reason, because
it renders the table it was compiled with against the file on disk.

**A `REGEN_*` run is MEANT to fail.** `REGEN_HUB_VERDICTS=1` /
`REGEN_DOCS=1` / `REGEN_HUB_CONTRACT=1` write the file and then panic on
purpose, telling you to read the diff and run again without the variable. The
failure is the receipt, not a problem.

Known Rust flakes — timing-sensitive, so they fail on a loaded box; re-run
alone before blaming your change: the `CHAIN_BUDGET` migration tests in
`store/schema/tests_upgrade.rs` (`FILE_OPEN_BUDGET` lifted the file-based
upgrade test to 30 s **on Windows only**, so on Linux
`opening_a_pre_work_graph_file_upgrades_it_within_budget` and the two
in-memory chains still hold `CHAIN_BUDGET` at 5 s), `service::add_project`,
and `fleet-agent`'s `conn::tests::report_frames_stay_under_the_frame_cap_and_carry_the_rest_over`
(it fails `Elapsed(())` in a parallel run and passes alone in 0.15 s).
`work::scale_tests::*` hold their wall-clock budgets only with
`FLEET_SCALE_BUDGETS=1` (CI sets it); without it an over-budget call is
printed, not failed — their query-plan checks always run.

`mcp::tools::tests::a_one_person_fleet_still_sees_its_unclaimed_rows` is the
opposite shape: it **passes in the full suite and fails run alone**, where
`list_sessions` answers 12 rows for a store holding 2. Not a leak (the master
is unrestricted by design and these are unclaimed rows it may see), but a
test that only pins under load pins nothing, so it is a real defect in the
test and not yet diagnosed. Ruled out already: cross-test pollution (it
fails with `--exact` alone), the machine's tmux server (`TMUX_TMPDIR` at an
empty dir changes nothing), and a seeded template (no migration inserts
`sessions`).

Not a flake:
`cargo test -p fleet-core --lib -- --test-threads=1` takes 23–25 minutes
(4.5k tests; measured on mercury, 2026-10-02), so a `timeout 600` wrapper
kills it mid-run and the last `test … ...` line names whichever test was in
flight — a slow run, not a hang. Run it in parallel (the default), or give a
sequential run a 30-minute budget.

## Releasing

Releases are cut manually with `scripts/release.sh <new-version>` — it bumps
the six version files (+ `Cargo.lock`), prefills a `CHANGELOG.md` section
from the Conventional Commits since the last tag, commits, and creates the
`vX.Y.Z` tag. Never edit the version fields by hand; run the script from a
clean `main`. See `docs/RELEASING.md`. Once the release is pushed,
`scripts/release-mobile.sh <same-version>` tags fleet-mobile's `main` so its
own workflow builds the signed APK under the same version.

`docs/control-api-reference.md` is generated from the MCP tool router. After
editing any `#[tool(...)]` description or the `generate_handler!` list,
regenerate it or CI fails:

```bash
REGEN_DOCS=1 cargo fleet-test -- reference_is_current
```

`src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md`
are generated from `src-tauri/src/backend/verdicts.rs`. After editing any row,
regenerate them or CI fails:

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen
```

Two more generated sets, same rule (`docs/architecture.md` → *Settings
metadata* / *Declarative pages*): after editing a settings `SPECS` row,
`REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current`; after
editing a page spec or the widget catalog,
`REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`.

`docs/form-spec.schema.json` (chat forms, `docs/forms.md`) and
`docs/chat-block.schema.json` (chat cards, `docs/chat-blocks.md`) are generated
from the Rust models, same rule: after editing `crates/fleet-core/src/pages/forms.rs`
or `chat_blocks.rs`, `REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current`.

## Architecture

Read [`docs/architecture.md`](docs/architecture.md) before changing a subsystem you have not touched in this session — frontend stores and row events, the service/store/SSH layers, settings metadata and declarative pages, client access, hub client mode (adding a desktop command), the assets catalog, downloads, voice: it names the files, the invariants and the `REGEN_*` commands.

## Conventions

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the
  frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote`
  (alias `shq`) in `crates/fleet-core/src/shell.rs`. Every value interpolated into an
  SSH/bash command string MUST be quoted with it. The former duplicate copies
  (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not
  reintroduce them.
- Every child process is built by `fleet_core::proc::command` /
  `std_command`, never `Command::new`: on Windows they set `CREATE_NO_WINDOW`,
  without which each `ssh.exe` a probe spawns flashes a console window.
  `no_eprintln_tests::production_code_spawns_through_proc` enforces it.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the
  guard across an `.await`.
- In tests, `Store::open_in_memory()` / `open_with_bus_in_memory` hand out a
  copy of a database migrated once per test process (`migrated_template_copy`);
  a test about the migrations themselves builds its database with
  `store::testgen` / `migrations_through` instead. The bundled SQLite is built
  with `SQLITE_DEFAULT_MEMSTATUS=0` (`.cargo/config.toml`), so it takes no
  process-wide lock per allocation.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command (a
  sync command runs on the macOS main thread). PTY input goes to the writer
  thread through its bounded channel — `E_PTY_BUSY` when it is full,
  `E_PTY_CLOSED` when the thread is gone; kill / reap / fd teardown runs on the
  `PtyParts` taken out under the lock, after the guard is released.
- Consult the hardening review, `docs/specs/2026-05-21-hardening-review.md`,
  before touching SSH command construction, the PTY, migrations, or the
  optimistic-merge / event-bus paths.
- Take a new migration's number from `origin/main`, not your checkout:
  `git ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1`.
  `scripts/check-migration-numbers.sh` (the pre-push hook, `verify.sh full`)
  fails a number main already used. When main takes yours first, merge
  `origin/main` and run `scripts/renumber-migrations.sh`: it resolves the
  MIGRATIONS conflict and moves the branch's migrations to the next free
  numbers. `migrations_are_contiguous_from_one` allows no gap, so a branch
  cannot hold a number ahead of main's next free one.

## Status & known issues

Read [`docs/status.md`](docs/status.md) before starting a feature or changing a milestone's area (move, host reboot, work graph, Jev, updates, Windows, federation): it says what is landed, what is built but OFF by default, and what waits on the owner.
