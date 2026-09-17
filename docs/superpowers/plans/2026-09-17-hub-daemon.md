# Hub Daemon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run claude-fleet headless as a `fleet-hub` daemon on any always-on box, reachable at a public URL, with no desktop app involved.

**Architecture:** The Rust backend is split into a Tauri-free `fleet-core` crate (service, store, SSH, tmux, MCP server) that both the desktop app and a new `fleet-hub` binary embed. The MCP server gains a configurable bind address and a Host allowlist; provisioning writes a public base URL into hosts instead of a loopback port and skips reverse tunnels when one is set; reconcile stops assuming a `local` host. The daemon is packaged as a Docker image with a Caddy sidecar for TLS.

**Tech Stack:** Rust 2021 (cargo workspace), tokio, axum 0.8, rmcp 1, rusqlite, clap 4 (derive), Docker (debian-bookworm-slim), Caddy, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-17-hub-daemon-design.md`

## Global Constraints

- Crate names: `fleet-core` (lib), `fleet-hub` (bin `fleet-hub`); the desktop package stays `claude-fleet` with lib name `claude_fleet_lib`.
- Workspace root is the repository root; the Tauri CLI keeps `src-tauri` as its project dir. Never move `tauri.conf.json`, `capabilities/`, `gen/`, `icons/`, `build.rs`.
- `fleet-core` must never depend on any `tauri*` crate. The CI job `hub-headless` (Task 8) builds `fleet-hub` on a runner without GTK/WebKit and is the regression guard.
- Every value interpolated into a shell string goes through `crate::shell::quote` (`shq`). Never hold the `Store` mutex guard across an `.await`.
- Production code logs through `tracing` only: no `println!`/`eprintln!` outside `#[cfg(test)]` (the `no_eprintln_tests` guard fails CI otherwise). The `fleet-hub` CLI is the one exception for user-facing output: it prints through a dedicated `out.rs` helper that the guard's allowlist names (Task 7).
- New `settings` keys: `hub.bind`, `hub.public_url`, `hub.allowed_hosts`, `hub.local_host`. They are read with `Store::get_setting` directly and are **not** registered in `service::settings::SPECS` (that registry requires a Settings-dialog row per key).
- Defaults: bind `127.0.0.1`, port `4180`, `hub.local_host` true when unset, plaintext on a non-loopback bind refused unless `--allow-plaintext`.
- On the `claude-fleet-trn` host, run `source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` before any `cargo` command (Tauri system libs live in that sysroot).
- Commit after every task with a Conventional Commit message. Do not push or open a PR until Task 10.
- Relative-path rule for moved files: every `include_str!` / `CARGO_MANIFEST_DIR` path that escapes the crate gets one extra `../` (the crate root moves from `src-tauri/` to `crates/fleet-core/`, one level deeper).

---

## File map

| Path | Responsibility |
|---|---|
| `Cargo.toml` (root, new) | Workspace: `crates/*` + `src-tauri`. |
| `Cargo.lock` (root, moved from `src-tauri/`) | Single workspace lock. |
| `crates/fleet-core/Cargo.toml` | Tauri-free library package. |
| `crates/fleet-core/src/lib.rs` | `pub mod` list of the moved modules + `rt`. |
| `crates/fleet-core/src/rt.rs` | Runtime-spawn seam (`install`, `spawn`). |
| `crates/fleet-core/src/{cancel,claude_agents,claude_cli,events,humanize,ipc_error,logging,projects,repo_url,shell,ssh,ssh_config,ssh_fake,tmux,validate,fleet_e2e_tests,no_eprintln_tests}.rs` | Moved unchanged (paths fixed). |
| `crates/fleet-core/src/{service,store,mcp}/` | Moved unchanged (paths fixed). |
| `crates/fleet-core/migrations/*.sql` | Moved from `src-tauri/migrations/`. |
| `crates/fleet-core/src/service/hub.rs` (new) | `HubBase` (base URL + port + public flag), `hub.*` setting keys, `read_local_host`. |
| `crates/fleet-hub/Cargo.toml` | Daemon binary package. |
| `crates/fleet-hub/src/main.rs` | clap entry: `init`, `serve`, `token`, `ssh-key`. |
| `crates/fleet-hub/src/config.rs` | Flag > env > setting > default resolution and validation. |
| `crates/fleet-hub/src/serve.rs` | Start ticks + MCP server, wait for a signal, shut down. |
| `crates/fleet-hub/src/out.rs` | The one place the CLI writes to stdout/stderr. |
| `crates/fleet-hub/Dockerfile` | Multi-stage image. |
| `deploy/hub/{docker-compose.yml,Caddyfile,fleet-hub.env.example,fleet-hub.service}` | Deployment templates. |
| `src-tauri/src/app_events.rs` (new) | `AppHandleEventBus` (moved out of `events.rs`). |
| `src-tauri/src/lib.rs`, `src-tauri/src/bootstrap/*.rs`, `src-tauri/src/commands/*.rs`, `src-tauri/src/pty.rs` | Path changes `crate::x` → `fleet_core::x`; `rt::install`. |
| `.github/workflows/{ci.yml,docs.yml,release.yml,hub-image.yml}` | Workspace commands, headless job, image build. |
| `scripts/{ci-local.sh,release.sh}`, `deny.toml`, `CLAUDE.md`, `docs/{hub.md,control-api.md,concepts.md,RELEASING.md}` | Paths and docs. |

---

### Task 1: Workspace root and single lock file

**Files:**
- Create: `Cargo.toml` (repo root)
- Move: `src-tauri/Cargo.lock` → `Cargo.lock`
- Modify: `.gitignore`, `deny.toml` (comment), `.github/workflows/ci.yml`, `.github/workflows/docs.yml`, `.github/workflows/release.yml:126`, `scripts/ci-local.sh:97-103`, `scripts/release.sh:44-46,95`, `CLAUDE.md` (Build & test section)

**Interfaces:**
- Produces: a cargo workspace whose root is the repo root; every later task runs `cargo … --workspace` from the root.

- [ ] **Step 1: Create the workspace manifest and move the lock**

```toml
# Cargo.toml (repository root)
[workspace]
resolver = "2"
members = ["src-tauri"]
```

```bash
git mv src-tauri/Cargo.lock Cargo.lock
printf '\n# cargo workspace target dir (root Cargo.toml)\n/target/\n' >> .gitignore
```

- [ ] **Step 2: Verify the desktop crate still builds from the root**

Run: `cargo test --workspace`
Expected: same pass count as before the move (the lock is reused; no dependency resolution changes — `git diff --stat Cargo.lock` shows only the rename).

- [ ] **Step 3: Point CI, local CI and the release script at the root**

`.github/workflows/ci.yml` rust job: delete the `defaults.run.working-directory: src-tauri` block; set `Swatinem/rust-cache` `workspaces: .`; replace the three cargo steps with:

```yaml
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

and the deny step with `- run: cargo deny check` (drop `--manifest-path`, drop the `working-directory:` line).

`.github/workflows/docs.yml`: delete the `defaults.run.working-directory: src-tauri` block, set `workspaces: .`, change `cargo doc --no-deps` to `cargo doc --no-deps -p claude-fleet`, and the artifact `path: src-tauri/target/doc` to `path: target/doc`.

`.github/workflows/release.yml` line 126: `workspaces: src-tauri` → `workspaces: .`.

`scripts/ci-local.sh` `run_rust`:

```bash
  step cargo fmt --all --check
  step cargo clippy --workspace --all-targets -- -D warnings
  step cargo test --workspace
  # No --config: cargo-deny finds ./deny.toml from the repo root on its own.
  step cargo deny check
```

(remove the `local manifest=…` line and the `--manifest-path` flags).

`scripts/release.sh`: lines 44–46 become

```bash
  cargo update -p "$CRATE" --offline >/dev/null 2>&1 || cargo update -p "$CRATE"
  echo "  Cargo.lock"
```

and line 95 `git add "${VERSION_FILES[@]}" Cargo.lock CHANGELOG.md`.

`deny.toml` header comment: replace the paragraph that says the workspace root is `src-tauri/` with: `# The cargo workspace root is the repository root (Cargo.toml); run \`cargo deny check\` from there.`

`CLAUDE.md` Build & test block:

```bash
pnpm install
pnpm test                       # frontend (Vitest)
pnpm check                      # Svelte/TS type-check
cargo test --workspace          # backend (all crates)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check                # licenses + advisories (cargo install cargo-deny --locked)
scripts/ci-local.sh             # all of the above in CI order; --rust-only / --frontend-only
```

- [ ] **Step 4: Run the local CI script**

Run: `scripts/ci-local.sh --rust-only`
Expected: fmt, clippy, test and deny all pass from the root.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore deny.toml .github scripts CLAUDE.md
git commit -m "build: cargo workspace rooted at the repository root"
```

---

### Task 2: `fleet-core` skeleton with the runtime-spawn seam

**Files:**
- Create: `crates/fleet-core/Cargo.toml`, `crates/fleet-core/src/lib.rs`, `crates/fleet-core/src/rt.rs`
- Modify: `Cargo.toml` (members), `src-tauri/Cargo.toml` (dependency), `src-tauri/src/lib.rs` (install), `src-tauri/src/service/hooks.rs:152,160`, `src-tauri/src/service/tick.rs:49,119`, `src-tauri/src/mcp/mod.rs:244`

**Interfaces:**
- Produces: `fleet_core::rt::install(handle: tokio::runtime::Handle)` and `fleet_core::rt::spawn<F>(fut: F) -> tokio::task::JoinHandle<F::Output>`.

- [ ] **Step 1: Write the failing tests**

`crates/fleet-core/src/rt.rs`:

```rust
//! Where core background tasks run.
//!
//! The Tauri app starts its ticks and the MCP server from the `setup`
//! closure on the main thread (inside macOS `did_finish_launching`), where
//! no tokio runtime is entered: a bare `tokio::spawn` there panics ("no
//! reactor running") and, because that callback cannot unwind, aborts the
//! process. The desktop therefore installs its runtime handle once, and
//! every core spawn goes through [`spawn`], which prefers the current
//! runtime (the daemon runs under `#[tokio::main]`) and falls back to the
//! installed one.

use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

static INSTALLED: OnceLock<Handle> = OnceLock::new();

/// Install the runtime an embedder wants core tasks to run on. Idempotent:
/// a second call is ignored.
pub fn install(handle: Handle) {
    let _ = INSTALLED.set(handle);
}

