# Releasing claude-fleet

Releases are cut **manually** with `scripts/release.sh`: it is the single
source of truth for version bumps and the changelog. Never edit the version
fields by hand. Pushing the tag it creates triggers the
[GitHub release job](#github-release-job), which builds the desktop bundles
and attaches them to a draft release for the owner to publish.

## Steps

1. Make sure `main` is green locally (mirror `.github/workflows/ci.yml`):

   ```bash
   git checkout main && git pull --ff-only
   (cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
   pnpm install --frozen-lockfile && pnpm run check && pnpm run test && pnpm run build
   ```

2. Pick the next version from the Conventional Commits since the last tag
   (`git log --oneline $(git describe --tags --abbrev=0)..HEAD`): `feat` → minor,
   `fix`/`perf`/`refactor`/`docs` → patch, `feat!`/`BREAKING CHANGE` → major.

3. Run the script:

   ```bash
   scripts/release.sh 0.3.0
   ```

   It refuses to run on a dirty tree, off `main`, or if `v0.3.0` already exists.
   When it pauses, open `CHANGELOG.md`, polish the generated section (the
   bullets are raw commit subjects), save, and press Enter.

4. Push the release commit and tag:

   ```bash
   git push origin main --follow-tags
   ```

5. Wait for the `release` workflow to finish (it starts on the tag push), then
   open the draft release it created, check the attached bundles, paste the
   CHANGELOG section into the notes if you want them inline, and **Publish**.
   See [GitHub release job](#github-release-job) below.

## GitHub release job

`.github/workflows/release.yml` runs on `push` of any `v*` tag — i.e. the tag
`scripts/release.sh` creates, pushed in step 4 — and on `workflow_dispatch`.
Both jobs are gated on `github.ref_type == 'tag'`: when dispatching by hand,
pick the tag under "Use workflow from"; dispatching from a branch is a no-op.
Runs are serialised per tag (`concurrency: release-<tag>`).

A small `create-release` job creates **one draft release** named
`claude-fleet vX.Y.Z` (or reuses an existing draft for that tag on a re-run)
and passes its id to three `tauri-apps/tauri-action` build legs, which attach
their bundles to it:

| Runner | Target | Assets |
|--------|--------|--------|
| `macos-latest` | `aarch64-apple-darwin` | `.app.tar.gz`, `.dmg` |
| `macos-latest` | `x86_64-apple-darwin` | `.app.tar.gz`, `.dmg` |
| `ubuntu-24.04` | native x86_64 | `.AppImage`, `.deb` |

Nothing is visible until the owner reviews the assets and clicks Publish.
Publishing fires `docs.yml` (`release: published`), which rebuilds the rustdoc
site. If a leg fails, fix and re-run the workflow from the same tag; the
existing draft is reused and its assets replaced.

### Signing caveat

**Nothing is code-signed or notarized.** There is no Apple Developer ID for
this project yet, so `release.yml` deliberately contains no signing step
(ad-hoc signing would only fake provenance). Until that changes:

- macOS: Gatekeeper blocks the downloaded app on first launch. Right-click the
  app and choose **Open**, or clear the quarantine flag:

  ```bash
  xattr -d com.apple.quarantine /Applications/claude-fleet.app
  ```

- Linux: the AppImage and `.deb` are unsigned, which is normal for those
  formats. Mark the AppImage executable (`chmod +x`) before running it.

When a Developer ID exists, add the `APPLE_*` secrets documented at
https://v2.tauri.app/distribute/sign/macos/ and pass them via `env:` on the
`tauri-action` step; nothing else in the workflow needs to change.

## What the script touches

| File | Change |
|------|--------|
| `package.json` | `"version"` |
| `src-tauri/tauri.conf.json` | `"version"` |
| `src-tauri/Cargo.toml` | `version =` under `[package]` |
| `src-tauri/Cargo.lock` | via `cargo update -p claude-fleet` (no dependency changes) |
| `CHANGELOG.md` | new `## [X.Y.Z] - YYYY-MM-DD` section under the header, bullets from `git log <last-tag>..HEAD` grouped `feat` → Added, `fix` → Fixed, `docs` → Documentation, everything else → Changed; plus a `[X.Y.Z]: …/releases/tag/vX.Y.Z` link reference at the bottom |

Then it commits `chore(release): vX.Y.Z` and creates the annotated tag
`vX.Y.Z`. It prints the push command but **does not push**.

## Verifying before you tag

`RELEASE_DRY_RUN=1 scripts/release.sh 0.3.0` edits the files and stops (no
`cargo update`, no commit, no tag) so you can inspect `git diff`. Discard with
`git checkout -- .` when done.

After a real run, before pushing:

```bash
git show --stat HEAD              # exactly 5 files: 3 version files, Cargo.lock, CHANGELOG.md
grep -n '"version"' package.json src-tauri/tauri.conf.json
grep -n '^version' src-tauri/Cargo.toml
git tag -n1 v0.3.0
```

If something is wrong: `git tag -d v0.3.0 && git reset --hard HEAD~1`, fix,
re-run.

## Docs workflow

`.github/workflows/docs.yml` builds rustdoc and deploys it to GitHub Pages on
`release: published` — i.e. when the owner publishes the draft that the
release job created. It can also be triggered by hand via `workflow_dispatch`
on the Actions tab.

## Generated docs

`docs/control-api-reference.md` is **generated** from the live MCP tool router —
do not edit it by hand. After changing any `#[tool(...)]` description or the
`generate_handler!` command list, regenerate and commit it, or CI fails:

```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
```

## Conventional Commits

Commit subjects drive both the version choice and the changelog grouping:

| Prefix      | Bump  | Changelog section |
|-------------|-------|-------------------|
| `feat:`     | minor | Added             |
| `fix:`      | patch | Fixed             |
| `docs:`     | patch | Documentation     |
| `perf:` / `refactor:` / `chore:` / `ci:` / `test:` / other | patch or none | Changed |

A `feat!:` / `fix!:` or a `BREAKING CHANGE:` footer means a **major** bump.
