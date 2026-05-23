# Release Automation & Docs Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a release-please CI release-PR bot that bumps the version across all four files and generates a changelog, an in-process-generated MCP/command reference gated for staleness in CI, rustdoc published to GitHub Pages, and a refresh of CLAUDE.md / README.md / a new RELEASING.md.

**Architecture:** release-please (GitHub Action, `rust` release-type rooted at `src-tauri/`) keeps an open release PR; merging it tags + publishes a GitHub Release. A `#[cfg(test)]` doc-generator module renders `docs/control-api-reference.md` from the live `FleetTools` tool router and asserts the committed file matches (the staleness gate, run by the existing `cargo test` CI job). A separate Pages workflow publishes `cargo doc` output on each release.

**Tech Stack:** Rust (Tauri 2 backend, rmcp 1.7), GitHub Actions, release-please v4, GitHub Pages.

---

## File Structure

- `src-tauri/src/mcp/doc_gen.rs` — **create.** `#[cfg(test)]` module: `render_reference()` builds the markdown from `FleetTools::tool_router().list_all()` + parsed Tauri commands; a test enforces the committed file matches.
- `src-tauri/src/mcp/mod.rs` — **modify.** Add `#[cfg(test)] mod doc_gen;`.
- `docs/control-api-reference.md` — **create (generated).** The committed reference.
- `release-please-config.json` — **create.** release-please package + extra-files + changelog config.
- `.release-please-manifest.json` — **create.** Bootstrap version map.
- `.github/workflows/release-please.yml` — **create.** The release-PR bot.
- `.github/workflows/docs.yml` — **create.** rustdoc → Pages on release.
- `docs/RELEASING.md` — **create.** Contributor release guide.
- `README.md` — **modify.** Badges, release workflow, doc links.
- `CLAUDE.md` — **modify.** Corrected LOC, landed features, releasing pointer.

---

## Task 1: Generated control-API reference + CI staleness gate

**Files:**
- Create: `src-tauri/src/mcp/doc_gen.rs`
- Modify: `src-tauri/src/mcp/mod.rs` (add module declaration)
- Create (generated): `docs/control-api-reference.md`

- [ ] **Step 1: Create the doc-generator module**

Create `src-tauri/src/mcp/doc_gen.rs`:

```rust
//! Generates the control-API tool reference from the live MCP tool router.
//!
//! The committed `docs/control-api-reference.md` must equal `render_reference()`;
//! the `reference_is_current` test enforces it (and so does CI via `cargo test`).
//! Regenerate after changing any tool with:
//!   REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
//!
//! Entirely `#[cfg(test)]`: it touches no production code path and adds no
//! public API surface.

use crate::mcp::FleetTools;

const HEADER: &str = "<!-- GENERATED FILE — do not edit by hand.\n     \
Regenerate with: REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current -->\n";

/// Render the full reference markdown.
pub(crate) fn render_reference() -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str("\n# claude-fleet Control API — Tool Reference\n\n");
    out.push_str(
        "Auto-generated from the embedded MCP tool router. \
See [`control-api.md`](control-api.md) for the narrative guide.\n\n",
    );

    // --- MCP tools ---
    out.push_str("## MCP tools\n\n");
    let mut tools = FleetTools::tool_router().list_all();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    for t in &tools {
        out.push_str(&format!("### `{}`\n\n", t.name));
        if let Some(d) = &t.description {
            out.push_str(d.trim());
            out.push_str("\n\n");
        }
        if let Some(props) = t.input_schema.get("properties").and_then(|v| v.as_object()) {
            if !props.is_empty() {
                let mut names: Vec<&String> = props.keys().collect();
                names.sort();
                let rendered: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
                out.push_str(&format!("Parameters: {}\n\n", rendered.join(", ")));
            }
        }
    }

    // --- Tauri commands ---
    out.push_str("## Tauri IPC commands\n\n");
    out.push_str("Frontend commands registered in `src/lib.rs`:\n\n");
    for cmd in tauri_commands() {
        out.push_str(&format!("- `{cmd}`\n"));
    }
    out.push('\n');
    out
}

