# Build, test, package, sign, release & distribute — the current baseline

Audit date: **2026-10-01**. Tree: `mellow-virgo` worktree of `claude-fleet`,
version **0.4.2** (`package.json`). Read-only audit — nothing was changed.

Purpose: an accurate baseline for a later evaluation of **Buildkite** as build
infrastructure. No migration is proposed here. §10 is the only forward-looking
section and it only *classifies* what exists as portable vs. provider-bound.

---

## 1. Inventory — every file involved in building or packaging

Searched the whole tree (excluding `node_modules/`, `target/`, `.worktrees/`).

### Rust / Cargo

| path | role |
|---|---|
| `Cargo.toml` | workspace root only — `resolver = "2"`, `members = ["crates/*", "src-tauri"]`. No `[workspace.dependencies]`, no `[profile.*]` overrides anywhere. |
| `Cargo.lock` | committed; `--locked` is used by the hub/agent/manifest builds and the Dockerfile. |
| `rust-toolchain.toml` | `channel = "stable"` (**floating, not pinned**), `components = ["rustfmt", "clippy"]`. |
| `.cargo/config.toml` | **does not exist** — no linker, target, or registry config in-repo. |
| `crates/fleet-core/Cargo.toml` | lib, `version 0.1.0`, `publish = false`. Features: `e2e`, `nl-detect`. |
| `crates/fleet-hub/Cargo.toml` | `[[bin]] fleet-hub`, version tracks the app. Feature `e2e`. Depends on `fleet-core` **with `nl-detect` on** (+~45 MB of lingua models). |
| `crates/fleet-agent/Cargo.toml` | `[[bin]] fleet-agent` + lib. Must never depend on `fleet-core` (enforced by a CI job). |
| `crates/fleet-proto/Cargo.toml` | wire types only; 3 deps. |
| `crates/fleet-update/Cargo.toml` | lib + **auto-discovered** `src/bin/fleet-release.rs`. `version 0.1.0`, `publish = false`. Feature `testkit` (dev-only). |
| `src-tauri/Cargo.toml` | the desktop app, `name = "claude-fleet"`, lib `claude_fleet_lib` with `crate-type = ["staticlib","cdylib","rlib"]`. Feature `devtools`. Per-OS dep blocks: `libc` (unix), `security-framework` (macOS), `windows-sys` (windows). |
| `src-tauri/build.rs` | `tauri_build::build()` only. |
| `crates/fleet-hub/build.rs` | stamps `FLEET_GIT_SHA` / `FLEET_BUILD_ID` into the binary; falls back to `git rev-parse HEAD`, else `unknown`/`local`. |
| `deny.toml` | cargo-deny: 6 RUSTSEC ignores (all tauri-transitive), license allowlist, `unknown-registry = "deny"`. |

### Frontend

| path | role |
|---|---|
| `package.json` | `packageManager: pnpm@10.34.5`, `engines.node ^20.19 || ^22.12 || >=24`. Scripts: `dev`, `build`, `preview`, `check`, `tauri`, `test`, `test:watch`. **No build/release scripts beyond these.** |
| `pnpm-lock.yaml`, `pnpm-workspace.yaml` | lockfile; workspace file contains only `allowBuilds: { esbuild: true }` (a pnpm-10-only key — pnpm 9 errors on this repo). |
| `.node-version` | `20`. |
| `vite.config.ts` | svelte plugin; dev server `:1420`, `strictPort`. |
| `vitest.config.ts` | jsdom, `vitest.setup.ts`, excludes `.worktrees/`, `.claude/worktrees/`, `target/`. |
| `svelte.config.js`, `tsconfig.json`, `index.html` | standard. |
| `.gitattributes` | `* text=auto eol=lf` — load-bearing: generated files are byte-compared. |

### Tauri

| path | role |
|---|---|
| `src-tauri/tauri.conf.json` | productName `claude-fleet`, identifier `sk.rlt.claude-fleet`, version `0.4.2`; `beforeDevCommand: pnpm dev`, `beforeBuildCommand: pnpm build`, `frontendDist: ../dist`; `bundle.targets: "all"`; `macOS.hardenedRuntime: false`; Windows `webviewInstallMode: downloadBootstrapper`, `nsis.installMode: currentUser`. |
| `src-tauri/tauri.conpty.conf.json` | overlay config adding three bundle resources (ConPTY). Applied with `--config`. |
| `src-tauri/capabilities/default.json` | permissions for the main window. |
| `src-tauri/licenses/ConPTY-LICENSE.txt` | shipped with the Windows bundle. |
| `src-tauri/vendor/` | **gitignored**, filled at build time by `scripts/fetch-conpty.sh`. |

### CI / CD

`.github/workflows/`: `ci.yml`, `release.yml` (815 lines), `hub-image.yml`,
`docs.yml`, `release-drift.yml`, `update-channels.yml`.
`.github/actions/fleet-release/action.yml` — one composite action.
`.github/release-drift-ignore` — allowlist of tags with no release.

**No GitLab CI, no Jenkinsfile, no Azure Pipelines, no CircleCI, no Travis, no
Makefile, no justfile, no PowerShell scripts** anywhere in the tree.

### Containers

| path | role |
|---|---|
| `crates/fleet-hub/Dockerfile` | 2-stage; `rust:1-bookworm` → `debian:bookworm-slim`. Builds only `fleet-hub`. |
| `.dockerignore` | `target node_modules dist .worktrees .git` |
| `deploy/hub/docker-compose.yml` | the shipped deployment; **pins `ghcr.io/martin-janci/fleet-hub:0.4.2`** (a version carrier of sorts — see §2). Plus `caddy:2`. |
| `deploy/hub/behind-proxy/docker-compose.yml` + `.env.example` | variant whose tag lives in `.env` as `FLEET_HUB_TAG`. |
| `deploy/hub/Caddyfile`, `fleet-hub.service`, `fleet-hub.env.example` | deployment, not build. |
| `deploy/hub/backup.sh`, `deploy/hub/upgrade.sh` | operator deployment scripts (tested in CI against a fake `docker`). |

### Scripts (`scripts/`, 29 files — all bash, all `#!/usr/bin/env bash`)

