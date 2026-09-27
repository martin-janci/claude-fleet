# Windows Desktop Client Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Land the phases in order; each phase is one PR.

**Goal:** Ship `claude-fleet` as a Windows desktop client for an existing Linux/macOS fleet: the local UI, the database, the MCP control API, hub-client mode, and SSH + remote tmux attach from a Windows machine. Windows is a *client* here, never a host.

**Out of scope (later, separate plans):** a Windows `fleet-agent`, Windows-native Claude sessions (no tmux backend), the `local` host on Windows, and `fleet-hub` on Windows.

**Architecture:** No architectural change. Everything Unix-only stays `#[cfg(unix)]`; Windows gets the smallest parallel branch that keeps behaviour correct. The `local` host is turned off on Windows through the switch a hub already uses (`hub.local_host=false`), so every local spawn (`tmux`, `bash -lc`, `claude`) is refused with `E_NOTFOUND` before it runs. The terminal stays `portable-pty`, which is ConPTY on Windows, running `ssh.exe -tt … tmux attach`.

**Baseline:** `main @ ae97d01b`. File:line references are from there.

## 0. Evidence: what actually fails today

Measured on Linux with `rustup target add x86_64-pc-windows-msvc` and `cargo check --target x86_64-pc-windows-msvc` (C build scripts stubbed out — `ring`/`rusqlite` need a real MSVC toolchain, so this checks Rust only, not linking):

| Crate / target | Result |
|---|---|
| `fleet-proto` lib | ✅ compiles |
| `fleet-hub` lib+bin | ✅ compiles (once fleet-core does) |
| `fleet-core` lib | 🔴 3 errors — `service/move_session/mod.rs:1042,1057` (`OpenOptionsExt::mode`), `service/provision.rs:747` (`PermissionsExt::from_mode`) |
| `claude-fleet` (src-tauri) lib | 🔴 3 errors — `pty.rs:203` (`libc::kill` / `pid_t` / `SIGKILL`) |
| `fleet-core` tests + `examples/carry_e2e.rs` | 🔴 67 errors across ~20 files: `std::os::unix` (29), `Permissions::from_mode` (17), `libc` (7), `ExitStatus::from_raw` (6), `Permissions::mode` (4) |
| `claude-fleet` tests | ✅ compiles |
| `fleet-agent` bin | 🔴 `main.rs:87` `install::lookup_user` (unix-only) — out of scope |

With the four one-line `#[cfg(unix)]` guards from Phase 1 applied, `cargo clippy -p fleet-core -p fleet-hub -p claude-fleet --target x86_64-pc-windows-msvc -- -D warnings` is clean.

The earlier audit's claim that fleet-core was already fully `cfg`-guarded was wrong for two library sites; it was right about `pty.rs`.

**Runtime blockers the compiler cannot see** (found by reading the code; each has a phase below):

1. **SSH multiplexing.** `ssh.rs` passes `ControlMaster=auto` / `ControlPath=…/cm-<host>.sock` / `ControlPersist` on every call (`mux_opts`, `ssh.rs:230`) and the PTY attach (`control_path_for_pty`, `ssh.rs:259`). Win32-OpenSSH has no ControlMaster support (no Unix-socket mux). Every call either fails or opens a fresh connection, and the mux-failure retry path (`ssh_diag::classify`) will misread it.
2. **`HOME`.** `ssh_config.rs:126` (`dirs_home`), `ssh.rs:1723` (`cache_dir`) and `service/catalog/mod.rs:59` (`expand_home`) read `HOME`, which Windows does not set. Result: `~/.ssh/config` is never found, so host discovery is empty.
3. **The `local` host.** `tmux.rs` spawns `tmux` / `claude` locally (`:347`, `:367`, `:382`, `:933`, …), `run_local_shell` spawns `bash -lc` (`ssh.rs:1096`). None exist on Windows.
4. **Token storage.** `backend/token_store.rs:115` falls back to a plain file; the `0600` at `:139` is `cfg(unix)`, so on Windows the hub client token sits in a file readable by anything running as the user, with default ACLs.
5. **Line endings.** `.gitattributes` only covers `src/lib/__fixtures__/*.raw.txt`. A Windows checkout with `core.autocrlf=true` turns generated files and fixtures into CRLF, so `reference_is_current`, `verdict_gen` and the shared recogniser fixture can fail on CI for reasons that have nothing to do with the code.

