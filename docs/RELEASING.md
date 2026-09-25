# Releasing claude-fleet

Releases are cut **manually** with `scripts/release.sh`: it is the single
source of truth for version bumps and the changelog. Never edit the version
fields by hand. Pushing the tag it creates triggers the
[GitHub release job](#github-release-job), which builds every asset, attaches
them to a draft release, checksums all of them, and **fails the run unless
the release is complete** — then the owner publishes it.

Two files decide what a release *is*, and neither has a second copy anywhere:

| file | decides |
|------|---------|
| `scripts/release.sh --list` | the six version carriers a bump writes (see [What the script touches](#what-the-script-touches)) |
| `scripts/release-assets.sh` | the build legs and the exact asset filenames the release must carry |

`scripts/check-version-consistency.sh` reads the first and is run by both
`ci.yml` and `release.yml`; `scripts/verify-release.sh` reads the second and
is the last job of `release.yml`. If you add a build target or rename an
asset, edit `scripts/release-assets.sh` and nothing else — the workflow
matrices, the packaging script's output names and the release gate are all
generated from it.

## Steps

1. Make sure `main` is green locally (mirror `.github/workflows/ci.yml`):

   ```bash
   git checkout main && git pull --ff-only
   cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
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

5. Wait for the `release` workflow to finish (it starts on the tag push). Its
   last job, `verify-release`, goes red if anything is missing — a green run
   means the draft carries all 11 assets and a `SHA256SUMS` that covers every
   one of them. Open the draft, check the assets, paste the CHANGELOG section
   into the notes if you want them inline, and **Publish**. See
   [GitHub release job](#github-release-job) below.

6. Release the phone app under the same version:

   ```bash
   scripts/release-mobile.sh 0.3.0
   ```

   It tags [fleet-mobile](https://github.com/martin-janci/fleet-mobile)'s
   `main` head `v0.3.0` — refusing if claude-fleet's own `v0.3.0` is not on
   GitHub yet, if fleet-mobile's CI has not passed on that commit, or if
   fleet-mobile already has `v0.3.0` somewhere else — and that tag starts
   fleet-mobile's `release.yml`: test, build, sign with the release key,
   publish a release with the APK. The claude-fleet release body already
   links to it. See [The phone app](#the-phone-app) below.

## The phone app

The Android app lives in its own repository and is built there, never here.
What ties the two together is the version: every claude-fleet `vX.Y.Z` has a
fleet-mobile `vX.Y.Z` cut from whatever fleet-mobile `main` was at the time,
and the app reports that version in its Settings screen.

- **Signing.** fleet-mobile's `release.yml` signs with a dedicated release
  key held in its four `ANDROID_*` Actions secrets and refuses to build
  without them. The keystore and its password are also in the owner's macOS
  keychain (`fleet-mobile-release-keystore-base64`,
  `fleet-mobile-release-keystore-password`) — the only other copy. Lose both
  and no future APK can update an installed one.
- **Debug builds** install as a separate app, `dev.claudefleet.mobile.debug`
  ("fleet-mobile debug"), because they carry the building machine's debug key
  and Android will not update a release-signed app with it.
- **versionCode** is fleet-mobile's release-workflow run number, so it only
  grows; `versionName` is the tag without the `v`.
- **iOS** is not released: there is no signing identity for it yet.

## GitHub release job

`.github/workflows/release.yml` runs on `push` of any `v*` tag — i.e. the tag
`scripts/release.sh` creates, pushed in step 4 — and on `workflow_dispatch`.
Every job is gated on `github.ref_type == 'tag'`: when dispatching by hand,
pick the tag under "Use workflow from"; dispatching from a branch is a no-op.
Runs are serialised per tag (`concurrency: release-<tag>`).

The jobs, in order:

| job | what it does | may it fail without failing the run? |
|-----|--------------|--------------------------------------|
| `version-consistency` | `scripts/check-version-consistency.sh --expect-tag` — the six carriers, the four `Cargo.lock` entries and this tag must all agree | **no** — everything else `needs:` it |
| `plan` | turns `scripts/release-assets.sh` into the two build matrices | **no** |
| `create-release` | creates (or reuses) the one draft, records the build commit in its body | **no** |
| `build` | three `tauri-action` legs; then renames the macOS updater bundle to carry the version | **no** |
| `agent-hub-binaries` | two native Linux legs → four tarballs | yes (`continue-on-error`) |
| `checksums` | downloads every asset on the release, hashes it, uploads `SHA256SUMS` | yes (`continue-on-error`) |
| `verify-release` | the release is complete and fully checksummed | **no — this is the gate** |

### Why some legs are allowed to fail and the release is not

Per-leg isolation is deliberate: `fail-fast: false` on both matrices, and
`continue-on-error: true` on `agent-hub-binaries` and `checksums`, so one
arch failing never cancels another, delays the desktop bundles or blocks the
draft. That isolation is about *build legs*. It does not extend to the
*release*: `verify-release` carries no `continue-on-error`, enumerates the
release's **actual** assets by numeric id, and fails the run unless

1. every asset `scripts/release-assets.sh assets <version>` declares is there,
2. nothing unexpected is attached, and
3. `SHA256SUMS` has a line for every other asset and names nothing that is not.

So a failed leg no longer hides behind a green checkmark: the run goes red at
the end, in one place, with an `::error::` naming each missing asset. Fix the
leg and re-run the workflow from the same tag — every asset is reproducible
from it, so a re-run is always a complete fix, and the existing draft is
reused with its assets replaced.

### The assets

11 per release, every one version-bearing:

| asset | built by |
|-------|----------|
| `claude-fleet_<v>_aarch64.dmg`, `claude-fleet_<v>_aarch64.app.tar.gz` | `build`, `macos-latest` / `aarch64-apple-darwin` |
| `claude-fleet_<v>_x64.dmg`, `claude-fleet_<v>_x64.app.tar.gz` | `build`, `macos-latest` / `x86_64-apple-darwin` |
| `claude-fleet_<v>_amd64.deb`, `claude-fleet_<v>_amd64.AppImage` | `build`, `ubuntu-24.04` |
| `fleet-agent-<v>-x86_64-unknown-linux-gnu.tar.gz`, `fleet-hub-<v>-…` | `agent-hub-binaries`, `ubuntu-22.04` |
| `fleet-agent-<v>-aarch64-unknown-linux-gnu.tar.gz`, `fleet-hub-<v>-…` | `agent-hub-binaries`, `ubuntu-22.04-arm` |
| `SHA256SUMS` | `checksums` |

The two `.app.tar.gz` bundles are the exception that needed fixing:
`tauri-action` names them `claude-fleet_<arch>.app.tar.gz` — no version — so
v0.2.21 through v0.2.34 all published byte-different files under two
identical names. The bundler hard-codes that name, so `build` renames the
*uploaded asset* afterwards (`scripts/rename-updater-asset.sh`, a `PATCH` on
the releases API), leaving `tauri-action`'s own build and upload untouched.
The new name comes from `scripts/release-assets.sh`, the same table
`verify-release` checks against.

### fleet-agent and fleet-hub binaries

`agent-hub-binaries` runs in parallel with the three desktop legs (no
dependency on or from `build`). It builds `fleet-agent` and `fleet-hub`
`--release --locked` for `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu` on native `ubuntu-22.04` / `ubuntu-22.04-arm`
runners (22.04 rather than 24.04 so the binaries need only **glibc 2.35+** on
the host — see `docs/hub.md`), then `scripts/package-linux-release.sh` packs
each binary with a `README.txt` — which records the **exact commit** the
binary was built from, not just the tag — and this repository's root
`LICENSE` file *if one exists* (as of this writing it does not: a tarball
built today carries no LICENSE rather than a fabricated one; once the owner
adds a root `LICENSE`/`LICENSE.md`/`LICENSE.txt`, future tarballs pick it up
automatically, verbatim, with no workflow change). That script also asserts
that the tarballs it wrote are exactly the ones `scripts/release-assets.sh`
declares for its leg, so a rename cannot land in one place and not the other.

Each leg uploads its own two tarballs straight to the release by the numeric
release id `create-release` produced, never by tag — see
`scripts/upload-release-asset.sh`: a *draft* release is not resolvable by tag
through GitHub's REST API — plus its own per-target checksums as a workflow
artifact.

### SHA256SUMS

One asset, covering **every** asset on the release. It used to cover four:
the checksums job merged only the two `agent-hub-binaries` legs' local sums
files against a hard-coded four-name whitelist, so an 11-asset release
shipped a 466-byte `SHA256SUMS` in which no desktop bundle appeared at all.

Now the `checksums` job enumerates the release's assets by id, downloads them
(`scripts/download-release-assets.sh`), hashes what GitHub actually serves
(`scripts/merge-sha256sums.sh`), and uploads the result. The per-target files
the build legs produced are still used — as a **cross-check**: every digest
they and the release have in common must match, so a corrupted or clobbered
upload fails the job instead of being certified. That job needs `build` as
well as `agent-hub-binaries`, because it can only checksum what has already
been uploaded.

If a leg failed, `checksums` still publishes a correct *partial* file for
what the release does carry — deciding that a partial release is
unacceptable is `verify-release`'s job, not this one, so the gate lives in
exactly one place.

`scripts/upload-release-asset.sh` (both jobs) deletes an existing same-named
asset before re-uploading, so a re-run replaces assets — but that
delete-then-upload is **not atomic**: if the re-upload fails right after the
delete succeeded, the release is left with no asset of that name until the
job runs again. This only matters on a re-run over an asset that already
exists; a first-time upload cannot hit it. It fails loudly when it happens —
an `::error::` annotation naming the exact asset — and `verify-release` then
fails the run for the missing asset regardless.

### Verifying a download

What a user does, and what you should do once on the draft before publishing:

```bash
v=0.3.0
curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v$v/SHA256SUMS
curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v$v/fleet-hub-$v-x86_64-unknown-linux-gnu.tar.gz
sha256sum -c SHA256SUMS --ignore-missing     # macOS: shasum -a 256 -c …
```

`--ignore-missing` checks only the files you actually have; each one must
print `OK`. The release body records the commit the bundles were built from,
and each `fleet-*` tarball repeats it in its `README.txt` — so a downloaded
tarball can always be traced back to a tree, even if the tag later moves.

### Hub image

`.github/workflows/hub-image.yml` also runs on push of any `v*` tag: it
builds and publishes the `fleet-hub` container image for `linux/amd64` and
`linux/arm64` (independently of the desktop-bundle legs above and of the
draft-release review step) — two native per-arch jobs pushed by digest (no
QEMU), then a `merge` job combines whichever digests exist into the real
tags. **arm64 is best-effort**: its leg may fail without blocking the
image — `merge` still runs (`if: !cancelled()`) and publishes an amd64-only
manifest under the same tags, so amd64's own publication is never slowed or
blocked by arm64, and the *run stays green* (a `continue-on-error` leg's
failure never turns the run red). That degradation is still visible: a
`::warning::` annotation and a job-summary note both say arm64 failed and
the manifest is amd64-only. amd64 failing is different: nothing is
published, and `merge` fails for real (no `continue-on-error` on that leg
or on `merge` itself) so it stays visible rather than leaving a stale
`latest`. See `scripts/merge-hub-digests.sh` and the workflow file for the
exact rule and the image name/tag scheme.

### Signing caveat

**Nothing is code-signed or notarized.** There is no Apple Developer ID for
this project yet, so `release.yml` deliberately contains no signing step
(ad-hoc signing would only fake provenance). Until that changes:

- macOS: Gatekeeper blocks the downloaded app on first launch with
  *"claude-fleet.app is damaged and can't be opened"*. Copy the app to
  `/Applications`, then clear the quarantine flag:

  ```bash
  xattr -dr com.apple.quarantine /Applications/claude-fleet.app
  ```

  Right-click → **Open** and the **Open Anyway** button in System Settings are
  the bypass for a *signed but un-notarized* app; they are unreliable for an
  unsigned one, so point users at `xattr`. The user-facing version of this is
  in the README's *Installing a release build* section.

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
| `crates/fleet-hub/Cargo.toml` | `version =` under `[package]` |
| `crates/fleet-proto/Cargo.toml` | `version =` under `[package]` |
| `crates/fleet-agent/Cargo.toml` | `version =` under `[package]` |
| `Cargo.lock` | via `cargo update` scoped to every crate bumped above (`claude-fleet`, `fleet-hub`, `fleet-proto`, `fleet-agent` — derived from `VERSION_FILES`, not hard-coded); no dependency changes |
| `CHANGELOG.md` | new `## [X.Y.Z] - YYYY-MM-DD` section under the header, bullets from `git log <last-tag>..HEAD` grouped `feat` → Added, `fix` → Fixed, `docs` → Documentation, everything else → Changed; plus a `[X.Y.Z]: …/releases/tag/vX.Y.Z` link reference at the bottom |

Then it commits `chore(release): vX.Y.Z` and creates the annotated tag
`vX.Y.Z`. It prints the push command but **does not push**.

## Verifying before you tag

`RELEASE_DRY_RUN=1 scripts/release.sh 0.3.0` edits the files and stops (no
`cargo update`, no commit, no tag) so you can inspect `git diff`. Discard with
`git checkout -- .` when done.

After a real run, before pushing:

```bash
git show --stat HEAD                                       # exactly 8 files: 6 carriers, Cargo.lock, CHANGELOG.md
scripts/check-version-consistency.sh --expect-tag v0.3.0   # the same gate release.yml runs first
git tag -n1 v0.3.0
```

That check is the one both `ci.yml` and `release.yml` run: it reads the
carrier list from `scripts/release.sh --list`, takes `package.json` as the
source of truth, and refuses any disagreement between the six carriers, the
four `Cargo.lock` member entries and the tag name. Running it here means a
mismatch costs a local re-run instead of a bad tag.

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
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
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
