---
name: release-and-ci
description: Use when verifying claude-fleet changes locally the way GitHub CI does (cargo fmt/clippy/test, frontend check/test/build, the doc-gen staleness gate) before pushing or merging, or when producing a release-please-equivalent version bump + CHANGELOG.md by hand because the automated release PR is not available. Triggers on "run CI locally", "same as CI", "version bump", "changelog", "bump to X.Y.Z", "is this release-ready".
---

# release-and-ci

Mirror claude-fleet's GitHub Actions locally: the **CI gate** (`.github/workflows/ci.yml`)
and the **release-please** version-bump + changelog (`release-please.yml` +
`release-please-config.json`). Use this so a release/version bump is reproducible
and never lost in a deleted worktree.

Run everything from the repo root unless noted. Backend commands need the Tauri
system libs (present on macOS dev boxes; a headless Linux box without dbus/gtk
fails in a build script — that is an environment gap, not your change).

## 1. CI gate — must pass before push/merge

Run all five, in this order. This is exactly what `ci.yml` runs.

```bash
# Rust job (cwd: src-tauri)
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cd ..

# Frontend job (cwd: repo root)
pnpm install --frozen-lockfile
pnpm run check      # svelte-check — 0 ERRORS required (warnings OK)
pnpm run test       # vitest
pnpm run build      # vite build
```

Green = every command exits 0, `pnpm run check` reports `0 ERRORS`, and `cargo test`
shows `0 failed`.

## 2. Doc-gen staleness gate

`docs/control-api-reference.md` is generated from the MCP tool router. `cargo test`
fails on `mcp::doc_gen::tests::reference_is_current` if it is stale. After changing
any `#[tool(...)]` description or the `generate_handler!` command list, regenerate
and commit it:

```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
```

Then re-run `cargo test` to confirm the test passes.

## 3. Release-please equivalent (version bump + CHANGELOG)

**Prefer the real thing:** land Conventional Commits on `main`; release-please keeps
an open "chore: release X.Y.Z" PR; merging it bumps versions + writes CHANGELOG +
tags `vX.Y.Z`. See `docs/RELEASING.md`. Only do this by hand when you need a local
release-numbered build and the PR is unavailable.

### Compute the next version

Baseline = the version in `.release-please-manifest.json` (NOT the git tag).
Inspect commits since the last release and pick the bump per the highest-ranked type:

| Commit prefix | Bump | CHANGELOG section |
|---|---|---|
| `feat!:` / `fix!:` / `BREAKING CHANGE:` footer | **major** | (note breaking) |
| `feat:` | **minor** | Features |
| `fix:` | patch | Bug Fixes |
| `perf:` | patch | Performance |
| `refactor:` | patch | Refactors |
| `docs:` | patch | Documentation |
| `chore:` / `test:` / `ci:` | none | hidden |

```bash
git log --pretty='%s' <last-release>..HEAD   # scan prefixes to pick the bump
```

A `feat:` present (no breaking) → minor bump (e.g. `0.2.0 → 0.3.0`).

### Apply the bump to ALL version files (keep in sync)

release-please updates these four + the manifest. Set them all to the same `X.Y.Z`:

- `package.json` → `$.version`
- `src-tauri/tauri.conf.json` → `$.version`
- `src-tauri/Cargo.toml` → `version = "X.Y.Z"` (package, not deps)
- `src-tauri/Cargo.lock` → the `name = "claude-fleet"` package entry's `version`
- `.release-please-manifest.json` → `{ "src-tauri": "X.Y.Z" }`

After editing `Cargo.toml`, run `cargo update -p claude-fleet --precise X.Y.Z` is NOT
needed — instead let a normal `cargo build`/`cargo test` rewrite `Cargo.lock`, then
confirm the `claude-fleet` entry shows the new version.

### Write CHANGELOG.md

Group commit subjects under the sections from the table (omit hidden types). Newest
release on top, with a `## [X.Y.Z]` heading and the date. Keep entries terse — copy
the commit subject minus the prefix.

### Commit

Match release-please's commit style so history stays consistent:

```bash
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock \
        .release-please-manifest.json CHANGELOG.md
git commit -m "chore: release X.Y.Z"
```

## 4. Build the app (optional, after CI is green)

```bash
pnpm tauri build                 # .app + .dmg (release-signed bundle)
pnpm tauri build --bundles app   # .app only — use if bundle_dmg.sh fails transiently
```

The `.dmg` step deletes the `.app` from `bundle/macos/` after packaging — rebuild
with `--bundles app` if you need the loose `.app` afterwards. Output:
`src-tauri/target/release/bundle/{macos,dmg}/`.

## Common mistakes

- **Bumping only some version files** — all four files + the manifest must match, or
  CI/release-please drifts. `Cargo.lock` is the one most often forgotten.
- **Hand-bumping on `main` when release-please is healthy** — creates a conflict with
  the release PR. Hand-bump only for a throwaway local build, on a branch.
- **Skipping `REGEN_DOCS`** after touching MCP tools — `cargo test` fails on the
  staleness gate even though your code is correct.
- **Treating `pnpm run check` warnings as failures** — only `ERRORS` block; a11y/CSS
  warnings are pre-existing and non-blocking.
- **Running `cargo` from repo root** — the manifest is in `src-tauri/`; either `cd`
  there or pass `--manifest-path src-tauri/Cargo.toml`.