/// Extract command identifiers from the `generate_handler![ … ]` block in lib.rs.
fn tauri_commands() -> Vec<String> {
    let src = include_str!("../lib.rs");
    let start = src
        .find("generate_handler![")
        .expect("generate_handler! macro present in lib.rs");
    let rest = &src[start + "generate_handler![".len()..];
    let end = rest.find(']').expect("closing ] of generate_handler!");
    rest[..end]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("//"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn doc_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/control-api-reference.md")
    }

    #[test]
    fn extracts_some_commands() {
        let cmds = tauri_commands();
        assert!(cmds.contains(&"commands::sessions::list_sessions".to_string()));
        assert!(cmds.len() > 20, "expected many commands, got {}", cmds.len());
    }

    #[test]
    fn renders_known_tool() {
        let md = render_reference();
        assert!(md.contains("### `list_sessions`"), "list_sessions tool missing");
    }

    #[test]
    fn reference_is_current() {
        let expected = render_reference();
        let path = doc_path();
        if std::env::var("REGEN_DOCS").is_ok() {
            std::fs::write(&path, &expected).expect("write control-api-reference.md");
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "\n\ndocs/control-api-reference.md is stale. Regenerate with:\n  \
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current\n"
        );
    }
}
```

- [ ] **Step 2: Declare the module**

In `src-tauri/src/mcp/mod.rs`, add alongside the other `mod` lines (near line 8-10):

```rust
#[cfg(test)]
mod doc_gen;
```

- [ ] **Step 3: Run the helper tests to verify the generator works (file not yet created → reference_is_current fails)**

Run: `cd src-tauri && cargo test --lib mcp::doc_gen -- --nocapture`
Expected: `extracts_some_commands` and `renders_known_tool` PASS; `reference_is_current` FAILS with the "is stale" message (the committed file does not exist yet).

- [ ] **Step 4: Generate the committed reference**

Run: `cd src-tauri && REGEN_DOCS=1 cargo test --lib mcp::doc_gen::tests::reference_is_current`
Expected: PASS (writes `docs/control-api-reference.md`).

- [ ] **Step 5: Verify the gate now passes clean**

Run: `cd src-tauri && cargo test --lib mcp::doc_gen`
Expected: all three tests PASS.

- [ ] **Step 6: Confirm the generated file looks right**

Run: `head -30 docs/control-api-reference.md`
Expected: the generated header comment, the `# claude-fleet Control API — Tool Reference` title, and `### \`...\`` tool entries.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/mcp/doc_gen.rs src-tauri/src/mcp/mod.rs docs/control-api-reference.md
git commit -m "feat(docs): generate control-API reference from tool router, gate staleness in CI"
```

> **CI note:** No `ci.yml` change is required — the existing `rust` job runs `cargo test`, which now includes `reference_is_current`. Any future edit to a tool description or the command list will fail CI until the reference is regenerated and committed.

---

## Task 2: release-please config + manifest

**Files:**
- Create: `release-please-config.json`
- Create: `.release-please-manifest.json`

- [ ] **Step 1: Create the manifest (bootstrap at current version)**

Create `.release-please-manifest.json`:

```json
{
  "src-tauri": "0.2.0"
}
```

- [ ] **Step 2: Create the config**

Create `release-please-config.json`:

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "packages": {
    "src-tauri": {
      "release-type": "rust",
      "package-name": "claude-fleet",
      "changelog-path": "../CHANGELOG.md",
      "extra-files": [
        { "type": "json", "path": "package.json", "jsonpath": "$.version" },
        { "type": "json", "path": "src-tauri/tauri.conf.json", "jsonpath": "$.version" }
      ]
    }
  },
  "changelog-sections": [
    { "type": "feat", "section": "Features" },
    { "type": "fix", "section": "Bug Fixes" },
    { "type": "perf", "section": "Performance" },
    { "type": "refactor", "section": "Refactors" },
    { "type": "docs", "section": "Documentation" },
    { "type": "chore", "hidden": true },
    { "type": "test", "hidden": true },
    { "type": "ci", "hidden": true }
  ]
}
```

