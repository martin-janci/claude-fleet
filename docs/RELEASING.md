# Releasing claude-fleet

Releases are cut **manually** with `scripts/release.sh`. GitHub Actions on this
repo is currently billing-blocked and the former CI release bot never ran,
so the script is the single source of truth for version bumps and the
changelog. Never edit the version fields by hand.

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

5. Create the GitHub Release from the tag (notes = the CHANGELOG section):

   ```bash
   gh release create v0.3.0 --title "v0.3.0" --notes-from-tag
   ```

   No binaries are attached — releases are tag + notes only.

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
`release: published`. It will start running again automatically once Actions
billing is restored; until then it can be triggered by hand via
`workflow_dispatch` on the Actions tab (once billing allows) or the rustdoc
site simply stays at its last published version.

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