## Global Constraints

From `CLAUDE.md`:

- Shell quoting goes through `crate::shell::quote` only. Windows does not change that: every command string still runs under the *remote* POSIX shell.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command. The teardown in `pty.rs` runs on `PtyParts` taken out from under the lock; keep it there.
- A new `settings` key, IPC command or MCP tool needs the usual rows (verdicts, generated reference). This plan adds no command and no tool.

---

## Phase 1 — Windows compiles (P0)

**PR:** `fix(windows): cfg-guard the unix-only calls so the desktop compiles on Windows`

- [ ] `crates/fleet-core/src/service/move_session/mod.rs:1042` — `TempFile::create`: gate the `OpenOptionsExt` import and the `.mode(0o600)` call on `#[cfg(unix)]`. The file lives in `std::env::temp_dir()`, which on Windows is already per-user (`%LOCALAPPDATA%\Temp`) — note that in the doc comment rather than adding an ACL.
  ```rust
  let mut o = std::fs::OpenOptions::new();
  o.write(true).create_new(true);
  #[cfg(unix)]
  o.mode(0o600);
  let f = o.open(&path)?;
  ```
- [ ] `crates/fleet-core/src/service/provision.rs:745` — `place_private_file`: `#[cfg(unix)]` on the `set_permissions` statement.
- [ ] `src-tauri/src/pty.rs:193` — platform-split the teardown kill, keeping the Unix comment and behaviour byte for byte:
  ```rust
  if let Some(mut child) = self.child {
      kill_hard(&mut child);
      reap(child);
  }

  /// SIGKILL on Unix (see the comment above for why not SIGHUP).
  #[cfg(unix)]
  fn kill_hard(child: &mut Box<dyn portable_pty::Child + Send + Sync>) { /* today's match */ }

  /// On Windows portable-pty's `kill()` is `TerminateProcess`: already a hard
  /// kill, so the SIGHUP-relays-a-newline problem does not arise.
  #[cfg(windows)]
  fn kill_hard(child: &mut Box<dyn portable_pty::Child + Send + Sync>) { let _ = child.kill(); }
  ```
  Move `libc` in `src-tauri/Cargo.toml:45` under `[target.'cfg(unix)'.dependencies]`, like fleet-core does.
- [ ] Make fleet-core's **test** code compile on Windows. Two mechanical rules, no test deleted or skipped on Unix:
  - a test that exercises a Unix-only behaviour (permissions, symlinks, `ExitStatus::from_raw`, process groups, real `tmux`/`bash`) gets `#[cfg(unix)]` on the test (or on its `mod`);
  - a shared helper used by portable tests (`ssh_fake.rs:25` `ExitStatusExt`) gets a small `exit_status(code)` shim with a `#[cfg(windows)]` arm (`std::os::windows::process::ExitStatusExt::from_raw(code as u32)`).
  Files, from the measurement: `move_session/claude_state.rs`, `add_project.rs`, `catalog/repo.rs`, `examples/carry_e2e.rs` (whole example `#[cfg(unix)]` via `required-features` or a `fn main` stub), `ssh_fake.rs`, `tmux.rs`, `provision.rs`, `catalog/import.rs`, `agent/e2e.rs`, `ssh.rs`, `trackers/tests_github.rs`, `move_session/mod.rs`, `account_usage_poll.rs`, `net/via_host.rs`, `transcript.rs`, `catalog/author.rs`, `account_usage.rs`.
- [ ] `.gitattributes`: `* text=auto eol=lf` plus the existing fixture line, so a Windows checkout reproduces the generated files byte for byte.

**Verify:** `scripts/ci-local.sh --rust-only` on Linux/macOS unchanged; `cargo clippy --workspace --exclude fleet-agent --all-targets --target x86_64-pc-windows-msvc -- -D warnings` clean (from Phase 2 on, in CI).

## Phase 2 — Windows CI (P0)

**PR:** `ci: add a windows-latest leg for the desktop crates`