/// Spawn on the current tokio runtime when inside one, else on the installed
/// handle. Panics when neither exists — that is a bootstrap bug, not a
/// runtime condition.
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match Handle::try_current() {
        Ok(h) => h.spawn(fut),
        Err(_) => INSTALLED
            .get()
            .expect("fleet_core::rt::spawn called outside a tokio runtime and before rt::install")
            .spawn(fut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_uses_the_current_runtime() {
        let v = spawn(async { 41 + 1 }).await.unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn spawn_from_a_plain_thread_uses_the_installed_handle() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        install(rt.handle().clone());
        // A plain OS thread: no runtime is current there.
        let out = std::thread::spawn(|| {
            let jh = spawn(async { "ran" });
            // Block on the join from outside any runtime.
            futures_lite_block_on(jh)
        })
        .join()
        .unwrap();
        assert_eq!(out, "ran");
    }

    /// Minimal block_on so the test needs no extra crate.
    fn futures_lite_block_on<T>(jh: JoinHandle<T>) -> T {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(jh).unwrap()
    }
}
```

- [ ] **Step 2: Create the crate and register it**

`crates/fleet-core/Cargo.toml`:

```toml
[package]
name = "fleet-core"
version = "0.1.0"
description = "Transport-agnostic core of claude-fleet: service layer, store, SSH, tmux, MCP control API"
authors = ["martin-janci"]
edition = "2021"
license = "MIT"

[dependencies]
tokio = { version = "1", features = ["full"] }
```

`crates/fleet-core/src/lib.rs`:

```rust
//! claude-fleet core: everything the desktop app and the `fleet-hub` daemon
//! share. No Tauri dependency — see `docs/superpowers/specs/2026-09-17-hub-daemon-design.md`.

pub mod rt;
```

Root `Cargo.toml` members: `members = ["crates/*", "src-tauri"]`.

`src-tauri/Cargo.toml` `[dependencies]`: add `fleet-core = { path = "../crates/fleet-core" }`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p fleet-core`
Expected: 2 passed.

- [ ] **Step 4: Route the five spawn sites through the seam and install the handle**

In `src-tauri/src/service/hooks.rs` (two sites), `src-tauri/src/service/tick.rs` (two sites) and `src-tauri/src/mcp/mod.rs` (one site) replace `tauri::async_runtime::spawn(` with `fleet_core::rt::spawn(`. Delete the two comment blocks in `tick.rs` that explain why `tauri::async_runtime::spawn` is used (that explanation now lives on `rt::install`). Keep `tauri::async_runtime::block_on` in `bootstrap/mcp.rs` as is.

In `src-tauri/src/lib.rs`, first statement inside `.setup(move |app| {`, before `let handle = app.handle().clone();`:

```rust
            // Core tasks (ticks, MCP server, hook side-jobs) spawn through
            // `fleet_core::rt`; give it this app's tokio runtime, since this
            // closure runs outside any runtime context. `block_on` executes
            // the future ON the Tauri runtime, so `Handle::current()` inside
            // it is that runtime's handle.
            tauri::async_runtime::block_on(async {
                fleet_core::rt::install(tokio::runtime::Handle::current());
            });
```

- [ ] **Step 5: Verify**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && grep -rn "tauri::async_runtime::spawn" src-tauri/src`
Expected: tests pass, clippy clean, the grep prints nothing.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/fleet-core src-tauri/Cargo.toml src-tauri/src
git commit -m "refactor(core): fleet-core crate with the rt::spawn runtime seam"
```

---

### Task 3: Move the core modules into `fleet-core`

**Files:**
- Move (git mv, see Step 1): from `src-tauri/src/` to `crates/fleet-core/src/`: `cancel.rs`, `claude_agents.rs`, `claude_cli.rs`, `events.rs`, `humanize.rs`, `ipc_error.rs`, `logging.rs`, `projects.rs`, `repo_url.rs`, `shell.rs`, `ssh.rs`, `ssh_config.rs`, `ssh_fake.rs`, `tmux.rs`, `validate.rs`, `fleet_e2e_tests.rs`, `no_eprintln_tests.rs`, `service/`, `store/`, `mcp/`; `src-tauri/migrations/` → `crates/fleet-core/migrations/`
- Create: `src-tauri/src/app_events.rs`
- Modify: `crates/fleet-core/Cargo.toml`, `crates/fleet-core/src/lib.rs`, `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/src/bootstrap/*.rs`, `src-tauri/src/commands/*.rs`, `src-tauri/src/pty.rs`, the eight path-sensitive files listed in Step 4

**Interfaces:**
- Produces: `fleet_core::{cancel, claude_agents, claude_cli, events, humanize, ipc_error, logging, mcp, projects, repo_url, rt, service, shell, ssh, ssh_config, store, tmux, validate}` as public modules. `fleet_core::events::{EventBus, RowChange, NoopEventBus}`; `claude_fleet_lib::app_events::AppHandleEventBus`.

- [ ] **Step 1: Move the files**

```bash
cd crates/fleet-core/src
for f in cancel claude_agents claude_cli events humanize ipc_error logging projects repo_url shell ssh ssh_config ssh_fake tmux validate fleet_e2e_tests no_eprintln_tests; do
  git mv ../../../src-tauri/src/$f.rs ./$f.rs
done
git mv ../../../src-tauri/src/service ./service
git mv ../../../src-tauri/src/store ./store
git mv ../../../src-tauri/src/mcp ./mcp
git mv ../../../src-tauri/migrations ../migrations
cd -
```

(`service/testdata/` and `service/catalog/` travel inside `service/`.)

- [ ] **Step 2: Declare the modules in the core crate**

`crates/fleet-core/src/lib.rs`:

```rust
//! claude-fleet core: everything the desktop app and the `fleet-hub` daemon
//! share. No Tauri dependency — see
//! `docs/superpowers/specs/2026-09-17-hub-daemon-design.md`.
//!
//! `service/` is the transport-agnostic command logic, `store/` the SQLite
//! layer, `ssh`/`tmux` the host transport, `mcp/` the control API server.

pub mod cancel;
pub mod claude_agents;
pub mod claude_cli;
pub mod events;
#[cfg(test)]
mod fleet_e2e_tests;
pub mod humanize;
pub mod ipc_error;
pub mod logging;
pub mod mcp;
#[cfg(test)]
mod no_eprintln_tests;
pub mod projects;
pub mod repo_url;
pub mod rt;
pub mod service;
pub mod shell;
pub mod ssh;
pub mod ssh_config;
#[cfg(test)]
pub mod ssh_fake;
pub mod store;
pub mod tmux;
pub mod validate;
```

`crates/fleet-core/Cargo.toml` dependencies (copied from `src-tauri/Cargo.toml` minus `tauri*` and `portable-pty`):

```toml
[dependencies]
sysinfo = "0.33"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
directories = "5"
regex = "1"
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
dashmap = "6"
async-trait = "0.1"
rmcp = { version = "1", features = ["server", "macros", "transport-streamable-http-server", "schemars"] }
axum = "0.8"
rand = "0.10"
uuid = { version = "1", features = ["v4"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", default-features = false, features = ["fmt", "env-filter", "std", "registry", "tracing-log", "smallvec"] }
tracing-appender = "0.2"
serde_yaml = "0.9"
sha2 = "0.10"
hex = "0.4"
base64 = "0.22"
toml = "0.8"

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[dev-dependencies]
tempfile = "3"
proptest = { version = "1", default-features = false, features = ["std"] }
```

Add `crates/fleet-core/.gitignore` containing `/proptest-regressions/`.

`src-tauri/Cargo.toml` `[dependencies]` becomes:

```toml
fleet-core = { path = "../crates/fleet-core" }
tauri = { version = "2", features = [] }
tauri-plugin-opener = "2"
tauri-plugin-dialog = "2"
tauri-plugin-clipboard-manager = "2"
sysinfo = "0.33"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
directories = "5"
portable-pty = "0.9"
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
tracing = "0.1"
```

with `[dev-dependencies] tempfile = "3"`. If `cargo build -p claude-fleet` later reports an unresolved crate name in `commands/`, `pty.rs` or `bootstrap/`, add that one dependency back to `src-tauri/Cargo.toml`; do not add any back speculatively.

- [ ] **Step 3: Extract `AppHandleEventBus` into the desktop crate**

Cut the `AppHandleEventBus` struct, its `impl` block and its `new` (lines ~250–282 of `crates/fleet-core/src/events.rs`, everything that mentions `tauri`) and paste them into `src-tauri/src/app_events.rs`:

```rust
//! The desktop's event bus: forwards every `RowChange` to the Svelte frontend
//! through `tauri::AppHandle::emit`. The trait and the test buses live in
//! `fleet_core::events`.

use fleet_core::events::{EventBus, RowChange};

pub struct AppHandleEventBus {
    handle: tauri::AppHandle,
}

impl AppHandleEventBus {
    pub fn new(handle: tauri::AppHandle) -> Self {
        Self { handle }
    }
}

impl EventBus for AppHandleEventBus {
    fn emit(&self, change: &RowChange) {
        let name = change.name();
        let payload = change.payload();
        let _ = tauri::Emitter::emit(&self.handle, name, payload);
    }
}
```

(Match the body to what the moved code actually did — the original `emit` may batch or spawn; keep its exact behaviour, only the location changes.) Update the module doc comment at the top of `events.rs` so it no longer says the bus emits via `tauri::AppHandle` (say: "the desktop's `AppHandleEventBus` lives in `src-tauri/src/app_events.rs`").

- [ ] **Step 4: Fix every path that escapes the crate (one extra `../`)**

| File (under `crates/fleet-core/src/`) | Old | New |
|---|---|---|
| `events.rs` | `include_str!("../../src/lib/events.ts")` | `include_str!("../../../src/lib/events.ts")` |
| `service/settings.rs` | `include_str!("../../../src/lib/fleet_settings.ts")` and `…/SettingsDialog.svelte` | `include_str!("../../../../src/lib/fleet_settings.ts")`, `…/SettingsDialog.svelte` |
| `service/names.rs` | `include_str!("../../../src/lib/names.json")` | `include_str!("../../../../src/lib/names.json")` |
| `service/provision.rs` | `include_str!("../../../skills/…")` (two) | `include_str!("../../../../skills/…")` |
| `mcp/doc_gen.rs` | `include_str!("../lib.rs")` | `include_str!("../../../../src-tauri/src/lib.rs")` (the `generate_handler!` list lives in the desktop crate) |
| `mcp/doc_gen.rs` | `join("../docs/control-api-reference.md")` | `join("../../docs/control-api-reference.md")` |
| `mcp/doc_gen.rs` | `include_str!("../../../docs/control-api.md")` | `include_str!("../../../../docs/control-api.md")` |
| `mcp/tools/tests.rs` | `include_str!("../../../../skills/…")`, `include_str!("../../../../docs/control-api.md")` | one more `../` on each |
| `service/repair.rs` | `include_str!("../commands/sessions.rs")` | `include_str!("../../../../src-tauri/src/commands/sessions.rs")` |
| `logging.rs` | `DEFAULT_FILTER: "warn,claude_fleet_lib=info,claude_fleet=info"` | `"warn,claude_fleet_lib=info,claude_fleet=info,fleet_core=info,fleet_hub=info"` |
| `mcp/doc_gen.rs` line ~159 (the regenerate hint text) | `cargo test --manifest-path src-tauri/Cargo.toml reference_is_current` | `cargo test -p fleet-core reference_is_current` |

`store/schema.rs` migration includes (`../../migrations/…`) and `service/pane_intel.rs` testdata includes are unchanged (they move with the crate).

`no_eprintln_tests.rs` line ~167: replace

```rust
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
```

with a scan over both crates' sources (keep the rest of the function, iterating over `roots`):

```rust
    // Both crates: the core and the desktop command layer.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [
        manifest.join("src"),
        manifest.join("../../src-tauri/src"),
    ];
```

Add to the guard's allowlist mechanism nothing yet; Task 7 adds `crates/fleet-hub/src/out.rs` handling.

- [ ] **Step 5: Rewire the desktop crate**

`src-tauri/src/lib.rs`: delete every `mod` line for a moved module and add `mod app_events;`. Replace `pub use events::{AppHandleEventBus, EventBus, NoopEventBus};` with `pub use app_events::AppHandleEventBus;`. Then, across `src-tauri/src/lib.rs`, `src-tauri/src/bootstrap/*.rs`, `src-tauri/src/commands/*.rs`, `src-tauri/src/pty.rs`, `src-tauri/src/app_events.rs`:

```bash
grep -rl "crate::" src-tauri/src | xargs sed -i -E \
  's/crate::(service|store|ssh|ipc_error|validate|cancel|mcp|logging|shell|events|ssh_config|tmux|projects|humanize|claude_cli|claude_agents|repo_url)\b/fleet_core::\1/g'
sed -i 's/crate::events::AppHandleEventBus/crate::app_events::AppHandleEventBus/' src-tauri/src/lib.rs
```

Then the bare `use` lines at the top of `lib.rs` (`use pty::PtyState; use service::tick::…; use store::Store;`) become `use fleet_core::service::tick::{spawn_account_usage_tick, spawn_reconcile_tick}; use fleet_core::store::Store;` and `use fleet_core::ssh` where `ssh::SshClient::new()` is used (or write `fleet_core::ssh::SshClient::new()`).

- [ ] **Step 6: Widen visibility as the compiler demands**

Run: `cargo build -p claude-fleet 2>&1 | grep -E "^error\[E0603\]|^error\[E0624\]|is private" -A3`

For every reported item in `crates/fleet-core` that is `pub(crate)` or `pub(super)`, change it to `pub`. Known ones from the spec's survey: `service::tick::{spawn_reconcile_tick, spawn_account_usage_tick}`, `service::hosts::local_home_dir` (only if reported). Repeat the build until it is clean. Do **not** widen `#[cfg(test)]` items; if a desktop test needs one, copy the helper into that test module instead.

- [ ] **Step 7: Verify everything**

Run:

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
grep -rn "tauri" crates/fleet-core/src | grep -v "^crates/fleet-core/src/[a-z_/]*.rs:[0-9]*:\s*//"
```

Expected: all tests pass (same count as before Task 3 plus the two `rt` tests), clippy clean, and the last grep prints nothing (only comments may mention Tauri). Then:

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
git status --short docs/
```

Expected: the reference is unchanged (no diff) — the tool list did not change.

- [ ] **Step 8: Run the desktop once**

Run (on a machine with the Tauri libs): `pnpm tauri dev` for a minute — the app starts, the sidebar lists sessions, the reconcile tick logs `reconcile tick enabled every …s`. If `rt::spawn` panicked at startup, the `block_on` install in Task 2 runs after a spawn site; move it earlier.

- [ ] **Step 9: Commit**

```bash
git add -A crates src-tauri
git commit -m "refactor(core): move service, store, ssh, tmux and mcp into fleet-core"
```

---

### Task 4: Configurable bind address and Host/Origin allowlist

**Files:**
- Modify: `crates/fleet-core/src/mcp/auth.rs` (`check_origin`, `check_request`, tests), `crates/fleet-core/src/mcp/mod.rs` (`AuthState`, `authorize`, `start`, tests), `src-tauri/src/bootstrap/mcp.rs`, `src-tauri/src/commands/mcp.rs` (the `mcp::start` call)

**Interfaces:**
- Produces: `auth::check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<(), StatusCode>`; `auth::check_request(headers, master, host_tokens, allowed: &[String]) -> Result<Caller, StatusCode>`; `auth::normalize_allowed_hosts(list: &[String]) -> Vec<String>`; `mcp::start(store, ssh, reg, tunnels, guards, bind: std::net::IpAddr, port: u16, token: String, allowed_hosts: Vec<String>) -> Result<CancellationToken, String>`.

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `auth.rs`)