Release-critical:
`release-assets.sh` (the asset manifest), `release.sh` (version bump + tag),
`check-version-consistency.sh`, `changelog-section.sh`, `check-ci-green.sh`,
`package-linux-release.sh`, `upload-release-asset.sh`,
`download-release-assets.sh`, `merge-sha256sums.sh`, `rename-updater-asset.sh`,
`verify-release.sh`, `check-release-drift.sh`, `release-manifest.sh`,
`update-channels.sh`, `release-key.sh`, `release-pubkeys.sh`,
`release-verify-sig.sh`, `merge-hub-digests.sh`, `release-mobile.sh`,
`fetch-conpty.sh`.

Test/dev: `ci-local.sh`, `hub-e2e.sh` (89 KB), `hub-deploy-scripts-test.sh`,
`release-update-scripts-test.sh`, `ag-test.sh`, `e2e-fake-claude.sh`,
`e2e-fake-jira.py`, `capture-tmux-fixture.sh`, `measure-session-start.sh`.

Other tooling: `tools/ag/` — an agent-launcher CLI (bash, `install.sh`,
drivers, lib). Shipped as a dev tool; **has its own CI job** but is not part of
the Rust build.

### Documentation about builds/releases

`docs/RELEASING.md` (717 lines — the authoritative narrative), `README.md`
§Development, `CLAUDE.md`, `docs/hub.md`, `docs/windows.md`, `docs/updates.md`,
`docs/superpowers/specs/2026-09-28-update-channel-design.md`,
`docs/superpowers/plans/2026-09-27-windows-desktop.md`.

---

## 2. The real Rust build commands

Every actual invocation found. Nothing uses `cross`, `cargo-zigbuild`, `cargo-make`,
`cargo-dist`, `cargo-bundle`, `sccache`, or `cargo-nextest`.

| command | where | wd | env / features / target | output |
|---|---|---|---|---|
| `cargo fmt --all --check` | ci.yml `rust` (Linux leg only via the shared job), `ci-local.sh`, pre-commit hook | repo root | — | none |
| `cargo clippy --workspace --all-targets -- -D warnings` | ci.yml `rust`, `ci-local.sh`, pre-commit | root | — | none |
| `cargo clippy --workspace --exclude fleet-agent --exclude fleet-hub --all-targets -- -D warnings` | ci.yml `rust-windows` | root | MSVC host | none |
| `cargo test --workspace` | ci.yml `rust`, `ci-local.sh`, README | root | — | test bins |
| `cargo test --workspace --exclude fleet-agent --exclude fleet-hub --no-fail-fast` | ci.yml `rust-windows` | root | — | — |
| `cargo deny check` | ci.yml `rust` (Linux only), `ci-local.sh` | root | reads `./deny.toml` | — |
| `cargo build -p fleet-hub --locked` | ci.yml `hub-headless`, `ci-local.sh` | root | **no Tauri libs present** — this is the regression guard | `target/debug/fleet-hub` |
| `cargo build -p fleet-agent --locked` | same | root | must build without `fleet-core` | `target/debug/fleet-agent` |
| `cargo test -p fleet-core -p fleet-hub --locked` | ci.yml `hub-headless` | root | — | — |
| `cargo test -p fleet-proto -p fleet-agent -p fleet-update --locked` | ci.yml `hub-headless` | root | — | — |
| `cargo build -p fleet-hub --features e2e --locked` | ci.yml `hub-headless` | root | `CARGO_TARGET_DIR=target/e2e` | `target/e2e/debug/fleet-hub` |
| `cargo build -p fleet-update --bin fleet-release --locked` | ci.yml `hub-headless` (debug), composite action (`--release`) | root | — | `fleet-release` |
| `cargo build -p fleet-agent -p fleet-hub --release --locked --target <triple>` | release.yml `agent-hub-binaries` | root | `FLEET_GIT_SHA`, `FLEET_BUILD_ID=rel-<tag>-<sha>` | `target/<triple>/release/{fleet-agent,fleet-hub}` |
| `cargo build -p fleet-hub --release --locked` | `crates/fleet-hub/Dockerfile` | `/src` | ARGs `FLEET_GIT_SHA`, `FLEET_BUILD_ID`; BuildKit cache mounts on `/usr/local/cargo/registry` and `/src/target` | `/fleet-hub` |
| `cargo doc --no-deps --workspace` | docs.yml (runs on `macos-latest`) | root | — | `target/doc` |
| `pnpm tauri build --debug --bundles nsis --config src-tauri/tauri.conpty.conf.json` | ci.yml `rust-windows` | root | — | `target/debug/bundle/nsis/*-setup.exe` |
| `cargo tauri build` (via `tauri-apps/tauri-action@v0`, `args:` from the manifest) | release.yml `build` | root | per-leg, see §3 | `target[/<triple>]/release/bundle/**` |
| `pnpm tauri dev` / `pnpm tauri build` | developer, README | root | — | `target/release/bundle/` |

Regeneration commands (fail CI if their output is stale) — all are `cargo test`
with an env gate:

```
REGEN_DOCS=1            cargo test -p fleet-core reference_is_current
REGEN_SETTINGS_DOCS=1   cargo test -p fleet-core settings_docs_are_current
REGEN_PAGE_DOCS=1       cargo test -p fleet-core page_docs_are_current
REGEN_HUB_VERDICTS=1    cargo test -p claude-fleet --lib verdict_gen
```
plus `REGEN_ENV`, `REGEN_HUB_CONTRACT`, `REGEN_LOCAL_ONLY`, `REGEN_TRACKER_GOLDENS`.
Note `verdict_gen` lives in `-p claude-fleet`, so **the generated-file check for
`src/lib/hub_verdicts.generated.json` requires the full Tauri system-library
stack**; it cannot be verified on a headless agent.

### Duplicated build logic

1. **Tauri Linux prerequisite apt list** is written twice verbatim —
   `ci.yml` (`rust` job) and `release.yml` (`build` job). Both carry a
   "keep in sync" comment. This is the only genuinely duplicated *list*.
2. **The headless build set** is expressed three times with slightly different
   membership: `ci.yml` `hub-headless` (`fleet-hub`, `fleet-agent`,
   `fleet-core`, `fleet-proto`, `fleet-update`), `ci-local.sh`'s headless
   fallback (same five), and `.githooks/pre-commit`'s fallback
   (`fleet-core` + `fleet-hub` only).
