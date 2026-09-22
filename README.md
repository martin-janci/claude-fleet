# claude-fleet

[![CI](https://github.com/martin-janci/claude-fleet/actions/workflows/ci.yml/badge.svg)](https://github.com/martin-janci/claude-fleet/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/martin-janci/claude-fleet?display_name=tag&sort=semver)](https://github.com/martin-janci/claude-fleet/releases)

A native cross-platform desktop app for managing long-lived [Claude Code](https://claude.com/claude-code) sessions running in tmux across multiple machines. Built with Rust + Tauri 2 + Svelte 5.

## Quickstart

```bash
pnpm install
pnpm tauri dev
```

On first launch the app walks you through setup — see the **[Getting Started guide](docs/getting-started.md)**.

## Installing a release build

Grab the bundle for your platform from the
[Releases page](https://github.com/martin-janci/claude-fleet/releases) —
`.dmg` for macOS (`aarch64` for Apple Silicon, `x86_64` for Intel),
`.AppImage` or `.deb` for Linux. Every filename carries the version, so a
download is always traceable to the release it came from.

### Verify what you downloaded

Each release attaches a `SHA256SUMS` asset covering **every** asset on that
release — desktop bundles included. If that file is incomplete, or any asset
the manifest declares is missing, the release run fails (`verify-release` in
`.github/workflows/release.yml`); the draft is still published by hand, so
treat a red release run as a reason not to trust the draft. Nothing here is
code-signed (see below), so this checksum is the only integrity check
available; it is worth the ten seconds.

From the directory you downloaded into, with `v0.3.0` replaced by the release
you took:

```bash
curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v0.3.0/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing     # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
```

`--ignore-missing` is what lets you check just the one bundle you took
instead of all of them; every file you *do* have must print `OK`. The release
notes also record the exact commit the bundles were built from.

### macOS — "claude-fleet.app is damaged and can't be opened"

Expect this dialog on first launch. **The app is not damaged.** Release
bundles are neither code-signed nor notarized — there is no Apple Developer ID
for this project, so `release.yml` deliberately omits the signing step (ad-hoc
signing would only fake provenance; see the
[signing caveat](docs/RELEASING.md#signing-caveat)). Anything you download
carries the `com.apple.quarantine` flag, Gatekeeper finds no signature to
check, and macOS reports that as "damaged".

To install:

1. Open the `.dmg` and drag **claude-fleet.app** into `/Applications`.
2. Clear the quarantine flag:

   ```bash
   xattr -dr com.apple.quarantine /Applications/claude-fleet.app
   ```

3. Open the app normally.

Notes:

- Do **not** click *Move to Bin* in the dialog — just Cancel, then run the
  command above.
- Right-click → **Open**, and the **Open Anyway** button under System Settings
  → Privacy & Security, are the workarounds for a *signed but un-notarized*
  app. They are unreliable here, because the binary carries no Developer ID
  signature at all. Use `xattr`.
- Don't reach for `sudo spctl --master-disable` ("Allow apps from: Anywhere").
  That disables Gatekeeper for every app on the machine; the `xattr` command
  above affects only this one.

Building from source (see [Development](#development)) sidesteps all of this —
a locally built `.app` is never quarantined.

### Linux

The `.AppImage` and `.deb` are unsigned, which is normal for those formats.
Mark the AppImage executable before running it:

```bash
chmod +x claude-fleet_*.AppImage
```

## Features

- **Multi-host** — attach to tmux sessions on any host in `~/.ssh/config`, plus
  `local`. SSH connections are multiplexed via per-host ControlMaster.
- **Project tree** — finds repos (and git worktrees) under a per-host projects
  base set in Settings → Projects, laid out as `<base>/<owner>/<repo>` (default
  `~/projects/github.com`) or flat `<base>/<repo>`; sessions are grouped under
  their project.
- **Account model** — each host's logged-in Claude account (email / org / tier)
  is auto-detected by probing the remote `~/.claude.json`. No credentials are
  ever read or stored.
- **Terminal pane** — a custom ANSI screen-buffer renderer shows the attached
  session live.
- **Prompt transfer** — send a prompt to one or many sessions at once.
- **Files, history & branches** — per-session worktree browser: changed files
  with inline diffs, a full file tree, an interactive commit graph, and a
  branch list. Git actions (stage & commit, checkout, create/delete branch,
  fetch/pull/push) run directly in the session's worktree.
- **Event-driven UI** — backend mutations emit row events; the frontend patches
  its stores in place rather than re-fetching.

## Documentation

- [Getting Started](docs/getting-started.md)
- [Concepts](docs/concepts.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Control API](docs/control-api.md)
- [Docs index](docs/README.md)

## Development

### Requirements

- macOS 13+ (primary) or Linux — CI runs on both (`macos-latest`,
  `ubuntu-24.04`) and tagged releases ship unsigned macOS `.dmg` (arm64 and
  x86_64) plus Linux `.AppImage`/`.deb` bundles (see `docs/RELEASING.md`)
- Rust 1.83+ (`rustup install stable`)
- Node 20 (`.node-version`) and pnpm 10 via `corepack enable` (or
  `npm i -g pnpm@10`). The workspace file uses the pnpm 10 `allowBuilds` key;
  if a local pnpm 9 prints `packages field missing or empty`, run
  `corepack pnpm@10 <cmd>` or `npx -y pnpm@10 <cmd>` instead.
- `cargo install cargo-deny --locked` for the local license/advisory audit
  step (`cargo deny check`, run by `scripts/ci-local.sh` and CI).
- Tauri 2 prerequisites: https://v2.tauri.app/start/prerequisites/

### Build & run

```bash
pnpm install
pnpm tauri dev      # dev mode (hot-reload frontend, debug Rust)
pnpm tauri build    # release bundle in src-tauri/target/release/bundle/
```

### Test

```bash
pnpm test                      # frontend (Vitest)
pnpm check                     # frontend Svelte/TS type-check
cargo test --workspace         # backend (all crates)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check                # licenses + advisories
```

Run `scripts/ci-local.sh` (or `--rust-only` / `--frontend-only`) before
pushing; it mirrors CI. Opt in to the fast pre-commit hook with
`git config core.hooksPath .githooks`.

### Project layout

```
src/lib/                       # Svelte 5 components + TS stores (hosts, sessions, projects, accounts, events)
crates/fleet-core/src/         # Rust backend: service/store/SSH/MCP, Tauri-free
src-tauri/src/commands/        # thin Tauri IPC handlers wrapping crates/fleet-core/src/service/
crates/fleet-core/migrations/  # SQLite migrations (registered in the MIGRATIONS table in crates/fleet-core/src/store/schema.rs)
docs/specs/         # per-iteration design specs
docs/plans/         # per-iteration implementation plans
CLAUDE.md           # orientation for Claude Code working in this repo
```

`fleet-hub` runs the same fleet headless as a daemon (no desktop app needed),
and `fleet-agent` reaches a host the hub cannot dial over SSH by connecting
outbound instead — see [docs/hub.md](docs/hub.md) for both.

## Known gaps

A hardening review (2026-05-21, see
[docs/specs/2026-05-21-hardening-review.md](docs/specs/2026-05-21-hardening-review.md))
catalogues open issues. Highest priority: SSH host-alias validation, migration
atomicity, and the single-global-PTY races in `TerminalView`. Handoff
(original spec §8.3) is replaced by Move to host… / `move_session`, and Freeze
(§8.4) is descoped; see
[ADR 0001](docs/adr/0001-descope-freeze-ship-move.md).

## Releasing & documentation

- Versioning and changelog are cut manually with `scripts/release.sh` — see
  [docs/RELEASING.md](docs/RELEASING.md).
- **Control API reference:** [docs/control-api-reference.md](docs/control-api-reference.md)
  (generated from source) and [docs/control-api.md](docs/control-api.md) (guide).
- **Rust API docs (rustdoc):** published to GitHub Pages on each release —
  https://martin-janci.github.io/claude-fleet/

## License

Personal project. `package.json` and `src-tauri/Cargo.toml` declare MIT; a
`LICENSE` file has not been added to the repository yet.
