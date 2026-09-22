# Release process unification — claude-fleet + property-management

**Date:** 2026-09-22
**Scope:** `martin-janci/claude-fleet` (+ its phone client `martin-janci/fleet-mobile`) and `martin-janci/property-management`
**Status:** audit complete, plan proposed — no code changed by this document

## Executive summary (SK)

1. **Vieme dnes povedať, aká verzia beží v produkcii? NIE.** V property-management je jediný záznam o produkčnom nasadení riadok v SQLite na `onyx.rlt.sk` s primárnym kľúčom `tag="main"`, ktorý sa pri každom deployi prepíše — bez verzie, bez commitu, bez digestu (`backend/servers/deploy-server/src/infra/store.rs:181`, `.../domain/release.rs:62-70`). V claude-fleet to vieme len pre hub (`/healthz` reportuje `CARGO_PKG_VERSION`), nie pre desktop ani pre telefón.
2. **Vieme povedať, ktorý image treba deploynúť? NIE.** Obe repo pinujú mutable tag: `deploy/hub/docker-compose.yml:26` (`fleet-hub:latest`) a `docker-compose.yml:69` (`${VERSION:-latest}`), pričom v property-management `latest` vzniká z branchu `dev` (`docker-build.yml:62` + default branch = `dev`).
3. **Sedia verzie Android / iOS / desktop? NIE, ani zďaleka.** Backend/web 0.3.218, Expo app 0.2.627 (hardcoded fallback, `frontend/apps/mobile/app.config.ts:239`), in-app konstanta 0.2.96 (`src/config/constants.ts:24`), KMP Android versionName 0.3.218 ale versionCode 518 (`mobile-native/androidApp/build.gradle.kts:17-20`), KMP iOS **prázdny string** (`Base.xcconfig:17-18`), fleet-mobile default 0.1.0 (`androidApp/build.gradle.kts:20-21`).
4. **Vieme si stiahnuť artefakt z GitHub Release? ČIASTOČNE.** claude-fleet áno (13/13 zelených runov, 11 assetov na release) — ale 4 z 10 releasov visia ako nepublikované drafty a 3 tagy release vôbec nemajú. property-management **nikdy nevytvorilo ani jeden GitHub Release** (`releases.totalCount = 0`) pri 218 verziách.
5. **Koreňová príčina v property-management:** `version-bump.yml` nedokáže pushnúť na chránený `dev` (GH006, required status checks) — `VERSION` je zamrznuté na 0.3.218 od 2026-06-16 a **žiadny carrier sa odvtedy nesynchronizoval**. Všetok „drift" je dôsledok tohto jedného výpadku.
6. **Koreňová príčina v claude-fleet:** pipeline funguje, ale končí manuálnou bránou (`draft: true`, `release.yml:85`) a nikde v CI sa nekontroluje, že šesť version-carrierov a git tag súhlasia.
7. **Mobil pre claude-fleet existuje** — `martin-janci/fleet-mobile` (KMP, Android + iOS, spec `docs/superpowers/specs/2026-09-18-fleet-mobile-design.md:5-7`) — a jeho verzia nie je nijako zviazaná s hubom, proti ktorému hovorí.
8. **Nič nie je podpísané** (macOS, Windows, Play, App Store) — vedome a zdokumentovane v claude-fleet, nevedome v property-management (`eas.json:83-85` stále obsahuje `APPLE_ID_PLACEHOLDER`).
9. **Blokujúcich zistení: 18.** Z toho 13 v property-management, 5 v claude-fleet.
10. **Cieľ plánu:** jeden zdroj verzie na repo, deterministická propagácia do všetkých carrierov vrátane `versionCode`/`CURRENT_PROJECT_VERSION`, digest-pinned image kontrakt, runtime `/version`, a jeden GitHub Release na tag so všetkými artefaktmi a checksumami. Wave 1 rieši výhradne traceability.

---

## Current state

### claude-fleet

Desktop-only Tauri 2 app (macOS + Linux) plus two server-side binaries (`fleet-hub`, `fleet-agent`) and a separate phone client repo. Releases are cut **manually** by `scripts/release.sh <X.Y.Z>` on a clean `main`; the script rewrites six version files, runs `cargo update`, prefills a CHANGELOG section from Conventional Commits, commits `chore(release): vX.Y.Z` and creates an annotated (SSH-signed) tag. It deliberately does not push (`scripts/release.sh:167-168`). Pushing the tag fires `release.yml` (draft release + three desktop legs + agent/hub tarballs + SHA256SUMS) and `hub-image.yml` (multi-arch ghcr push). release-please was removed in `1ff6912d` and does not exist on `origin/main`.

> **Warning about this checkout.** `/mnt/sda4/projects/github.com/martin-janci/claude-fleet` (branch `main`) is **1225 commits behind `origin/main`**; this worktree `.worktrees/jhkljh` is 1224 behind. Both still physically contain `release-please-config.json`, `.release-please-manifest.json` (`:2` → `"src-tauri": "0.2.0"`), `.github/workflows/release-please.yml` and a `docs/RELEASING.md:3` that claims release-please automation. Every claude-fleet statement below is read from `origin/main` via `git show origin/main:<path>` unless it explicitly says "stale checkout". Nothing in this stale tree should be edited; `git fetch && git pull` resolves it.

#### Version-carrier table — claude-fleet

| file | field | current value | bumped by | in sync? |
|---|---|---|---|---|
| `package.json:3` | `version` | 0.2.34 | `scripts/release.sh:13` | yes |
| `src-tauri/tauri.conf.json:4` | `version` | 0.2.34 | `scripts/release.sh:13` | yes |
| `src-tauri/Cargo.toml:3` | `[package] version` | 0.2.34 | `scripts/release.sh:13` | yes |
| `crates/fleet-hub/Cargo.toml:3` | `version` | 0.2.34 | `scripts/release.sh:13` | yes |
| `crates/fleet-proto/Cargo.toml:3` | `version` | 0.2.34 | `scripts/release.sh:13` | yes |
| `crates/fleet-agent/Cargo.toml:3` | `version` | 0.2.34 | `scripts/release.sh:13` | yes |
| `Cargo.lock:677,1355,1410,1429` | member `version` | 0.2.34 | `scripts/release.sh:68-109` (`cargo update -p`) | yes |
| `crates/fleet-core/Cargo.toml:3` | `version` | **0.1.0** | nothing — excluded by `scripts/release.sh:9-12` | deliberate exception, **unenforced** (no `publish = false`) |
| `Cargo.lock:1374-1375` | `fleet-core` | 0.1.0 | — | matches the exception |
| `CHANGELOG.md:11` | latest section | `## [0.2.34] - 2026-09-21` | `scripts/release.sh:111-152` | yes; **holes** at 0.2.27 and 0.2.5–0.2.20 |
| git tag `vX.Y.Z` | tag name | v0.2.34 | `scripts/release.sh:165`, pushed by hand | **no** — v0.2.27 never tagged; v0.2.5–v0.2.20 never tagged |
| ghcr `fleet-hub` tag | `type=semver,pattern={{version}}` (`hub-image.yml:26`) | derived from the **git tag** | `hub-image.yml` on `v*` | **never verified** against `crates/fleet-hub/Cargo.toml:3` |
| runtime `app_version` | `crates/fleet-core/src/app_version.rs:23-25` → `embedder.unwrap_or(env!("CARGO_PKG_VERSION"))` | embedder's version; **0.1.0 if `set()` never called** | compile time; set at `src-tauri/src/lib.rs:24`, `crates/fleet-hub/src/main.rs:158` | fragile |
| `crates/fleet-agent/src/conn.rs:474` | `agent_version` announced to hub | own `CARGO_PKG_VERSION` | compile time | stored (`crates/fleet-core/src/agent/registry.rs:31`), **never compared** |
| `fleet-mobile:androidApp/build.gradle.kts:20-21` | `versionCode` / `versionName` | `?: 1` / `?: "0.1.0"`, fed by `-PversionName=<tag>` `-PversionCode=<run number>` (`:18`) | fleet-mobile's own CI | **no link to claude-fleet at all** |
| *stale checkout* `package.json:3` | `version` | 0.2.4 | dead release-please | 30 releases behind |
| *stale checkout* `.release-please-manifest.json:2` | `src-tauri` | 0.2.0 | dead release-please | 34 releases behind |

#### Artifact table — claude-fleet

| artifact | built by | tag scheme | published to | traceable to version? |
|---|---|---|---|---|
| `claude-fleet_<v>_aarch64.dmg` / `_x64.dmg` | `release.yml:98-104` + `tauri-action@v0` (`:139-144`) | name from `src-tauri/tauri.conf.json:4` | GitHub Release asset | yes (but from config, **not** the tag) |
| `claude-fleet_<v>_amd64.deb` / `_amd64.AppImage` | `release.yml:105-107` (`--bundles appimage,deb`) | same | GitHub Release asset | yes |
| `claude-fleet_aarch64.app.tar.gz` / `_x64.app.tar.gz` | `tauri-action` default | **no version in filename** (identical bytes-differing names on v0.2.21/.23/.26/.32/.33/.34) | GitHub Release asset | **no** |
| `fleet-agent-<v>-<target>.tar.gz`, `fleet-hub-<v>-<target>.tar.gz` | `release.yml:166-206` → `scripts/package-linux-release.sh:101-120` | `${TAG#v}` (`:36`), cross-checked vs crate (`:63-77`) | GitHub Release asset via `scripts/upload-release-asset.sh` | yes |
| `SHA256SUMS` | `release.yml:239-292` → `scripts/merge-sha256sums.sh` | fixed name; **covers only the 4 Linux tarballs** (466 bytes = 4 lines on v0.2.23/.26/.32/.33) | GitHub Release asset | partial — 4 of 11 assets |
| `ghcr.io/martin-janci/fleet-hub:{version}` / `:latest` / `:sha-…` | `hub-image.yml:79-89` push-by-digest + `:161-165` manifest merge | `hub-image.yml:25-28` (semver from git tag, `latest` on `v*`, `type=sha`) | ghcr | yes — but **never referenced from the Release** |
| rustdoc site | `docs.yml:29-37` on `release: published` | none — single unversioned Pages site | GitHub Pages | **no** — and 6/6 runs have **failed** |
| Windows `.msi`/`.exe` | nothing (no `windows-latest` leg, `release.yml:98-107`) | n/a | nowhere | n/a |
| updater `.sig` / `latest.json` | nothing (no `tauri-plugin-updater` anywhere) | n/a | nowhere | n/a |
| fleet-mobile APK / IPA | `fleet-mobile` own CI | `-PversionName`/`-PversionCode` | `gh release list -R martin-janci/fleet-mobile` → **empty** | **no** |

### property-management

Monorepo: Rust backend (3 servers), 4+ web apps, an Expo/React-Native app (**PPT Management**, bundle `three.two.bit.ppt.management`) and a Kotlin-Multiplatform app (**Reality Portal**, bundle `three.two.bit.ppt.reality`) — two *different products*, not legacy duplicates (`CLAUDE.md:50-51`). `VERSION` at the repo root is the declared single source of truth; `scripts/bump-version.sh` writes it and calls `scripts/update-version.sh`, which fans it out. Default branch is `dev`; `main` is 2446 commits ahead / 3462 behind (diverged) and last version-bumped 2026-06-04.

> **The load-bearing break.** `version-bump.yml` pushes with `secrets.GITHUB_TOKEN` (`:29`) to a `dev` protected by required status checks. Every real bump is rejected: run 33848105439 log ends `remote: error: GH006: Protected branch update failed for refs/heads/dev.` / `remote: - 2 of 2 required status checks are expected.` / `ERROR: failed to push version bump after 3 attempts`. The 3-attempt retry (`:81-108`, added by `7da0b4eef`) only handles non-fast-forward. `VERSION` has therefore not moved since `adb748a61 2026-06-16 chore(version): bump to 0.3.218`, and every "success" since is the `.research/`-only skip path (`:37-55`). **All drift below is arrears, not churn.**

#### Version-carrier table — property-management (values from `origin/dev`)