3. **ConPTY fetch + verify** appears twice (ci.yml `rust-windows`,
   release.yml `build`), with near-identical but separately-written
   verification steps (one uses `::error::`+exit, the other also deletes the
   uploaded installer).
4. **`rel-<tag>-<sha>` build-ID expression** is written in three places:
   `release.yml` `agent-hub-binaries`, `release.yml` `manifest`,
   `hub-image.yml` `build`. Deliberate (an updater compares them) and held to
   one form by `scripts/release-update-scripts-test.sh`.
5. **`ci-local.sh` is a hand-maintained mirror of `ci.yml`** — the single
   largest duplication in the repo by intent, and it is explicitly documented
   as such. It omits: the macOS `ag` leg, `shellcheck`, the `rust-windows` job
   entirely, and the hub-e2e log collection.

Everything else that *could* have been duplicated is deliberately
single-sourced: `scripts/release-assets.sh` is the one table for build legs,
runner labels, targets, build args and asset filenames; `scripts/release.sh
--list` is the one list of version carriers.

---

## 3. Platform matrix

The authoritative table is `scripts/release-assets.sh` → `read_legs()`:

```
leg                            | kind      | runner           | target                      | args
desktop-aarch64-apple-darwin   | desktop   | macos-latest     | aarch64-apple-darwin        | --target aarch64-apple-darwin
desktop-x86_64-apple-darwin    | desktop   | macos-latest     | x86_64-apple-darwin         | --target x86_64-apple-darwin
desktop-x86_64-linux           | desktop   | ubuntu-24.04     | (native)                    | --bundles appimage,deb
desktop-x86_64-windows         | desktop   | windows-latest   | (native)                    | --bundles nsis --config src-tauri/tauri.conpty.conf.json
bins-x86_64-unknown-linux-gnu  | bins      | ubuntu-22.04     | x86_64-unknown-linux-gnu    |
bins-aarch64-unknown-linux-gnu | bins      | ubuntu-22.04-arm | aarch64-unknown-linux-gnu   |
checksums                      | checksums | ubuntu-24.04     |                             |
manifest                       | manifest  | ubuntu-24.04     |                             |
```

`since` column: the Windows leg ships from **0.3.4**, the manifest leg from
**0.4.1**. Older versions legitimately have fewer assets.

### Linux

- **Desktop**: x86_64 only, **native** on `ubuntu-24.04`. No arm64 desktop
  bundle. Formats: `.deb` + `.AppImage` (`--bundles appimage,deb`).
  Unsigned — normal for both formats. AppImage needs `chmod +x`.
- **Daemon binaries**: `fleet-hub` + `fleet-agent` for **x86_64** and
  **aarch64**, each on its own native GitHub-hosted runner
  (`ubuntu-22.04` / `ubuntu-22.04-arm`). **No QEMU, no `cross`.**
  - `-22.04` is deliberate: glibc **2.35** floor, so the binaries also run on
    Debian 12 / Ubuntu 22.04. A 24.04 build would require glibc ≥ 2.39.
  - **gnu, not musl** — explicitly: musl has never been verified to build.
- **Required system libraries (desktop only)**, installed identically in
  `ci.yml` and `release.yml`:
  `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev
  libayatana-appindicator3-dev librsvg2-dev libgtk-3-dev libsoup-3.0-dev
  libjavascriptcoregtk-4.1-dev pkg-config`.
  The `bins` legs and the Dockerfile need **none** of these — that is the
  point of the `hub-headless` CI job.
- **Docker**: only for `fleet-hub`. `linux/amd64` + `linux/arm64`, one native
  runner per arch (`ubuntu-24.04` / `ubuntu-24.04-arm`), **pushed by digest**,
  then combined with `docker buildx imagetools create`.
  Image: `ghcr.io/<owner>/fleet-hub`. Tags: `{{version}}`, `sha-<commit>`, and
  `latest` **only** for a tag with no `-` in it.
  Cache: `type=gha` scoped per platform.
- **Artifact names/locations**: `dist/<bin>-<version>-<triple>.tar.gz` +
  `dist/SHA256SUMS.<triple>` (written by `scripts/package-linux-release.sh`);
  desktop bundles under `target/release/bundle/{deb,appimage}/`.

### Windows

- **Target**: `x86_64-pc-windows-msvc` — implicit (the runner's host triple;
  **no `--target` is passed**). **MSVC, not GNU.** No ARM64 Windows build.
- Runner `windows-latest`. Built as a **client only** — `fleet-agent` and
  `fleet-hub` are excluded from the Windows clippy/test jobs by design
  (Unix-only).
- **Installer**: **NSIS only** (`--bundles nsis`), `installMode: currentUser`
  → per-user, no admin rights. No MSI.
- **WebView2**: `webviewInstallMode: downloadBootstrapper` — the installer
  downloads the runtime if missing.
- **ConPTY is bundled**: `scripts/fetch-conpty.sh` pulls the
  `Microsoft.Windows.Console.ConPTY` NuGet package, **version-pinned
  (`1.24.260710001`) and SHA-256-pinned**, into the gitignored
  `src-tauri/vendor/conpty/`; `tauri.conpty.conf.json` adds `conpty.dll`,
  `OpenConsole.exe` and the license as bundle resources. Both CI and release
  then **verify** the three files sit next to the exe *and* appear inside the
  NSIS payload (read with `7z l`). In release.yml a failure here **deletes the
  already-uploaded installer** so the release cannot be published without it.
- **Signing: none.** There is no Authenticode certificate; SmartScreen warns
  on first run (documented in README and `release-assets.sh`).
- Secrets: none Windows-specific.
- Artifact: `claude-fleet_<v>_x64-setup.exe`, from
  `target/release/bundle/nsis/`.
- `git config --global core.autocrlf false` is run **before checkout** on every
  Windows job.

### macOS

- **Both architectures**, as two separate single-arch legs on `macos-latest`
  (Apple Silicon): `aarch64-apple-darwin` native, `x86_64-apple-darwin`
  cross-compiled with `rustup target add`. **No universal binary.**
- **Minimum macOS version**: not configured anywhere (`tauri.conf.json` sets
  no `minimumSystemVersion`); README states "macOS 13+ (primary)" as a
  development requirement only.