```rust
    fn headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        h.insert(header::AUTHORIZATION, "Bearer s3cret".parse().unwrap());
        h
    }

    #[test]
    fn allowlisted_host_and_origin_pass_others_still_403() {
        let allowed = normalize_allowed_hosts(&["Fleet.Example.com".into()]);
        // Bare host and port-qualified authority both match, case-insensitively.
        assert!(check_request(&headers("fleet.example.com", None), "s3cret", &[], &allowed).is_ok());
        assert!(check_request(&headers("FLEET.example.com:443", None), "s3cret", &[], &allowed).is_ok());
        assert!(check_request(
            &headers("fleet.example.com", Some("https://fleet.example.com")),
            "s3cret", &[], &allowed
        ).is_ok());
        // Loopback keeps working with a non-empty list.
        assert!(check_request(&headers("127.0.0.1:4180", None), "s3cret", &[], &allowed).is_ok());
        // Not listed → 403 before the token is looked at.
        assert_eq!(
            check_request(&headers("evil.example.com", None), "s3cret", &[], &allowed),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            check_request(&headers("fleet.example.com", Some("https://evil.example.com")), "s3cret", &[], &allowed),
            Err(StatusCode::FORBIDDEN)
        );
        // An empty list is today's behaviour: loopback only.
        assert_eq!(
            check_request(&headers("fleet.example.com", None), "s3cret", &[], &[]),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn normalize_allowed_hosts_trims_lowercases_and_drops_empties() {
        let out = normalize_allowed_hosts(&[" A.Example.com ".into(), "".into(), "b:8443".into()]);
        assert_eq!(out, vec!["a.example.com".to_string(), "b:8443".to_string()]);
    }
```

Update the existing calls in `auth.rs` tests: every `check_request(&h, "s3cret", &[…])` gets a fourth argument `&[]`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core mcp::auth`
Expected: compile error — `check_request` takes 3 arguments, `normalize_allowed_hosts` not found.

- [ ] **Step 3: Implement**

In `auth.rs`, replace `check_origin` and `check_request`:

```rust
/// Lower-cased, trimmed allowlist entries (`host` or `host:port`), empties
/// dropped. Built once at server start from the hub's configuration.
pub fn normalize_allowed_hosts(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The host part of a `Host`-header authority: brackets and port stripped,
/// lower-cased.
fn authority_host(value: &str) -> String {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_ascii_lowercase();
    }
    v.split(':').next().unwrap_or(v).to_ascii_lowercase()
}

/// True when `value` is loopback or names an allowlisted host, matched as
/// the full authority (`host:port`) or as the bare host.
fn host_allowed(value: &str, allowed: &[String]) -> bool {
    if is_loopback_host(value) {
        return true;
    }
    let full = value.trim().to_ascii_lowercase();
    let host = authority_host(value);
    allowed.iter().any(|a| *a == full || *a == host)
}

/// True when an `Origin` is a loopback `http(s)` origin or one whose
/// authority is allowlisted.
fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    let after_scheme = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    match after_scheme {
        Some(rest) => host_allowed(rest.split('/').next().unwrap_or(rest), allowed),
        None => false,
    }
}

/// Layer 1 — DNS-rebinding defense. An `Origin`/`Host` is validated only when
/// present; a non-browser MCP client legitimately omits `Origin`. Loopback is
/// always accepted; a hub exposed at a public URL adds that URL's host to
/// `allowed`. `Err(403)` on anything else.
pub fn check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<(), StatusCode> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin.to_str().map(|o| origin_allowed(o, allowed)).unwrap_or(false) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    if let Some(host) = headers.get(header::HOST) {
        if !host.to_str().map(|h| host_allowed(h, allowed)).unwrap_or(false) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

pub fn check_request(
    headers: &HeaderMap,
    master_token: &str,
    host_tokens: &[HostTokenRow],
    allowed: &[String],
) -> Result<Caller, StatusCode> {
    check_origin(headers, allowed)?;
    let presented =
        bearer_token(headers.get(header::AUTHORIZATION)).ok_or(StatusCode::UNAUTHORIZED)?;
    resolve_token(presented, master_token, host_tokens).ok_or(StatusCode::UNAUTHORIZED)
}
```

Keep `origin_is_loopback` (it has its own test) implemented as `origin_allowed(origin, &[])`.

In `mcp/mod.rs`:

```rust
#[derive(Clone)]
struct AuthState {
    master: Arc<String>,
    store: Arc<Mutex<Store>>,
    /// Non-loopback `Host`/`Origin` values accepted besides loopback (see
    /// `auth::check_origin`). Empty on the desktop.
    allowed_hosts: Arc<Vec<String>>,
}
```

`authorize`: `auth::check_request(request.headers(), &state.master, &host_tokens, &state.allowed_hosts)`.

`start` signature and body:

```rust
pub async fn start(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    guards: McpGuards,
    bind: std::net::IpAddr,
    port: u16,
    token: String,
    allowed_hosts: Vec<String>,
) -> Result<CancellationToken, String> {
    // The desktop always passes loopback (see `bootstrap::mcp`); only the
    // hub daemon binds a routable address, and only behind TLS or an
    // explicit `--allow-plaintext`.
    let addr = SocketAddr::from((bind, port));
    …
        let auth_state = AuthState {
            master: Arc::new(token),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(auth::normalize_allowed_hosts(&allowed_hosts)),
        };
```

(replace the `Ipv4Addr::LOCALHOST` line and its invariant comment; the `tracing::info!` line prints `addr` already.)

Update the two test constructions of `AuthState` in `mod.rs` with `allowed_hosts: Arc::new(vec![])`, and add to `mcp_and_hook_routes_serve_behind_shared_auth` a second app built with `allowed_hosts: Arc::new(vec!["fleet.example.com".into()])` asserting a request with `Host: fleet.example.com` and the master bearer returns 200 and one with `Host: other.example.com` returns 403.

Add a `start` test:

```rust
    #[tokio::test]
    async fn start_binds_the_requested_address() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let guards = McpGuards::new(Arc::new(|_| {}));
        let shutdown = start(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            guards,
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            0, // any free port is fine: we only check the bind succeeds
            "tok".into(),
            vec![],
        )
        .await;
        assert!(shutdown.is_ok(), "{shutdown:?}");
        shutdown.unwrap().cancel();
    }
```

(If `start` with port 0 is awkward because the bound port is not returned, bind an ephemeral listener first to pick a free port, drop it, and pass that port.) Note: `start` spawns via `rt::spawn`, which inside `#[tokio::test]` uses the current runtime.

Callers: `src-tauri/src/bootstrap/mcp.rs` and `src-tauri/src/commands/mcp.rs` pass `std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)`, `port`, `token`, `Vec::new()`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace mcp && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass, including the new allowlist, routing and bind tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/mcp src-tauri/src
git commit -m "feat(mcp): configurable bind address and Host/Origin allowlist"
```

---

### Task 5: `HubBase` — public base URL for provisioning, tunnels skipped

**Files:**
- Create: `crates/fleet-core/src/service/hub.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (`pub mod hub;`), `crates/fleet-core/src/service/hooks_install.rs` (`hook_url`, `hook_entry`, `merge_hook_into_settings_json`, `install_hook_at`, `auto_install_local_hook`, tests), `crates/fleet-core/src/service/provision.rs` (`provision_one`, `provision_hook`, `provision_host_with_token`, `provision_hosts`, `reestablish_tunnels`, tests), `crates/fleet-core/src/mcp/tools/fleet.rs` (`provision_hosts` tool), `src-tauri/src/commands/mcp.rs` (`mcp_configure`, `install_fleet_hook`), `src-tauri/src/bootstrap/mcp.rs`

**Interfaces:**
- Produces:

```rust
// crates/fleet-core/src/service/hub.rs
pub const SETTING_BIND: &str = "hub.bind";
pub const SETTING_PUBLIC_URL: &str = "hub.public_url";
pub const SETTING_ALLOWED_HOSTS: &str = "hub.allowed_hosts";
pub const SETTING_LOCAL_HOST: &str = "hub.local_host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubBase { pub url: String, pub port: u16, pub public: bool }
impl HubBase {
    pub fn loopback(port: u16) -> Self;
    pub fn public(url: &str, port: u16) -> Result<Self, IpcError>;   // E_VALIDATE on bad URL
    pub fn read(s: &Store) -> Result<Self, IpcError>;                 // from settings; E_PROVISION when no token yet
    pub fn mcp_url(&self) -> String;                                  // "<url>/mcp"
    pub fn hook_url(&self) -> String;                                 // "<url>/hook"
    pub fn host(&self) -> String;                                     // authority of `url` ("fleet.example.com:8443")
}
pub fn read_local_host(s: &Store) -> bool;                            // hub.local_host, default true
```

- `hooks_install::hook_entry(hook_url: &str, token: &str)`, `merge_hook_into_settings_json(existing, hook_url: &str, token)`, `install_hook_at(path, hook_url: &str, token)`, `auto_install_local_hook(store, base: &HubBase)`; the old `hook_url(port)` is deleted.
- `provision::provision_one(ssh, host, base: &HubBase, token)`, `provision_hook(ssh, host, hook_url: &str, token)`, `provision_host_with_token(store, ssh, tunnels, host, base: &HubBase, rotate)`, `provision_hosts(store, ssh, tunnels, base: &HubBase, rotate)`, `reestablish_tunnels(store, tunnels, base: &HubBase)`.

- [ ] **Step 1: Write the failing tests for `HubBase`**

`crates/fleet-core/src/service/hub.rs` (tests at the bottom; implementation in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn loopback_base_uses_the_port() {
        let b = HubBase::loopback(4180);
        assert_eq!(b.url, "http://127.0.0.1:4180");
        assert_eq!(b.mcp_url(), "http://127.0.0.1:4180/mcp");
        assert_eq!(b.hook_url(), "http://127.0.0.1:4180/hook");
        assert_eq!(b.host(), "127.0.0.1:4180");
        assert!(!b.public);
    }

    #[test]
    fn public_base_strips_trailing_slash_and_validates() {
        let b = HubBase::public("https://Fleet.Example.com/", 4180).unwrap();
        assert_eq!(b.url, "https://fleet.example.com");
        assert_eq!(b.hook_url(), "https://fleet.example.com/hook");
        assert_eq!(b.host(), "fleet.example.com");
        assert!(b.public);
        let with_port = HubBase::public("http://10.0.0.5:8443", 4180).unwrap();
        assert_eq!(with_port.host(), "10.0.0.5:8443");
        for bad in ["ftp://x", "fleet.example.com", "https://x/mcp", "https://x?y=1", ""] {
            let e = HubBase::public(bad, 4180).unwrap_err();
            assert_eq!(e.code, crate::ipc_error::codes::E_VALIDATE, "{bad}");
        }
    }

    #[test]
    fn read_prefers_the_public_url_setting() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(crate::mcp::SETTING_TOKEN, "tok").unwrap();
        s.set_setting(crate::mcp::SETTING_PORT, "4321").unwrap();
        assert_eq!(HubBase::read(&s).unwrap(), HubBase::loopback(4321));
        s.set_setting(SETTING_PUBLIC_URL, "https://fleet.example.com").unwrap();
        let b = HubBase::read(&s).unwrap();
        assert_eq!(b.url, "https://fleet.example.com");
        assert_eq!(b.port, 4321);
        assert!(b.public);
    }

    #[test]
    fn read_refuses_without_a_master_token() {
        let s = Store::open_in_memory().unwrap();
        let e = HubBase::read(&s).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_PROVISION);
    }

    #[test]
    fn local_host_defaults_to_true_and_reads_false() {
        let s = Store::open_in_memory().unwrap();
        assert!(read_local_host(&s));
        s.set_setting(SETTING_LOCAL_HOST, "false").unwrap();
        assert!(!read_local_host(&s));
        s.set_setting(SETTING_LOCAL_HOST, "true").unwrap();
        assert!(read_local_host(&s));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core service::hub`
Expected: compile error (module missing).

- [ ] **Step 3: Implement `hub.rs`**

```rust
//! Where the hub is reachable, as every host must be told: the base URL its
//! hooks and MCP entry point at. Loopback + reverse tunnel on the desktop;
//! a public URL on a `fleet-hub` daemon (no tunnels then).

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;

pub const SETTING_BIND: &str = "hub.bind";
pub const SETTING_PUBLIC_URL: &str = "hub.public_url";
pub const SETTING_ALLOWED_HOSTS: &str = "hub.allowed_hosts";
pub const SETTING_LOCAL_HOST: &str = "hub.local_host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubBase {
    /// `http://127.0.0.1:<port>` or the public URL, no trailing slash.
    pub url: String,
    /// The listening port (the tunnel's remote port on the desktop).
    pub port: u16,
    /// True for a public URL: hooks post to it directly, no tunnel.
    pub public: bool,
}