> **Risk note (from spec §B):** `release-type: rust` rooted at `src-tauri/` updates `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` natively; `extra-files` (resolved from repo root) cover `package.json` and `src-tauri/tauri.conf.json`; `changelog-path: ../CHANGELOG.md` (relative to the package dir) targets the repo-root `CHANGELOG.md`. These path semantics are verified by the first bot PR in Task 3 — confirm all four version files change together.

- [ ] **Step 3: Validate both files are well-formed JSON**

Run: `node -e "JSON.parse(require('fs').readFileSync('release-please-config.json','utf8')); JSON.parse(require('fs').readFileSync('.release-please-manifest.json','utf8')); console.log('ok')"`
Expected: prints `ok`.

- [ ] **Step 4: Commit**

```bash
git add release-please-config.json .release-please-manifest.json
git commit -m "ci: add release-please config and manifest"
```

---

## Task 3: release-please workflow

**Files:**
- Create: `.github/workflows/release-please.yml`

- [ ] **Step 1: Create the workflow**

Create `.github/workflows/release-please.yml`:

```yaml
name: release-please

on:
  push:
    branches: [main]

permissions:
  contents: write
  pull-requests: write

jobs:
  release-please:
    runs-on: ubuntu-latest
    steps:
      - uses: googleapis/release-please-action@v4
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json
```

- [ ] **Step 2: Validate workflow YAML**

Run: `node -e "const f=require('fs').readFileSync('.github/workflows/release-please.yml','utf8'); if(!f.includes('release-please-action@v4')) throw new Error('action missing'); console.log('ok')"`
Expected: prints `ok`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release-please.yml
git commit -m "ci: add release-please release-PR workflow"
```

> **Manual verification (post-merge to main, cannot run locally):** After this branch merges to `main`, the action runs and opens a **"chore: release X.Y.Z"** PR. Open that PR's "Files changed" and confirm the version moves in **all four** files — `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` — and that `CHANGELOG.md` is created at repo root. If `Cargo.lock` did not change, add a generic updater for it (see RELEASING.md troubleshooting in Task 5). Merging that PR creates the tag + GitHub Release.

> One-time repo setting: under **Settings → Actions → General → Workflow permissions**, ensure "Allow GitHub Actions to create and approve pull requests" is enabled, or release-please cannot open its PR.

---

## Task 4: rustdoc → GitHub Pages workflow

**Files:**
- Create: `.github/workflows/docs.yml`

- [ ] **Step 1: Create the workflow**

Create `.github/workflows/docs.yml`:

```yaml
name: docs

on:
  release:
    types: [published]
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: false

jobs:
  rustdoc:
    runs-on: macos-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    defaults:
      run:
        working-directory: src-tauri
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri
      - run: cargo doc --no-deps
      - name: Add index redirect
        run: echo '<meta http-equiv="refresh" content="0;url=claude_fleet_lib/index.html">' > target/doc/index.html
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: src-tauri/target/doc
      - id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 2: Validate the lib doc directory name**

Run: `grep -n 'name = "claude_fleet_lib"' src-tauri/Cargo.toml`
Expected: matches (confirms the redirect target `claude_fleet_lib/index.html` is correct).

- [ ] **Step 3: Validate workflow YAML**