- **Xcode**: only the runner's preinstalled toolchain; no `xcode-select`
  pinning, no `DEVELOPER_DIR`.
- **Signing: yes. Notarization: no.**
  - Secrets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
    `APPLE_SIGNING_IDENTITY`, read by tauri's own bundler.
  - The certificate is a **free Personal Team "Apple Development"**
    certificate. The reason is explicitly *not* provenance: the desktop stores
    its hub token in the login keychain, and macOS keys a keychain partition to
    the signer's Team ID — without one it falls back to the build's cdhash and
    every update re-prompts.
  - A macOS leg **fails closed** if any of the three secrets is empty.
  - Apple's **WWDR G3 intermediate** is downloaded and SHA-256-verified at
    build time (`dcf21878c77f…`) and imported into the System keychain —
    without it `codesign` fails `errSecInternalComponent`.
  - A post-build step runs `codesign -dvvv` + `codesign --verify --deep
    --strict` and **fails the leg** unless a `TeamIdentifier` is present and
    `Authority=` equals `APPLE_SIGNING_IDENTITY`.
  - `hardenedRuntime: false` — only matters for notarization.
  - **No entitlements file exists** in the repo.
  - Consequence: Gatekeeper blocks a downloaded build; users must run
    `xattr -dr com.apple.quarantine /Applications/claude-fleet.app`. This text
    is auto-inserted into every release body.
  - Certificate **expires yearly**; renewal is a manual Xcode + 3-secret
    rotation. Backup copies live in the owner's macOS keychain
    (`claude-fleet-apple-dev-p12-base64`, `…-password`).
- Artifacts: `claude-fleet_<v>_{aarch64,x64}.dmg` and
  `claude-fleet_<v>_{aarch64,x64}.app.tar.gz`.
  The `.app.tar.gz` is the bundler's updater artifact; **tauri's own updater
  plugin is NOT a dependency** — the fleet has its own update engine
  (`fleet-update`). The bundler hard-codes an *unversioned* name, so
  `scripts/rename-updater-asset.sh` renames the **uploaded asset** afterwards
  via the releases API. (Six releases shipped byte-different files under two
  identical names before this existed.)

### Mobile