impl HubBase {
    pub fn loopback(port: u16) -> Self {
        Self { url: format!("http://127.0.0.1:{port}"), port, public: false }
    }

    /// A public base: `http(s)://host[:port]`, nothing after the authority
    /// (a single trailing `/` is tolerated and stripped). Lower-cased.
    pub fn public(url: &str, port: u16) -> Result<Self, IpcError> {
        let trimmed = url.trim().trim_end_matches('/').to_ascii_lowercase();
        let rest = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .ok_or_else(|| IpcError::new(codes::E_VALIDATE, "public URL must start with http:// or https://"))?;
        if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                "public URL must be scheme://host[:port] with no path or query",
            ));
        }
        Ok(Self { url: trimmed, port, public: true })
    }

    /// From settings: `hub.public_url` when set, else loopback on `mcp.port`.
    /// Refuses (`E_PROVISION`) when the control API has no master token yet.
    pub fn read(s: &Store) -> Result<Self, IpcError> {
        let port = crate::mcp::settings::configured_port(s)?;
        match s.get_setting(SETTING_PUBLIC_URL)?.filter(|u| !u.trim().is_empty()) {
            Some(url) => Self::public(&url, port),
            None => Ok(Self::loopback(port)),
        }
    }

    pub fn mcp_url(&self) -> String { format!("{}/mcp", self.url) }
    pub fn hook_url(&self) -> String { format!("{}/hook", self.url) }

    /// The authority part (`host[:port]`), for the Host allowlist.
    pub fn host(&self) -> String {
        self.url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    }
}

/// `hub.local_host`: whether this hub's own machine is a fleet host.
/// Unset → true (the desktop); the daemon sets it false by default.
pub fn read_local_host(s: &Store) -> bool {
    !matches!(s.get_setting(SETTING_LOCAL_HOST).ok().flatten().as_deref(), Some("false"))
}
```

Add `pub mod hub;` to `service/mod.rs` (alphabetical, after `hosts`). Run `cargo test -p fleet-core service::hub` → 5 passed.

- [ ] **Step 4: Update the hook builders and their tests**

`hooks_install.rs`:

```rust
/// One Claude Code `type: "http"` hook entry pointing at `hook_url`
/// (`HubBase::hook_url()`). The bearer token rides in an `Authorization`
/// header — never in a process argv (SEC-3).
pub fn hook_entry(hook_url: &str, token: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": hook_url,
        "headers": { "Authorization": format!("Bearer {token}") },
        "timeout": HOOK_TIMEOUT_SECS
    })
}
```

Delete `pub fn hook_url(port: u16)`. `merge_hook_into_settings_json(existing: &str, hook_url: &str, token: &str)`: `let fleet_prefix = hook_url.to_string();` and `hook_entry(hook_url, token)`. `install_hook_at(settings_path, hook_url: &str, token)` passes it through. `auto_install_local_hook(store: &Mutex<Store>, base: &HubBase)` calls `install_hook_at(&path, &base.hook_url(), &token)` and logs `url = %base.hook_url()` instead of `port`.

Tests in `hooks_install.rs`: replace every `4180` port argument with the string `"http://127.0.0.1:4180/hook"` (e.g. `install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok")`, `hook_entry("http://127.0.0.1:4180/hook", "tok")`, `merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "tok")`); the assertions on the URL string stay. Add one test:

```rust
    #[test]
    fn merge_hook_with_a_public_base_writes_that_url() {
        let out = merge_hook_into_settings_json("", "https://fleet.example.com/hook", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["url"], "https://fleet.example.com/hook");
        // Re-merging under a different base replaces the fleet entries (no duplicates).
        let out2 = merge_hook_into_settings_json(&out, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }
```

If that last assertion fails, `strip_fleet` only matches the current prefix: make it also strip any entry whose `url` ends with `/hook` and whose `headers.Authorization` starts with `Bearer ` (a fleet-shaped entry), so a base-URL change never leaves a stale hook behind. Keep the legacy `command` match.

- [ ] **Step 5: Update provisioning**

`provision.rs`:

```rust
pub async fn provision_one(
    ssh: &dyn SshExec,
    host: &str,
    base: &HubBase,
    token: &str,
) -> Result<(), IpcError> {
    …
    let merged = merge_mcp_entry(&existing, &base.mcp_url(), token)?;
    …
    provision_hook(ssh, host, &base.hook_url(), token).await?;
    Ok(())
}

pub async fn provision_hook(ssh: &dyn SshExec, host: &str, hook_url: &str, token: &str) -> Result<(), IpcError> {
    …
    let merged = super::hooks_install::merge_hook_into_settings_json(&existing, hook_url, token)?;
    …
}

pub async fn provision_host_with_token(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tunnels: &Arc<TunnelSupervisor>,
    host: &str,
    base: &HubBase,
    rotate: bool,
) -> Result<(), IpcError> {
    let (token, minted) = resolve_host_token(store, host, rotate)?;
    provision_one(ssh, host, base, &token).await?;
    commit_host_token(store, host, &token, minted)?;
    // A public hub is reached directly; only a loopback hub needs the
    // reverse tunnel so the host's 127.0.0.1:<port> lands on this machine.
    if host != "local" && !base.public {
        tunnels.ensure(host, base.port, base.port);
    }
    …
}
```

`provision_hosts(store, ssh, tunnels, base: &HubBase, rotate)` passes `base` through. `reestablish_tunnels(store, tunnels, base: &HubBase)`: first line `if base.public { return Ok(()); }`, then `tunnels.ensure(&h.alias, base.port, base.port)`. `use crate::service::hub::HubBase;` at the top.

Provision tests: introduce `fn base() -> HubBase { HubBase::loopback(PORT) }` next to the existing `PORT` const; every `provision_one(&ssh, host, url, TOKEN, PORT)`-shaped call becomes `provision_one(&ssh, host, &base(), TOKEN)`; `expected_settings()` becomes `merge_hook_into_settings_json("", &base().hook_url(), TOKEN)`. Add:

```rust
    #[tokio::test]
    async fn reestablish_tunnels_is_a_no_op_for_a_public_base() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("mefistos").unwrap();
            s.set_host_provisioned("mefistos", true).unwrap();
        }
        let spawned = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&spawned);
        let tunnels = Arc::new(TunnelSupervisor::with_spawner(
            Box::new(move |host: &str, _r: u16, _m: u16| {
                seen.lock().unwrap().push(host.to_string());
                Box::pin(async { Ok(()) })
            }),
            std::time::Duration::from_millis(10),
        ));
        reestablish_tunnels(&store, &tunnels, &HubBase::public("https://fleet.example.com", 4180).unwrap()).unwrap();
        assert!(tunnels.snapshot().is_empty(), "no tunnel for a public hub");
        reestablish_tunnels(&store, &tunnels, &HubBase::loopback(4180)).unwrap();
        assert_eq!(tunnels.snapshot().len(), 1, "loopback hub tunnels provisioned hosts");
    }
```

Match the spawner closure's type to `TunnelSpawner` as declared in `tunnel.rs` (read it; the shape above is illustrative — copy the signature from the existing `with_spawner` tests in that file).

- [ ] **Step 6: Update the callers**

`crates/fleet-core/src/mcp/tools/fleet.rs` `provision_hosts` tool:

```rust
        let base = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::hub::HubBase::read(&s).map_err(to_mcp_err)?
        };
        let res = crate::service::provision::provision_hosts(&self.store, &self.ssh, &self.tunnels, &base, p.rotate)
```

Update the tool's `#[tool(description = …)]` text: replace "(reverse SSH tunnel for remote hosts)" with "(reverse SSH tunnel for remote hosts when the hub is loopback-only; a hub with a public URL is reached directly)". Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` and commit the regenerated `docs/control-api-reference.md`.