- [ ] `.github/workflows/ci.yml` `rust` job: add `windows-latest` to the matrix with `--workspace --exclude fleet-agent` on that leg only (`fleet-agent` is Unix-only by design; say so in a comment). `cargo fmt` and `cargo deny` stay Linux-only as today.
- [ ] Set `git config --global core.autocrlf false` before checkout on the Windows leg (belt and braces with `.gitattributes`).
- [ ] Tests that need `tmux`, `bash` or `ssh` on PATH already skip when the binary is missing (check each guard; any that assume `/bin/sh` get `#[cfg(unix)]` in Phase 1 instead).
- [ ] Frontend job: add `windows-latest` to the `pnpm test` / `pnpm check` matrix at line 182 — cheap, and it catches path-separator assumptions in tests.
- [ ] A `tauri build --debug --no-bundle` step on the Windows leg, so the WebView2 / `tauri-build` resource path is exercised on every PR, not first at release.

## Phase 3 — Windows runtime basics (P0)

**PR:** `fix(windows): home dir, no local host, no ssh mux`

- [ ] **Home directory.** Replace the three `HOME` reads in library code (`ssh_config.rs:126`, `ssh.rs:1723`, `catalog/mod.rs:59`) with one `crate::paths::home_dir()` that returns `std::env::home_dir()` (fixed for Windows in Rust 1.86 — `USERPROFILE`; the workspace sets no `rust-version` and CI runs stable). Tests keep setting `HOME`; on Windows they set `USERPROFILE` too through the same helper.
- [ ] **`cache_dir`** on Windows: `%LOCALAPPDATA%\claude-fleet` (the ControlPath directory is moot there, see next item, but logs and other callers still use it).
- [ ] **No SSH multiplexing on Windows.** One switch in `SshClient`: `fn mux_supported() -> bool { cfg!(unix) }`.
  - `mux_opts` (`ssh.rs:230`) returns only the non-mux flags (`ConnectTimeout`, `BatchMode`, `ServerAlive*`) when it is false;
  - `control_path_for_pty` / `attach_mux_opts` (`pty.rs:388`) give the attach no `ControlPath`;
  - the app-exit `-O exit` sweep (`ssh.rs:858`, `:893`) is a no-op;
  - `is_mux_failure` is never consulted, so a plain exit 255 is not retried as a dead master.
  Unit tests pin each of the four under both values of the switch (inject it, don't `cfg` the test).
- [ ] **Cost of no mux, measured and bounded.** Every reconcile / list / pane capture now pays a full SSH handshake. Before this PR merges: time one reconcile pass over 3 hosts from a Windows box, with and without the hub. If the tick is too slow, raise the reconcile staleness on Windows (it is a setting) and say in `docs/windows.md` that **hub-client mode is the recommended Windows setup** — the hub does the SSH from Linux with mux, the desktop only opens the one attach connection per terminal.
- [ ] **No `local` host.** At desktop startup on Windows, call the same switch a hub with `hub.local_host=false` uses (`service::hub` `LOCAL_HOST_DISABLED`), so `ensure_local_allowed` refuses every local spawn with `E_NOTFOUND` and the existing self-heal ghosts stale `local` rows. The UI already copes with a hub that has no local host; confirm Hosts / New session / Add project hide `local`.
- [ ] **`ssh.exe`.** Resolve `ssh` from PATH as today; if it is not found, the Hosts view says "Install the OpenSSH Client (Settings → Optional features)" instead of a raw spawn error. `E_SHELL` with that message, from one place in `ssh.rs`.

## Phase 4 — Terminal on ConPTY (P1)

Manual test matrix, run on Windows 11 against a Linux host, standalone *and* hub-client mode. File what fails as issues; fix in this phase only what blocks daily use.

- [ ] Attach, type, paste (small and > 4 KiB — the writer's bounded channel and `E_PTY_BUSY`), resize (window and split), detach, reattach, switch sessions quickly 20×.
- [ ] **The stray newline on switch** (the reason for the Unix SIGKILL). Verify `TerminateProcess` does not let `ssh.exe` relay a trailing `\n` into the pane. If it does, the fix is to drop the master before the kill on Windows — not to emulate a signal.
- [ ] `reap` (`pty.rs:230`): check that the capped inline wait is enough for a terminated `ssh.exe`; the timeouts may need a Windows value.
- [ ] Colours and the alternate screen through ConPTY (`TERM`/`COLORTERM` are forced in `attach_env`, `pty.rs:408`); ConPTY re-renders — watch for duplicated lines on resize in `ansi.ts`.
- [ ] The ssh escape char (`-e none`) and `ConnectTimeout` from `attach_argv` are honoured by Win32-OpenSSH.
- [ ] Keyboard: every `⌘` chord has its `Ctrl` form (`detectMac` in `App.svelte:426` already switches), and none collides with a terminal key the pane needs (`Ctrl+C`, `Ctrl+D`, `Ctrl+Shift+J`).

## Phase 5 — Secure token store (P1, before public distribution)

**PR:** `feat(windows): keep the hub client token in Credential Manager`

- [ ] `backend/token_store.rs`: a `#[cfg(windows)]` `impl TokenStore for OsTokenStore` over Windows Credential Manager (`CredWriteW` / `CredReadW` / `CredDeleteW`, `CRED_TYPE_GENERIC`, persist `LOCAL_MACHINE`), target name from the existing `SERVICE`/`ACCOUNT` pair. Same shape as the macOS impl: no child process, token never on an argv.
- [ ] Dependency: `windows-sys` with only `Win32_Security_Credentials` + `Win32_Foundation`, under `[target.'cfg(windows)'.dependencies]` — the same reasoning that put `security-framework` under macOS only and rejected `keyring` (`src-tauri/Cargo.toml:58`). `cargo deny check` must stay clean.
- [ ] `ERROR_NOT_FOUND` → `Ok(None)` / idempotent clear, mirroring `errSecItemNotFound`.
- [ ] Migration from the file fallback: on first `get`, if the file exists and the credential does not, move it in and delete the file.
- [ ] Tests: the existing file-store tests stay for Linux (`cfg(all(test, not(any(target_os = "macos", windows))))`); a Windows-only round-trip test under a random target name, cleaned up.

## Phase 6 — Package and release (P1)

- [ ] `scripts/release-assets.sh`: a `desktop-x86_64-windows` leg on `windows-latest`, `--bundles nsis` (MSI optional), asset names `claude-fleet_{v}_x64-setup.exe` (+ `.nsis.zip` / `.sig` for the updater); teach `rename-updater-asset.sh` the name.
- [ ] `tauri.conf.json`: `bundle.windows` — WebView2 `downloadBootstrapper`, an icon `.ico`, `nsis.installMode: currentUser` (no admin needed).
- [ ] Updater: the `latest.json` gains a `windows-x86_64` platform entry; the release-drift check covers it.
- [ ] **Decision for the owner:** Authenticode code signing (certificate cost, SmartScreen warnings without it). The first release can ship unsigned and say so in the release notes.
- [ ] `docs/windows.md`: install, OpenSSH Client prerequisite, "hub-client mode recommended", what does not exist on Windows (local host, agent, hub). Link it from README and `docs/hub.md`.

## Phase 7 — Later (separate plans, not started here)

- Windows `fleet-agent` (`is_root` via `geteuid`, `/proc`, `getpwnam_r`, systemd install, process groups — `crates/fleet-agent/src/main.rs:87` and around).
- A Windows `local` host: Claude Code sessions without tmux (ConPTY-hosted, or WSL-backed `tmux` through `wsl.exe`). WSL is the cheaper path and would keep the tmux model intact — worth a spike before designing anything native.
- A Rust-native SSH pool (`russh`) to get multiplexing back on Windows, only if the Phase 3 measurement says standalone mode is too slow and hub-client mode is not an answer for the user.

## Order and size

| Phase | Priority | Size | Blocks |
|---|---|---|---|
| 1 compile | P0 | S (lib 4 lines; tests mechanical, ~20 files) | 2 |
| 2 CI | P0 | S | 3–6 |
| 3 runtime basics | P0 | M | 4 |
| 4 ConPTY matrix | P1 | M (mostly manual) | 6 |
| 5 token store | P1 | S–M | 6 |
| 6 package | P1 | M | — |
| 7 later | P2 | L | — |