**Not in this repository.** No Gradle, no Xcode project, no
`src-tauri/gen/android|apple`. Android lives in a separate repo
`fleet-mobile`, tagged with the same version by
`scripts/release-mobile.sh <version>` (which refuses unless claude-fleet's own
release exists on GitHub and fleet-mobile's CI is green). Its signing uses four
`ANDROID_*` secrets **in that repo**. iOS is not released (no signing
identity).

---

## 4. Tauri

- **Tauri 2** (`"$schema": https://schema.tauri.app/config/2`; deps
  `tauri = "2"`, `tauri-build = "2"`, `@tauri-apps/cli ^2`,
  `@tauri-apps/api ^2`).
- **Workspace shape**: the Tauri crate is `src-tauri` (a workspace member
  alongside `crates/*`), so the cargo target dir is **`<root>/target`**, not
  `src-tauri/target`. README still says `src-tauri/target/release/bundle/` —
  stale; release.yml's signature check defensively searches *both*.
- **Plugins**: `tauri-plugin-opener`, `tauri-plugin-dialog`,
  `tauri-plugin-clipboard-manager`. No updater plugin, no shell plugin.
- **Native deps beyond Tauri**: `portable-pty` (the PTY, desktop-only),
  `rusqlite` with `bundled` (compiles SQLite from C — needs a **C compiler**),
  `ring` (needs `cc`), `security-framework` (macOS), `windows-sys` (Windows),
  Microsoft ConPTY (Windows, vendored at build time).
- **Frontend**: pnpm 10 + Vite 6 + Svelte 5, output to `dist/`.
- **Ordering** is Tauri's own: `beforeBuildCommand: pnpm build` runs *inside*
  `cargo tauri build`. `pnpm install` is therefore a separate, earlier step in
  every workflow; `pnpm build` is **not** called explicitly in release.yml.
- **Bundle targets**: `"all"` in the config, overridden per leg by
  `--bundles` for Linux and Windows. macOS legs pass only `--target`, so they
  take `"all"` → `.app`, `.dmg`, `.app.tar.gz`.
- **Updater configuration**: none in `tauri.conf.json`. Updates are the
  fleet's own system: a signed `release-manifest.json` per release plus signed
  `stable.json` / `beta.json` channel documents on an orphan git branch
  `update-channels`, all **minisign**-signed with `RELEASE_SIGNING_KEY` and
  verified against the public key compiled into
  `crates/fleet-update/src/keys.rs` before publication.

### The actual dependency chain

```
git tag vX.Y.Z  (scripts/release.sh, local, manual)
        │
        ├─► version-consistency  (check-version-consistency.sh --expect-tag)
        │        6 carriers + 4 Cargo.lock entries + compose image pin + tag
        │        ── gate: nothing below runs if this fails
        │
        ├─► plan              release-assets.sh ──► 2 matrices (JSON)
        ├─► create-release    CHANGELOG section ──► draft release (id)
        │
        ├─► build  (4 legs, fail-fast:false, not continue-on-error)
        │     apt / ConPTY fetch / Apple secrets+WWDR
        │       ↓
        │     pnpm install --frozen-lockfile
        │       ↓
        │     tauri-action ──► pnpm build (frontend) ──► cargo tauri build
        │       ↓                                        (Rust + bundle)
        │     codesign verify (macOS) │ 7z payload verify (Windows)
        │       ↓
        │     upload to draft  ──► rename-updater-asset.sh (macOS only)
        │
        ├─► agent-hub-binaries  (2 legs, continue-on-error)
        │     cargo build --release --locked --target <triple>
        │       ↓  FLEET_GIT_SHA / FLEET_BUILD_ID=rel-<tag>-<sha>
        │     package-linux-release.sh ──► 2 tarballs + SHA256SUMS.<triple>
        │       ↓  (asserted against release-assets.sh)
        │     upload-release-asset.sh (by numeric release id)
        │     upload-artifact sha256sums-<triple>
        │
        ├─► manifest  (continue-on-error, 45 min)
        │     download every asset ──► unpack hub tarball ──► `fleet-hub compat`
        │     wait ≤30 min for hub-image.yml's digest on the release body
        │     write release-manifest.json ──► minisign ──► fleet-release verify
        │     ──► upload .json + .minisig
        │
        ├─► checksums  (continue-on-error)
        │     download every asset ──► merge-sha256sums.sh (cross-checked
        │     against each leg's SHA256SUMS.<triple>) ──► upload SHA256SUMS
        │
        ├─► verify-release   ◄── THE GATE (no continue-on-error)
        │     every declared asset present, nothing extra, every asset in
        │     SHA256SUMS
        │
        ├─► publish  (default needs-succeeded ⇒ draft stays a draft on any
        │     failure) ──► updateRelease{draft:false} ──► best-effort
        │     `gh workflow run docs.yml`
        │
        └─► channel  (after publish) ──► update-channels.sh add <tag>
              ──► signed stable.json / beta.json pushed to orphan branch

in parallel, same tag push:
hub-image.yml: version-consistency ─► meta ─► build×2 (push by digest, gha
cache) ─► merge (imagetools create) ─► record-digest (writes digest block
into the release body, pair-aware splice)
```

---

## 5. Current CI/CD, job by job

### `ci.yml`

Triggers: `push` to `main`, **every** `pull_request`, `workflow_dispatch`.
No concurrency group. No job-level timeouts (one step-level: 5 min).
No retries anywhere.

| job | runner(s) | installs | cache | commands | artifacts |
|---|---|---|---|---|---|
| `version-consistency` | ubuntu-24.04 | `sqlite3` (apt, conditional) | none | `check-version-consistency.sh`; release-asset-manifest smoke (`node -p`); `changelog-section.sh`; `hub-deploy-scripts-test.sh` | none |
| `ag` | macos-latest, ubuntu-24.04 | none | none | `scripts/ag-test.sh`; `shellcheck` (Linux only) | none |
| `rust` | macos-latest, ubuntu-24.04 | Tauri apt list (Linux); `cargo-deny` via `taiki-e/install-action` (Linux) | `Swatinem/rust-cache@v2`, `workspaces: .` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`, `cargo deny check` (Linux only) | none |
| `rust-windows` | windows-latest | pnpm, Node 20, ConPTY | rust-cache key `windows`, pnpm cache | clippy + test (excluding `fleet-agent`/`fleet-hub`, `--no-fail-fast`); `pnpm install --frozen-lockfile`; `fetch-conpty.sh`; `pnpm tauri build --debug --bundles nsis --config …conpty…`; ConPTY-in-exe-dir + ConPTY-in-NSIS checks | none |
| `hub-headless` | ubuntu-24.04 | `minisign` (apt) — **deliberately no Tauri libs** | rust-cache key `hub-headless` | `cargo build -p fleet-hub --locked`; `cargo test -p fleet-core -p fleet-hub --locked`; `cargo build -p fleet-agent --locked`; `cargo test -p fleet-proto -p fleet-agent -p fleet-update --locked`; `cargo build -p fleet-update --bin fleet-release --locked`; `release-update-scripts-test.sh`; `cargo build -p fleet-hub --features e2e` into `target/e2e`; `hub-e2e.sh` (timeout 5 min) | `hub-e2e-logs` on failure (7-day retention, filtered to `*.log/.sse/.err` — never the data dirs, which hold tokens and SQLite DBs) |
| `frontend` | macos-latest, ubuntu-24.04, windows-latest | pnpm, Node 20 | `setup-node` pnpm cache | `pnpm install --frozen-lockfile`, `pnpm run check`, `pnpm run test`, `pnpm run build`, `pnpm audit --audit-level=high` | none |

Job dependencies in `ci.yml`: **none** — all six jobs are independent and run
in parallel. Secrets used: **none** (only the implicit `GITHUB_TOKEN`).

Non-obvious runner assumptions in `ci.yml`:
- `hub-e2e` needs **tmux, jq, python3** — present on `ubuntu-24.04`, absent on
  the macOS runners, which is why that job is Linux-only.
- `7z` is assumed present on `windows-latest` (used to read the NSIS payload).
- `shellcheck` is assumed present on `ubuntu-24.04`.
- The `ag` macOS leg deliberately exercises **stock `/bin/bash` 3.2**.
- `sqlite3` is installed only if missing.

### `release.yml`

Trigger: `push` tag `v*`, plus `workflow_dispatch`. **Every job gated on
`github.ref_type == 'tag'`.** `concurrency: release-<tag>`,
`cancel-in-progress: false`. `permissions: contents: write` at workflow level;
`publish` additionally takes `actions: write`.

Jobs, conditions and failure policy: see the chain in §4 and the table in
`docs/RELEASING.md` §"GitHub release job". The two structural rules worth
carrying forward:

- **per-leg isolation** — `fail-fast: false` on both matrices;
  `continue-on-error: true` on `agent-hub-binaries`, `manifest`, `checksums`.
- **per-release completeness** — `verify-release` has **no**
  `continue-on-error` and is the single gate; `publish` hangs off it on the
  *default* needs-succeeded condition, which is the entire definition of
  "publish automatically, draft only when something went wrong".
- Three jobs use `if: !cancelled() && needs['version-consistency'].result ==
  'success'` and therefore **re-state the gate by hand** — because
  `!cancelled()` overrides the needs-succeeded default.

Timeouts: `agent-hub-binaries` 20 min, `manifest` 45 min. Retries: none.

### `hub-image.yml`

Trigger: `push` tag `v*` + dispatch; every job tag-gated (with a documented
caveat: a dispatch at an *old branch* runs that branch's ungated copy).
`permissions: contents: read, packages: write`; `meta` drops to `{}`;
`record-digest` is a separate job purely so the image-pushing job never holds
`contents: write`. arm64 leg is `continue-on-error: true`; amd64 is not.
`merge` runs on `!cancelled()` and fails for real if the amd64 digest is
missing. Timeout 30 min on the build legs.

### `docs.yml`

`release: published` + dispatch. Runs `cargo doc --no-deps --workspace` on
**macos-latest**, then GitHub Pages actions.
**Has never succeeded — 11/11 red**, all on `actions/configure-pages`
(Pages was never enabled on the repo). `enablement: true` is an untested
hypothesis. Also: the `release: published` trigger no longer fires, because
the publication is performed by `GITHUB_TOKEN`; `release.yml` compensates with
a best-effort `gh workflow run docs.yml`, which is likewise unverified.

### `release-drift.yml`

Daily cron `17 6 * * *` + dispatch. `contents: write` (needed only so drafts
are listed) + `issues: write`. Runs `scripts/check-release-drift.sh`, which
drives `verify-release.sh` over the newest 10 releases, plus a **ghcr lookup**
of the compose image pin — the only check anywhere that talks to a registry.
Exit 1 = drift found (normal); exit 2 = could not check (red job, issue left
alone). Maintains exactly one `release-drift`-labelled issue.
**The issue create/update/close steps have never run against GitHub.**

### `update-channels.yml`

Weekly cron `17 6 * * 1` + dispatch with 7 typed inputs.
`contents: write`, `concurrency: update-channels` (shared with release.yml's
`channel` job). Timeout 20 min. Uses the composite action, then
`scripts/update-channels.sh resign|edit`.

### Secrets, complete list

`GITHUB_TOKEN` (implicit), `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`RELEASE_SIGNING_KEY`. **Five.** Nothing else. (fleet-mobile's four
`ANDROID_*` secrets live in its own repo.)

---

## 6. Local developer workflow

Documented, in README §Development:

```bash
pnpm install
pnpm tauri dev          # dev: hot-reload frontend + debug Rust
pnpm tauri build        # release bundle
pnpm test | pnpm check  # frontend
cargo test --workspace | cargo clippy … | cargo fmt --all --check | cargo deny check
scripts/ci-local.sh [--rust-only|--frontend-only|--hub-e2e]
git config core.hooksPath .githooks   # opt-in pre-commit
```

There is **no `./build.sh`, no `make`, no `just`**. `scripts/ci-local.sh` is
the single "run everything" entry point.

`ci-local.sh` behaviours that differ from CI:
- It **auto-degrades**: if `pkg-config --exists gtk+-3.0` fails it silently
  runs a *headless subset* (fmt + clippy/test scoped to the five non-Tauri
  crates). A developer on a headless box therefore never runs
  `cargo test --workspace`, and never exercises `src-tauri` or
  `REGEN_HUB_VERDICTS`.
- It **resolves pnpm three ways**: `pnpm` on PATH if ≥ 10, else
  `corepack pnpm`, else `npx -y pnpm@10`.
- It **skips** `release-update-scripts-test.sh` when `minisign` is missing and
  **skips** `--hub-e2e` when `tmux` is missing — both with a message, not a
  failure.
- `--hub-e2e` is opt-in because it drives this machine's **real tmux server**
  for Hub A.

The pre-commit hook is path-triggered (Rust paths → fmt+clippy; frontend paths
→ `ci-local.sh --frontend-only`), runs an optional untracked
`.githooks/pre-commit.local` first, and **re-keys `CARGO_TARGET_DIR` to the
current worktree** if it was inherited from another checkout.

### Where "works on my machine" is currently load-bearing

1. **`cargo` is a shell function on the owner's Mac** pointing at
   `/Volumes/CargoSD/target` — an external volume. Numerous plan documents
   instruct `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/<name>` before
   any cargo command, and note that *scripts bypass the wrapper*. Nothing in
   the repo encodes this; builds fail with ENOSPC or a missing volume if it is
   absent.
2. **Release tags are SSH-signed only because the owner's `~/.gitconfig` sets
   `tag.gpgsign=true`.** `scripts/release.sh` runs `git tag -a` — on any other
   machine the same script produces an **unsigned** tag.
   `docs/RELEASING.md` names this explicitly as a gap.
3. **Apple certificate renewal** is a yearly manual Xcode export from the
   owner's login keychain; the only backups are two keychain items on that
   machine.
4. **The minisign release secret key** exists in exactly two places: the GitHub
   secret and `claude-fleet-release-signing-key` in the owner's macOS keychain
   (or `~/claude-fleet-release.key` elsewhere). `scripts/release-key.sh` is
   explicitly a once-per-lifetime, run-it-on-your-own-machine script, and
   refuses to re-run.
5. **`scripts/release.sh` requires a logged-in `gh`** (for the CI gate) and
   network access, unless `RELEASE_DRY_RUN=1` / `RELEASE_SKIP_CI_CHECK=1`.
6. **`hub-e2e.sh` prefers `/tmp/claude-$(id -u)`** and deliberately avoids
   `$TMPDIR` because macOS's long per-user path overflows the 108-byte unix
   socket limit.
7. **Homebrew bash first on PATH** is required for `hub-e2e.sh` on macOS per
   several plan docs (the stock `/bin/bash` is 3.2 and the script is not
   3.2-clean — unlike `tools/ag`, which is).
8. **Stale `node_modules` after a pull** produces
   `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"` — the
   documented fix is `pnpm install --frozen-lockfile`.

---

## 7. Dependencies on machine state (not defined by the repository)

### Pinned in-repo (reproducible)

| thing | pin | file |
|---|---|---|
| pnpm | `10.34.5` | `package.json` `packageManager` |
| Node (major) | `20` | `.node-version`, and `node-version: '20'` in every workflow |
| Node (range) | `^20.19 \|\| ^22.12 \|\| >=24` | `package.json` `engines` |
| npm deps | exact | `pnpm-lock.yaml` (`--frozen-lockfile` everywhere) |
| Rust crates | exact | `Cargo.lock` (`--locked` on hub/agent/manifest/docker builds — **not** on `cargo test --workspace`, clippy, or `tauri build`) |
| ConPTY | version + SHA-256 | `scripts/fetch-conpty.sh` |
| Apple WWDR G3 | SHA-256 | `release.yml` |
| rust base image | `rust:1-bookworm` (**floating minor**) | `Dockerfile` |
| runtime base image | `debian:bookworm-slim` (**floating**) | `Dockerfile` |
| hub image pin | `0.4.2` | `deploy/hub/docker-compose.yml` |

### NOT pinned / supplied by the machine

- **Rust toolchain version.** `rust-toolchain.toml` says `channel = "stable"` —
  a floating pin, by explicit decision ("tracks stable rather than pinning a
  specific version so local builds match what CI resolves"). CI uses
  `dtolnay/rust-toolchain@stable`. README claims "Rust 1.83+". **A stable
  release that introduces a new clippy lint breaks CI with no code change.**
- **Rust targets**: `rustup target add <triple>` is run ad hoc in release.yml
  only when `matrix.target != ''`. Not declared in `rust-toolchain.toml`.
- **`cargo-deny`**: installed per-run via `taiki-e/install-action@v2` in CI
  (unpinned version); locally `cargo install cargo-deny --locked`.
- **Tauri system libraries** (Linux): apt-installed per run, **no versions
  pinned**, list duplicated in two workflows.
- **C toolchain / `pkg-config`**: required by `rusqlite` (`bundled` SQLite) and
  `ring`. Present via `build-essential` on Linux, Xcode CLT on macOS, MSVC on
  Windows — never asserted.
- **CLI tools assumed present on the runner, never installed or version-checked**:
  `gh` (every release script), `jq`, `tmux`, `python3`, `7z`, `shellcheck`,
  `sha256sum`/`shasum`, `tar`, `curl`, `unzip`, `sed`/`awk`, `git`, `docker` +
  `buildx`, `node`, `timeout`, `sort -V`, `mapfile` (bash ≥ 4 in several
  scripts).
- **Explicitly installed per run**: `minisign` (apt, twice — `ci.yml`
  `hub-headless` and the composite action), `sqlite3` (conditional).
- **`CARGO_TARGET_DIR`**: not set in CI; set by convention (and by a shell
  function) on the owner's machine; re-keyed by the pre-commit hook.
- **Caches** are GitHub-specific services: `Swatinem/rust-cache@v2` (4 distinct
  keys: default, `windows`, `hub-headless`, `agent-hub-binaries-<target>`,
  `release-manifest`, `<os>-<target|native>`), `setup-node`'s pnpm cache,
  `type=gha` buildx cache, and the Dockerfile's BuildKit cache mounts.
- **Git worktree state**: `vitest.config.ts` must exclude `.worktrees/` and
  `.claude/worktrees/` or it runs duplicate suites; this repo is routinely
  worked on from linked worktrees.
- **Network at build time**: crates.io, npm registry, apt, `api.nuget.org`
  (ConPTY), `apple.com` (WWDR cert), ghcr, GitHub API. **Nothing is vendored.**

---

## 8. Generated files that CI enforces

A build agent must be able to run these, or the corresponding CI test fails:

| generated file(s) | regenerated by | needs Tauri libs? |
|---|---|---|
| `docs/control-api-reference.md` | `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` | no |
| `docs/settings-reference.md`, tables in `docs/work-graph.md`, `docs/decisions.md` | `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current` | no |
| `docs/page-spec.schema.json`, `docs/page-catalog.json`, `src/lib/pages/registry.generated.json` | `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current` | no |
| `src/lib/hub_verdicts.generated.json`, the refusal table in `docs/hub.md` | `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` | **yes** |

Also `REGEN_ENV`, `REGEN_HUB_CONTRACT`, `REGEN_LOCAL_ONLY`,
`REGEN_TRACKER_GOLDENS` exist in the test suite.

---

## 9. Security / supply-chain posture (as built today)

- `cargo deny check` — licenses + advisories + `unknown-registry = "deny"`,
  Linux leg only.
- `pnpm audit --audit-level=high` — a **blocking** step in the `frontend` job
  on all three OSes.
- Every externally-fetched build input is hash-pinned (ConPTY package, WWDR
  cert). Container base images and the Rust toolchain are not.
- `SHA256SUMS` covers **every** asset on a release and is cross-checked against
  per-leg digests computed on the builder.
- Release artifacts carry provenance: each tarball's `README.txt` and the
  release body record the **exact commit**, and `FLEET_BUILD_ID=rel-<tag>-<sha>`
  is compiled into the hub binary and the image and signed into the manifest.
- Signed documents: `release-manifest.json(.minisig)` and the channel docs,
  minisign, verified against `crates/fleet-update/src/keys.rs` **before** upload.
- Workflow hygiene the authors enforce consistently and that any port must
  preserve: **no `${{ }}` expression carrying tag/ref/user text is ever
  interpolated into a `run:` body** — everything arrives through `env:`. Asset
  names are validated against `^[A-Za-z0-9._-]+$` before any API call.
  Job permissions are narrowed per job.

---

## 10. Portable build logic vs. GitHub-Actions-specific orchestration

This is the split that matters for a Buildkite evaluation.

### Already portable (plain bash / cargo / pnpm; runs anywhere)

- `scripts/release-assets.sh` — the asset/leg manifest. Emits GH matrix JSON as
  *one* of its subcommands (`matrix <kind>`); `legs`, `assets`, `runner` are
  provider-neutral. A Buildkite dynamic pipeline would read the same table.
- `scripts/check-version-consistency.sh`, `scripts/release.sh`,
  `scripts/changelog-section.sh` — pure local tooling.
- `scripts/package-linux-release.sh`, `scripts/merge-sha256sums.sh`,
  `scripts/fetch-conpty.sh`, `scripts/release-pubkeys.sh`,
  `scripts/release-verify-sig.sh`, `scripts/merge-hub-digests.sh` — pure, no
  GitHub API.
- `scripts/ci-local.sh`, `scripts/hub-e2e.sh`,
  `scripts/hub-deploy-scripts-test.sh`,
  `scripts/release-update-scripts-test.sh`, `scripts/ag-test.sh` — the whole
  test surface, already runnable outside CI. `hub-e2e.sh` reads `CI` and
  `GITHUB_ENV` but degrades cleanly without them.
- `crates/fleet-hub/Dockerfile` — provider-neutral; only the two build args
  matter.
- Every `cargo` and `pnpm` invocation in §2.

### GitHub-specific, would need replacing

| concern | current mechanism |
|---|---|
| orchestration | `jobs:`/`needs:`/`strategy.matrix` + `fromJSON` of a generated matrix |
| conditional gating | `if: github.ref_type == 'tag'`, `!cancelled()`, `needs[x].result` |
| per-leg tolerance | `continue-on-error`, `fail-fast: false` |
| runner selection | `runs-on` labels (`macos-latest`, `ubuntu-22.04-arm`, `windows-latest`) — the runner labels are *data in `release-assets.sh`*, which helps |
| rust caching | `Swatinem/rust-cache@v2` (6 cache keys) |
| node caching | `actions/setup-node` `cache: 'pnpm'` + `pnpm/action-setup@v4` |
| toolchain install | `dtolnay/rust-toolchain@stable`, `taiki-e/install-action@v2` |
| the Tauri build+upload | **`tauri-apps/tauri-action@v0`** — the single largest coupling: it runs `beforeBuildCommand`, `cargo tauri build`, *and* uploads to a release id. A port must split this into `pnpm tauri build <args>` + an explicit upload. |
| artifact passing between jobs | `actions/upload-artifact@v4` / `download-artifact@v4` (`sha256sums-<target>`, `digests-<arch>`) |
| release object | `actions/github-script@v7` (create/reuse draft, publish, splice the digest block) + `gh` CLI in scripts |
| asset upload | `scripts/upload-release-asset.sh` — portable bash, but hard-wired to the **GitHub releases API** and to a *numeric release id* (because drafts aren't resolvable by tag) |
| secrets | `secrets.*` → env |
| step annotations / summaries | `::error::`, `::warning::`, `$GITHUB_STEP_SUMMARY`, `$GITHUB_OUTPUT`, `$GITHUB_ENV` — used in **many** scripts, not just workflows (`release-manifest.sh`, `update-channels.sh`, `upload-release-asset.sh`, `hub-e2e.sh`) |
| container registry auth | `docker/login-action@v3` with `GITHUB_TOKEN` → ghcr |
| multi-arch image | `docker/setup-buildx-action@v3` + `docker/build-push-action@v6` (`push-by-digest`) + `docker/metadata-action@v5` + `imagetools create`; buildx cache `type=gha` |
| schedules | `on.schedule` cron (drift check, weekly re-sign) |
| manual parameterised runs | `workflow_dispatch.inputs` with `type: choice` (update-channels) |
| cross-workflow coupling | `hub-image.yml` writes a digest onto the release body; `release.yml`'s `manifest` job **polls the release body for up to 30 minutes** waiting for it. Two independent runs communicating through a GitHub release object. |
| cross-workflow dispatch | `gh workflow run docs.yml` |
| the CI gate | `scripts/check-ci-green.sh` asks the **GitHub Actions API** whether `ci.yml` is green for a sha — `scripts/release.sh` refuses to tag otherwise |
| GitHub Pages | `configure-pages` / `upload-pages-artifact` / `deploy-pages` |
| drift reporting | `gh issue create/comment/close` |

### Structural facts a Buildkite design must preserve

1. **The asset manifest is the single source of truth.** Build legs, runner
   labels, target triples, build args and expected filenames all come from
   `scripts/release-assets.sh`. The packaging script, the renamer and the
   release gate all *assert against it*. Any new pipeline should generate its
   steps from the same table, not restate it.
2. **`verify-release` is the only gate; publication is its consequence.** The
   "publish automatically, draft only when something went wrong" rule is not a
   written rule — it falls out of `publish` depending on `verify-release` with
   the default needs-succeeded condition. Reproducing it requires an
   equivalent of "this step runs only if every dependency actually succeeded",
   distinct from "this step runs even after a failure".
3. **Three separate per-release gates run the *same* script**:
   `check-version-consistency.sh` runs in `ci.yml`, `release.yml` and
   `hub-image.yml` (the last two with `--expect-tag`).
4. **Native-only cross-building.** Every non-host target is either a native
   runner (`ubuntu-22.04-arm`, `ubuntu-24.04-arm`) or an Apple-toolchain
   cross-compile (`x86_64-apple-darwin` on an arm64 Mac). A Buildkite setup
   needs real arm64 Linux agents and real macOS agents — there is no QEMU or
   `cross` fallback to lean on, and the glibc-2.35 floor means the Linux
   daemon binaries must be built on a **22.04-era** image specifically.
5. **The glibc floor and the Ubuntu version are a contract**, documented in
   `release-assets.sh` and `docs/hub.md`.
6. **macOS signing needs a real keychain** on a real macOS agent (certificate
   import, WWDR intermediate into the System keychain, `codesign`).
7. **Two workflows must stay able to run on the same tag concurrently** and
   rendezvous (`manifest` waits for `hub-image.yml`'s digest). This is the
   single most awkward piece to port and the most fragile piece today.
8. `release.yml`'s concurrency is **per tag**, `cancel-in-progress: false`;
   `update-channels` is a **shared** concurrency group across two workflows,
   protecting a git branch from two writers.

---

## 11. Observations worth flagging (facts, not recommendations)

1. `docs.yml` has **never** succeeded (11/11 red) and both of its current
   "fixes" (`enablement: true`, the `gh workflow run` dispatch) are documented
   in-repo as untested hypotheses.
2. `release-drift.yml`'s issue create/update/close steps have **never executed
   against GitHub**.
3. README says `pnpm tauri build` outputs to `src-tauri/target/release/bundle/`;
   the workspace layout puts it at `target/release/bundle/`. `release.yml`
   already searches both.
4. `rust-toolchain.toml` tracking `stable` means CI breakage can arrive from a
   Rust release with no commit to this repo.
5. `cargo test --workspace`, `cargo clippy --workspace` and `cargo tauri build`
   do **not** pass `--locked`; only the hub/agent/manifest/docker builds do.
6. `scripts/upload-release-asset.sh` is documented as **non-atomic**
   (delete-then-upload) — on a re-run over a *published* release the asset is
   briefly absent.
7. `ci-local.sh`'s silent degradation on a headless box means a developer can
   be "green locally" without ever compiling `src-tauri`.
8. The Tauri apt package list is the one list duplicated verbatim across two
   workflows with no single source.
9. No desktop build exists for **Linux arm64** or **Windows arm64**.
10. `manifest`'s 30-minute wait on another workflow's output is the longest and
    least-contained dependency in the release path.

---

### Files a follow-up Buildkite design should read first

1. `scripts/release-assets.sh` — the table everything else is generated from.
2. `docs/RELEASING.md` — the reasoning behind every gate.
3. `.github/workflows/release.yml` — its header comments state the
   continue-on-error / gate contract explicitly.
4. `scripts/ci-local.sh` — the already-portable expression of `ci.yml`.
5. `scripts/verify-release.sh` + `scripts/check-release-drift.sh` — the
   definition of "a complete release".