Run: `node -e "const f=require('fs').readFileSync('.github/workflows/docs.yml','utf8'); if(!f.includes('deploy-pages@v4')||!f.includes('cargo doc')) throw new Error('bad'); console.log('ok')"`
Expected: prints `ok`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/docs.yml
git commit -m "ci: publish rustdoc to GitHub Pages on release"
```

> **Manual prerequisite (cannot run locally):** In **Settings → Pages**, set Source = "GitHub Actions". Documented in RELEASING.md. Trigger once via `workflow_dispatch` to verify before relying on it.

---

## Task 5: RELEASING.md

**Files:**
- Create: `docs/RELEASING.md`

- [ ] **Step 1: Create the guide**

Create `docs/RELEASING.md`:

```markdown
# Releasing claude-fleet

Releases are automated with [release-please](https://github.com/googleapis/release-please).
You never bump versions or write the changelog by hand.

## How it works

1. Land changes on `main` using **Conventional Commits** (see below).
2. The `release-please` workflow watches `main` and maintains an open
   **"chore: release X.Y.Z"** pull request. It computes the next semantic
   version from the commit types, updates every version file, and rewrites
   `CHANGELOG.md`.
3. When you're ready to ship, **merge that PR**. release-please creates the git
   tag (`vX.Y.Z`) and a **GitHub Release** with the generated notes.
4. Publishing the Release triggers the `docs` workflow, which builds rustdoc and
   deploys it to GitHub Pages.

No binaries are built or attached — releases are tag + notes only.

## Conventional Commits

Commit subjects drive the version bump and changelog section:

| Prefix      | Bump  | Changelog section |
|-------------|-------|-------------------|
| `feat:`     | minor | Features          |
| `fix:`      | patch | Bug Fixes         |
| `perf:`     | patch | Performance       |
| `refactor:` | patch | Refactors         |
| `docs:`     | patch | Documentation     |
| `chore:` / `test:` / `ci:` | none | hidden |

A `feat!:` / `fix!:` or a `BREAKING CHANGE:` footer triggers a **major** bump.

## Version files kept in sync

release-please updates all four on each release:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`

Config lives in `release-please-config.json` (a `rust`-type package rooted at
`src-tauri/`, with `package.json` and `tauri.conf.json` as JSON `extra-files`)
and `.release-please-manifest.json` (the current version).

## Generated docs

`docs/control-api-reference.md` is **generated** from the live MCP tool router —
do not edit it by hand. After changing any `#[tool(...)]` description or the
`generate_handler!` command list, regenerate and commit it, or CI fails:

\```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
\```

## One-time setup

- **Settings → Actions → General → Workflow permissions:** enable
  "Allow GitHub Actions to create and approve pull requests".
- **Settings → Pages:** set Source = "GitHub Actions" (for the rustdoc site).

## Troubleshooting

- **No release PR appears:** check the workflow permissions setting above; check
  the `release-please` workflow run logs.
- **A version file didn't update:** confirm its path/jsonpath in
  `release-please-config.json`. If `Cargo.lock` is not updated by the `rust`
  type for this layout, add a `generic` extra-file targeting the
  `name = "claude-fleet"` package entry's `version` line in `Cargo.lock`.
- **Wrong bump computed:** check the commit prefixes since the last tag.
```

- [ ] **Step 2: Verify it renders (no broken code fences)**

Run: `grep -c '```' docs/RELEASING.md`
Expected: an even number (all fences closed).

- [ ] **Step 3: Commit**

```bash
git add docs/RELEASING.md
git commit -m "docs: add RELEASING guide"
```

---

## Task 6: README refresh

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Read the current README to match its tone/structure**

Run: `cat README.md`
Expected: see existing content; preserve any sections not covered below.

- [ ] **Step 2: Add badges directly under the top-level title**

Insert immediately after the first `# ...` heading line:

```markdown
[![CI](https://github.com/martin-janci/claude-fleet/actions/workflows/ci.yml/badge.svg)](https://github.com/martin-janci/claude-fleet/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/martin-janci/claude-fleet?display_name=tag&sort=semver)](https://github.com/martin-janci/claude-fleet/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
```

- [ ] **Step 3: Add a "Releasing & docs" section near the end (before any license footer)**

```markdown
## Releasing & documentation

- Versioning and changelog are automated via release-please — see
  [`docs/RELEASING.md`](docs/RELEASING.md).
- **Control API reference:** [`docs/control-api-reference.md`](docs/control-api-reference.md)
  (generated from source) and [`docs/control-api.md`](docs/control-api.md) (guide).
- **Rust API docs (rustdoc):** published to GitHub Pages on each release —
  https://martin-janci.github.io/claude-fleet/
```

- [ ] **Step 4: Verify the links resolve to real paths**

Run: `ls docs/RELEASING.md docs/control-api-reference.md docs/control-api.md LICENSE 2>&1`
Expected: all four paths listed without "No such file" (LICENSE may not exist — if it errors, drop the License badge in Step 2).

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: README badges, release workflow, and doc links"
```

---

## Task 7: CLAUDE.md refresh

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Verify the real LOC and migration count to write accurate numbers**

Run:
```bash
echo "rust:    $(find src-tauri/src -name '*.rs' | xargs wc -l | tail -1 | awk '{print $1}')"
echo "frontend:$(find src -name '*.ts' -o -name '*.svelte' | xargs wc -l | tail -1 | awk '{print $1}')"
grep -oE '"00[0-9]' src-tauri/src/store.rs | sort -u | tail -3
```
Expected: rust ≈ 15,700; frontend ≈ 12,900; the highest migration id (use it in Step 3 instead of assuming `006`).

- [ ] **Step 2: Correct the LOC line in the "What this is" section**

Replace:
```
managing long-lived Claude Code sessions running in tmux across multiple
machines over SSH. ~5,100 LOC Rust, ~6,600 LOC frontend.
```
with (use the rounded numbers from Step 1):
```
managing long-lived Claude Code sessions running in tmux across multiple
machines over SSH. ~15,700 LOC Rust, ~12,900 LOC frontend.
```

- [ ] **Step 3: Update the "Status & known issues" section to note landed features and the release flow**

Replace the first sentence of that section:
```
Iterations 1–4a are landed (multi-host, accounts, cross-host sessions, prompt
transfer, async/events rework), plus the MCP control API.
```
with:
```
Iterations 1–4a are landed (multi-host, accounts, cross-host sessions, prompt
transfer, async/events rework), plus the MCP control API, background sessions,
the background reconcile tick, fleet_health roll-up, and the persistent session
event timeline (session_history).
```

- [ ] **Step 4: Add a "Releasing" subsection at the end of the "Build & test" section**

After the `cargo fmt --check` line's surrounding block in "Build & test", add:

```markdown
## Releasing

Versions and `CHANGELOG.md` are automated by release-please from Conventional
Commits — never bump versions by hand. See `docs/RELEASING.md`.

`docs/control-api-reference.md` is generated from the MCP tool router. After
editing any `#[tool(...)]` description or the `generate_handler!` list,
regenerate it or CI fails:

\```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
\```
```

- [ ] **Step 5: Verify edits applied and fences balanced**

Run: `grep -n 'release-please\|15,700\|REGEN_DOCS' CLAUDE.md && grep -c '```' CLAUDE.md`
Expected: the new lines appear; fence count is even.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: refresh CLAUDE.md — LOC, landed features, releasing"
```

---

## Final verification

- [ ] **Backend tests + lints pass (includes the staleness gate):**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all pass. (If `cargo` fails in a build script for missing Tauri system libs, that's the documented environment gap, not a code error — run on a Mac dev box.)

- [ ] **Frontend still green:**

Run: `pnpm run check && pnpm test`
Expected: same baseline as `main` (the pre-existing `localStorage is undefined` failures are unrelated).

- [ ] **Post-merge manual checks** (tracked in RELEASING.md): release-please opens its PR with all four version files changing; Pages source is set; trigger `docs` once via `workflow_dispatch`.
```