`src-tauri/src/bootstrap/mcp.rs`: after `mcp::start(...)` succeeds, `let base = { let s = store.lock()…; HubBase::read(&s) }` and `reestablish_tunnels(store, tunnels, &base)`; the `mcp::start` call passes loopback + `Vec::new()` (Task 4).

`src-tauri/src/commands/mcp.rs` `mcp_configure`: after `rt.set_running`, compute `base` the same way and call `reestablish_tunnels(&store, &tunnels, &base)` and `hooks_install::auto_install_local_hook(&store, &base)`. `install_fleet_hook`: replace `let port = …` with `let base = fleet_core::service::hub::HubBase::read(&*lock(&store)?)?;`, call `install_hook_at(&settings_path, &base.hook_url(), &token)`, and format the message with `base.hook_url()`.

- [ ] **Step 7: Verify**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && grep -rn "hook_url(port\|127.0.0.1:{" crates/fleet-core/src/service`
Expected: green; the grep prints nothing (no port-formatted URL left in provisioning; `commands/mcp.rs` `status()` keeps its loopback display URL, that is the desktop's own address).

- [ ] **Step 8: Commit**

```bash
git add crates src-tauri docs/control-api-reference.md
git commit -m "feat(provision): HubBase — public base URL for hooks and MCP entries, tunnels only for a loopback hub"
```

---

### Task 6: `hub.local_host` opt-out in reconcile

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs` (`ReconcileDeps`, `real`, `fake*`, `reconcile_sessions_with`, `reconcile_one_host`, `list_sessions`, `refresh_sessions`, `reconcile_now`), `crates/fleet-core/src/service/reconcile_tests.rs`

**Interfaces:**
- Produces: `ReconcileDeps::real(ssh: &Arc<SshClient>, local_host: bool) -> Arc<Self>`; `ReconcileDeps::fake_without_local(exec, probe_timeout) -> Arc<Self>` (test); field `pub(super) local_host: bool`.

- [ ] **Step 1: Write the failing test** (in `reconcile_tests.rs`, next to the existing `Fleet` fixture; add a constructor variant)

```rust
impl Fleet {
    /// Like `new`, but the hub has no `local` host (a `fleet-hub` daemon):
    /// no `local` row is created and the fake never answers for it.
    fn new_without_local(hosts: &[&str]) -> Self {
        let bus = Arc::new(RecordingEventBus::new());
        let store = Store::open_with_bus_in_memory(bus.clone()).expect("store");
        for h in hosts {
            store.upsert_host(h).unwrap();
        }
        let fake = FakeSsh::new();
        let exec_fake = fake.clone();
        let deps = ReconcileDeps::fake_without_local(
            move |alias| {
                Box::new(RemoteTmux { client: exec_fake.clone(), host: alias.to_string() })
            },
            Duration::from_secs(5),
        );
        bus.take();
        Self { store: Mutex::new(store), bus, fake, deps }
    }
}

#[tokio::test]
async fn reconcile_without_local_host_never_creates_or_probes_local() {
    let f = Fleet::new_without_local(&["mefistos"]);
    f.fake.on_host(
        "mefistos",
        Match::script(LIST_SCRIPT),
        Reply::Exit { code: 1, stdout: b"no server running\n".to_vec(), stderr: Vec::new() },
    );
    reconcile_sessions_with(&f.store, &f.deps).await.unwrap();
    let hosts = f.store.lock().unwrap().list_hosts().unwrap();
    assert!(hosts.iter().all(|h| h.alias != "local"), "no local row: {hosts:?}");
    assert!(
        f.fake.calls().iter().all(|c| c.host != "local"),
        "local must never be probed"
    );
    assert!(f.reachable("mefistos"));
}

#[tokio::test]
async fn reconcile_without_local_host_skips_a_copied_local_row() {
    // A state.db copied from a desktop carries a `local` row; the daemon
    // leaves it alone and never probes it.
    let f = Fleet::new_without_local(&["local", "mefistos"]);
    f.fake.on_host(
        "mefistos",
        Match::script(LIST_SCRIPT),
        Reply::Exit { code: 1, stdout: b"no server running\n".to_vec(), stderr: Vec::new() },
    );
    reconcile_sessions_with(&f.store, &f.deps).await.unwrap();
    assert!(f.fake.calls().iter().all(|c| c.host != "local"));
}
```

Use the fixture's existing accessor names (`f.reachable(alias)`, `f.fake.calls()` or whatever `FakeSsh` exposes — read `ssh_fake.rs` and use its real method for "calls seen"; if none exists, register no reply for `local` so any probe of it returns the fake's "unexpected call" error and the pass would fail).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleet-core reconcile_without_local`
Expected: compile error, `fake_without_local` not found.

- [ ] **Step 3: Implement**

`reconcile.rs`, in `ReconcileDeps`:

```rust
    /// Whether this hub's own machine is a fleet host. False on a
    /// `fleet-hub` daemon (`hub.local_host = false`): no `local` row is
    /// created, an existing one is never probed, and the local Claude
    /// account is not read.
    pub(super) local_host: bool,
```

`real`:

```rust
    pub(super) fn real(ssh: &Arc<SshClient>, local_host: bool) -> Arc<Self> {
        …
            local_home: local_host.then(crate::service::hosts::local_home_dir),
            local_host,
        })
    }
```

`fake_with_shell`: `local_host: true`. Add:

```rust
    #[cfg(test)]
    pub(crate) fn fake_without_local(
        exec: impl Fn(&str) -> Box<dyn TmuxExec> + Send + Sync + 'static,
        probe_timeout: std::time::Duration,
    ) -> Arc<Self> {
        let mut deps = Self::fake(exec, probe_timeout);
        Arc::get_mut(&mut deps).expect("fresh Arc").local_host = false;
        deps
    }
```

`reconcile_sessions_with`: wrap step 0 (`upsert_host("local")` + the `sync_local_account` block) in `if deps.local_host { … }`; in step 1 replace `s.upsert_host("local")?;` with `if deps.local_host { s.upsert_host("local")?; }` and filter the snapshot: `.filter(|h| deps.local_host || h.alias != "local")` before the `.map`. Search the rest of the function and `reconcile_one_host_with` for other `"local"` special cases that would probe the local machine (e.g. a `local` reachability short-circuit) and guard them the same way.

The four `ReconcileDeps::real(ssh)` call sites become `ReconcileDeps::real(ssh, local_host(store))` with a private helper in `reconcile.rs`:

```rust
/// `hub.local_host` for this pass; a poisoned lock counts as "true" (the
/// desktop default) so reconcile keeps its old behaviour on error.
fn local_host(store: &Mutex<Store>) -> bool {
    store
        .lock()
        .map(|s| crate::service::hub::read_local_host(&s))
        .unwrap_or(true)
}
```

(`reconcile_one_host`, `list_sessions`, `refresh_sessions`, `reconcile_now` all have `store` in scope.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p fleet-core reconcile && cargo clippy --workspace --all-targets -- -D warnings`
Expected: the two new tests pass; every existing reconcile test still passes (they run with `local_host: true`).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service
git commit -m "feat(reconcile): hub.local_host opt-out — a daemon hub has no local host"
```

---

### Task 7: The `fleet-hub` binary

**Files:**
- Create: `crates/fleet-hub/Cargo.toml`, `crates/fleet-hub/src/main.rs`, `crates/fleet-hub/src/config.rs`, `crates/fleet-hub/src/serve.rs`, `crates/fleet-hub/src/out.rs`
- Modify: `crates/fleet-core/src/no_eprintln_tests.rs` (scan `fleet-hub` too, allowlist `out.rs`)

**Interfaces:**
- Consumes: `fleet_core::{logging, rt, mcp, service::{hub, tick, provision, tunnel, account_usage}, ssh, cancel, store, events::NoopEventBus}` as produced by Tasks 2–6.
- Produces: the `fleet-hub` CLI (`init`, `serve`, `token show|regenerate`, `ssh-key`); `config::Resolved` and `config::resolve`.

- [ ] **Step 1: Write the failing config tests** (`crates/fleet-hub/src/config.rs`, bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn opts() -> HubOptions {
        HubOptions {
            data_dir: None, bind: None, port: None, public_url: None,
            allowed_host: vec![], local_host: None, allow_plaintext: false, log_dir: None,
        }
    }

    #[test]
    fn defaults_are_loopback_no_public_url_no_local_host() {
        let r = resolve(&opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(r.bind, "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert_eq!(r.port, 4180);
        assert_eq!(r.public_url, None);
        assert!(!r.local_host);
        assert_eq!(r.allowed_hosts, Vec::<String>::new());
        assert_eq!(r.log_dir, r.data_dir.join("logs"));
    }

    #[test]
    fn flag_beats_env_beats_setting_beats_default() {
        let settings = |k: &str| match k {
            "mcp.port" => Some("5000".to_string()),
            "hub.bind" => Some("10.0.0.1".to_string()),
            _ => None,
        };
        let e = env(&[("FLEET_HUB_PORT", "6000"), ("FLEET_HUB_BIND", "10.0.0.2")]);
        let mut o = opts();
        o.port = Some(7000);
        let r = resolve(&o, &e, &settings).unwrap();
        assert_eq!(r.port, 7000, "flag wins");
        assert_eq!(r.bind.to_string(), "10.0.0.2", "env beats setting");
        let r = resolve(&opts(), &env(&[]), &settings).unwrap();
        assert_eq!(r.port, 5000, "setting beats default");
        assert_eq!(r.bind.to_string(), "10.0.0.1");
    }

    #[test]
    fn allowed_hosts_default_to_the_public_url_host() {
        let mut o = opts();
        o.public_url = Some("https://Fleet.Example.com".into());
        let r = resolve(&o, &env(&[]), &|_| None).unwrap();
        assert_eq!(r.allowed_hosts, vec!["fleet.example.com".to_string()]);
        let e = env(&[("FLEET_HUB_ALLOWED_HOSTS", "a.example.com, b.example.com:8443")]);
        let r = resolve(&o, &e, &|_| None).unwrap();
        assert_eq!(r.allowed_hosts, vec!["a.example.com".to_string(), "b.example.com:8443".to_string()]);
    }

    #[test]
    fn plaintext_on_a_routable_bind_is_refused_unless_allowed() {
        let mut o = opts();
        o.bind = Some("0.0.0.0".into());
        o.public_url = Some("http://fleet.example.com".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--allow-plaintext"), "{e}");
        o.allow_plaintext = true;
        assert!(resolve(&o, &env(&[]), &|_| None).is_ok());
        // https is always fine; loopback is always fine.
        o.allow_plaintext = false;
        o.public_url = Some("https://fleet.example.com".into());
        assert!(resolve(&o, &env(&[]), &|_| None).is_ok());
        o.bind = Some("127.0.0.1".into());
        o.public_url = Some("http://fleet.example.com".into());
        assert!(resolve(&o, &env(&[]), &|_| None).is_ok());
    }

    #[test]
    fn bad_values_are_reported_by_name() {
        let mut o = opts();
        o.bind = Some("not-an-ip".into());
        assert!(resolve(&o, &env(&[]), &|_| None).unwrap_err().contains("bind"));
        let mut o = opts();
        o.public_url = Some("fleet.example.com".into());
        assert!(resolve(&o, &env(&[]), &|_| None).unwrap_err().contains("public URL"));
        let e = env(&[("FLEET_HUB_LOCAL_HOST", "yes")]);
        assert!(resolve(&opts(), &e, &|_| None).unwrap_err().contains("local_host"));
    }
}
```

- [ ] **Step 2: Create the crate**

`crates/fleet-hub/Cargo.toml`:

```toml
[package]
name = "fleet-hub"
version = "0.2.18"
description = "Headless claude-fleet hub: runs the fleet without the desktop app"
authors = ["martin-janci"]
edition = "2021"
license = "MIT"

[[bin]]
name = "fleet-hub"
path = "src/main.rs"

[dependencies]
fleet-core = { path = "../fleet-core" }
clap = { version = "4", features = ["derive", "env"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
directories = "5"
```

(Version tracks the desktop's; `scripts/release.sh` bumps it in Task 8.)

`crates/fleet-hub/src/out.rs`:

```rust
//! The CLI's only stdout/stderr writer. Everything else logs through
//! `tracing`; the `no_eprintln` guard allowlists this file by name.

use std::io::Write;

pub fn line(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{s}");
}

pub fn error(s: &str) {
    let mut e = std::io::stderr().lock();
    let _ = writeln!(e, "fleet-hub: {s}");
}
```

- [ ] **Step 3: Implement `config.rs`**

```rust
//! Flag > `FLEET_HUB_*` env > `settings` row > default.

use clap::Args;
use fleet_core::service::hub::{HubBase, SETTING_ALLOWED_HOSTS, SETTING_BIND, SETTING_LOCAL_HOST, SETTING_PUBLIC_URL};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

pub const DEFAULT_BIND: &str = "127.0.0.1";

/// Options shared by `init` and `serve`. Env names are spelled out so
/// `--help` shows them; the precedence itself is applied in [`resolve`].
#[derive(Args, Debug, Clone, Default)]
pub struct HubOptions {
    /// Data directory holding state.db and logs/ [env: FLEET_HUB_DATA_DIR]
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Listen address [env: FLEET_HUB_BIND] [default: 127.0.0.1]
    #[arg(long)]
    pub bind: Option<String>,
    /// Listen port [env: FLEET_HUB_PORT] [default: 4180]
    #[arg(long)]
    pub port: Option<u16>,
    /// Public base URL hosts and clients reach this hub at, e.g. https://fleet.example.com [env: FLEET_HUB_PUBLIC_URL]
    #[arg(long)]
    pub public_url: Option<String>,
    /// Extra Host/Origin values to accept (repeatable) [env: FLEET_HUB_ALLOWED_HOSTS, comma-separated]
    #[arg(long = "allowed-host")]
    pub allowed_host: Vec<String>,
    /// Treat this machine as a fleet host too [env: FLEET_HUB_LOCAL_HOST] [default: false]
    #[arg(long, action = clap::ArgAction::Set)]
    pub local_host: Option<bool>,
    /// Permit a non-loopback bind with an http:// public URL (container-internal use only) [env: FLEET_HUB_ALLOW_PLAINTEXT]
    #[arg(long)]
    pub allow_plaintext: bool,
    /// Log directory [env: FLEET_HUB_LOG_DIR] [default: <data-dir>/logs]
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub data_dir: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub public_url: Option<String>,
    pub allowed_hosts: Vec<String>,
    pub local_host: bool,
    pub log_dir: PathBuf,
}

impl Resolved {
    pub fn base(&self) -> Result<HubBase, String> {
        match &self.public_url {
            Some(u) => HubBase::public(u, self.port).map_err(|e| e.message),
            None => Ok(HubBase::loopback(self.port)),
        }
    }
}

/// Platform default data dir (same app id as the desktop, so a copied
/// `state.db` lands where the desktop would look for it on that OS).
pub fn default_data_dir() -> PathBuf {
    directories::ProjectDirs::from("sk", "rlt", "claude-fleet")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/var/lib/fleet-hub"))
}

fn pick(flag: Option<String>, env: &HashMap<String, String>, env_key: &str, setting: Option<String>) -> Option<String> {
    flag.or_else(|| env.get(env_key).cloned())
        .or(setting)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `settings` reads a `settings` row by key (None when there is no store yet).
pub fn resolve(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    settings: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved, String> {
    let data_dir = opts
        .data_dir
        .clone()
        .or_else(|| env.get("FLEET_HUB_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(default_data_dir);

    let bind_s = pick(opts.bind.clone(), env, "FLEET_HUB_BIND", settings(SETTING_BIND))
        .unwrap_or_else(|| DEFAULT_BIND.to_string());
    let bind: IpAddr = bind_s.parse().map_err(|e| format!("bind '{bind_s}' is not an IP address: {e}"))?;

    let port = match pick(opts.port.map(|p| p.to_string()), env, "FLEET_HUB_PORT", settings("mcp.port")) {
        Some(p) => p.parse::<u16>().map_err(|e| format!("port '{p}': {e}"))?,
        None => fleet_core::mcp::DEFAULT_PORT,
    };

    let public_url = pick(opts.public_url.clone(), env, "FLEET_HUB_PUBLIC_URL", settings(SETTING_PUBLIC_URL));
    let base = match &public_url {
        Some(u) => HubBase::public(u, port).map_err(|e| format!("public URL: {}", e.message))?,
        None => HubBase::loopback(port),
    };

    let allowed_raw: Vec<String> = if !opts.allowed_host.is_empty() {
        opts.allowed_host.clone()
    } else if let Some(v) = env.get("FLEET_HUB_ALLOWED_HOSTS").or(settings(SETTING_ALLOWED_HOSTS).as_ref()) {
        v.split(',').map(str::to_string).collect()
    } else if base.public {
        vec![base.host()]
    } else {
        vec![]
    };
    let allowed_hosts = fleet_core::mcp::normalize_allowed_hosts(&allowed_raw);

    let local_host = match pick(opts.local_host.map(|b| b.to_string()), env, "FLEET_HUB_LOCAL_HOST", settings(SETTING_LOCAL_HOST)) {
        None => false,
        Some(v) if v == "true" => true,
        Some(v) if v == "false" => false,
        Some(v) => return Err(format!("local_host must be true or false, got '{v}'")),
    };

    let plaintext_public = base.public && base.url.starts_with("http://");
    let allow_plaintext = opts.allow_plaintext
        || env.get("FLEET_HUB_ALLOW_PLAINTEXT").is_some_and(|v| v == "1" || v == "true");
    if !bind.is_loopback() && plaintext_public && !allow_plaintext {
        return Err(format!(
            "refusing to serve plaintext http on {bind}: use an https:// public URL, \
             bind to 127.0.0.1 behind a TLS proxy, or pass --allow-plaintext"
        ));
    }

    let log_dir = opts
        .log_dir
        .clone()
        .or_else(|| env.get("FLEET_HUB_LOG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.join("logs"));

    Ok(Resolved { data_dir, bind, port, public_url: public_url.map(|_| base.url.clone()), allowed_hosts, local_host, log_dir })
}
```

`fleet_core::mcp::normalize_allowed_hosts` must be re-exported: in `crates/fleet-core/src/mcp/mod.rs` add `pub use auth::normalize_allowed_hosts;`. (`settings.as_ref()` in the `or(...)` chain: write it so it type-checks — compute `let from_setting = settings(SETTING_ALLOWED_HOSTS);` first.)

Run: `cargo test -p fleet-hub config` → 5 passed.

- [ ] **Step 4: Implement `main.rs`**

```rust
//! `fleet-hub` — claude-fleet without the desktop app. See `docs/hub.md`.

mod config;
mod out;
mod serve;

use clap::{Parser, Subcommand};
use config::HubOptions;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "fleet-hub", version, about = "Headless claude-fleet hub")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the data dir and state.db, mint the master token, print it once.
    Init {
        #[command(flatten)]
        opts: HubOptions,
        /// Mint a fresh master token even if one exists.
        #[arg(long)]
        regenerate_token: bool,
    },
    /// Run the hub until SIGTERM/SIGINT.
    Serve {
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Show or rotate the master token.
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Print this hub's SSH public key (generated on first use).
    SshKey,
}

#[derive(Subcommand)]
enum TokenCmd {
    Show,
    Regenerate,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let result = match cli.cmd {
        Cmd::Init { opts, regenerate_token } => serve::init(&opts, &env, regenerate_token),
        Cmd::Serve { opts } => serve::serve(&opts, &env).await,
        Cmd::Token { cmd, opts } => serve::token(&opts, &env, matches!(cmd, TokenCmd::Regenerate)),
        Cmd::SshKey => serve::ssh_key(),
    };
    match result {
        Ok(code) => code,
        Err(msg) => {
            out::error(&msg);
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_every_subcommand() {
        Cli::try_parse_from(["fleet-hub", "init", "--public-url", "https://x.example.com"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "serve", "--bind", "0.0.0.0", "--allow-plaintext"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "token", "show"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "token", "regenerate"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "ssh-key"]).unwrap();
        assert!(Cli::try_parse_from(["fleet-hub", "bogus"]).is_err());
    }
}
```

- [ ] **Step 5: Implement `serve.rs`**

```rust
//! The subcommands' bodies: opening the store, starting the same ticks and
//! server the desktop starts, and stopping them on a signal.

use crate::config::{resolve, HubOptions, Resolved};
use crate::out;
use fleet_core::events::NoopEventBus;
use fleet_core::mcp::{self, settings::ensure_master_token, McpGuards};
use fleet_core::service::hub::{SETTING_ALLOWED_HOSTS, SETTING_BIND, SETTING_LOCAL_HOST, SETTING_PUBLIC_URL};
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

/// Resolve options against the settings in `<data-dir>/state.db` when it
/// exists (a second `resolve` pass: the first has no store to read).
fn resolve_with_store(opts: &HubOptions, env: &HashMap<String, String>) -> Result<(Resolved, Arc<Mutex<Store>>), String> {
    let first = resolve(opts, env, &|_| None)?;
    std::fs::create_dir_all(&first.data_dir).map_err(|e| format!("create data dir {}: {e}", first.data_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&first.data_dir, std::fs::Permissions::from_mode(0o700));
    }
    let db_path = first.data_dir.join("state.db");
    let store = Store::open_with_bus(&db_path, Arc::new(NoopEventBus)).map_err(|e| {
        format!(
            "failed to open the claude-fleet database at {}: {e}\n\
             If the file is corrupt, deleting it resets all hub state — hosts, projects and sessions are re-discovered.",
            db_path.display()
        )
    })?;
    fleet_core::service::provision::set_private_mode(&db_path);
    let resolved = {
        let settings = |k: &str| store.get_setting(k).ok().flatten();
        resolve(opts, env, &settings)?
    };
    Ok((resolved, Arc::new(Mutex::new(store))))
}

/// Persist the resolved `hub.*` values (and force the API on) so MCP tools
/// that read settings — provisioning above all — see what the process runs with.
fn persist(store: &Mutex<Store>, r: &Resolved) -> Result<(), String> {
    let s = store.lock().map_err(|_| "store lock poisoned".to_string())?;
    let set = |k: &str, v: &str| s.set_setting(k, v).map_err(|e| format!("write setting {k}: {e}"));
    set(mcp::SETTING_ENABLED, "true")?;
    set(mcp::SETTING_PORT, &r.port.to_string())?;
    set(SETTING_BIND, &r.bind.to_string())?;
    set(SETTING_PUBLIC_URL, r.public_url.as_deref().unwrap_or(""))?;
    set(SETTING_ALLOWED_HOSTS, &r.allowed_hosts.join(","))?;
    set(SETTING_LOCAL_HOST, if r.local_host { "true" } else { "false" })?;
    if !r.local_host {
        // A state.db copied from a desktop carries a `local` row; hide it so
        // nothing lists or probes it (reconcile skips it regardless).
        if s.list_hosts().map_err(|e| e.to_string())?.iter().any(|h| h.alias == "local") {
            let _ = s.set_host_hidden("local", true);
        }
    }
    Ok(())
}

pub fn init(opts: &HubOptions, env: &HashMap<String, String>, regenerate: bool) -> Result<ExitCode, String> {
    let (r, store) = resolve_with_store(opts, env)?;
    persist(&store, &r)?;
    let token = {
        let s = store.lock().map_err(|_| "store lock poisoned".to_string())?;
        if regenerate {
            s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token()).map_err(|e| e.to_string())?;
        }
        ensure_master_token(&s).map_err(|e| e.message)?
    };
    out::line(&format!("data dir: {}", r.data_dir.display()));
    out::line(&format!("listen:   {}:{}", r.bind, r.port));
    out::line(&format!("public:   {}", r.public_url.as_deref().unwrap_or("(none — loopback + reverse tunnels)")));
    out::line("master token (shown once; `fleet-hub token show` prints it again):");
    out::line(&token);
    Ok(ExitCode::SUCCESS)
}

pub fn token(opts: &HubOptions, env: &HashMap<String, String>, regenerate: bool) -> Result<ExitCode, String> {
    let (_r, store) = resolve_with_store(opts, env)?;
    let s = store.lock().map_err(|_| "store lock poisoned".to_string())?;
    if regenerate {
        s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token()).map_err(|e| e.to_string())?;
    }
    out::line(&ensure_master_token(&s).map_err(|e| e.message)?);
    Ok(ExitCode::SUCCESS)
}

pub fn ssh_key() -> Result<ExitCode, String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("HOME is not set")?;
    let key = home.join(".ssh").join("id_ed25519");
    let pubkey = key.with_extension("pub");
    if !pubkey.exists() {
        std::fs::create_dir_all(key.parent().unwrap()).map_err(|e| format!("create ~/.ssh: {e}"))?;
        let st = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "fleet-hub", "-f"])
            .arg(&key)
            .status()
            .map_err(|e| format!("run ssh-keygen: {e}"))?;
        if !st.success() {
            return Err(format!("ssh-keygen exited with {st}"));
        }
    }
    let text = std::fs::read_to_string(&pubkey).map_err(|e| format!("read {}: {e}", pubkey.display()))?;
    out::line(text.trim_end());
    Ok(ExitCode::SUCCESS)
}

pub async fn serve(opts: &HubOptions, env: &HashMap<String, String>) -> Result<ExitCode, String> {
    let (r, store) = resolve_with_store(opts, env)?;
    match fleet_core::logging::init(&r.data_dir) {
        Ok(dir) => tracing::info!(log_dir = %dir.display(), "file logging on"),
        Err(e) => {
            fleet_core::logging::init_stderr_fallback();
            tracing::warn!(error = %e, "file logging unavailable; logging to stderr only");
        }
    }
    persist(&store, &r)?;
    let token = {
        let s = store.lock().map_err(|_| "store lock poisoned".to_string())?;
        ensure_master_token(&s).map_err(|e| e.message)?
    };
    let base = r.base()?;

    let ssh = Arc::new(fleet_core::ssh::SshClient::new());
    let reg = fleet_core::cancel::CancellationRegistry::new();
    let tunnels = Arc::new(fleet_core::service::tunnel::TunnelSupervisor::new());
    // No desktop to approve a destructive-call confirmation: log it. The
    // `mcp.confirm_destructive` setting is off by default; docs/hub.md says
    // to leave it off on a hub.
    let guards = McpGuards::new(Arc::new(|req: &fleet_core::mcp::guard::ConfirmRequest| {
        tracing::warn!(tool = %req.tool, nonce = %req.nonce, "confirmation requested but this hub has no approver; disable mcp.confirm_destructive");
    }));

    let shutdown = mcp::start(
        Arc::clone(&store), Arc::clone(&ssh), Arc::clone(&reg), Arc::clone(&tunnels), guards,
        r.bind, r.port, token, r.allowed_hosts.clone(),
    )
    .await?;
    if let Err(e) = fleet_core::service::provision::reestablish_tunnels(&store, &tunnels, &base) {
        tracing::warn!(error = %e.message, "re-establishing host tunnels failed");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), public = ?r.public_url, bind = %r.bind, port = r.port, local_host = r.local_host, "fleet-hub serving");

    fleet_core::service::tick::spawn_reconcile_tick(Arc::clone(&store), Arc::clone(&ssh));
    let usage_cache = Arc::new(Mutex::new(fleet_core::service::account_usage::UsageCache::new()));
    fleet_core::service::tick::spawn_account_usage_tick(Arc::clone(&store), Arc::clone(&ssh), usage_cache, Arc::new(NoopEventBus));

    wait_for_signal().await;
    tracing::info!("fleet-hub stopping");
    shutdown.cancel();
    tunnels.stop_all();
    ssh.shutdown_all();
    Ok(ExitCode::SUCCESS)
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
```

Adjust names to what the core actually exports (`mcp::guard::ConfirmRequest` is `pub`; `service::tick::spawn_*` were made `pub` in Task 3 Step 6; `CancellationRegistry::new()` returns `Arc<Self>` as in `lib.rs`). If `logging::init` also needs the log dir to differ from `<data-dir>/logs`, add `pub fn init_in(log_dir: &Path)` to `logging.rs` that `init` delegates to, and call it with `r.log_dir`.

- [ ] **Step 6: Teach the print guard about the new crate**

`crates/fleet-core/src/no_eprintln_tests.rs`: add `manifest.join("../fleet-hub/src")` to `roots`, and skip the file named `out.rs` under that root (its whole purpose is to print). Implement the skip where files are collected: `if root.ends_with("fleet-hub/src") && path.file_name() == Some(OsStr::new("out.rs")) { continue; }`.

- [ ] **Step 7: Verify**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo run -p fleet-hub -- --help
D=$(mktemp -d)
cargo run -p fleet-hub -- init --data-dir "$D" --public-url https://fleet.example.com
cargo run -p fleet-hub -- token --data-dir "$D" show
ls -la "$D"                              # state.db is 0600, dir 0700
timeout 5 cargo run -p fleet-hub -- serve --data-dir "$D" --bind 127.0.0.1 --port 4199; echo "exit=$?"
```

Expected: tests and lints green; `init` prints a 64-hex token; `token show` prints the same token; `serve` logs `fleet-hub serving` and, killed by `timeout`, exits via the SIGTERM path (exit 124 from `timeout` is fine; the log shows `fleet-hub stopping`). While it runs, in another shell:

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:4199/mcp -H 'Content-Type: application/json' -d '{}'   # 401
```

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-hub crates/fleet-core/src/no_eprintln_tests.rs crates/fleet-core/src/mcp/mod.rs Cargo.lock
git commit -m "feat(hub): fleet-hub daemon — init, serve, token, ssh-key"
```

---

### Task 8: CI headless build job, release script, docs.yml, CLAUDE.md

**Files:**
- Modify: `.github/workflows/ci.yml`, `scripts/ci-local.sh`, `scripts/release.sh:10`, `CLAUDE.md`, `docs/RELEASING.md`

- [ ] **Step 1: Add the `hub-headless` job** to `ci.yml` after `rust`:

```yaml
  # The regression guard for the crate split: fleet-hub must build on a box
  # with none of the Tauri system libraries. If this job fails with a
  # gtk/webkit/pkg-config error, a Tauri dependency leaked into fleet-core.
  hub-headless:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: .
          key: hub-headless
      - run: cargo build -p fleet-hub --locked
      - run: cargo test -p fleet-core -p fleet-hub --locked
```

- [ ] **Step 2: Local CI**

In `scripts/ci-local.sh` `run_rust`, after the deny step:

```bash
  # Mirrors the hub-headless CI job (no Tauri libs needed).
  step cargo build -p fleet-hub --locked
```

and make `--rust-only` tolerate a box without the Tauri libs: before `step cargo clippy --workspace …`, add

```bash
  if ! pkg-config --exists gtk+-3.0 2>/dev/null; then
    echo "ci-local: no Tauri system libs (gtk+-3.0); running the headless subset only" >&2
    step cargo fmt --all --check
    step cargo clippy -p fleet-core -p fleet-hub --all-targets -- -D warnings
    step cargo test -p fleet-core -p fleet-hub
    step cargo deny check
    step cargo build -p fleet-hub --locked
    return
  fi
```

- [ ] **Step 3: Release script bumps the hub too**

`scripts/release.sh` line 10: `VERSION_FILES=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml crates/fleet-hub/Cargo.toml)`. Because `cargo update -p "$CRATE"` only refreshes `claude-fleet`, add after it: `cargo update -p fleet-hub --offline >/dev/null 2>&1 || cargo update -p fleet-hub`.

Run: `RELEASE_DRY_RUN=1 scripts/release.sh 9.9.9` on a scratch branch → both `Cargo.toml` files show `9.9.9`; then `git checkout -- .` to discard.

- [ ] **Step 4: CLAUDE.md and RELEASING.md**

`CLAUDE.md`: under **What this is** add one sentence: "The Rust side is a workspace: `crates/fleet-core` (Tauri-free service/store/SSH/MCP), `crates/fleet-hub` (headless daemon, see `docs/hub.md`), `src-tauri` (the desktop app)." Under **Build & test** add `cargo build -p fleet-hub --locked   # headless hub (no Tauri libs needed)`. Replace the `REGEN_DOCS` line with `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`. In **Architecture**, change "**Backend** (`src-tauri/src/`)" to "**Backend** (`crates/fleet-core/src/`, thin Tauri handlers in `src-tauri/src/commands/`)" and adjust the file references (`ssh.rs`, `tmux.rs`, `store/`, `events.rs`, `cancel.rs` now live in the core; `pty.rs` stays in `src-tauri`). Add a bullet: "**Hub daemon** (`crates/fleet-hub`): the same core headless; `hub.*` settings, `HubBase` in `service/hub.rs`."