| file | field | current value | bumped by | in sync? |
|---|---|---|---|---|
| `VERSION` | whole file | **0.3.218** | `version-bump.yml` → `bump-version.sh patch` — **currently failing (GH006)** | source of truth, frozen since 2026-06-16 |
| `backend/VERSION` | whole file | **0.2.239** | **nothing** — absent from `update-version.sh` and from the add-list | **no** — orphan, 979 patches stale |
| `backend/Cargo.toml:25` | `[workspace.package] version` | 0.3.218 | `update-version.sh:105` | yes |
| `backend/crates/*/Cargo.toml`, `backend/servers/{api,reality,accounting}-server/Cargo.toml` | `version.workspace = true` | 0.3.218 | inherited (11 members) | yes by construction |
| `backend/Cargo.lock` | 11 member entries | 0.3.218 | `update-version.sh:120-127` — **never `git add`ed** (`version-bump.yml:88-94`) | currently yes, by accident (rides in on feature PRs) |
| `backend/servers/deploy-server/Cargo.toml:13` | explicit `version` (crate is workspace-`exclude`d, `backend/Cargo.toml:22`) | 0.3.218 | `update-version.sh:134-139` — **never `git add`ed** | fragile; undocumented anywhere |
| `backend/servers/deploy-server/Cargo.lock` | `deploy-server` | 0.3.218 | `update-version.sh:140-150` — **never `git add`ed** | fragile |
| `package.json:4` (root) | `version` | **0.3.0** | `update-version.sh:98` writes it, `version-bump.yml:88-94` never stages it | **no** — 218 patches stale |
| `frontend/package.json` | *no `version` key* | — | `update_package_json`'s guard (`update-version.sh:86`) skips it | invisible; docs claim it is synced (`docs/git-workflow.md:142`) |
| `frontend/apps/admin-web/package.json` | *no `version` key* | — | nothing | **no** — a deployed product surface with no version identity |
| `frontend/apps/{mobile,ppt-web,reality-web}/package.json` | `version` | 0.3.218 | `version-bump.yml` glob | yes (mobile's is read by nothing) |
| `frontend/apps/accounting-web/package.json` | `version` | **0.1.0** | glob, but no bump has run | **no** |
| `frontend/packages/accounting-api-client/package.json` | `version` | **0.1.0** | glob | **no** |
| `frontend/packages/e2e/package.json:25` | `version` | **0.3.11** | glob; clobbered by squash-merge `c046c3f0e`, never re-synced | **no** |
| `frontend/packages/{admin-ui,api-client,dev-panel,reality-api-client,screen-map,shared,sitemap,ui-kit,vite-plugin-ppt-worktree}/package.json` | `version` | 0.3.218 | glob | yes |
| `frontend/apps/mobile/app.config.ts:239` | Expo `version` | **`config.version ?? '0.2.627'`** — and `app.json` has no `version` key, so the literal always wins | **nothing** | **no** — this is what ships to both stores |
| `frontend/apps/mobile/app.config.ts:260` | `ios.buildNumber` | `?? '1'` | EAS remote (`eas.json:5`, `:59`) | not traceable |
| `frontend/apps/mobile/app.config.ts:326` | `android.versionCode` | `?? 1` | EAS remote | not traceable |
| `frontend/apps/mobile/app.config.ts:442-443` | `runtimeVersion.policy: 'appVersion'` | resolves to **0.2.627** | derived from the frozen literal | OTA channel frozen forever |
| `frontend/apps/mobile/src/config/constants.ts:24` | `APP_VERSION` | **`'0.2.96'`** | **nothing** (its own comment at `:22` claims a bump script does) | **no** — this is what user feedback is stamped with (`src/onboarding/FeedbackManager.ts:270`) |
| `frontend/apps/mobile/src/config/constants.ts:29` | `BUILD_NUMBER` | `process.env.BUILD_NUMBER ?? '1'`; no workflow sets it | nothing | always `'1'` |
| `mobile-native/gradle.properties:16-17` | `app.versionName` / `app.versionCode` | 0.3.218 / **3218** | `update-version.sh:183-184` | **dead** — no Gradle file reads either (only `health-check.sh:257` greps for the key) |
| `mobile-native/androidApp/build.gradle.kts:30-31` | `versionCode` / `versionName` | **518** / 0.3.218 (+`-dev`/`-staging` suffix at `:52`/`:62`) | re-derived from `../VERSION` (`:9`) with `MAJOR*10000+MINOR*100+PATCH` (`:17-20`) | **no** — contradicts gradle.properties; **non-monotonic** (0.4.0 → 400 < 518) |
| `mobile-native/iosApp/Configurations/Base.xcconfig:17-18` | `MARKETING_VERSION` / `CURRENT_PROJECT_VERSION` | **`$(inherited)` → empty** (nothing assigns them anywhere: 5 grep hits repo-wide, all in these 2 files) | **nothing** — the comment at `:14` claiming `bump-version.sh` drives it is false | **no** |
| `mobile-native/iosApp/iosApp/Resources/Info.plist:19-22` | `CFBundleShortVersionString` / `CFBundleVersion` | empty strings at build time | the unset xcconfig | **no** — App Store Connect will reject the upload |
| `docs/api/typespec/main.tsp:32-33` | `@info(#{})` | **empty — no `version` key at all** | `update-version.sh:195` seds a pattern that matches nothing, then prints ✓ at `:197` | **no** — published OpenAPI carries no version |
| `README.md:3` | version badge | **0.2.201** | nothing | **no** |
| `docs/index.md:49` | "main.tsp … (v0.1.68)" | a number that exists in no file | hand-typed | **no** |
| `docker/.env.example:128`, `docker-compose.yml:69,123,170,191` | `VERSION` / image tag | `latest` | manual | not a version |
| `backend/servers/*/src/routes/health.rs` (`api-server` `:61`) | runtime `/health` `version` | `env!("CARGO_PKG_VERSION")` = 0.3.218 | compile time | **the one correctly wired runtime surface** |

#### Artifact table — property-management

| artifact | built by | tag scheme | published to | traceable to version? |
|---|---|---|---|---|
| `ghcr.io/martin-janci/ppt-api-server`, `ppt-reality-server` | `docker-build.yml` (matrix `:32`) | `docker-build.yml:56-62`: branch, pr, `{{version}}`, `{{major}}.{{minor}}`, bare sha, `latest` on default branch | ghcr | **no** in practice — no `v*` tag since 2026-06-04, so only `dev`/sha/`latest` exist |
| `ghcr.io/martin-janci/ppt-web`, `ppt-reality-web`, `ppt-admin-web` | `docker-frontend.yml` (`:34-39`) | `:60-64` — **no `{{major}}.{{minor}}`, no `event=pr`** (asymmetric with backend) | ghcr | **no** — and 599/683 runs failed; no success since 2026-06-16 |
| `ppt-accounting-server` image | **nothing** — `docker-build.yml:32` is `[api-server, reality-server]` only, while `backend/Cargo.toml` lists `servers/accounting-server` | n/a | nowhere | a shipped service with **no image at all** |
| `ghcr.io/martin-janci/ppt-caddy` | **no workflow/script** builds it (`docker/caddy/Dockerfile` exists but nothing references it) | `:latest`, hand-pushed | ghcr, by hand | **no** — yet `preflight` hard-requires the container (`release.rs:114`) |
| `ppt-frontend-dev:local` | hand-built only; the sole build command lives in `docs/superpowers/plans/2026-05-06-deploy-server-phase-0-1.md:313` | `:local` | local daemon (overridable via `PPT_FRONTEND_IMAGE`, `main.rs:139`) | **no** |
| prod-candidate record `{tag, images{4}}` | `release.yml:81-90` → `POST https://onyx.rlt.sk/api/release` | git tag verbatim **with the `v`** — metadata-action publishes **without** it | deploy-server SQLite | **structurally broken** (see F-P1) |
| auto-deploy record | `docker-build.yml:138-142` / `docker-frontend.yml:93-95` → `POST /api/deploy {"tag":"main"\|"dev"}` | branch name | one SQLite row keyed on `tag` (`store.rs:181-183`) | **no** — no version, no sha, no digest, no history |
| `android-debug-apk` (Reality Portal) | `mobile-native.yml:63-69` | fixed name, no version/sha | Actions artifact — and **matches zero files** (flavors nest one level deeper; `if-no-files-found` defaults to `warn`) | **no** |
| `ios-framework` (KMP `shared.framework`) | `mobile-native.yml:87-95` | fixed name | Actions artifact (14 MB) | **no** — it is a framework, never an app |
| Expo Android APK/AAB, iOS IPA (PPT Management) | `eas-build-android.yml:159-163`, `eas-build-ios.yml:154-158` (`--no-wait`) | EAS build UUID + remote build number | expo.dev — **0 of 156 runs have ever succeeded** (secrets gate `eas-build-android.yml:122-136`) | **no** |
| Play / App Store submission | `eas-build-android.yml:196-201`, `eas-build-ios.yml:191-205` (`eas submit --latest`) | store-assigned | never exercised; `eas.json:83-85` still `*_PLACEHOLDER`, `store-metadata/ios` absent, Play key file absent | **no** |
| signed Reality Portal AAB / `.xcarchive` | `scripts/build-android.sh`, `scripts/build-ios.sh` — invoked by **no workflow** | `RealityPortal-<env>.<ext>`, no version | nowhere | **no** |
| **GitHub Release + assets** | **nothing** — repo-wide grep for `gh release`/`softprops`/`upload-release-asset` across every workflow → 0 hits | n/a | **nowhere; `releases.totalCount = 0`** | **no** |

---

## Findings

Verdicts are the adversarial verifier's: **CONFIRMED** = all evidence reproduced; **CORRECTED** = substance stands, a stated fact was wrong (correction inline); **UNVERIFIED** = no verification pass ran.

### Blockers

**F-P1 — `release.yml` posts image refs with a `v` prefix that is never published** · property-management · CONFIRMED
`release.yml:76` `TAG="${GITHUB_REF#refs/tags/}"` keeps the `v`, and `:82` builds `$IMAGE_PREFIX/ppt-api-server:$TAG` → `:v0.3.58`. But `docker-build.yml:59` / `docker-frontend.yml:62` are `type=semver,pattern={{version}}`, publishing `0.3.58`. Neither workflow has `type=ref,event=tag` or `pattern=v{{version}}`. The only version-addressed release path names five refs that cannot resolve; a promote tears down the live colour and then 404s on pull.

**F-P2 — both image workflows paths-filter their tag trigger, so a release tag builds nothing** · property-management · CORRECTED
`docker-build.yml:3-11` and `docker-frontend.yml:3-12` put `branches`, `paths` and `tags: ['v*']` under one `push:` key. A `v*` tag pushed onto a commit already on `dev` carries an empty commit list, so `paths` can never match — the images `release.yml` polls for are **always** starved, not merely "may never be built". *Verifier correction:* the survey's `--limit 60` run-list window post-dates every tag by months, so "no tag-ref docker-build run" proves nothing and is dropped; the mechanism alone carries the finding.

**F-P3 — `release.yml` has never succeeded: 4 runs, 4 failures, last 2026-06-04** · property-management · CORRECTED
All four runs died at step 2 "Wait for backend + frontend image workflows" (`:20`, timeout at `:55-58`), with steps 3–4 skipped. *Verifier correction:* the claim that the `actions: read` fix (`:12-14`) was added after the last run is a **shallow-clone artifact** — `git show v0.3.58:.github/workflows/release.yml` already contains it at `:14`, and the file is byte-identical to `origin/dev`. So the wait step failed **with** the permission granted and with both image workflows green on v0.3.58/v0.3.1/v0.2.433. Real cause still unknown; logs expired (HTTP 410).

**F-P4 — zero GitHub Releases have ever existed** · property-management · CORRECTED
`releases.totalCount = 0` against `VERSION` 0.3.218; no root CHANGELOG (`git ls-tree -r origin/dev | grep -i changelog` → only `.claude/commands/changelog.md`); `docs/git-workflow.md:117` promises an unimplemented "publish" step. *Verifier correction:* "nothing in the repo can create one" is false — `.claude/commands/release.md:64,69,71` is a 98-line `/release` runbook that tags, pushes `main --tags` and calls `gh release create`. It contradicts `docs/git-workflow.md` on every point (operates on `main`, `git add -A` at `:58`, `chore(release):` prefix) and emits a dead `OWNER/REPO` compare link at `:85`. **Three mutually inconsistent release procedures now exist.**

**F-P5 — `version-bump.yml` cannot push to `dev`; `VERSION` frozen since 2026-06-16** · property-management · CONFIRMED (verifier-added root cause)
`version-bump.yml:29` uses `secrets.GITHUB_TOKEN`; `dev` requires 2 status checks a direct push can never satisfy. Run 33848105439: `remote: error: GH006: Protected branch update failed for refs/heads/dev.` The retry loop (`:81-108`) only handles non-fast-forward. `git log origin/dev --grep='chore(version)'` → 0 hits across 1498 commits. Every other property-management version finding is downstream of this.

**F-P6 — auto-deploy identifies the release by branch name** · property-management · CONFIRMED
`docker-build.yml:136,142` POST `{"tag":"main"|"dev"}`; `release.rs:52` expands it to `ppt-*:main`; `store.rs:181-183` upserts one row with `ON CONFLICT(tag)`. `domain/release.rs:62-70` has no version, no sha, no digest field. *Verifier addition:* `release.rs:154` writes `state: ReleaseState::Staging` **unconditionally**, even for `target: "prod"` (`:155`), so `current_release_for("prod", …)` cannot even find what the auto-deploy put on prod.

**F-P7 — `latest` is built from `dev`, and the "Production Deployment" compose defaults to `latest`** · property-management · CONFIRMED
Default branch is `dev`; `docker-build.yml:62` `type=raw,value=latest,enable={{is_default_branch}}`. `docker-compose.yml:1` `# Docker Compose for Production Deployment` with `${VERSION:-latest}` at `:69,123,170,191`; `docker/.env.example:128` `VERSION=latest`; `docker/README.md:23-31` is `cp docker/.env.example .env` + `docker compose up -d` with no version step. Following the documented production procedure deploys unstable dev.

**F-P8 — frontend images have failed every run since 2026-06-16 while backend keeps deploying** · property-management · CONFIRMED
`docker-frontend.yml` 20/20 most recent runs `failure` (newest 2026-09-08); last success 2026-06-16. `docker-build.yml` last success 2026-09-05 and its `trigger-deploy` (`:136`) keeps POSTing `{"tag":"dev"}`, pulling a June frontend against a September backend and API schema. Nothing notices: the frontend's own deploy job is gated `needs: build` (`:93-95`).

**F-P9 — a running frontend container cannot report its version** · property-management · CONFIRMED
`docker/nginx/ppt-web.nginx.conf.template:71,73` returns a literal `"OK\n"`. `docker/frontend/ppt-web.Dockerfile:46,47,51` accepts only `VITE_API_URL`/`VITE_WS_URL`/`VITE_API_DEFAULT`, matched by `docker-frontend.yml:77-80` — no version arg. `git grep -c LABEL origin/dev -- docker/` → **0**, so the only OCI labels are metadata-action's, whose `image.version` is the *branch name* on branch builds. `admin-web/package.json` has no version key either.

**F-P10 — iOS (Reality Portal) ships an empty `CFBundleShortVersionString`** · property-management · CONFIRMED
`Base.xcconfig:17-18` `MARKETING_VERSION = $(inherited)` / `CURRENT_PROJECT_VERSION = $(inherited)`; `Info.plist:19-22` interpolates both. Repo-wide grep → 5 hits, all in those two files; `project.yml` applies the xcconfigs at project (`:44-47`) and target (`:83-86`) level and sets only `SWIFT_VERSION` (`:60`); `scripts/build-ios.sh:130,153,166` pass no override. ASC rejects such an upload, and `Configuration.swift:102` shows the empty string in-app. `Base.xcconfig:14` actively lies: "driven by the repo-root VERSION file via scripts/bump-version.sh".

**F-P11 — Expo app version is a hardcoded 0.2.627 and it freezes the OTA channel** · property-management · CONFIRMED
`app.config.ts:239` `version: config.version ?? '0.2.627'`; `app.json` is exactly `{ "plugins": [...] }` with no `version`, so the literal always wins. `:442-443` `runtimeVersion.policy: 'appVersion'` therefore pins every OTA build to one eternal runtime bucket regardless of native ABI. `update-version.sh:7-14` names no Expo target.

**F-P12 — two contradictory Android `versionCode` formulas; the shipping one regresses** · property-management · CONFIRMED
`update-version.sh:63-69` computes `MAJOR*1e6+MINOR*1e3+PATCH` → 3218 into `gradle.properties:17`; `mobile-native/androidApp/build.gradle.kts:17-20` ignores it and computes `MAJOR*10000+MINOR*100+PATCH` → **518**, which is what lands in the APK. 0.4.0 → **400 < 518**, so the next minor bump is permanently unuploadable to Play; 0.5.18 also → 518 (collision). The `PATCH -gt 999` guard (`update-version.sh:58`) protects the formula nobody uses.

**F-P13 — no mobile binary ever reaches a GitHub Release, and EAS has never once succeeded** · property-management · CONFIRMED
`eas-build-android.yml` / `eas-build-ios.yml`: 156 runs each, 0 success, 156 failure. Step-level proof from run 34275490326: `9 Verify required EAS secrets -> failure`, everything downstream `skipped`; `gh secret list` returns only `AUTO_APPROVE_TOKEN` (no `EXPO_TOKEN`, no `EXPO_PROJECT_ID`). Both files claim at `:75` / `:66` that "the release workflow may also call this workflow after tagging" — `release.yml` contains no such call. `release.yml:81-86` is images-only; no mobile artifact, no mobile version.

**F-C1 — the audited claude-fleet checkout is 1225 commits stale and contains deleted release-please machinery** · claude-fleet · CONFIRMED / CORRECTED severity
`git rev-list --left-right --count HEAD...origin/main` → `0 1225` (worktree: 1224). Stale `.release-please-manifest.json:2` = `"src-tauri": "0.2.0"`; stale `docs/RELEASING.md:3-4` claims release-please automation while stale `CHANGELOG.md:8-9` says the opposite in the same tree; stale `CLAUDE.md:34-35` — the file fed to agents — repeats the release-please claim. `release-please.yml` is 33 runs / 0 success. *Verifier correction:* this is an **un-fetched local clone**, not a repository defect: `origin/main` has none of these files and `git merge-base --is-ancestor HEAD origin/main` → 0. `git fetch && git pull` resolves it in full; the stale blobs must **not** be edited.

**F-C2 — claude-fleet's mobile client exists in a second repo and is version-unlinked** · claude-fleet · CORRECTED (survey declared mobile nonexistent)
There is no Tauri mobile target (`git ls-tree -r origin/main | grep -iE 'src-tauri/gen|android|ios|gradle|xcode'` → no matches; `tauri.conf.json:29` `"targets": "all"` means desktop formats only). **But** `martin-janci/fleet-mobile` is a live public KMP repo with `androidApp/` and `iosApp/`, and its approved design and plan are checked into claude-fleet: `docs/superpowers/specs/2026-09-18-fleet-mobile-design.md:5-7` and `docs/superpowers/plans/2026-09-18-fleet-mobile.md:5`. Its carriers are `fleet-mobile:androidApp/build.gradle.kts:20-21` (`?: 1` / `?: "0.1.0"`, fed by `-PversionName`/`-PversionCode` at `:18`), wholly decoupled from 0.2.34, and `gh release list -R martin-janci/fleet-mobile` is empty. The user's version-sync ask **does** have a subject; the fix is cross-repo, not `scripts/release.sh`.

**F-C3 — releases strand as unpublished drafts** · claude-fleet · CORRECTED count
`release.yml:85` `draft: true` by design (`:9-11`), published by hand. *Verifier correction:* as of today v0.2.34 **is** published and Latest; **four** drafts remain — v0.2.31, v0.2.30, v0.2.29, v0.2.28 — each carrying the complete 11-asset set (~190 MB apiece). So users on those four versions cannot download the build they run, and nothing upstream is missing: the only gap is the human press.

**F-C4 — version 0.2.27 was bumped by a hand commit that bypassed `scripts/release.sh`** · claude-fleet · CONFIRMED
`fa8f3a30 chore: bump to 0.2.27` rewrote all six carriers + `Cargo.lock` (7 files, no CHANGELOG), is an ancestor of `origin/main`, and note the subject is `chore:` not `chore(release):`. No `v0.2.27` tag exists, so neither `release.yml` nor `hub-image.yml` fired: no bundles, no tarballs, no image, no release. The bump shows up as a product bullet at `CHANGELOG.md:102` inside 0.2.28's `### Changed`. It is the only non-`chore(release):` bump in the whole history — and nothing prevents or detects it.

**F-C5 — nothing in CI verifies that the six version carriers agree** · claude-fleet · CONFIRMED
`ci.yml` (132 lines, jobs `rust`/`hub-headless`/`frontend`) has no version step: `:35-37` fmt/clippy/test, `:46` `cargo deny check`, `:80` `hub-e2e.sh`, `:128-132` the frontend steps. The repo's only cross-check is `scripts/package-linux-release.sh:63-77` — tag vs `fleet-agent`/`fleet-hub` crates only, at release time, inside a job marked `release.yml:169` `continue-on-error: true`. `Cargo.toml` is 3 lines with no `[workspace.package] version`, so six carriers is exactly right and `.githooks/pre-commit:11-20` checks none of them.

### Major

**F-P14 — release manifest omits `admin-web`, and the deployer silently skips it** · property-management · CONFIRMED
`release.yml:81-86` has 4 keys; `docker-frontend.yml:38-39` builds a 5th image; `release.rs:63-66` (`build_staging_images`) knows `admin-web`. `blue_green.rs:95-97` turns the omission into a no-op — `admin_web_image: rel.images.get("admin-web").cloned()` with a comment blaming "older Release rows (pre #268)", which is false since the current `release.yml` always produces such rows — and `blue_green.rs:630` skips its Caddy route too. Contrast the strict `require(rel, …)` used for the other four at `:91-94`. Promoting a tagged candidate deploys prod **without** the super-admin control plane, silently.

**F-P15 — images are pulled by mutable tag with no digest resolution** · property-management · CONFIRMED
`blue_green.rs` `pull_image` → `CreateImageOptions { from_image: image.to_string(), ..Default::default() }` then `docker.create_image(Some(opts), None, None)` — no digest resolution, no platform, and no registry credentials (the 2nd arg is where auth goes; `docs/runbooks/deploy-server-prereqs.md` has no `docker login ghcr.io` step). `git grep RepoDigests` → 0 hits. Blue and green can land on different digests behind one tag.

**F-P16 — preflight validates postgres, caddy and networks but never the images** · property-management · CONFIRMED
`release.rs:106-116` builds the `PreflightContext`; `preflight.rs:80-91` checks env vars, the postgres container, the target DB, the network and caddy (+ `check_caddy_admin` `:140`). `grep -ic image preflight.rs` → **0**. The first registry contact is `pull_image` inside `deployer.deploy(&spec)` (`release.rs:149`), i.e. after the colour swap has begun — so F-P1 and F-P8 surface as a mid-deploy 404.

**F-P17 — the post-deploy health grace discards the version it is handed** · property-management · CONFIRMED
`api-server/src/routes/health.rs:61` returns `version: env!("CARGO_PKG_VERSION")` (= 0.3.218, kept in step by `update-version.sh:105`) on the very endpoint the blue/green wait loop calls. `deploy-server/src/infra/health.rs:47-59` reads only `resp.status().is_success()`; the body is never parsed. The one signal that would have caught the stale-frontend and branch-tag problems is already on the wire and thrown away.

**F-P18 — the deploy API is write-only** · property-management · CONFIRMED
Full route list (`router.rs:67-107`): GETs only for `/api/worktrees`, `/api/worktree/{name}`, `/api/logs/{name}`, `/api/audit`, `/health`; `/api/deploy`, `/api/wake/{target}`, `/api/release`, `/api/promote`, `/api/rollback` are all POST. `pmctl`'s `Status`/`List` (`pmctl.rs:41,43`) are worktree introspection. "Which image is on prod, and what would rollback land on" requires SSH + SQLite.

**F-P19 — no deploy record survives the host** · property-management · CONFIRMED
`docker-build.yml:138-142` fires a curl and discards the response, under `:98` `continue-on-error: true` — a deploy can silently not happen with a green workflow. No GitHub Deployment, no step summary, no committed log. *Verifier correction:* `docs/runbooks/` has **six** files plus `decisions/`, not four — none is a deploy log.

**F-P20 — `latest` and the tag vocabulary differ between backend and frontend images** · property-management · CONFIRMED
`docker-build.yml:56-62` emits six tag types; `docker-frontend.yml:60-64` emits four, missing `{{major}}.{{minor}}` **and** `event=pr`. `ppt-api-server:0.3` can exist while `ppt-web:0.3` never will. Both use `type=sha,prefix=`, stripping the one hint that a tag is a commit. *Verifier addition:* each workflow's only proof that the tag resolves is silenced — `docker-build.yml:86` / `docker-frontend.yml:85` end `imagetools inspect … 2>/dev/null || true`. And `workflow_dispatch` from any branch publishes that branch name as a deployable ghcr tag (`docker-build.yml:71` + `type=ref,event=branch`).

**F-P21 — no check anywhere asserts a carrier equals `VERSION`** · property-management · CORRECTED
`health-check.sh:142-151` validates only the *format* of `VERSION`, and `:255-258` merely greps that the string `app.versionName` exists in `gradle.properties` (a `check_warn`, not a failure). *Verifier corrections:* (a) `frontend/packages/e2e` is not a glob miss — it has a `version` key and would be rewritten; it is stuck because no bump has run (F-P5), after a squash-merge clobber; (b) the drift inventory is incomplete — `accounting-web` and `accounting-api-client` are both at **0.1.0** and appear in no survey table.

**F-P22 — `gradle.properties` version values are written by CI and read by nothing** · property-management · CONFIRMED
Repo-wide grep for `app.versionName|app.versionCode` → exactly 5 hits: `gradle.properties:16-17`, `health-check.sh:257`, `update-version.sh:183-184`. No Gradle file reads either property. Worse than no sync: `health-check.sh:258` reports `check_pass "gradle.properties has version info"` and `docs/git-workflow.md:143` certifies it, so an auditor reads 3218 and ships 518.

**F-P23 — `APP_VERSION = '0.2.96'` is what the app reports to the backend** · property-management · CONFIRMED
`src/config/constants.ts:24` (comment at `:22` claims a bump script maintains it; none does) and `:29` `BUILD_NUMBER = process.env.BUILD_NUMBER ?? '1'` with no workflow setting it. Both are constructed into `FeedbackManager` at module scope (`src/onboarding/FeedbackManager.ts:270`) and attached to every feedback/diagnostic payload (`:61`). Third independent mobile version literal in one app.

**F-P24 — TypeSpec API-version sync is a silent no-op** · property-management · CONFIRMED
`docs/api/typespec/main.tsp:32-33` is an empty `@info(#{})` with no `version` token anywhere in the file, so `update-version.sh:195`'s sed matches nothing while `:197` prints ✓ unconditionally and `:215` lists it as updated. `version-bump.yml:94` stages a file that never changes; `docs/git-workflow.md:144` advertises it. The published OpenAPI carries no version.

**F-P25 — docs, hook and CI disagree three ways about what bumps the version** · property-management · CONFIRMED
`README.md:129` "Auto-bumps patch on every commit via pre-commit hook"; `justfile:23` "(version bump, formatting checks)"; `scripts/pre-commit:12` "happens automatically via CI after merge to main"; `docs/git-workflow.md:126` and `CLAUDE.md:159` "after merge to `main`" — against `version-bump.yml:3-5` `branches: [dev]` with a `.research/` skip (`:37-55`). `CLAUDE.md:236-237` also misstates both `release.yml` (real trigger: tag `v*`) and `version-bump.yml` (real trigger: push to `dev`) as "Push to `main`", and omits both EAS workflows from the CI table entirely.

**F-P26 — the documented version-sync target list is wrong in four ways** · property-management · CONFIRMED
`docs/git-workflow.md:139-144` lists four targets. `update-version.sh` also writes root `package.json` (`:98`), `backend/Cargo.lock` (`:110-129`), `backend/servers/deploy-server/Cargo.toml` (`:134-139`) and its `Cargo.lock` (`:140-150`) — none staged by `version-bump.yml:88-94`. It names `frontend/package.json`, which has no `version` key and is a permanent no-op (`:86`). It credits `main.tsp` with a version that does not exist. It omits every mobile/Expo/iOS carrier.

**F-P27 — the only prod promote+rollback procedure is buried and linked from nothing** · property-management · CONFIRMED
`pmctl promote` / `pmctl rollback` appear only in `docs/runbooks/deploy-server-prereqs.md:345-363`, under `:261` `## 12. Prod-on-different-server (Phase 5 opt-in)`, in a document whose `:3` reads "One-time setup steps that must be completed before Phase 1 server can run." `grep -rn 'runbooks/' docs/index.md README.md CLAUDE.md docs/CLAUDE.md` → nothing links the directory. `grep -i -E 'git-workflow|runbook|release|deploy|version' docs/index.md` → **zero hits**, and `docs/git-workflow.md:54-76`'s release section ends at `git push origin "v$(cat VERSION)"` with no mention of images, candidates, promote or rollback.

**F-P28 — no CHANGELOG, no release notes source** · property-management · CONFIRMED
`docs/git-workflow.md:68-70` asks for `--body "Release notes…"` with no source. No CHANGELOG file exists. `/api/release` accepts `notes?` (`.claude/skills/ppt-deploy/references/api.md:17`) and `release.yml:88` never populates it. `docs/testability-and-implementation.md:1041` "Rollback plan documented and tested" and `:1043` "Release notes published" are both unchecked, and `docs/non-functional-requirements.md` has no release/deploy/rollback requirement at all (`grep '^#+.*(deploy|release|version|rollback)'` → nothing; "rollback" appears nowhere in 51 KB).

**F-P29 — the documented branch model has not been walkable for 3.5 months** · property-management · CONFIRMED
`CLAUDE.md:107-108` and `docs/git-workflow.md:23,56` declare `dev`→`main` releases only; `justfile:233-247` teaches branching **from `main`**. Reality: `main` at 0.3.58 (last version commit `bbf34b32b 2026-06-04`), `main..dev` = 1498, `dev..main` = 2446, status `diverged`.

**F-P30 — the manual build script points at a different GHCR namespace and clobbers `:latest`** · property-management · CONFIRMED
`docker/scripts/docker-build.sh:11,25` default `REGISTRY=ghcr.io/hanibalsk`, and `docker/scripts/synology-setup.sh:157` writes that into the generated `.env`, while `docker/.env.example:127` and the compose defaults are `ghcr.io/martin-janci`. `docker-build.sh:89-90` unconditionally adds a second `--tag $REGISTRY/ppt-$name:latest`, so one `--push` from a laptop overwrites the `latest` the "production" compose defaults to.

**F-P31 — release signing silently degrades to debug signing** · property-management · verifier-added, CONFIRMED
`mobile-native/androidApp/build.gradle.kts:89-93`: `if (keystoreFile.exists()) { signingConfig = signingConfigs.getByName("release") }` — **no `else { throw }`**. No workflow provisions a keystore, so any `bundleProductionRelease` in CI emits a debug-signed, minified AAB that Play rejects at upload, with a green build. Compounded by `:37-42`, where `storePassword`/`keyPassword` default to `""` rather than failing.

**F-P32 — two Android flavors carry a `-dev`/`-staging` suffix in `versionName`** · property-management · verifier-added, CONFIRMED
`build.gradle.kts:52` `versionNameSuffix = "-dev"` and `:62` `"-staging"`. A naive "all carriers equal `cat VERSION`" gate fails on those flavors; any parity check must compare the *stripped* name.

**F-P33 — `ppt-caddy` is required by preflight and built by no automation** · property-management · CORRECTED
`docs/runbooks/deploy-server-prereqs.md:30` `docker pull ghcr.io/martin-janci/ppt-caddy:latest    # built by Phase 0 task P0.5`; `release.rs:114` hard-requires the container. *Verifier correction:* `docker/caddy/Dockerfile` **does exist** (an xcaddy build, `:5-14`) and `deploy-server-prereqs.md:27` names it, so "cannot be reconstituted from the repo" is wrong. What is real: no workflow, justfile recipe or script builds or pushes it (grep → 0), so the published image is hand-made and drifts from a Dockerfile nothing rebuilds; and `:32` `cp deploy/caddy/Caddyfile.template …` points at a `deploy/` directory that does not exist (the file lives under `infra/caddy/`).

**F-C6 — `release.yml` never checks the git tag against `tauri.conf.json`** · claude-fleet · CONFIRMED
`release.yml:139-144` passes `tauri-action` only `releaseId` and `args` — no version input, no tag assertion. Desktop bundle names come from `src-tauri/tauri.conf.json:4`; tarball names come from `${TAG#v}` (`scripts/package-linux-release.sh:36`). They agree today by luck. `workflow_dispatch` (`release.yml:36`) plus `docs/RELEASING.md:47-49` ("pick the tag under Use workflow from") make the drift window a documented operation, and a single release could advertise three different versions with all checksums valid.

**F-C7 — the two `.app.tar.gz` assets carry no version in their filename** · claude-fleet · CONFIRMED
`claude-fleet_aarch64.app.tar.gz` is 12103979 / 12119015 / 12117215 / 11800508 / 11662184 / 10098895 bytes on v0.2.34 / .33 / .32 / .26 / .23 / .21 — identical names, different bytes. No rename step exists, and they are outside `SHA256SUMS` too (`package-linux-release.sh:128` hashes only `dist/*.tar.gz`, `out="dist"` at `:38`).

**F-C8 — `SHA256SUMS` covers 4 of 11 assets; no desktop bundle has a published checksum** · claude-fleet · CONFIRMED
466 bytes (4 lines) on every release that has it, while each release carries 11 assets. `scripts/merge-sha256sums.sh:9-12,77` actively enforces that only those four names may appear. The release-time cross-check is one-directional by construction (`release.yml:286-291` uses `comm -23`, catching only over-listing) and its job is `continue-on-error: true` (`:242`). `docs/RELEASING.md:101-104` concedes the scope ("confirm it lists all four tarballs"); `README.md:17-22` never mentions verification. These are also the unsigned artifacts users are told to de-quarantine by hand.

**F-C9 — the shipped hub deployment pins `:latest`** · claude-fleet · CONFIRMED
`deploy/hub/docker-compose.yml:26` `image: ghcr.io/martin-janci/fleet-hub:latest`; `docs/hub.md:25-27` says a `latest` tag "should exist"; `:39-42` warns a given `latest` may carry an amd64-only manifest. `grep -iE 'rollback|roll back'` over `docs/hub.md` → **0 hits**, and no heading covers upgrade or rollback. The `{{version}}` and `sha-` tags that would make a deployment traceable are published (`hub-image.yml:25-28`) and referenced by no deploy file or doc. *Verifier addition:* unlike `release.yml` (all four jobs gated `if: github.ref_type == 'tag'` at `:49,92,167,240`), `hub-image.yml`'s `meta`/`build`/`merge` have **no ref gate**, so a `workflow_dispatch` from any branch pushes a `sha-<sha>` image from unreleased code into the namespace operators pull from.

**F-C10 — the hub image's semver tag is never checked against its crate version** · claude-fleet · CONFIRMED
`hub-image.yml:26` derives the tag from the git ref; the binary reports `crates/fleet-hub/Cargo.toml:3` via `main.rs:158` and serves it at `serve.rs:781`. No comparison exists in all 185 lines of `hub-image.yml`, though `package-linux-release.sh:70-76` makes exactly that assertion for the tarball path. On a plain tag push the Dockerfile (`crates/fleet-hub/Dockerfile:14`, `--locked`) makes them agree by construction; the reachable drift is via `workflow_dispatch`, a re-tag, or the carrier drift F-C5 permits.

**F-C11 — three tags have no GitHub Release despite green runs and CHANGELOG entries** · claude-fleet · CORRECTED
v0.2.22, v0.2.24, v0.2.25 (and v0.2.0-phase2) have no release row, while `CHANGELOG.md:527,315,225` documents all three and `chore(release):` commits exist. *Verifier correction:* the mechanism is not a failed `create-release` — runs 35430601415 / 35479591153 / 35497802125 all concluded **success** with `create-release success`. The drafts were created with assets and then **deleted by hand**. So the needed guard is a post-hoc per-tag "a release exists with the expected assets" assertion, not create-release hardening. `docs/hub.md:233`'s `v=0.3.0` is a placeholder, not one of these versions.

**F-C12 — the agent/hub and checksum jobs are `continue-on-error`** · claude-fleet · CORRECTED
`release.yml:169` and `:242` both `continue-on-error: true` by design (`:234-238`), so a release missing its tarballs — or carrying a **partial** `SHA256SUMS` that verifies and looks authoritative — shows a green run; `docs/RELEASING.md:98-101,114-119` puts the burden on a human checking per-job badges. *Verifier correction:* v0.2.21's 6-asset shape is **not** an instance of this — `agent-hub-binaries` did not exist at that tag (added by `494f371f`); run 35347096627 has only 4 jobs. The hazard is latent: all 13 `release.yml` runs are green and every release from v0.2.23 on carries the full 11 assets.

**F-C13 — no code signing on any platform, and no Windows leg** · claude-fleet · CONFIRMED
`release.yml:17-19` states the absence explicitly, `:27-31` names the `APPLE_*` secrets to add, `:70` bakes the "damaged and can't be opened" explanation into every release body, `docs/RELEASING.md:142-144` and `README.md:24` document the workaround. The matrix (`:98-107`) has no `windows-latest` while `tauri.conf.json:29` says `"targets": "all"`. Honest and documented — but it is a hard prerequisite for fleet-mobile store distribution, and no decision trigger is recorded.

**F-C14 — no Tauri updater, so installed desktop versions drift forever** · claude-fleet · CONFIRMED
`git grep -iE 'updater|plugin-updater|createUpdaterArtifacts' origin/main -- '*.json' '*.toml' '*.yml'` → no output. `tauri.conf.json` is 38 lines with no `plugins`/`updater` key; `package.json:20-25` has no updater plugin. The only upgrade path is the manual download at `README.md:19-22`. Two machines that installed on different days run different versions indefinitely, with no signal.

**F-C15 — no documented compatibility matrix across desktop, hub, agent and phone** · claude-fleet · CONFIRMED
The only safety net is `fleet-proto`'s handshake, which is a *protocol* version and documented as having nothing to negotiate yet (`docs/hub.md:328-332,352-353`). The hub stores `agent_version` (`crates/fleet-core/src/agent/registry.rs:31,238`, surfaced at `service/hosts.rs:34`) and never compares it to its own. `fleet-core` is excluded from versioning (`scripts/release.sh:9-12`). With fleet-mobile live there are now **four** independently-versioned components and no stated supported pairing.

**F-C16 — `docs.yml` has failed on every published release** · claude-fleet · CORRECTED (verifier-added)
`docs.yml:3-5` fires on `release: published`. All **6** runs have concluded `failure` — v0.2.34 (35665462240), v0.2.33, v0.2.32, v0.2.26, v0.2.23, v0.2.21. So `README.md:169-170` ("published to GitHub Pages on each release") and `docs/RELEASING.md:200-202` are false for every release ever cut, **independently** of the draft question, and publishing a release is punished with a red check — plausibly part of why drafts accumulate.

**F-C17 — `scripts/release.sh` tags without checking that CI was green** · claude-fleet · **UNVERIFIED**
Preconditions at `:22-28` are: one arg, plain semver (`:19`), branch `main`, clean tree, tag absent, version differs. No `gh run` / status check. `docs/RELEASING.md:11` asks the operator to run the checks locally and `:19-21` to eyeball commit prefixes for the next version, while the script computes the CHANGELOG from those same commits (`:115-133`) but takes the version as a hand-typed argument. *No verification pass ran on this finding — treat the mechanism as read from the file and the consequence as inference.*

**F-C18 — `docs/RELEASING.md` step 1 inlines a partial CI mirror** · claude-fleet · CONFIRMED
`:15-16` omits `ci.yml:46` `cargo deny check`, `:132` `pnpm audit --audit-level=high` and `:80` `hub-e2e.sh`, while `scripts/ci-local.sh:2-3` exists precisely as the ordered mirror and is cited by `CLAUDE.md:29` and `README.md:133` but **not** by RELEASING.md (`git grep ci-local -- docs/RELEASING.md` → no match). The one place it matters is the pre-tag check.

**F-C19 — `create-release` reuses only drafts, so the documented re-run is unsafe after publication** · claude-fleet · verifier-added
`release.yml:62-64` scans for `r.tag_name === tag && r.draft`; a published release never matches, so the job falls through to `createRelease` with the same `tag_name` (`:80-87`). Yet `docs/RELEASING.md:116-119` instructs an unconditional re-run from the same tag, *after* step 5 has the operator publish. Re-running post-publication either 422s or creates a second release object; nothing warns.

**F-C20 — `prerelease: false` is hardcoded, so there is no pre-release channel** · claude-fleet · verifier-added
`release.yml:86` plus `scripts/release.sh:19`'s plain-semver assertion mean `0.3.0-rc.1` cannot be cut at all and a beta cannot be marked. For a project whose actual failure mode is unreviewed drafts piling up, an RC channel is the natural alternative and is structurally unavailable.

**F-C21 — nothing records which commit a release was built from** · claude-fleet · verifier-added
The release body (`release.yml:69-79`) links only CHANGELOG.md and README.md at the tag; no job writes the build SHA, toolchain or runner image into the notes or any asset, and `scripts/package-linux-release.sh:109-117` writes a README.txt with `$bin $version ($TARGET)` and no SHA. Meanwhile `hub-image.yml:28` publishes `type=sha`. The container path is commit-traceable and the desktop/tarball path is not; `docs/RELEASING.md` never states the asymmetry.

### Minor

**F-P34 — `mobile-native` CI builds only debug artifacts, and the APK upload matches zero files** · property-management · CONFIRMED
`mobile-native.yml:63-69` uploads `…/apk/debug/*.apk`, but three product flavors (`build.gradle.kts:47-48`) nest output under `apk/<flavor>/debug/`; `actions/upload-artifact`'s default `if-no-files-found: warn` keeps the job green. Verified: runs 35210781811 / 35210777818 / 35209425680 contain only `ios-framework`. The iOS job (`:87-95`) links a simulator debug framework and never compiles the app. No tag trigger, no `workflow_dispatch`, no release variant — and `CLAUDE.md:232` claims "Gradle build + unit tests". `scripts/build-android.sh` / `build-ios.sh` are invoked by no workflow.

**F-P35 — EAS staging builds never increment their build number** · property-management · CONFIRMED
`eas.json:5` `appVersionSource: "remote"`; `autoIncrement` appears exactly once, at `:59` in `production`. Development, simulator and staging have none, so every push-to-dev staging build reuses one remote build number on top of one frozen version. The authoritative number lives off-repo, so no tag or release note can ever assert "this commit produced build N".

**F-P36 — `eas submit` cannot run for either store** · property-management · CONFIRMED
`eas.json:83-85` `APPLE_ID_PLACEHOLDER` / `ASC_APP_ID_PLACEHOLDER` / `APPLE_TEAM_ID_PLACEHOLDER`; `:89` `metadataPath: "./store-metadata/ios"` — directory absent; `:93` `serviceAccountKeyPath` → a file absent (only `store-metadata/android/google-play-service-account.json.template` exists). The submit steps (`eas-build-android.yml:196-201`, `eas-build-ios.yml:191-205`) have no prerequisite gate, unlike the build-side secret gate — so it fails after a paid production build. The only disclosure is a workflow comment (`eas-build-ios.yml:36-37`).

**F-P37 — a deleted tag `v0.3.53` leaves runs but no ref** · property-management · CONFIRMED
`release.yml` run 26983010642 has headBranch `v0.3.53`; the tag is absent from local and remote lists. A non-semver `dispatcher-lock` ref also sits in `refs/tags` (local only). Tags are mutable and unaudited.

**F-P38 — `ppt-frontend-dev:local` build instructions live only in a plan document** · property-management · CORRECTED
`main.rs:139` `std::env::var("PPT_FRONTEND_IMAGE").unwrap_or_else(|_| "ppt-frontend-dev:local".into())` is the real default (the survey cited `docker.rs:559`, which is inside an `#[ignore]`d test at `:544-545`), so it **is** overridable per host. The only build command is `docs/superpowers/plans/2026-05-06-deploy-server-phase-0-1.md:313`; `tests/smoke.rs:12` is `#[ignore]`d on the image's absence; grep across `.github docker justfile scripts` → 0 hits.

**F-P39 — README badge and mobile prerequisites are stale; `justfile` has no release recipe** · property-management · CONFIRMED
`README.md:3` badge `version-0.2.201`; `README.md:39-49` documents only the KMP path and never mentions Expo/EAS; `:16-19` pins `pnpm 8+`. `justfile:178-200` has `version`/`bump-*`/`sync-version` and `:250-258` `ci`/`pr-ready`, with no release, tag, promote, rollback or "what's deployed" recipe — `just --list` therefore implies releasing is bumping a number.

**F-C22 — `fleet-core` frozen at 0.1.0 by convention, unenforced, and it can leak into a version report** · claude-fleet · CORRECTED
`crates/fleet-core/Cargo.toml:3` = 0.1.0 while siblings are 0.2.34; `scripts/release.sh:9-13` documents the exclusion and admits it is unenforced (no `publish = false`). `app_version.rs:23-25` `embedder.unwrap_or(env!("CARGO_PKG_VERSION"))` with only two `set()` callers, so a future embedder silently reports 0.1.0 through `fleet_health`, the diagnostics bundle and the usage User-Agent. *Verifier correction:* `release.sh:12` points readers at issue **#152, which is closed** and titled "v0.2.22 shipped the host agent (#136) with no changelog entry" — it mentions fleet-core only as a side note. **Nothing tracks this gap.**

**F-C23 — every crate declares MIT and there is no `LICENSE` file** · claude-fleet · CONFIRMED
`git ls-tree --name-only origin/main | grep -i license` → nothing. `package.json:19` and `:7` of all five crate manifests declare MIT. `scripts/package-linux-release.sh:82-86` correctly refuses to fabricate one and writes `License: MIT (see the repository)` (`:104`); `docs/RELEASING.md:77-81` documents that adding a root LICENSE fixes it with no workflow change. The sibling `fleet-mobile` repo already ships one. `README.md:172-175` understates it as two manifests, not six.

**F-C24 — the CHANGELOG jumps 0.2.21 → 0.2.4** · claude-fleet · CONFIRMED
`CHANGELOG.md:567` `## [0.2.21]` is immediately followed by `:1074` `## [0.2.4]` — sixteen versions (0.2.5–0.2.20) exist as bump commits with no section, no tag and no release. The header note at `:8-9` covers only "before 0.2.4". A reader cannot tell "unrecorded" from "never shipped", nor that artifact-bearing releases begin at 0.2.21.

**F-C25 — the rustdoc site is a single unversioned Pages deployment** · claude-fleet · CONFIRMED
`docs.yml:29-35` uploads `target/doc` with no version path; `:24` checks out with no `ref:`, so a `workflow_dispatch` documents a branch rather than a release; `concurrency: group: pages` (`:13-15`) means each deploy overwrites the last. `README.md:169-170` promises an unversioned URL. (Moot in practice — see F-C16.)

**F-C26 — release bodies are byte-identical boilerplate** · claude-fleet · CONFIRMED
`release.yml:69-79` emits a fixed quarantine paragraph plus a link to CHANGELOG.md at the tag. Proven, not eyeballed: with each node's own tag masked, 9 of 10 bodies hash to the same 643 bytes and v0.2.21 differs only in trailing whitespace. The real notes already exist one step earlier (`CHANGELOG.md:11-14`).

**F-C27 — docs index and getting-started contradict reality** · claude-fleet · CONFIRMED
`docs/README.md:11` "— versioning & changelog automation." vs `docs/RELEASING.md:3` "Releases are cut **manually**". `docs/getting-started.md:15` "No release has been published yet, so for now run claude-fleet from source." and `:26` "Once releases are published…" vs `README.md:17-22`'s full install section and 14 tags. `docs/RELEASING.md:197-198`'s only failure path is pre-push tag deletion — nothing covers F-C4's "bump landed on main, tag never pushed".

---

## Target design

### 1. Single source of truth and propagation

**claude-fleet:** `package.json:version` stays the source; `scripts/release.sh:13`'s `VERSION_FILES` stays the propagation mechanism, but becomes **verifiable**. The script gains `--list` (printing exactly the paths it writes) and CI gains a `version-consistency` job that reads that list plus the four `Cargo.lock` member entries, asserts byte equality, and on a tag additionally asserts `github.ref_name == "v" + <version>`. `crates/fleet-core` is the one declared exception: it gets `publish = false`, is named in an explicit allowlist the check reads, and `app_version::set` becomes non-optional (no `unwrap_or(CARGO_PKG_VERSION)` fallback) so 0.1.0 can never be reported as an app version.

**property-management:** `VERSION` stays the source. Three changes make it real: (a) `version-bump.yml` stops pushing directly to a protected `dev` — it opens an auto-merging PR with an app token, or `dev`'s ruleset gains a bypass for that identity (see Open question 1); (b) `update-version.sh --list` becomes the single carrier inventory, consumed by the workflow's `git add --pathspec-from-file=-`, by the drift gate and by the docs, so a newly added carrier cannot be silently dropped; (c) `backend/VERSION` is deleted, `main.tsp` gains a real `@info(#{ version: "…" })` with a post-condition (`grep -q` or fail), and every published app gets a `version` field, with genuinely private packages in an explicit allowlist so "unversioned" is a recorded decision.

**Mobile derivation — deterministic, one formula, no literals:**

| target | field | derivation from `X.Y.Z` |
|---|---|---|
| Android (both apps) | `versionName` | `X.Y.Z` verbatim; flavor suffixes allowed but stripped before comparison |
| Android (both apps) | `versionCode` | **`X*1_000_000 + Y*1_000 + Z`** — the `update-version.sh:63-69` formula. Monotonic for Y,Z ≤ 999 and int32-safe to X ≤ 2147. Gradle **reads** `app.versionCode` via `providers.gradleProperty(...)` with **no fallback**; the second formula at `build.gradle.kts:17-20` is deleted. |
| iOS (both apps) | `MARKETING_VERSION` | `X.Y.Z` — written into a generated, committed `Configurations/Version.xcconfig` that `Base.xcconfig` `#include`s |
| iOS (both apps) | `CURRENT_PROJECT_VERSION` | the same integer as `versionCode`, so Android and iOS build numbers are the same number |
| Expo | `version` | read from `VERSION` (or the app's own `package.json`) at config time, **no literal fallback** |
| Expo | `android.versionCode` / `ios.buildNumber` | `appVersionSource: "local"`, both set to the derived integer; `autoIncrement` removed |
| Expo | `runtimeVersion` | **not** `policy: 'appVersion'` — `policy: 'fingerprint'` (or a manual value bumped only on native changes), so OTA buckets track the native ABI |
| Expo in-app | `APP_VERSION` / `BUILD_NUMBER` | deleted as literals; read `Constants.expoConfig.version` and `Application.nativeBuildVersion` |

### 2. One version or several?

**claude-fleet: one version across desktop, hub, agent and proto — and an independent-but-pinned version for `fleet-mobile`.** The four in-repo components are released from one commit by one script and already agree; forcing them apart would buy nothing and lose the "same tag = same build" property the tarball checks rely on. `fleet-mobile` is a separate repo with its own store cadence (a store review cannot be held to a hub patch), so it keeps its own semver — but every fleet-mobile release records the **minimum hub version** it requires, and claude-fleet's `docs/RELEASING.md` publishes the supported pairing. *Trade-off:* two version lines to reason about, and the pairing table is hand-maintained until a CI check reads it.

**property-management: one repo version (`VERSION`) for backend + web, and independent-but-traceable versions for the two mobile products.** Backend and web ship together through one deploy and must agree — one number. Tying a store release to a number bumped by every backend patch would mean ~200 store versions a year with no user-visible change, and Reality Portal and PPT Management are *different products* on different cadences. So each mobile app gets its own `MAJOR.MINOR.PATCH`, and every mobile build records the repo `VERSION` and commit SHA as build metadata (in the release asset manifest and in the app's diagnostics payload), so a crash report maps to a backend generation. *Trade-off:* "are the versions in sync?" becomes "does this app's recorded backend generation match prod?" — a lookup rather than a string compare, which is why F-P17's runtime assertion and the release manifest matter. **This is the one place where the owner's literal ask ("the versions must match") is answered with a deliberate no; Open question 3 puts it to the owner.**

### 3. Image tagging contract

Every service image, in both repos, carries exactly this tag set — from **one shared tag block** (a reusable workflow or composite action), so backend and frontend are never addressable differently:

| tag | when | mutable? | may a deploy manifest pin it? |
|---|---|---|---|
| `X.Y.Z` | only on a `v*` tag | no | **this is the human-readable release tag** |
| `X.Y` | only on a `v*` tag | yes (moves within a minor) | no |
| `latest` | **only on a `v*` tag** — never on a branch push | yes | no |
| `sha-<full sha>` | every push (`prefix=sha-,format=long`) | no | yes, for debugging |
| `<branch>` | branch pushes only | yes | no — refused by the deployer |
| `@sha256:<digest>` | always resolved at release time | **immutable** | **yes — this is what manifests pin** |

Every Dockerfile gains `LABEL org.opencontainers.image.version`, `.revision`, `.source`, `.created`, fed by `--build-arg APP_VERSION` + `GIT_SHA`; the frontend images bake the same values into the bundle. `imagetools inspect` after push loses its `|| true`. The tag trigger is a separate, **unfiltered** `on.push.tags` entry so a `v*` tag always builds every image.

**"Which image do I deploy for version X" becomes one command:**
```
gh release view vX.Y.Z --json assets -q '.assets[]|select(.name=="release-manifest.json").url' | xargs curl -sL
```
returning `{version, commit, images: {"api-server":"ghcr.io/…/ppt-api-server@sha256:…", …}, mobile: {…}}` — the same file the deployer consumes.

### 4. Runtime version surface

Every deployed thing answers `GET /version` (or extends `/health`) with `{version, commit, built_at, image_digest}`:

- Rust servers already have the version (`api-server/src/routes/health.rs:61`, `fleet-hub/src/serve.rs:781`) — add `commit` + `built_at` from build args.
- Frontend images: a `/version` nginx location serving a JSON baked at build time, replacing `"OK\n"` (`ppt-web.nginx.conf.template:73`).
- `deploy-server`: `GET /api/releases?target=prod` and `GET /api/targets/{t}` returning version, commit, per-service digest, state and `promoted_at`.
- `deploy-server`'s `grace_check` (`health.rs:47-59`) **parses the body** and fails the promote unless `version` and `commit` equal the intended ones, for all services.
- Desktop/mobile apps display their own version next to the newest published release, so a stale install is visible even without an updater.

### 5. What every release publishes as GitHub Release assets

| asset | claude-fleet | property-management |
|---|---|---|
| desktop bundles (`.dmg`, `.deb`, `.AppImage`, `.app.tar.gz`) | yes — **every filename versioned** | n/a |
| server binaries tarballs | `fleet-agent`, `fleet-hub` per target | n/a |
| `release-manifest.json` (version, commit, image digests, mobile build ids) | hub image digest | all service digests + both mobile apps |
| Android `.aab` + `.apk` | via `fleet-mobile`'s own release, cross-linked | Reality Portal signed AAB; PPT Management AAB |
| iOS | a TestFlight/App Store build record (id, version, build number) — IPAs are not distributable outside the store | same, both apps |
| `SHA256SUMS` | **covering every asset on the release**, verified by a job that enumerates the release's actual assets and fails on any gap | same |
| release notes | that version's CHANGELOG section **verbatim in the body** | same, generated from Conventional Commits |

A final, **non-`continue-on-error`** `verify-release` job gates publication on the complete expected asset set being present and checksummed. Per-leg isolation is kept (one arch failing must not cancel another), but the *release* is not declared complete until it contains everything it promises.

### 6. How a release is triggered, and who bumps what

**Do not unify the tooling — the two repos have genuinely different shapes.** claude-fleet is a single-product desktop app with a curated changelog and a human who wants to eyeball bundles; property-management is a high-velocity monorepo with ~200 patch bumps a quarter and a blue/green deployer. release-please is dead in both (33/33 failures, deleted) and stays dead.

| | claude-fleet | property-management |
|---|---|---|
| who bumps | a human runs `scripts/release.sh X.Y.Z` on clean `main` | `version-bump.yml` patch-bumps `dev` continuously (via PR, so branch protection is satisfied); a human runs `bump-version.sh minor\|major` for a deliberate bump |
| version derivation | script derives the next version from Conventional Commits; a hand-typed value is an explicit, echoed override | unchanged |
| pre-tag gate | script refuses to tag unless CI on the exact `HEAD` is green, and unless the consistency check passes | a `release` workflow refuses unless the drift gate and all image builds are green on that commit |
| trigger | push `vX.Y.Z` (`--follow-tags`) | one `workflow_dispatch` "cut release" → tags `vX.Y.Z`, fast-forwards `main` (or `main` is dropped — Open question 2), builds every image, then publishes |
| draft? | **no** — published automatically once `verify-release` passes; RC channel available via `X.Y.Z-rc.N` + `prerelease: true` | no draft |
| deploy | operator pins `fleet-hub:vX.Y.Z` (or a digest) from the release manifest | `pmctl promote vX.Y.Z --target prod`, digests from the release manifest, refused unless every ref resolves |

---

## Work plan

Each task is independently shippable. **Wave 1 is traceability only** — it stops the bleeding without touching mobile version semantics or store credentials.

### Wave 0 — unblock the workspace

#### T0 — Refresh the stale claude-fleet checkout
- **Repo:** claude-fleet (local only)
- **Files:** none — `git fetch && git pull` on `/mnt/sda4/…/claude-fleet`, then rebase `.worktrees/jhkljh` onto `origin/main`
- **Change:** bring the working tree to `origin/main` so the dead release-please files and the stale `CLAUDE.md:34-35` / `docs/RELEASING.md:3` disappear. **Edit nothing in the stale tree.**
- **Acceptance:** `/usr/bin/git rev-list --left-right --count HEAD...origin/main` prints `0 0`; `git ls-tree -r --name-only HEAD | grep -i release-please` exits 1
- **Depends on:** nothing. **Blocks:** every claude-fleet task (F-C1)

### Wave 1 — traceability

#### T1 — Unfreeze property-management's version bump
- **Repo:** property-management · **Files:** `.github/workflows/version-bump.yml`
- **Change:** stop pushing directly to protected `dev`. Replace the 3-attempt direct push (`:81-108`) with either a GitHub App / PAT identity allowed to bypass, or a `chore(version)` PR with auto-merge. Keep the `.research/` skip (`:37-55`) and the `chore(version)` loop-guard.
- **Acceptance:** a push to `dev` that touches product code results in a landed `chore(version): bump to X.Y.Z` commit on `dev` within one CI cycle; `gh run list --workflow version-bump.yml --status failure --limit 20` is empty afterwards
- **Depends on:** nothing. **Blocks:** T2, T7 (F-P5)

#### T2 — One carrier inventory + a required drift gate (property-management)
- **Repo:** property-management · **Files:** `scripts/update-version.sh`, `.github/workflows/version-bump.yml`, new `.github/workflows/version-drift.yml`, `justfile`, `backend/VERSION` (delete), `docs/api/typespec/main.tsp`, `frontend/apps/admin-web/package.json`, `frontend/package.json`, `package.json`, `frontend/apps/accounting-web/package.json`, `frontend/packages/accounting-api-client/package.json`, `frontend/packages/e2e/package.json`
- **Change:** add `update-version.sh --list`; have the bump workflow stage exactly that list (`git add --pathspec-from-file=-`) and fail if the script left anything unstaged; delete `backend/VERSION`; give `main.tsp` a real `@info(#{ version })` and make the sed assert its own post-condition; add a `version` field to every published app; add `version-drift.yml` (required on PRs to `dev`) asserting every listed carrier equals `cat VERSION`, with an explicit allowlist for intentionally unversioned packages. Mobile carriers join this gate in Wave 3.
- **Acceptance:** `./scripts/update-version.sh && git diff --exit-code` is clean on a fresh checkout; `just check-version` exits 0; introducing a deliberate mismatch fails the new workflow
- **Depends on:** T1 (F-P21, F-P26, F-P24, F-C? none)

#### T3 — Version-consistency check in claude-fleet CI
- **Repo:** claude-fleet · **Files:** `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/release.sh`, `crates/fleet-core/Cargo.toml`, `crates/fleet-core/src/app_version.rs`
- **Change:** add `scripts/release.sh --list`; add a fast `version-consistency` job to `ci.yml` (every PR + push to `main`) asserting the six carriers and the four `Cargo.lock` member entries are byte-identical, with `fleet-core` in a declared allowlist; add the same job as a **gating, non-`continue-on-error`** first job in `release.yml` that additionally asserts `github.ref_name == "v" + version` before any build leg runs; add `publish = false` to `fleet-core`; make `app_version::set` mandatory so there is no 0.1.0 fallback.
- **Acceptance:** `gh workflow run ci.yml` green on `main`; hand-editing one carrier makes CI red; pushing a tag whose name disagrees with `tauri.conf.json` fails `release.yml` before `tauri-action` runs
- **Depends on:** T0 (F-C5, F-C6, F-C10, F-C22)

#### T4 — Complete and versioned release assets (claude-fleet)
- **Repo:** claude-fleet · **Files:** `.github/workflows/release.yml`, `scripts/package-linux-release.sh`, `scripts/merge-sha256sums.sh`, `docs/RELEASING.md`, `README.md`
- **Change:** rename the two `.app.tar.gz` assets to carry the version; replace the four-name whitelist in `merge-sha256sums.sh` with "every asset attached to this release"; add a final `verify-release` job (not `continue-on-error`) that enumerates the release's actual assets by id and fails unless the complete expected set is present and every asset has a `SHA256SUMS` line; record the build SHA in the release body and in each tarball's `README.txt`; point `README.md`'s install section at `SHA256SUMS`.
- **Acceptance:** `gh release view vX.Y.Z --json assets` shows 11 assets, all version-bearing, and `SHA256SUMS` has 11 lines; deleting one asset and re-running `verify-release` turns the run red
- **Depends on:** T3 (F-C7, F-C8, F-C12, F-C21)

#### T5 — Make a property-management tag actually build every image
- **Repo:** property-management · **Files:** `.github/workflows/docker-build.yml`, `.github/workflows/docker-frontend.yml`, new shared tag block (reusable workflow), `.github/workflows/release.yml`
- **Change:** split the tag trigger into its own unfiltered `on.push.tags` entry in both workflows; move both `tags:` blocks into one reusable workflow so backend and frontend publish an identical tag set (`{{version}}`, `{{major}}.{{minor}}`, `sha-<long>`, branch, `latest` **only on `v*`**); add `accounting-server` to `docker-build.yml`'s matrix; drop the `|| true` from both `imagetools inspect` steps; fix the `v`-prefix mismatch in exactly one place (strip in `release.yml:76`, or add `pattern=v{{version}}`) and assert every manifest ref resolves before registering a candidate.
- **Acceptance:** pushing a throwaway `v0.0.0-test` tag produces all six `ppt-*:0.0.0-test` images; `docker buildx imagetools inspect` succeeds for each ref in the posted manifest; no `:latest` is published by a branch push
- **Depends on:** nothing (F-P1, F-P2, F-P7, F-P20, and the missing accounting image)

#### T6 — Version identity in every image and at runtime (property-management)
- **Repo:** property-management · **Files:** `docker/backend/Dockerfile`, `docker/frontend/{ppt-web,reality-web,admin-web}.Dockerfile`, `docker/nginx/*.nginx.conf.template`, `.github/workflows/docker-{build,frontend}.yml`, `backend/servers/*/src/routes/health.rs`
- **Change:** pass `--build-arg APP_VERSION` + `GIT_SHA`; add the OCI labels to every Dockerfile; bake both into the frontend bundle and serve `GET /version` → `{version, commit, built_at}` instead of `"OK\n"`; extend the Rust `/health` payload with `commit` and `built_at`.
- **Acceptance:** `curl https://<host>/version` returns the released version and commit; `docker buildx imagetools inspect <ref> --format '{{json .Manifest}}'` shows `org.opencontainers.image.version` equal to the release version, not a branch name
- **Depends on:** T5 (F-P9)

#### T7 — Fix the frontend image build
- **Repo:** property-management · **Files:** `docker/frontend/ppt-web.Dockerfile` (and siblings), `pnpm-lock.yaml` as needed
- **Change:** diagnose and fix the 599/683 failure (candidates visible in-file: `ppt-web.Dockerfile:11` pins `pnpm@9.15.0`, `:29` runs a bare `pnpm install` with no `--frozen-lockfile` against a workspace now on `vite ^8` / `vitest ^4.1.7`, and the deps stage copies a fixed manifest list at `:18-27` that must be updated whenever a package is added). Then make staleness impossible to ignore: `docker-frontend.yml`'s build becomes a required check, and the deployer refuses a deploy whose five refs do not share one `org.opencontainers.image.revision`.
- **Acceptance:** `gh run list --workflow docker-frontend.yml --limit 5` all `success`; a hand-crafted mixed-revision deploy request is refused
- **Depends on:** T5, T6 (F-P8)

### Wave 2 — real releases

#### T8 — One release workflow for property-management
- **Repo:** property-management · **Files:** `.github/workflows/release.yml` (rewrite), new `.github/workflows/release-cut.yml`, `.claude/commands/release.md` (delete or reduce to a pointer), `justfile`, new `CHANGELOG.md`
- **Change:** one `workflow_dispatch`-triggered path: assert the drift gate + CI green on `HEAD`, bump, generate the CHANGELOG section from Conventional Commits, tag `vX.Y.Z`, build every image via `workflow_call`/`needs` (replacing the 15-minute poll at `release.yml:20-58`), resolve every image to a digest, **create a published GitHub Release** with the CHANGELOG section as the body and `release-manifest.json` + `SHA256SUMS` attached, then register the prod candidate from the manifest (5 services, generated from one shared service list). Delete the two contradictory procedures.
- **Acceptance:** `gh release view v<next>` shows the notes and both assets; `gh run list --workflow release.yml --limit 1` is `success`; `jq -r '.images["admin-web"]' release-manifest.json` is a `@sha256:` ref that `imagetools inspect` resolves
- **Depends on:** T2, T5, T7 (F-P3, F-P4, F-P14, F-P28)

#### T9 — Make the deploy record answerable and safe
- **Repo:** property-management · **Files:** `backend/servers/deploy-server/src/api/router.rs`, `.../api/release.rs`, `.../infra/{blue_green,preflight,health,store}.rs`, `.../bin/pmctl.rs`, `.github/workflows/docker-{build,frontend}.yml`
- **Change:** POST bodies carry `{version, commit_sha, images:{svc:"name@sha256:…"}}`; the store keys history on `(target, deployed_at)` with `tag` as an attribute and gains `version`, `commit_sha`, per-service digest; `state` is set from the actual target (fixing the unconditional `Staging` at `release.rs:154`); preflight gains an image-existence check reporting all missing refs together with resolved digests; `grace_check` parses the body and asserts version+commit per service; add `GET /api/releases?target=` and `GET /api/targets/{t}` plus `pmctl releases` / `pmctl whats-deployed`; drop `continue-on-error` from the deploy jobs and write a GitHub Deployment + step summary.
- **Acceptance:** `pmctl whats-deployed prod` prints version, commit and five digests matching `curl https://<prod>/version`; a deploy request naming a nonexistent tag is refused **before** any teardown; `curl $DEPLOY_URL/api/releases?target=prod` returns more than one historical row after two deploys
- **Depends on:** T6, T8 (F-P6, F-P15, F-P16, F-P17, F-P18, F-P19)

#### T10 — Close claude-fleet's manual gate
- **Repo:** claude-fleet · **Files:** `.github/workflows/release.yml`, `.github/workflows/docs.yml`, `docs/RELEASING.md`, `scripts/release.sh`
- **Change:** publish automatically once `verify-release` passes (draft only when a leg failed); support `X.Y.Z-rc.N` with `prerelease: true` (relaxing `release.sh:19`) as the review channel that replaces the draft habit; put the version's CHANGELOG section verbatim in the body; add a scheduled check that flags any `v*` tag without a complete release and any draft older than a few hours (catching F-C11's hand-deleted releases and F-C4's untagged bump); make `release.sh` refuse to tag unless CI on `HEAD` is green and derive the next version from the commits; fix `docs.yml` so publishing is not punished with a red run.
- **Acceptance:** pushing a tag yields a **published** release with 11 assets and real notes, and a green `docs.yml`; `gh release list` shows no drafts; the scheduled check opens an issue when a tag's release is deleted
- **Depends on:** T4 (F-C3, F-C16, F-C17, F-C19, F-C20, F-C11, F-C4)

#### T11 — Pin the hub image and document upgrade/rollback
- **Repo:** claude-fleet · **Files:** `deploy/hub/docker-compose.yml`, `docs/hub.md`, `.github/workflows/hub-image.yml`
- **Change:** pin `fleet-hub:vX.Y.Z` (or a digest) in the shipped compose with `latest` documented as a convenience only; add an "Upgrade and rollback" section with the exact commands and the one-liner that reads the running version back (`/healthz` already reports it, `docs/hub.md:119`); add `if: github.ref_type == 'tag'` to `hub-image.yml`'s jobs so a `workflow_dispatch` cannot push an unversioned image from unreleased code; gate the semver tag on the tag matching `crates/fleet-hub/Cargo.toml` via the shared check from T3; name the image digest in the release manifest.
- **Acceptance:** `grep image deploy/hub/docker-compose.yml` shows a pinned version; `docker compose pull` does not change the running version; a `workflow_dispatch` from a branch produces no ghcr push
- **Depends on:** T3, T10 (F-C9, F-C10)

### Wave 3 — mobile version parity

#### T12 — Generate the iOS version (property-management)
- **Repo:** property-management · **Files:** `scripts/update-version.sh`, new `mobile-native/iosApp/Configurations/Version.xcconfig`, `mobile-native/iosApp/Configurations/Base.xcconfig`, `scripts/build-ios.sh`, `.github/workflows/version-bump.yml`
- **Change:** write `MARKETING_VERSION` / `CURRENT_PROJECT_VERSION` into a generated, committed `Version.xcconfig` that `Base.xcconfig` `#include`s; add it to the `--list` inventory; fix the false comment at `Base.xcconfig:14`; add a post-archive assertion that `CFBundleShortVersionString` is non-empty and equals the intended version.
- **Acceptance:** `xcodebuild -showBuildSettings | grep MARKETING_VERSION` prints the version; `plutil -p` on the built app shows non-empty `CFBundleShortVersionString` and `CFBundleVersion`
- **Depends on:** T2 (F-P10)

#### T13 — One Android `versionCode` formula (property-management)
- **Repo:** property-management · **Files:** `mobile-native/androidApp/build.gradle.kts`, `scripts/update-version.sh`, `mobile-native/gradle.properties`, `scripts/health-check.sh`
- **Change:** delete the arithmetic at `build.gradle.kts:17-20`; read `app.versionCode` / `app.versionName` via `providers.gradleProperty(...)` with **no fallback** so a missing value fails the build; keep `MAJOR*1e6+MINOR*1e3+PATCH` as the one formula with its documented `MINOR,PATCH ≤ 999` limits and a matching guard; make `health-check.sh` compare the property *value* against `VERSION` instead of grepping the key; add a CI assertion that the assembled APK's `versionCode` is strictly greater than the last released one, comparing the `-dev`/`-staging`-stripped `versionName`.
- **Acceptance:** `aapt dump badging <apk> | grep versionCode` prints `3218` for `VERSION=0.3.218`; removing `app.versionCode` fails the Gradle build; a simulated `0.4.0` yields a code greater than 0.3.218's
- **Depends on:** T2 (F-P12, F-P22, F-P32)

#### T14 — Expo version from the source, literals deleted (property-management)
- **Repo:** property-management · **Files:** `frontend/apps/mobile/app.config.ts`, `frontend/apps/mobile/app.json`, `frontend/apps/mobile/eas.json`, `frontend/apps/mobile/src/config/constants.ts`, `frontend/apps/mobile/src/onboarding/FeedbackManager.ts`, a new `app.config.version.test.ts`
- **Change:** `app.config.ts` reads the version from `VERSION`/its own `package.json` with **no literal fallback**; `appVersionSource: "local"` with derived `versionCode`/`buildNumber` and `autoIncrement` removed; `runtimeVersion` switched off `appVersion`; `APP_VERSION`/`BUILD_NUMBER` derived from `expo-constants` / `expo-application`; a jest test asserts `expo config --json | jq -r .version` equals `cat VERSION`.
- **Acceptance:** `pnpm -F mobile exec expo config --json | jq -r '.version,.android.versionCode,.ios.buildNumber'` prints the version and the derived integer twice; the new test fails if a literal is reintroduced
- **Depends on:** T2, T13 (F-P11, F-P23, F-P35)

#### T15 — Mobile artifacts on the release (property-management)
- **Repo:** property-management · **Files:** `.github/workflows/mobile-native.yml`, `.github/workflows/eas-build-{android,ios}.yml`, `.github/workflows/release.yml`, `mobile-native/androidApp/build.gradle.kts`
- **Change:** fix the APK upload path to `…/apk/*/debug/*.apk` and set `if-no-files-found: error` on both uploads; add version+sha to artifact names; make release signing **fail** rather than silently fall back to debug (`build.gradle.kts:89-93`) and remove the empty-string password defaults; add a `v*`-tag path that builds a signed AAB and an iOS archive; have the release workflow drive the EAS production builds at the same version, wait for a terminal state, and attach the AABs plus a mobile section in `release-manifest.json` (version, build number, EAS build id, commit); add a "verify submit prerequisites" gate mirroring the build-side secret gate; fix the false "the release workflow may also call this workflow" comments.
- **Acceptance:** `gh release view v<next> --json assets` lists the AABs; `gh run download <mobile run>` yields a non-empty APK; a production release build without a keystore fails the job
- **Depends on:** T8, T12, T13, T14 (F-P13, F-P31, F-P34, F-P36, and F-P36's placeholders via Open question 4)

#### T16 — claude-fleet ↔ fleet-mobile version pairing
- **Repos:** claude-fleet + fleet-mobile · **Files:** `docs/RELEASING.md`, `docs/hub.md`, `crates/fleet-core/src/agent/registry.rs`, `fleet-mobile:androidApp/build.gradle.kts`, fleet-mobile release workflow
- **Change:** state the supported pairing between desktop app, hub image, `fleet-agent` and fleet-mobile (e.g. same minor, hub ≥ app) in `docs/RELEASING.md`; have the hub warn when a registered agent's `agent_version` falls outside it and surface the tuple in `fleet_health`; give fleet-mobile a real version source (not `?: "0.1.0"`) and a GitHub Release per store build recording the minimum hub version it requires; cross-link both release pages.
- **Acceptance:** `docs/RELEASING.md` contains the pairing table; `gh release list -R martin-janci/fleet-mobile` is non-empty; a deliberately old agent produces a hub warning
- **Depends on:** T0, T10 (F-C2, F-C15)

### Wave 4 — documentation and residual risk

#### T17 — property-management release documentation
- **Repo:** property-management · **Files:** new `docs/RELEASING.md`, new `docs/runbooks/release-and-rollback.md`, new `docs/runbooks/mobile-release.md`, `docs/index.md`, `docs/git-workflow.md`, `CLAUDE.md`, `README.md`, `scripts/pre-commit`, `justfile`, `docs/non-functional-requirements.md`, `docs/runbooks/deploy-server-prereqs.md`
- **Change:** see **Documentation deliverables** below. Includes correcting `CLAUDE.md:236-237`, `docs/git-workflow.md:126`, `scripts/pre-commit:12`, `README.md:3,129`, `docs/index.md:49`, and the `deploy/caddy/` → `infra/caddy/` path at `deploy-server-prereqs.md:32`.
- **Acceptance:** `grep -iE 'release|rollback|deploy|version' docs/index.md` is non-empty; a doc check fails when the CI table names a trigger that disagrees with `.github/workflows/`
- **Depends on:** T8, T9, T15 (F-P25, F-P26, F-P27, F-P29, F-P33, F-P39)

#### T18 — claude-fleet documentation and license
- **Repo:** claude-fleet · **Files:** `docs/README.md`, `docs/getting-started.md`, `docs/RELEASING.md`, `CHANGELOG.md`, new `LICENSE`
- **Change:** `docs/README.md:11` names the real mechanism; `getting-started.md:15,26` leads with the download path; `RELEASING.md` step 1 becomes one line (`scripts/ci-local.sh`) and gains the "bump landed, tag never pushed" recovery plus the desktop-vs-container commit-traceability note; the CHANGELOG header states exactly which version ranges never shipped and that artifact-bearing releases begin at 0.2.21; add a root MIT `LICENSE` (the packaging script already picks it up verbatim).
- **Acceptance:** `git grep -c ci-local docs/RELEASING.md` ≥ 1; `ls LICENSE` succeeds and a fresh tarball contains it; `grep 'No release has been published' docs/getting-started.md` exits 1
- **Depends on:** T0, T10 (F-C18, F-C23, F-C24, F-C27)

#### T19 — Signing, Windows and the updater decision
- **Repo:** claude-fleet (+ property-management for stores) · **Files:** `.github/workflows/release.yml`, `src-tauri/tauri.conf.json`, `package.json`, `docs/RELEASING.md`
- **Change:** record a decision with a trigger, not a standing caveat: obtain `APPLE_*` + a Windows certificate and enable the documented `env:` passthrough (`release.yml:27-31`), add a `windows-latest` leg, and wire `tauri-plugin-updater` with signed update artifacts — or state explicitly that unsigned GitHub Releases are the only channel and qualify `"targets": "all"`.
- **Acceptance:** either signed/notarized assets plus a `latest.json` on the release, or a dated decision record in `docs/RELEASING.md` naming the condition that would change it
- **Depends on:** T10 (F-C13, F-C14) · **Gated on Open question 4**

#### T20 — Tag hygiene and history
- **Repos:** both · **Files:** repository rulesets, `.github/workflows/*`
- **Change:** tag protection making `v*` immutable once pushed; a failed release leaves the tag plus a visible failed/prerelease object rather than being deleted; non-release refs (`dispatcher-lock`) move out of `refs/tags`; a scheduled check asserts tag set == release set in both repos.
- **Acceptance:** deleting a pushed `v*` tag is refused; the scheduled check is green with no missing releases
- **Depends on:** T8, T10 (F-C11, F-P37)

---

## Documentation deliverables

| doc | repo | status | must answer |
|---|---|---|---|
| `docs/RELEASING.md` | property-management | **create** | the single source of truth; the complete carrier table (generated from `update-version.sh --list`); the one command that cuts a release; the image tag contract incl. the `ppt-` prefix / service-key asymmetry; what lands on the GitHub Release; how to find the digest live per target; how to roll back |
| `docs/runbooks/release-and-rollback.md` | property-management | **create** | tag → images → candidate → promote → verify → rollback, end to end, with the exact `pmctl` invocations lifted out of `deploy-server-prereqs.md:345-363`; what `rollback --target=prod` will land on and how to check first |
| `docs/runbooks/mobile-release.md` | property-management | **create** | for both products: which workflow builds what, which profile ships to which track, where version and build number come from, how they tie to the release tag, who presses submit, and what is currently blocked (credentials) |
| `docs/index.md` | property-management | **fix** | add a "Release & Operations" section linking `docs/RELEASING.md`, every runbook, and the mobile doc; drop the invented `main.tsp (v0.1.68)` at `:49`; regenerate or CI-check so a new runbook cannot be orphaned |
| `docs/git-workflow.md` | property-management | **fix** | correct the bump trigger (`dev`, not `main`) at `:126`; the complete carrier list at `:139-144`; continue the release section past `git push origin "vX.Y.Z"` or hand off to the runbook; state the branch model actually in force |
| `CLAUDE.md` | property-management | **fix** | correct `:236` (`release.yml` → tag `v*`) and `:237` (`version-bump.yml` → push to `dev`); add rows for `eas-build-android.yml`, `eas-build-ios.yml` and `deploy-server.yml`; correct `:232`'s "unit tests" claim for `mobile-native.yml`; correct `:159` |
| `README.md` | property-management | **fix** | generate or drop the `:3` badge; delete the false pre-commit bump claim at `:129`; name both mobile stacks at `:39-49` with links to their release docs; refresh the `:16-19` prerequisites |
| `docs/non-functional-requirements.md` | property-management | **extend** | release cadence, deploy-downtime expectation, rollback objective (how fast, by whom, verified how), version-parity requirement across surfaces, and how many app versions the API must support |
| `CHANGELOG.md` | property-management | **create** | one generated section per released version, reproduced in the release body and in the candidate's `notes` |
| `.claude/commands/release.md` | property-management | **delete/reduce** | must not describe a third, `main`-based procedure; reduce to a pointer at `docs/RELEASING.md` |
| `docs/RELEASING.md` | claude-fleet | **fix** | step 1 → `scripts/ci-local.sh`; the app/hub/agent/fleet-mobile pairing table; the "bump landed, tag never pushed" recovery; the desktop-vs-container commit-traceability asymmetry; the RC channel; when the re-run instruction is unsafe (published release) |
| `docs/hub.md` | claude-fleet | **fix** | an Upgrade & Rollback section: pin `vX.Y.Z`, roll back to the previous tag, and the one command that reports the running image digest + version |
| `docs/README.md`, `docs/getting-started.md` | claude-fleet | **fix** | `:11` names the manual mechanism; getting-started leads with the download path and stops claiming no release exists |
| `CHANGELOG.md` header | claude-fleet | **fix** | which version ranges were pre-tooling bumps that never shipped, and that tagged artifact-bearing releases begin at 0.2.21 |
| `LICENSE` | claude-fleet | **create** | MIT text, backing six manifests that already declare it |

---

## Open questions for the owner

1. **How should `version-bump.yml` be allowed to write to `dev`?** A GitHub App / PAT with a ruleset bypass is one line of config but weakens branch protection for that identity; an auto-merging `chore(version)` PR keeps protection intact but costs a CI cycle per bump (~200/quarter). **Recommendation: the auto-merging PR.** The bump is not urgent, and a bypass identity is exactly the kind of exception that later gets reused for something that should not bypass checks.

2. **Is `main` still meant to be property-management's release branch, or is it abandoned?** It is 2446 ahead / 3462 behind `dev`, last version-bumped 2026-06-04, and the `justfile` teaches branching from it while the docs forbid it. **Recommendation: drop `main`.** Release from `dev` by tag, delete the branch (or make it a fast-forward-only mirror of the last release), and fix `justfile:233-247`. A dev→main PR with thousands of commits in both directions is not a reviewable release.

3. **Should the two mobile products share the repo `VERSION`?** Forcing Reality Portal's store version onto a number bumped by every backend patch means ~200 store versions a year with no user-visible change. **Recommendation: no — per-app semver, with the repo `VERSION` and commit recorded as build metadata** (see Target design §2). This is the one deliberate deviation from the literal ask, and it needs your yes.

4. **Are there Apple Developer and Google Play accounts, and an Expo project?** `eas.json:83-85` still holds `APPLE_ID_PLACEHOLDER`, `store-metadata/ios` does not exist, and `gh secret list` has no `EXPO_TOKEN`/`EXPO_PROJECT_ID` — which reads as "never enrolled" rather than "credentials rotated". **Recommendation: answer this before Wave 3 T15.** If no accounts exist, T15 degrades to "build and attach signed artifacts to the GitHub Release" and store submission is explicitly out of scope until enrolment; the same answer gates claude-fleet's T19 signing decision.

5. **Which of the two mobile products ships first?** The Expo app has all the release scaffolding and all the recent commits but zero successful builds; the KMP app has green CI and a working versionCode path but no iOS version and no release build. **Recommendation: Expo/PPT Management first** (it is the live product by commit activity), with Reality Portal's iOS version fixed in T12 regardless, because an empty `CFBundleShortVersionString` is a latent build-breaker either way.

6. **Are the four stranded claude-fleet drafts (v0.2.28–v0.2.31) deliberate or forgotten?** **Recommendation: publish all four, then land T10.** The assets are complete and checksummed; leaving them unpublished means four versions users cannot download. If any of them is known-bad, delete the tag *and* record why in the CHANGELOG rather than leaving a draft.

7. **Should claude-fleet ship a Windows build?** `tauri.conf.json:29` says `"targets": "all"` and the matrix has no `windows-latest` leg, so no `.msi` is ever produced and no doc says so. **Recommendation: either add the leg or qualify the config comment** — an unqualified "all" is a promise the pipeline does not keep.

8. **Is `docker-compose.yml` ("Production Deployment") live anywhere, or is it dead documentation?** The blue/green deployer on `onyx.rlt.sk` also deploys prod. **Recommendation: decide and delete the loser.** If compose is live (Synology?), F-P7 is an active incident; if it is dead, it is a docs cleanup — and today nobody can tell which.