`docs/RELEASING.md`: mention that the release bumps `crates/fleet-hub/Cargo.toml` and that the hub image workflow (Task 9) runs on the tag.

- [ ] **Step 5: Verify and commit**

Run: `scripts/ci-local.sh --rust-only` → green.

```bash
git add .github scripts CLAUDE.md docs/RELEASING.md
git commit -m "ci: headless fleet-hub build job; release bumps the hub crate"
```

---

### Task 9: Packaging and documentation

**Files:**
- Create: `crates/fleet-hub/Dockerfile`, `.dockerignore`, `deploy/hub/docker-compose.yml`, `deploy/hub/Caddyfile`, `deploy/hub/fleet-hub.env.example`, `deploy/hub/fleet-hub.service`, `.github/workflows/hub-image.yml`, `docs/hub.md`
- Modify: `docs/control-api.md` (Enabling it / Provisioning / Security sections), `docs/concepts.md` (Control API & tunnels), `docs/README.md` (index)

- [ ] **Step 1: Dockerfile and ignore file**

`crates/fleet-hub/Dockerfile` (build context is the repo root):

```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml
# The workspace lists src-tauri; give cargo an empty lib so resolution
# succeeds without building the desktop crate.
RUN mkdir -p src-tauri/src && echo '' > src-tauri/src/lib.rs
COPY skills ./skills
COPY docs ./docs
COPY src/lib/events.ts src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte src/lib/names.json ./src/lib/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build -p fleet-hub --release --locked \
 && cp target/release/fleet-hub /fleet-hub

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-client git ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --create-home --shell /usr/sbin/nologin fleet \
 && mkdir -p /var/lib/fleet-hub && chown fleet:fleet /var/lib/fleet-hub
COPY --from=build /fleet-hub /usr/local/bin/fleet-hub
USER fleet
ENV FLEET_HUB_DATA_DIR=/var/lib/fleet-hub
VOLUME ["/var/lib/fleet-hub", "/home/fleet/.ssh"]
EXPOSE 4180
ENTRYPOINT ["tini", "--", "fleet-hub"]
CMD ["serve"]
```

The `COPY src/lib/...` line exists because core tests `include_str!` those frontend files; `cargo build` (not test) does not compile `#[cfg(test)]` code, so if the build succeeds without them, drop that line. Verify with `docker build -f crates/fleet-hub/Dockerfile -t fleet-hub:dev .` on a host with Docker (hetzner; this repo's trn host has none).

`.dockerignore` at the repo root: `target`, `node_modules`, `dist`, `.worktrees`, `.git`.

- [ ] **Step 2: Compose, Caddy, env, systemd**

`deploy/hub/docker-compose.yml`:

```yaml
services:
  fleet-hub:
    image: ghcr.io/martin-janci/fleet-hub:latest
    restart: unless-stopped
    env_file: fleet-hub.env
    environment:
      FLEET_HUB_BIND: 0.0.0.0
      # Plaintext only on the compose network; Caddy terminates TLS.
      FLEET_HUB_ALLOW_PLAINTEXT: "1"
    volumes:
      - hub-data:/var/lib/fleet-hub
      - ./ssh:/home/fleet/.ssh
  caddy:
    image: caddy:2
    restart: unless-stopped
    ports: ["80:80", "443:443"]
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy-data:/data
volumes:
  hub-data:
  caddy-data:
```

`deploy/hub/Caddyfile`:

```
{$FLEET_HUB_DOMAIN} {
    reverse_proxy fleet-hub:4180 {
        # The hub's rebinding guard accepts loopback and its own public
        # host; rewriting Host keeps the default allowlist sufficient.
        header_up Host 127.0.0.1:4180
        # Long polls (wait_for_session, run_prompt) stream keep-alives.
        flush_interval -1
    }
}
```

`deploy/hub/fleet-hub.env.example`:

```
# Copy to fleet-hub.env and edit. The same value drives both containers.
FLEET_HUB_DOMAIN=fleet.example.com
FLEET_HUB_PUBLIC_URL=https://fleet.example.com
```

(Compose reads `FLEET_HUB_DOMAIN` for the Caddyfile substitution: add `env_file: fleet-hub.env` to the `caddy` service as well.)

`deploy/hub/fleet-hub.service`:

```ini
[Unit]
Description=claude-fleet hub daemon
After=network-online.target
Wants=network-online.target

[Service]
User=fleet
EnvironmentFile=/etc/fleet-hub.env
ExecStart=/usr/local/bin/fleet-hub serve
Restart=on-failure
StateDirectory=fleet-hub
Environment=FLEET_HUB_DATA_DIR=/var/lib/fleet-hub

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 3: Image workflow**

`.github/workflows/hub-image.yml`:

```yaml
name: hub-image
on:
  push:
    tags: ['v*']
  workflow_dispatch:
permissions:
  contents: read
  packages: write
jobs:
  image:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - uses: docker/metadata-action@v5
        id: meta
        with:
          images: ghcr.io/${{ github.repository_owner }}/fleet-hub
          tags: |
            type=semver,pattern={{version}}
            type=raw,value=latest,enable=${{ startsWith(github.ref, 'refs/tags/v') }}
            type=sha
      - uses: docker/build-push-action@v6
        with:
          context: .
          file: crates/fleet-hub/Dockerfile
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

- [ ] **Step 4: `docs/hub.md`**

Write it with these sections, in this order, each with the exact commands:

1. **What it is** — one paragraph: the same fleet, headless, one hub per fleet; the desktop app is optional.
2. **Setup with Docker (recommended)** — `mkdir -p ~/fleet-hub/ssh && cd ~/fleet-hub`, download `deploy/hub/*`, `cp fleet-hub.env.example fleet-hub.env` (edit the domain), `docker compose run --rm fleet-hub init` (prints the master token), `docker compose run --rm fleet-hub ssh-key` (prints the key), add the key to `~/.ssh/authorized_keys` on every host, put an `~/.ssh/config` with one `Host <alias>` block per machine into `./ssh/config` (mode 0600, owned by uid 1000), `docker compose up -d`, `curl -s https://fleet.example.com/mcp -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fleet_health","arguments":{}}}'`.
3. **Add and provision hosts** — with any MCP client (Claude Code: `claude mcp add --transport http fleet https://fleet.example.com/mcp --header "Authorization: Bearer <token>"`): `discover_hosts` (reads the mounted ssh config), `add_host`, `provision_hosts`; what provisioning writes (the public `/hook` and `/mcp` URLs, no tunnels); restart Claude on each host.
4. **Bare binary** — `cargo build -p fleet-hub --release`, copy to `/usr/local/bin`, `/etc/fleet-hub.env`, the systemd unit; run behind your own TLS proxy or bind loopback and use Tailscale (then no public URL: the hub behaves like the desktop, with reverse tunnels).
5. **Configuration** — the flag/env/setting/default table from the spec, plus `--allow-plaintext` and the precedence rule.
6. **Migrating from the desktop** — quit the app, copy `state.db` from the desktop's data dir (macOS `~/Library/Application Support/sk.rlt.claude-fleet/state.db`; Linux `~/.local/share/claude-fleet/state.db`) into the hub data dir, `fleet-hub init`, start, `provision_hosts` again (URLs change). The copied `local` row is hidden automatically.
7. **Coexistence with the desktop** — a host provisioned by the hub reports its hooks to the hub only; a desktop app still lists that host's sessions through reconcile but loses hook-driven `idle`/`working`, task completion and safe-kill until it becomes a hub client (sub-project 5). Do not run `provision_hosts` from both.
8. **Security notes** — bearer tokens, TLS via Caddy, `state.db` 0600, `mcp.confirm_destructive` has no approver on a hub (leave it off), how to rotate (`fleet-hub token regenerate`, then reconfigure clients; `provision_hosts { rotate: true }` for host tokens).
9. **Troubleshooting** — `401` (token), `403` (Host allowlist: set `--allowed-host` when the proxy does not rewrite Host), `bind: address in use`, `refusing to serve plaintext`, hosts `skipped: unreachable` (ssh key / config), hooks not arriving (host cannot reach the public URL; `curl` it from the host).

- [ ] **Step 5: Existing docs**

`docs/control-api.md`: in *Enabling it*, add a paragraph "On a `fleet-hub` daemon the API is always on and reachable at the hub's public URL; see `hub.md`." In *Provisioning hosts* step 3 and 5, say the URL is `http://127.0.0.1:<port>` on the desktop and the public URL on a hub; step 6 (tunnel) "loopback hubs only". In *Security → Localhost only*, replace with: "The desktop binds `127.0.0.1` and this is not configurable there. A `fleet-hub` daemon binds the configured address and adds its public host to the Host/Origin allowlist; plaintext on a routable bind is refused unless explicitly allowed." `docs/concepts.md` *Control API & tunnels*: add two sentences about the hub daemon and that tunnels exist only for a loopback hub. `docs/README.md`: add `hub.md` to the index.

- [ ] **Step 6: Verify the docs test still passes and commit**

Run: `cargo test -p fleet-core reference_is_current mentions` (the narrative-guide test in `doc_gen.rs` checks `docs/control-api.md` mentions every tool; it must stay green).

```bash
git add crates/fleet-hub/Dockerfile .dockerignore deploy .github/workflows/hub-image.yml docs
git commit -m "feat(hub): Docker image, compose with Caddy, systemd unit, docs/hub.md"
```

---

### Task 10: Manual acceptance on hetzner and the PR

**Files:** none in the repo beyond the PR description (record the run there).

- [ ] **Step 1: Build and push a dev image**

On a machine with Docker (hetzner, `claude-fleet-htz`), from a checkout of this branch:

```bash
docker build -f crates/fleet-hub/Dockerfile -t fleet-hub:dev .
```

Edit `deploy/hub/docker-compose.yml` locally to `image: fleet-hub:dev` for this run.

- [ ] **Step 2: Bring the hub up**

```bash
mkdir -p ~/fleet-hub/ssh && cp deploy/hub/* ~/fleet-hub/ && cd ~/fleet-hub
cp fleet-hub.env.example fleet-hub.env   # set a real domain that resolves to this box
docker compose run --rm fleet-hub init   # note the token
docker compose run --rm fleet-hub ssh-key
# add the key to ~/.ssh/authorized_keys on claude-fleet-trn and claude-fleet-oci
# write ./ssh/config with Host blocks for both; chown 1000:1000, chmod 600
docker compose up -d && docker compose logs -f fleet-hub | head -20   # "fleet-hub serving"
```

- [ ] **Step 3: Exercise the API over HTTPS with the desktop app closed**

From a phone browser (any JSON-RPC client) or `curl` on a laptop, with `URL=https://<domain>/mcp` and `TOK=<token>`, call in order: `fleet_health` (expect `db_ready: true`, `hosts_total` 0), `add_host` for trn and oci, `provision_hosts` (expect `provisioned` for both, detail without "tunnel"), `list_sessions`, `send_prompt` to an idle trn session with `"reply with the word pong"`, `wait_for_session { until: "turn_gt", turn: <turn_seq_before> }` (expect `satisfied`), `session_transcript` (contains `pong`). Then: a wrong token → `401`; `-H 'Host: evil.example.com'` → `403` (send it straight to the hub port from inside the box: `docker compose exec caddy wget …` or `curl --resolve`).

- [ ] **Step 4: Hooks and restart**

`session_history` for that session shows a `Stop` after the prompt (hook arrived over HTTPS). `docker compose restart fleet-hub`; `fleet_health` again with the same token → same hosts and sessions.

- [ ] **Step 5: Record and ship**

Restore the compose image line, then push the branch and open the PR with `gh pr create` (title `feat: headless fleet-hub daemon`), pasting the commands and outputs of Steps 3–4 into the description under **Manual acceptance**. Use the `martin-janci` token per `gh account switch on trn` memory. Run `superpowers:requesting-code-review` before asking for merge.

---

## Self-review

**Spec coverage.** Workspace + crate split → Tasks 1–3. Runtime seam → Task 2. Event bus placement → Task 3 Step 3. Daemon CLI (`init`, `serve`, `token`, `ssh-key`), config precedence, persistence of `hub.*`, forced `mcp.enabled` → Task 7. Bind address + allowlist → Task 4. Base URL provisioning + tunnel skip → Task 5. No implicit local host + hiding a copied `local` row → Task 6 and Task 7 `persist`. Plaintext refusal → Task 7. Packaging, image workflow, `docs/hub.md`, coexistence and migration sections → Task 9. CI headless job, scripts, `CLAUDE.md`, `RELEASING.md` → Task 8 (Task 1 for the path moves). Manual acceptance → Task 10. Error handling from the spec: bind failure → `mcp::start` `Err` propagates to exit 1 (Task 7 `main`); URL validation → `HubBase::public` (Task 5) surfaced with exit 1, as the spec's error-handling section says. Store open failure → actionable message (Task 7). SIGTERM → `wait_for_signal` (Task 7).

**Placeholders.** None: every step names files and shows code or exact commands. Two steps say "match the real signature" (`TunnelSpawner` closure shape in Task 5, `AppHandleEventBus::emit` body in Task 3) because the plan must not invent code it did not read; both tell the implementer where the truth is.

**Type consistency.** `HubBase` fields `url/port/public` and methods `mcp_url/hook_url/host` are used identically in Tasks 5, 7. `mcp::start` signature (bind, port, token, allowed_hosts) matches Tasks 4 and 7. `ReconcileDeps::real(ssh, local_host)` in Task 6 only. `normalize_allowed_hosts` re-export in Task 7 Step 3 matches its definition in Task 4. `spawn_reconcile_tick(store, ssh)` / `spawn_account_usage_tick(store, ssh, cache, bus)` match `tick.rs` as read.
