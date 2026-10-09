# Application updates: Update Channel, release manifest, desired state — design

Status: **design. Built: S1** (the `fleet-update` crate), **S4a** (the hub
side: `/update/*`, the `updater` token, the desired / observed tables,
`update_status` / `update_admin`, the channel refresh tick), **S2** (CI
signs `release-manifest.json` and writes the `stable` / `beta` channels on
`update-channels`), **S3** (`fleet-hub update check`, Git mode over
fleet-core's HTTPS client), **S4b** (`X-Fleet-Client`, `update:decision`, hub-e2e section U,
`update:changed`, `fleet_health.updates`, `update_status { target }`)
**S5** (`fleet-hub healthcheck --ready --json`, `fleet-hub backup`) **and S6**
(`fleet-updater`, the `auto-update` compose profile), **S2b** (`nightly.yml`)
**and S7** (the desktop updates itself). The rest is not built. What S2 and S4 still owe is
listed under S2b and S4b in §12.

It answers two findings of the release-process review
(`docs/superpowers/plans/2026-09-22-release-process-unification.md`):
**F-C14** ("no Tauri updater, so installed desktop versions drift forever")
and **F-C15** ("no documented compatibility matrix across desktop, hub, agent
and phone"). It also settles T19's updater half: we build one.

## Súhrn (SK)

- **Dva zdroje, jeden engine.** `UpdateChannel` je Rust trait v novom
  crate `fleet-update`. Má dve implementácie. `GitUpdateChannel` číta
  podpísaný kanálový dokument, ktorý CI publikuje z `main` a z tagov.
  `HubUpdateChannel` sa pýta Hubu. Rozhodnutie počíta tá istá čistá funkcia
  `decide()`, ktorú Hub používa pre celú flotilu.
- **Dve autority, dve veci.** Hub rozhoduje *politiku* (ktorá verzia, kedy,
  komu). Podpisový kľúč releasu rozhoduje *obsah* (čo je artefakt). Hub
  môže vyberať iba spomedzi podpísaných releasov a nikdy nevnesie vlastnú
  URL ani digest. Klient teda neurčuje politiku („nie je múdrejší než Hub“),
  ale obsah overuje.
- **Manifest sú dva dokumenty.** Nemenný `release-manifest.json` opisuje
  jeden release a jeho artefakty. Meniteľný `channels/<track>.json` nesie
  `recommended`, `minimum`, `mandatory`, `rollback` a `withdrawn`. Oba sú
  podpísané, kanálový navyše `sequence` a `expires_at` (ochrana proti
  replay a freeze).
- **Kompatibilita je v celých číslach protokolov, nie v semver.** Kód už má
  tri okná: `CONTRACT_REVISION`, `PROTO_VERSION` a peer `PROTO`. Manifest
  ich zverejní a Hub z nich počíta `update_required` a `client_too_new`.
  Semver minimum je iba politika navrch.
- **Update protokol nesmie nikdy prestať fungovať.** Nová route `/update`
  má vlastnú zamrznutú schému (`update_proto: 1`) a je vyňatá z
  `E_HUB_CONTRACT`. Klient, ktorého Hub pre nesúlad kontraktu odmieta, sa
  vďaka tomu stále dozvie, na čo sa má aktualizovať.
- **Docker je prvý cieľ a je to samotný Hub.** Jediný kontajner tohto repa
  je `fleet-hub`. `fleet-updater` je malý sidecar: zautomatizuje dnešný
  `deploy/hub/upgrade.sh`, pripína image na digest a pridáva automatický
  rollback vrátane obnovy DB zálohy, ak kandidát spustil migráciu.
- **Poradie MVP** zodpovedá handoveru, rozdelené na rezy S1–S9 (§12).
  Otvorené otázky pre vlastníka sú v §13.

## 0. Where we start

Nothing in the fleet updates itself today. What does exist:

| component | ships as | update today | compatibility gate today |
|---|---|---|---|
| hub (`fleet-hub`) | `ghcr.io/martin-janci/fleet-hub:<v>` (semver tag, `:latest` on stable, `sha-` tag; manifest digest written on the release by `record-digest`), or a tarball + `fleet-hub.service` | `deploy/hub/upgrade.sh <v>`: pull → `backup.sh` → stop → move `FLEET_HUB_TAG` → up → `fleet-hub healthcheck` → `fleet-hub --version` → `fleet_health.version`. On failure it **prints** the rollback. | serves `CONTRACT_REVISION = 5` in the `/events` ready frame |
| agent (`fleet-agent`) | tarball, installed by hand, `fleet-agent install` writes a systemd unit | none ("a deployed `fleet-agent` has no self-update", `fleet-proto` `Welcome` doc) | `PROTO_VERSION = 1`, `MIN_SUPPORTED_PROTO = 1`, close code 4001, `judge_proto` |
| desktop (Tauri) | dmg / `.app.tar.gz` / AppImage / deb / NSIS on the GitHub release | none (no `tauri-plugin-updater`, no signing key, no `latest.json`) | `MIN_HUB_CONTRACT = MAX_HUB_CONTRACT = 5` → `E_HUB_CONTRACT` |
| phone (fleet-mobile, KMP) | signed APK on fleet-mobile's release; iOS built on a Mac, not distributed | none (no `REQUEST_INSTALL_PACKAGES`) | `HubContract.kt` `MIN_HUB_CONTRACT = 0`, `MAX = 5`; an absent contract reads as Ok |
| hub↔hub | same binary | — | peer `PROTO = 1`, exact match, otherwise `incompatible` |

Also true today, and relied on below:

- One version number across the train. `scripts/release.sh` bumps six
  carriers, `release-mobile.sh` tags fleet-mobile with the same `vX.Y.Z`,
  and an `-rc.N` tag is a GitHub prerelease that never moves `:latest` or
  the compose pin.
- `scripts/release-assets.sh` `LEGS` is the one table of assets.
  `SHA256SUMS` covers every asset, and `verify-release` gates on it.
- **No build identity is compiled in.** There is no git SHA in any binary;
  `GIT_SHA` reaches only the tarball `README.txt`.
- **No client version is recorded.** `client_tokens` has no version or
  platform column. Agent versions live only in the in-memory
  `AgentRegistry` (`agent_status`).
- `/healthz` is unauthenticated and deliberately carries no version. There
  is no `/version` or `/readyz`.
- The store refuses a database migrated by a newer release
  (`store/schema.rs` downgrade guard). So **rolling back a hub across a
  migration means restoring the pre-upgrade backup**, never just an image
  swap.
- There are no Docker "worker" nodes in this repository. The handover's
  "Docker node" maps to the **hub container** now; "worker" maps to
  `fleet-agent`, a systemd binary. A future containerised worker fits the
  same updater (§8.6).

## 1. Fixed decisions (from the handover — not reopened)

| # | Decision |
|---|---|
| F1 | Standalone (no hub): Git/CI is the authority. The client consumes a CI artifact built from a specific commit and never compiles Git. |
| F2 | With a hub: the hub is the only authority for update policy. Clients do not watch Git or GitHub releases. A client is never smarter than the hub. |
| F3 | One abstraction, `UpdateChannel`, with a Git and a Hub implementation, chosen by "is a hub configured". The rest of the engine is shared. |
| F4 | A release manifest expresses current / recommended / minimum-supported / mandatory / rollback, not just "latest". It carries a compatibility matrix that includes protocol versions. |
| F5 | Docker: automatic, immutable image by digest (never `:latest` as an identity), application health check, automatic rollback. The updater is small and holds no fleet business logic. |
| F6 | One state machine for every platform. Platform code is only the last layer. |
| F7 | Source (`git` / `hub`) is separate from release track (`stable` / `beta` / `nightly`). |
| F8 | The update system is the first use of a general desired-state control plane (desired vs observed, converge), not an isolated auto-updater. |

MVP order (handover): manifest → `UpdateChannel` → Git channel → Hub channel
→ desired/observed → Docker → digests → health check → rollback → desktop →
mobile → rollouts, beta/nightly, windows.

## 2. Goal, scope, non-goals

**Goal.** Every fleet component can learn, from one protocol, whether it is
current, may update, must update, or is too new for its hub. The hub
container converges to the digest its hub policy names, and rolls itself back
without a person when the candidate is unhealthy. The Updates page shows the
fleet's versions against the desired ones.

**In scope:** the manifest and channel formats, and their signing in CI; the
`fleet-update` crate (types, `decide`, the state machine, `UpdateChannel`);
the `/update` wire protocol; the hub's policy, desired and observed tables and
admin tools; `fleet-updater` for the hub container; the desktop and Android
adapters; the Updates page.

**Non-goals (this design):**

- Configuration, plugins, feature flags or Claude Code runtime versions under
  the same desired-state tables. The tables are shaped for them (F8, §7.4),
  but only `artifact` desired state is built.
- A hub governing another hub over federation. A peer link carries no
  desired state.
- Store distribution: Play Store, App Store and TestFlight. They are
  adapters to add once there are accounts (release review, Open question 4).
- Signed OCI images (cosign). The digest in a signed manifest is the MVP
  guarantee (§10).
- Delta updates.

## 3. Decisions (this design)

| # | Decision | Why |
|---|---|---|
| U1 | **Two authorities.** The *release key* is the authority for content: which bytes, which digest, which compatibility. The *hub*, or Git/CI when standalone, is the authority for policy: which signed release, when, and for whom. Every artifact a client installs must be named by a manifest signed with the release key. A hub decision only *selects* one. | Reconciles F2 with "the updater must not blindly trust a URL from the hub". A compromised hub can at most choose among real releases, bounded by the publisher's signed floor (U6). |
| U2 | **The manifest is two documents.** A per-release `release-manifest.json` is immutable, one per version. A per-track `channels/<track>.json` is mutable, ordered by `sequence` and expires at `expires_at`. Both are minisign-signed with one release key. | "Recommended / mandatory / rollback / withdrawn" are decided *after* a release is cut, so they cannot live in an immutable file. `sequence` and `expires` block replay and freeze attacks (a lightweight version of TUF). |
| U3 | **Compatibility is protocol integers.** The manifest publishes each build's windows: the contract it serves and accepts, `agent_proto` min and max, `peer_proto`, and the DB schema. Semver floors are *policy* on top (`update.<component>.minimum`). | The code already decides compatibility on `CONTRACT_REVISION` / `PROTO_VERSION` / peer `PROTO`, never on semver. Publishing those windows *is* the compatibility matrix F-C15 asks for. It is generated, never hand-written. |
| U4 | **The update wire is frozen and exempt from the contract gate.** `POST /update/check` and `POST /update/report` carry `update_proto: 1`. They are outside MCP and never refused with `E_HUB_CONTRACT`. The desktop's `require_confirmed_contract` and the phone's `Refused` state do not apply to them. | The client that most needs an update is the one the contract gate refuses. It must still be able to ask what to install. |
| U5 | **One pure `decide()`** in `fleet-update` is used by `GitUpdateChannel` (with a local policy) and by the hub (with the fleet policy). The inputs are the channel doc, the policy, the caller's identity and its observed state. The output is a `Decision`. | F3: "the rest of the engine stays the same". A single decision function is testable against fixtures in Rust and mirrored in Kotlin only for display. |
| U6 | **The client obeys the hub within the publisher's signed bounds.** It refuses a target that is `withdrawn`, or below the channel's signed `minimum_supported` unless the target is the channel's signed `rollback`. Otherwise it does what the hub says, including "hold". | This is a security floor, not policy. It stops a compromised hub from pushing a known-vulnerable old release, and it is the only case where the client says no. |
| U7 | **The hub's own updater is local-autonomous for health.** `fleet-updater` takes *what* to run from the hub and decides *whether it is healthy* itself. Rollback needs no hub, because the hub is the thing being replaced. | The authority cannot judge its own candidate while it is down. The updater holds only mechanics (F5). |
| U8 | **The desktop installer is `tauri-plugin-updater`, fed by our decision.** We build its update response ourselves from `Decision.artifact`. The Tauri updater signature uses the same minisign release key. | This reuses a maintained installer for NSIS, AppImage and macOS `.app.tar.gz`. There is one key, not two. `.deb` has no Tauri installer, so it is notify-only. |
| U9 | **Tracks:** `stable` is `vX.Y.Z` tags, `beta` is `-rc.N` tags (today's RC channel), `nightly` is every green `main` commit, versioned `X.Y.Z-dev.N.g<sha7>`, where `X.Y.Z` is the *next* patch and `N` is the commit count since the last tag. | Keeps semver ordering (`dev.17` < `dev.18` < `rc.1` < final), and Docker tags stay legal: `+build` is already refused. This is F1's "CI artifact of a specific `main` commit". |
| U10 | **Channel docs live on an orphan branch, `update-channels`**, written only by CI, served from `raw.githubusercontent.com`. Git mode then literally reads Git. | Durable and diffable, with history as an audit log. The signature makes the CDN untrusted. Pages is out, because `docs.yml` replaces the whole site on each deploy. |
| U11 | **Build identity is compiled in.** `fleet_core::build_info` provides `{version, commit, build_id, built_at}` from `build.rs` (`FLEET_GIT_SHA` / `FLEET_BUILD_ID` from CI, falling back to `git rev-parse`, then `"unknown"`). The binaries report it everywhere they report a version. | Without it, "is it running the build we asked for" is only a version-string check (§8.4). |
| U12 | **Observed state is durable.** New tables record every target's reported version, build, protocol and phase: the hub itself, each agent host, each paired client token. The registry stays the live view. | Today client versions are never recorded and agent versions are lost on a hub restart. The Updates page and rollouts need both. |

## 4. The release manifest (per release, immutable)

The manifest is published as the release asset `release-manifest.json`, plus
`release-manifest.json.minisig`. Both are covered by `SHA256SUMS` and added
to `scripts/release-assets.sh` as a `manifest` leg. It is generated by a new
`scripts/release-manifest.sh` from the `LEGS` table, the release's actual
assets (the same enumeration `merge-sha256sums.sh` does), and the hub image
digest `record-digest` already has. It is never written by hand. A new
`verify-release` rule: every asset in the manifest has the digest
`SHA256SUMS` has.

```json
{
  "schema": 1,
  "release": {
    "version": "0.3.4",
    "track": "stable",
    "commit": "a83f19d0c2…",
    "build_id": "rel-v0.3.4-a83f19d0c2…",
    "published_at": "2026-09-30T10:12:00Z",
    "assets_base": "https://github.com/martin-janci/claude-fleet/releases/download/v0.3.4/",
    "notes_url": "https://github.com/martin-janci/claude-fleet/releases/tag/v0.3.4"
  },
  "compatibility": {
    "contract":    { "hub_serves": 5, "desktop_accepts": [5, 5], "mobile_accepts": [0, 5] },
    "agent_proto": { "hub_accepts": [1, 1], "agent_speaks": 1 },
    "peer_proto":  { "speaks": 1, "accepts": [1, 1] },
    "store":       { "schema_to": 73, "opens_down_to": 73 },
    "update_proto": 1
  },
  "components": {
    "hub": {
      "version": "0.3.4",
      "artifacts": [
        { "kind": "oci", "image": "ghcr.io/martin-janci/fleet-hub",
          "digest": "sha256:…index…",
          "platforms": { "linux/amd64": "sha256:…", "linux/arm64": "sha256:…" } },
        { "kind": "tarball", "target": "x86_64-unknown-linux-gnu",
          "name": "fleet-hub-0.3.4-x86_64-unknown-linux-gnu.tar.gz",
          "sha256": "…", "size": 12345678 }
      ]
    },
    "agent": {
      "version": "0.3.4",
      "artifacts": [
        { "kind": "tarball", "target": "x86_64-unknown-linux-gnu", "name": "fleet-agent-0.3.4-…", "sha256": "…", "size": 0 }
      ]
    },
    "desktop": {
      "version": "0.3.4",
      "artifacts": [
        { "kind": "tauri", "platform": "macos-aarch64", "name": "claude-fleet_0.3.4_aarch64.app.tar.gz", "sha256": "…", "size": 0, "tauri_signature": "…minisign…" },
        { "kind": "tauri", "platform": "windows-x86_64", "name": "claude-fleet_0.3.4_x64-setup.exe", "sha256": "…", "size": 0, "tauri_signature": "…" },
        { "kind": "tauri", "platform": "linux-x86_64", "variant": "appimage", "name": "claude-fleet_0.3.4_amd64.AppImage", "sha256": "…", "size": 0, "tauri_signature": "…" },
        { "kind": "download", "platform": "linux-x86_64", "variant": "deb", "name": "claude-fleet_0.3.4_amd64.deb", "sha256": "…", "size": 0 }
      ]
    },
    "android": {
      "version": "0.3.4",
      "artifacts": [
        { "kind": "apk", "url": "https://github.com/martin-janci/fleet-mobile/releases/download/v0.3.4/fleet-mobile-0.3.4.apk",
          "sha256": "…", "size": 0, "version_code": 412, "signer_sha256": "…cert…" }
      ]
    },
    "ios": { "version": "0.3.4", "artifacts": [ { "kind": "notify" } ] }
  }
}
```

Rules:

- Asset URLs are derived. `name` resolves against the signed
  `release.assets_base`; only the cross-repo APK carries a full `url`.
  An artifact matches a caller's `platform` (`{os, arch, variant}`) by
  `<os>-<arch>` plus an optional `variant` (`appimage` / `deb`). `oci`
  matches the variant `oci` and the `linux/<arch>` key, and `tarball`
  matches the variant `tarball` and the target triple. An unknown `kind`
  parses as `unknown` and never matches, so a new kind never breaks an
  older reader. A hub-served mirror (§6.4) may serve
  the same bytes from elsewhere; the sha256 is what is trusted.
- The `compatibility` block is emitted by the binaries themselves, never
  typed. A new hidden subcommand, `fleet-hub compat --json`, prints its
  constants, and CI captures it. The agent's and the phone's constants are
  read the same way (`fleet-agent compat --json`; for the phone,
  `HubContractDriftTest` already reads the desktop's). A test pins the
  output to the constants, so the matrix cannot drift from the code.
- `store.opens_down_to` is the oldest schema this binary will open. It is
  used only for the rollback analysis in §8.3: the downgrade guard means a
  previous hub opens only databases at or below its own `schema_to`.
- Android is on the same train: `release-mobile.sh` runs after the release,
  so the android section is filled in by a second, signed **manifest
  amendment**, `release-manifest.android.json`, which fleet-mobile's
  `release.yml` requests through `repository_dispatch` once its APK is
  published. Until that lands, `android` is absent and the decision is
  `notify` from the version only. (Alternative in Open question 2.)
- fleet-mobile's release publishes a versioned APK name and a
  `SHA256SUMS`. Today the APK name carries no version and there are no
  sums.

## 5. The channel document (per track, mutable)

```json
{
  "schema": 1,
  "track": "stable",
  "sequence": 118,
  "generated_at": "2026-09-30T10:20:00Z",
  "expires_at":  "2026-10-14T10:20:00Z",
  "current": "0.3.4",
  "recommended": "0.3.4",
  "minimum_supported": { "hub": "0.3.0", "agent": "0.2.30", "desktop": "0.3.0", "android": "0.3.0" },
  "mandatory": [ { "version": "0.3.4", "components": ["hub"], "reason": "security", "deadline": "2026-10-03T00:00:00Z" } ],
  "rollback": "0.3.3",
  "withdrawn": [ { "version": "0.3.2", "reason": "migration 070 corrupts usage_daily on arm64" } ],
  "releases": [
    { "version": "0.3.4", "manifest": "…/v0.3.4/release-manifest.json", "manifest_sha256": "…" },
    { "version": "0.3.3", "manifest": "…/v0.3.3/release-manifest.json", "manifest_sha256": "…" }
  ]
}
```

- Written to `update-channels:<track>.json` (+ `.minisig`) by CI only:
  - `release.yml`'s `publish` job on a stable or rc tag;
  - a new `nightly.yml` on every green `main` push (it keeps the last 10
    releases in `releases`);
  - a new `channel-edit.yml` (`workflow_dispatch`, environment-protected)
    for the publisher's after-the-fact edits: withdraw, mandatory, move
    `recommended` back.
  - The branch has a ruleset: only the CI app pushes, and no force push.
- `sequence` is strictly increasing. A client or hub stores the highest one
  it has seen and refuses a lower one. A document past `expires_at` is
  *stale*: it is still usable for "what is installed", but it never
  produces `update_available` or `update_required`, and it raises an
  attention item. `nightly.yml`, plus a weekly re-sign of stable and beta,
  keeps the documents fresh.
- `current` is the newest on the track. `recommended` is what a policy of
  `notify` / `automatic` targets; it usually equals `current` and lags it
  when the publisher holds a release back.
- Nightly carries no `minimum_supported` and no `mandatory`. It is the
  bleeding edge, and its floor is `stable`'s.

## 6. The update protocol

### 6.1 Identity of the caller

The hub derives `target` from the credential, never from the body:

| credential | target | component |
|---|---|---|
| paired client token (`client_tokens.id`) | `client:<id>` | the body's `component` must be `desktop` or `android` / `ios` |
| agent token (`fleet-hub agent-token <host>`) | `agent:<host alias>` | `agent` |
| new **`updater`** token mode | `hub:self` (or, later, `node:<name>`) | the updater acts for `hub` |
| master token | `operator` | read-only use: `check` for any component, no `report` |

`updater` is a new `CLIENT_MODES` entry, paired like `peer`
(`fleet-hub pair --mode updater`). Like federation's D5, it can call exactly
the `/update/*` routes, nothing else, and is refused by every MCP tool.

### 6.2 `POST /update/check`

Request:

```json
{
  "update_proto": 1,
  "component": "desktop",
  "platform": { "os": "macos", "arch": "aarch64", "variant": "tauri" },
  "installed": { "version": "0.3.3", "commit": "…", "build_id": "…" },
  "speaks": { "contract_accepts": [5, 5] },
  "phase": "idle",
  "attempt": null
}
```

`speaks` is the caller's own window: `contract_accepts` for a client,
`agent_proto` for an agent. The hub trusts it for *compatibility* only; it
is a statement about the caller.

Response, a `Decision`:

```json
{
  "update_proto": 1,
  "component": "desktop",
  "status": "update_available",
  "source": "hub",
  "track": "stable",
  "mode": "notify",
  "installed": "0.3.3",
  "target": {
    "version": "0.3.4",
    "mandatory": false,
    "deadline": null,
    "manifest": { "url": "…/v0.3.4/release-manifest.json", "sha256": "…" },
    "channel":  { "url": "…/update-channels/stable.json", "sequence": 118 },
    "artifact": { "kind": "tauri", "platform": "macos-aarch64", "name": "claude-fleet_0.3.4_aarch64.app.tar.gz", "sha256": "…", "size": 0, "tauri_signature": "…" },
    "url": "https://github.com/…/v0.3.4/claude-fleet_0.3.4_aarch64.app.tar.gz",
    "evidence": { "channel": "<stable.json, verbatim>", "channel_sig": "<.minisig>",
                  "manifest": "<release-manifest.json, verbatim>", "manifest_sig": "<.minisig>" }
  },
  "reason": { "code": "newer_recommended", "text": "0.3.4 is recommended for this hub." },
  "next_check_secs": 21600
}
```

**Evidence.** The hub relays the signed channel document and the target's
manifest *verbatim*, meaning the exact bytes that were signed. The caller
verifies the target from them alone, with no second fetch and no trust in
the relay (`verify_target`, §7):

1. both signatures hold;
2. the manifest's sha256 is what the channel lists for the target;
3. the publisher permits the target (U6);
4. the artifact is one the manifest carries for the caller's platform.

A target without evidence is refused (`E_UPDATE_UNVERIFIED`). The documents
are a few KB.

`status` is one closed set, an enum in `fleet-update` mirrored in
`HubContract.kt`:

| status | meaning | what the client does |
|---|---|---|
| `up_to_date` | installed = target, or ahead but still compatible | nothing |
| `update_available` | a newer target, not required | per `mode`: `notify` shows it, `automatic` downloads and installs at the next quiet point |
| `update_required` | installed is incompatible with this hub, below the policy floor, or mandatory past its deadline | blocks the gated features with "Update required to connect to this Hub", offers the install |
| `client_too_new` | installed accepts no contract / proto the hub serves | says "this hub is older than this app; ask the operator to update the hub". A rollback artifact is offered only where the platform can downgrade (Docker, agent). |
| `rollback` | desired < installed, by the operator's or channel's rollback | Docker and agent converge down; desktop and mobile show it and never self-downgrade |
| `hold` | policy `manual`, a paused rollout, or outside a maintenance window | nothing; the dashboard says why |
| `unknown` | the hub has no fresh channel doc, or no artifact for this platform | nothing; retry after `next_check_secs` |

`/update/check` is safe to call repeatedly. It writes only the caller's
`last_checked_at` and its observed row.

### 6.3 `POST /update/report`

This carries the caller's observed state and each state-machine transition
(§7):

```json
{
  "update_proto": 1,
  "component": "hub",
  "installed": { "version": "0.3.4", "commit": "…", "build_id": "…", "digest": "sha256:…" },
  "phase": "success",
  "attempt": "01J9…",
  "from": "0.3.3",
  "to": "0.3.4",
  "detail": { "health": "ready", "soak_secs": 120 },
  "error": null
}
```

- Reports are idempotent on (`target`, `attempt`, `phase`).
- An updater that reports while the hub is down queues the report on disk
  and replays it in order.
- Every client also sends `X-Fleet-Client: <component>/<version>
  (<os>-<arch>; build <sha7>; contract <min>-<max>)` on *every* request.
  The hub updates the observed row from it at most once a minute (the
  `last_seen_at` pace). The Updates page then knows a client's version
  without the client ever calling `/update`. Migration 060's auth-epoch
  trigger must exempt that column, as it exempts `last_seen_at`.

### 6.4 Push, not only poll

- **Clients.** A new `/events` kind, `update:decision`, is sent when the
  decision for that token's target changes (policy edit, new channel doc,
  rollout step). Clients also check on start and every `next_check_secs`.
- **Agents.** New `HubFrame::Desired { component, version, digest?,
  manifest_url, manifest_sha256 }` and `AgentFrame::Observed { … }` frames
  in `fleet-proto`. They are additive: an unknown `kind` after the handshake
  is ignored, so no `PROTO_VERSION` bump. An agent too old to know them is
  covered by the 4001 refusal plus `/update/check` with its agent token.
- **Mirror (later).** The hub may cache the manifest and artifacts it
  selected and serve them at `/update/artifact/<sha256>` for clients that
  cannot reach GitHub. The client still verifies sha256 against the signed
  manifest.

### 6.5 Exemption from the contract gate

- **Desktop:** `backend/remote.rs` `require_confirmed_contract` gets an
  explicit allowlist of one transport call, `update_check` /
  `update_report`, that bypasses it. A test asserts that the allowlist is
  exactly those.
- **Phone:** `FleetRepository`'s `Refused` path still runs the update
  check.
- **Hub:** `/update/*` never reads the ready frame's contract, and its
  schema only ever grows additively under `update_proto: 1`. A breaking
  change is `update_proto: 2`, served *alongside* 1 for at least two
  releases, and never replaces it.

## 7. The engine (`crates/fleet-update`)

A new workspace crate. It is Tauri-free, has **no `fleet-core`
dependency**, and does no I/O except through traits, so `fleet-core`,
`src-tauri`, `fleet-updater` and (later) `fleet-agent` can all depend on it.
This mirrors how `fleet-agent` depends only on `fleet-proto`.

```rust
pub enum Source { Git, Hub }
pub enum Track { Stable, Beta, Nightly }
pub enum Component { Hub, Agent, Desktop, Android, Ios }

pub struct CheckRequest { /* §6.2 */ }
pub struct Decision { /* §6.2 */ }
pub struct Report { /* §6.3 */ }

/// The decision, and its target proven against the signed documents.
pub struct CheckOutcome { pub decision: Decision, pub verified: Option<VerifiedTarget> }

#[async_trait::async_trait]
pub trait UpdateChannel: Send + Sync {
    fn source(&self) -> Source;
    async fn check(&self, req: &CheckRequest) -> Result<CheckOutcome, UpdateError>;
    async fn report(&self, report: &Report) -> Result<(), UpdateError>;
}

/// The seam both channels and the updater fetch through
/// (fleet-core's HTTP/1 client in the hub and desktop; a tiny one in fleet-updater).
#[async_trait::async_trait]
pub trait Fetch: Send + Sync {
    async fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, UpdateError>;
}

pub struct GitUpdateChannel<F: Fetch> { fetch: F, track: Track, policy: LocalPolicy, key: PublicKeys, seen: SequenceStore }
pub struct HubUpdateChannel<T: HubTransport> { transport: T }

pub fn decide(input: &DecideInput) -> Decision;          // pure
pub fn verify_channel(bytes: &[u8], sig: &str, keys: &TrustedKeys, track: Track, seen: u64, now: i64) -> Result<VerifiedChannel, VerifyError>;
pub fn verify_manifest(bytes: &[u8], sig: &str, keys: &TrustedKeys) -> Result<ReleaseManifest, VerifyError>;
/// Both channels end here: the one check between "a channel said so" and "install it".
pub fn verify_target(d: &Decision, keys: &TrustedKeys, platform: &Platform, seen: u64, now: i64) -> Result<Option<VerifiedTarget>, VerifyError>;
```

**Selection** (F3):

- **Standalone desktop:** no hub paired → `GitUpdateChannel`, with the track
  and mode read from local settings.
- **Paired desktop:** resolved once at startup, like every other command →
  `HubUpdateChannel`.
- **Phone:** always paired, so always the hub channel. There is no Git mode
  on mobile.
- **Hub:** its own source is `GitUpdateChannel` (the release
  infrastructure), under the hub's policy. It serves `HubUpdateChannel` to
  everything below it. That is the handover's "Release infrastructure →
  Manifest → Hub" arrow.
- **`fleet-updater`:** `HubUpdateChannel` with its `updater` token. With
  `--standalone`, a hub-less Docker host, it uses `GitUpdateChannel`.

### 7.1 `decide()`

Inputs:

- `channel` (verified, maybe stale);
- the manifests for the releases it lists (fetched lazily, cached by
  sha256);
- `policy` for this component, and for the target's org when set;
- `hub_speaks`: the hub's own `compatibility`, meaning the running hub's;
- the caller's `installed` + `speaks`;
- `target_id`, and `rollout`.

Order (the first rule that matches wins):

1. No channel → `unknown` (`no_channel`); a stale channel → `hold`
   (`channel_stale`). A newer release with no verified artifact for the
   caller's platform is `unknown` (`no_artifact`), never `up_to_date`.
2. `installed` accepts nothing the hub serves (`contract_accepts` excludes
   `hub_speaks.contract.hub_serves`, or the agent proto is outside
   `hub_accepts`):
   - the caller is **ahead** of the hub → `client_too_new`, with target set
     to the newest release whose window fits (Docker and agent only);
   - otherwise → `update_required`, with the target chosen by rule 7's
     selection. A *required* target may go past `recommended` up to
     `current` when nothing at or below `recommended` fits the hub, so a
     lagging recommendation never strands a caller the hub has already moved
     past. When nothing fits at all: `update_required` with no target
     (`no_compatible_release`).
3. The operator pinned `desired` for this target or component →
   `rollback` if the pin is below installed, `update_*` if above, else
   `up_to_date`. Mandatory iff the pin says so.
4. Installed is `withdrawn`, below the channel's `minimum_supported`, or
   below the policy floor `update.<component>.minimum` →
   `update_required`.
5. A channel `mandatory` entry covers installed <
   `mandatory.version` → `update_available` with `mandatory: true` before
   the `deadline`, `update_required` after it.
6. The policy `mode` is `manual`, the rollout is paused, the target is
   outside the current rollout wave, or it is outside the maintenance
   window → `hold`.
7. `recommended` > installed → `update_available`. The **target selection
   invariant** applies here and in rules 2 and 4: the hub only ever names a
   release whose compatibility window contains the hub's own serving
   contract / proto. It walks `releases` newest-first until one fits and
   never names one that would strand the caller.
8. Otherwise → `up_to_date`.

The same fixtures drive the Rust tests and the phone's display mapping. The
fixture is `crates/fleet-update/tests/decide_cases.json`, in the style of
`work_keys`' shared fixture.

### 7.2 The state machine

`UpdatePhase` is one enum in `fleet-update`, reported verbatim:

```
idle → checking → available → downloading → verifying → ready → installing → validating → success
                                   │             │                     │            │
                                   └──── failed ←┴─────────────────────┘            │
                                                                         validating ─┴→ failed → rolling_back → recovered
                                                                                                     └→ rollback_failed (operator)
```

- `attempt` is a ULID made at `downloading` and persisted locally. After an
  install that restarts the process, the new build finds the attempt and
  moves `validating → success` (or `failed`).
- `verifying` is always: the sha256 matches the signed manifest, and the
  signature is valid. For Tauri the minisign signature is checked a second
  time by the plugin. For OCI, the pulled image's `RepoDigests` contains the
  manifest's digest. For APK, the signer cert sha256 matches the manifest's
  and the installed app's.
- `failed` from any phase before `installing` is harmless: back to `idle`,
  and the same attempt is not retried for `update.retry_backoff_secs`.
- `(target, version)` pairs that reached `failed` / `rollback_failed` go on
  a local **bad list**. A converging target never re-tries a bad version
  until the desired version changes or the operator clears it. This is the
  loop breaker.

Per platform (the last layer, F6):

| phase | Docker (hub) | agent (later) | desktop | Android | iOS |
|---|---|---|---|---|---|
| downloading | `docker pull image@digest` | tarball | plugin download | `HttpClient` to app cache | — |
| installing | swap containers (§8) | versioned dir + symlink + `systemctl restart` via `systemd-run` | plugin install + relaunch | `PackageInstaller` session (user confirms) | open App Store / TestFlight link |
| validating | health gates + soak (§8.4) | reconnect `Hello.agent_version == target` in T | the relaunched app reports `success` on first start | next launch reports version | next launch reports version |
| rolling_back | previous container (+ DB restore) | symlink back | **none**: `failed` + notice | **none** | **none** |

### 7.3 Local policy (Git mode) and hub policy

These are registry settings (`service/settings.rs` `SPECS`, a new `update.*`
namespace, every key in the new user guide `docs/updates.md`, with an
`update_settings_are_in_the_user_guide` test in the same style as `work.*`):

| key | kind | default | notes |
|---|---|---|---|
| `update.track` | Choice `stable\|beta\|nightly` | `stable` | the hub's track for the fleet; the standalone desktop's own |
| `update.hub.mode` | Choice `manual\|notify\|automatic` | `notify` | `automatic` needs `fleet-updater` running |
| `update.agent.mode` | Choice | `notify` | |
| `update.desktop.mode` | Choice | `notify` | |
| `update.mobile.mode` | Choice `manual\|notify` | `notify` | there is no silent install on a phone |
| `update.<component>.minimum` | Version (new `Kind`) | empty | the policy floor; below it → `update_required` |
| `update.check_interval_secs` | SecsMin | `21600` | hub → channel, client → hub |
| `update.soak_secs` | Secs | `120` | §8.4 |
| `update.retry_backoff_secs` | Secs | `3600` | |
| `update.window` | (later) cron-ish | empty | maintenance window |

"Mandatory" has two sources: the publisher's channel `mandatory`, and the
operator's floor. A policy mode `mandatory` is not a fourth mode; it *is*
the floor.

### 7.4 Desired / observed tables (hub, migration 079)

```sql
-- what the operator (or a rollout) wants, per target or per component default
CREATE TABLE update_desired (
  id INTEGER PRIMARY KEY,
  scope TEXT NOT NULL,              -- 'component' | 'target'
  component TEXT NOT NULL,          -- hub | agent | desktop | android | ios
  target TEXT,                      -- NULL for scope=component; 'agent:<alias>', 'client:<id>', 'hub:self'
  org_id TEXT,                      -- NULL = every org
  kind TEXT NOT NULL DEFAULT 'artifact',  -- F8: later 'config', 'plugin', 'flag', 'runtime'
  version TEXT NOT NULL,
  digest TEXT,                      -- OCI digest when component runs in a container
  mandatory INTEGER NOT NULL DEFAULT 0,
  reason TEXT,
  set_by TEXT NOT NULL,             -- 'operator' | 'rollout:<id>' | 'channel'
  set_at TEXT NOT NULL,
  row_version INTEGER NOT NULL DEFAULT 1,   -- compare-and-set, E_CONFLICT as in M14
  UNIQUE (scope, component, target, org_id, kind)
);

-- what each target last said about itself
CREATE TABLE update_observed (
  target TEXT PRIMARY KEY,
  component TEXT NOT NULL,
  platform TEXT,
  version TEXT NOT NULL,
  commit_sha TEXT,
  build_id TEXT,
  digest TEXT,
  speaks TEXT,                      -- JSON: contract window / proto
  phase TEXT NOT NULL,              -- UpdatePhase
  attempt TEXT,
  last_error TEXT,
  reported_at TEXT NOT NULL,
  last_checked_at TEXT
);

-- the transition log (retention like session_history)
CREATE TABLE update_events (
  id INTEGER PRIMARY KEY, target TEXT NOT NULL, attempt TEXT, phase TEXT NOT NULL,
  from_version TEXT, to_version TEXT, detail TEXT, at TEXT NOT NULL,
  UNIQUE (target, attempt, phase)
);

-- rollouts (S9; created now so the decide() input is stable)
CREATE TABLE update_rollouts (
  id INTEGER PRIMARY KEY, component TEXT NOT NULL, version TEXT NOT NULL,
  waves TEXT NOT NULL DEFAULT '[100]', wave INTEGER NOT NULL DEFAULT 0,
  wave_started_at TEXT, paused_at TEXT, halt_failure_ratio REAL NOT NULL DEFAULT 0.2,
  created_at TEXT NOT NULL
);
```

- **The channel cache.** A verified channel doc plus its sequence goes in
  `update_channel_cache (track PRIMARY KEY, sequence, body, sig,
  fetched_at)`. Manifests are cached by sha256.
- **Cohorts.** A target is in wave *w* iff
  `sha256(target ‖ version)[0..2] mod 100 < waves[w]`. The cohort is stable
  per release and varies across releases.
- **Rollout movement.** A rollout advances after `soak`, when the wave's
  failure ratio is below `halt_failure_ratio`. Otherwise it pauses itself
  and raises an attention item.

## 8. `fleet-updater` — the Docker target (MVP)

### 8.1 Shape

- **The binary.** `crates/fleet-updater`, depending on `fleet-update`
  plus a minimal Docker Engine API client over the unix socket (HTTP/1,
  about 6 calls: `images/create`, `images/<ref>/json`,
  `containers/create|start|stop|rename|json`). It uses no `bollard`, to stay
  small and auditable, and it carries no fleet business logic (F5).
- **The image.** `ghcr.io/martin-janci/fleet-updater`, published by
  `hub-image.yml`.
- **The compose service.** `deploy/hub/docker-compose.yml` and
  `behind-proxy/` get an `updater` service with `/var/run/docker.sock`
  mounted, the hub's data volume mounted read-write (for the backup and the
  restore), and its own state volume. It is behind the compose profile
  `auto-update`: off unless the operator opts in, and `upgrade.sh` stays
  the manual path.
- **Its own updates.** `fleet-updater` is not updated by itself in the MVP.
  It changes rarely and is bumped with `docker compose pull updater`. A
  self-update is later work: two-phase, where the updater starts its
  successor and exits only once the successor reports.
- **The state file.** `/var/lib/fleet-updater/state.json`, written
  atomically (tmp + fsync + rename):

```json
{
  "target": "hub:self",
  "current":  { "version": "0.3.3", "digest": "sha256:…", "container": "fleet-hub", "schema": 73 },
  "candidate": null,
  "previous_known_good": { "version": "0.3.2", "digest": "sha256:…", "container": "fleet-hub-prev", "schema": 72, "backup": "backups/pre-0.3.3-20260929T0301Z.db" },
  "phase": "idle",
  "attempt": null,
  "bad": [ { "version": "0.3.4-dev.3.g1a2b3c4", "digest": "sha256:…", "at": "…", "why": "ready timeout" } ],
  "desired": { "version": "0.3.4", "digest": "sha256:…", "fetched_at": "…" },
  "pending_reports": []
}
```

### 8.2 The loop

1. **Check.** Every `check_interval`, and immediately on an `/events`
   `update:decision`, call `/update/check` as `hub:self`. If the hub is
   unreachable, keep the last `desired` and do nothing new.
2. **Verify the intent.** Fetch the release manifest the decision names and
   verify its signature with the compiled-in key. Require the decision's
   digest to equal the manifest's `components.hub.artifacts[kind=oci]` digest
   for this arch or the index. The hub cannot name a digest the publisher
   did not sign.
3. **Download.** `docker pull image@digest`, then check that the image's
   `RepoDigests` contains it.
4. **Backup.** `docker exec fleet-hub fleet-hub backup --prefix pre-<v>
   --json` (`fleet_core::store::backup`):
   - opens `state.db` read-only and never migrates it;
   - copies it with `VACUUM INTO`, checks the copy with `PRAGMA
     integrity_check`, and renames it out of `.part` only then;
   - prints `{path, schema, bytes}` and never overwrites a file.

   The updater needs no sqlite on the host. Pruning to 3 per version is the
   updater's job, as `upgrade.sh` prunes through `backup.sh`.
5. **Install.** Stop `fleet-hub` (SIGTERM, the compose
   `stop_grace_period`). Rename it `fleet-hub-prev`, replacing an older
   `-prev`. Create `fleet-hub` from the same config (env, volumes, networks,
   labels) with `image@digest`, then start it.
   - **As built (S6):** the old container is removed, not renamed: a
     stopped `-prev` would still carry compose's service labels, so compose
     would see two containers for one service. The previous build's create
     config and image id are kept in the state file instead (`previous`),
     which brings it back as fast; the failed candidate's log is kept in
     the state volume rather than as a `-failed-<v>` container.
   - The hub cannot run blue/green: SQLite has a single writer, and there
     is one port. So "candidate" means *replaces*, and the old container is
     kept stopped for a fast rollback.
6. **Validate.** Run the health gates in §8.4.
7. **Pass → activate.** `previous_known_good ← current`,
   `current ← candidate`, report `success`. The old `-prev` container is
   removed only once the next update succeeds, so the last good one is
   always present.
8. **Fail → roll back.** §8.3.

### 8.3 Rollback, and the database

This is the part a plain image swap gets wrong.

- **Candidate did not migrate** (`fleet-hub healthcheck --json` reported the
  same `schema`, or never came up): stop the candidate, rename it
  `fleet-hub-failed-<v>`, rename `fleet-hub-prev` back, start it, run the
  health gates, report `failed` → `recovered`.
- **Candidate migrated** (`schema` > the previous `schema_to`): the previous
  binary will refuse the database (downgrade guard). So: stop the candidate,
  move `state.db*` aside to `failed-<v>-<ts>/` (never deleted), restore the
  `pre-<v>` backup taken in step 4, start the previous container, run the
  health gates, and report `recovered` with `detail.data_restored: true`.
  **The candidate's writes during its validation window are lost**, and the
  report says so. That is why the soak is short, and why the dashboard shows
  it.
- **Previous fails too** → `rollback_failed`: stop, leave everything in
  place, alert (the updater's own log, plus the pending report). It is
  operator territory. It never loops.
- **After `success`** there is no automatic rollback. An operator rollback
  is a *new desired state* (`update_admin rollback`, §9). When it crosses a
  migration it requires `restore_backup: true` and names the backup it will
  restore, and the confirmation says what is lost. Otherwise it is refused
  with `E_CONFLICT` "rolling back to 0.3.3 needs the 0.3.4 migration undone
  — restore <backup>?".

### 8.4 Health gates

"Container running" is not health (F5). The candidate passes only when all
of these hold within `ready_timeout` (default 90 s), then keep holding for
`update.soak_secs`:

1. **Live.** The container state is `running`, its restart count is still
   0, and the image `HEALTHCHECK` (`fleet-hub healthcheck`, `/healthz`) is
   `healthy`.
2. **Ready.** `fleet-hub healthcheck --ready --json`, run through `docker
   exec`, which exposes nothing on the network.
   - `serve` rewrites `<data dir>/run/ready.json` every 5 s once the store
     has migrated and the listener is bound, and removes the file when it
     stops.
   - The check adds liveness (the `/healthz` probe) and freshness (the
     heartbeat is at most 20 s old and the pid is running). A killed hub's
     leftover file is stale, and stale is not ready.

   ```json
   { "ready": true, "live": true, "fresh": true,
     "hub": { "ready": true, "pid": 7154, "version": "0.3.4", "commit": "a83f19d…",
              "build_id": "rel-v0.3.4-a83f19d0c2…", "contract": 5, "agent_proto": [1, 1],
              "peer_proto": 1, "schema": 74, "started_at": 1790612008, "heartbeat_at": 1790612013,
              "checks": { "store": "ok", "listener": "ok", "first_reconcile": "ok",
                          "reconcile_failures": 0 } } }
   ```

   `first_reconcile` is one of:
   - `ok`: a pass of this process has finished clean. It is a latch: a
     later failed pass does not undo it;
   - `pending`: no pass has finished yet;
   - `failed`: passes finished, none of them clean, which is not ready;
   - `disabled`: `reconcile.interval_secs=0`, which counts as ready.

   `reconcile_failures` is the running count of failed passes since the
   last clean one (`fleet_health.hub.reconcile.consecutive_failures`). It
   is reported and never part of `ready`.

   The commit and build ID come from `crates/fleet-hub/build.rs`:
   `FLEET_GIT_SHA` / `FLEET_BUILD_ID` from CI (`release.yml`, and the
   `hub-image.yml` build args, since the Docker context has no `.git`), else
   `git rev-parse HEAD`, else `unknown` / `local`. A release's build ID is
   `rel-<tag>-<commit>` in all three places (the tarballs, the image and the
   manifest), never a workflow run id: the image is built by another run,
   and a re-run job is another attempt.
3. **Identity.** `version`, `commit` and `build_id` equal the manifest's
   `release` (U11), and the container's image ID resolves to the desired
   digest. This is what "the updater verifies it started the right build"
   means.
4. **Soak.** No restart, `ready` stays true, and no `fleet_health`
   `hub.reconcile_failures` increase beyond one.

The design keeps `/healthz` unversioned: a public version string helps an
attacker and nobody else. The handover's `/health/live`, `/health/ready` and
`/version` therefore map to `/healthz` (live, public),
`healthcheck --ready --json` (ready + version, local only), and the
`fleet_health` tool (version, authenticated). A loopback-only
`/readyz` can be added if a proxy needs it.

### 8.5 Standalone Docker (no hub above)

The hub *is* the top of the tree, so "standalone" for the updater means
`fleet-updater --standalone`. It reads `GitUpdateChannel` with the local
`update.hub.mode` / `update.track`, passed as env since the updater has no
settings store. The loop and the gates are the same.

### 8.6 Other targets on the same updater

- A future containerised worker, `node:<name>`: same loop. Blue/green is
  possible there (no shared single-writer DB), so `install` can start the
  candidate beside the current one and switch after the gates.
- `fleet-agent` (S9): the same state machine, run by the agent itself.
  `fleet-agent update` runs the apply phase in a transient `systemd-run`
  unit so it survives the agent's restart. The layout is versioned dirs
  `/opt/fleet-agent/<v>/` with a `current` symlink. `validating` means the
  hub saw `Hello.agent_version == target` within T. Rollback means the
  symlink goes back.
- Bare-binary hub (`fleet-hub.service`): the same layout as the agent's.

## 9. Hub API for operators (MCP tools, contract-bound)

New tools, each in `generate_handler!`, with verdict rows and a regenerated
`docs/control-api-reference.md`:

| tool | who | what |
|---|---|---|
| `update_status` | readonly+ | the dashboard read: per component summary (`hub 1/1`, `agents 17/20`, `desktop 8/11`, `android 5/9` at desired), per target rows (observed, desired, decision status, phase, last error), the channel's sequence and freshness, active rollouts |
| `update_admin` | master (and `full` in the org it owns, for the org-scoped rows) | actions: `set_policy`, `pin` (desired for a target or component), `unpin`, `rollback` (§8.3 confirm rules), `rollout_start`, `rollout_pause`, `rollout_resume`, `rollout_abort`, `clear_bad`, `refresh_channel` |
| `update_check_for` | readonly+ | "what would target X be told": `decide()` dry run, for the UI's "why" popover |

- **Error codes.** `E_CONFLICT` for a stale `row_version`, and
  `E_UPDATE_UNVERIFIED` (new) for a channel or manifest that fails its
  signature.
- **Attention reasons** in `fleet_health`: `update_required` (a target is
  blocked), `update_failed`, `update_rolled_back`, `rollback_failed`,
  `channel_stale`, `rollout_paused`.
- **Org scope.** M5's `OrgScope` filters `update_status` rows by the
  observed target's org. A per-host token sees only its host.

**Desktop commands** get a `verdicts.rs` row each:

| command | paired (hub-client mode) | standalone |
|---|---|---|
| `update_check` | a new `Verdict::RoutedUpdate`: routed to `/update/check`, **not** an MCP tool, and exempt from the contract gate (§6.5). `tests_routing.rs` learns the variant. | `GitUpdateChannel` |
| `update_install` | `SameInBoth`: it always installs on this machine, from the `Decision` it is given | same |
| `update_status` | `Routed { tool: "update_status" }` | a one-row status of this desktop, from the Git channel |
| `update_admin` | `Routed { tool: "update_admin" }` | writes the local `update.*` settings; `pin` / `rollout_*` refuse with `E_UNSUPPORTED` "needs a hub" |

Then `REGEN_HUB_VERDICTS=1`, and a `REASONS` entry for any refusal the UI
can reach (`src/lib/hub_verdicts.test.ts`).

## 10. Security

| threat | control |
|---|---|
| GitHub release, CDN or raw content tampered | minisign signature on the manifest and channel doc (U2); sha256 per artifact in the signed manifest; Tauri's own minisign check; APK signer pin |
| hub compromised | it can only select signed releases (U1); the client refuses withdrawn or below-signed-floor targets (U6); the updater re-verifies the digest against the signed manifest (§8.2 step 2) |
| replay of an old channel doc (freeze or downgrade) | `sequence` monotonic per track, stored per client and hub; `expires_at` |
| updater token stolen | the `updater` mode reaches only `/update/*`; `report` on `hub:self` can only lie about observed state and cannot change desired |
| release key compromise | `keys` in the channel doc: the current key signs `next` keys, and clients accept a rotation only signed by a key they hold. The key lives in an environment-protected Actions secret `RELEASE_SIGNING_KEY` (minisign), used only by `release.yml`, `nightly.yml` and `channel-edit.yml`, and backed up in the owner's keychain like the Apple identity. |
| Docker image swapped under a tag | digests only; tags are informational |
| downgrade by an operator to a vulnerable release | the same U6 floor, unless it is the channel's signed `rollback` |

Later: cosign-signed OCI images, verified by `fleet-updater` in addition to
the digest, and SLSA provenance from the workflows.

## 11. UI — Fleet → Updates

The desktop gets `UpdatesView.svelte`, reached from the sidebar's Fleet
section, with state in `src/lib/updates.ts`, reading `update_status`. It
re-reads on the `update:changed` row event (a new kind in `events.rs`, the
same patch-in-place rule as every store). It shows:

```
Claude Fleet 0.3.4 · stable · channel #118 (fresh)

Hub        0.3.4  ✓            1 / 1
Agents     0.3.4               17 / 20   ↑ 3 available
Desktop    0.3.4                8 / 11   ⚠ 1 required
Android    0.3.4                5 / 9

Martin-MacBook   Desktop 0.3.4   ✓ healthy
dev-server-3     Agent   0.3.3   ↑ 0.3.4 available
android-s24      Android 0.2.36  ⚠ update required (contract 1, hub serves 5)
```

- Each row opens "why", which calls `update_check_for`, plus the admin
  actions.
- The rollout strip shows the wave, its failure ratio, and pause / resume.
- The desktop's own update prompt is a banner ("0.3.4 is available —
  Restart to update"). When `update_required`, it is the same blocking empty
  state the contract skew uses today, with an **Update now** button. The
  footer shows **both** versions when paired (`app 0.3.3 · hub 0.3.4`); it
  currently shows only the hub's.
- The phone gets a Settings → Updates row, and the "Update required to
  connect to this Hub" screen in place of today's `Refused` banner for this
  case. See
  fleet-mobile's `docs/superpowers/specs/2026-09-28-mobile-update-adapter.md`.

## 12. Slices (the handover's MVP order, made concrete)

Each slice lands on its own, green, with its tests.

| slice | handover step | what | where | done when |
|---|---|---|---|---|
| **S1** ✅ | 1, 2 | `fleet-update` crate: manifest + channel types, serde, minisign verify, `decide()`, `UpdatePhase`, `UpdateChannel` trait, the fixture `decide_cases.json` | `crates/fleet-update/` | the fixture cases pass; the verify tests cover a bad signature, a lower sequence, expiry, rotation |
| **S2** ✅ | 3, 7 | CI writes, signs and uploads `release-manifest.json` from 0.4.1 (`manifest` job, before `checksums`; `verify-release` requires it through the `manifest` leg of `release-assets.sh`); `fleet-hub compat` (the windows the shipped binary states); `build_info` (U11, with S5); the `update-channels` branch written only by `scripts/update-channels.sh` — `add` after `publish` (stable → stable + beta, rc → beta), `edit` and the weekly `resign` from `update-channels.yml`; every document checked by `fleet-release verify` against `keys.rs` before it leaves the runner; `scripts/release-key.sh` for the owner | `release.yml`, `update-channels.yml`, `fleet-update` `publish.rs` + `bin/fleet-release.rs`, `scripts/release-manifest.sh`, `scripts/update-channels.sh`, `scripts/release-key.sh`, `scripts/release-assets.sh` | `scripts/release-update-scripts-test.sh` (CI hub-headless): a manifest and channels signed by a throwaway key verify with the fleet's verifier, an rc moves only `beta.json`, a key `keys.rs` does not name pushes nothing |
| **S2b** ✅ | 3 | what S2 left: `nightly.yml` (hub image + agent/hub tarballs on green `main`, `-dev.N.g<sha>`; desktop nightly in Open question 3) and `nightly.json`; the ruleset that lets only CI push `update-channels` (the owner's, in the repository settings) | `nightly.yml`, `hub-image.yml` | a `main` push moves `nightly.json` |
| **S3** ✅ | 3 | `service::update::git_check` (a `GitCheck` from the hub's settings, pin and last-seen sequence, over `HttpsFetch`), `hub_self_request`; `fleet-hub update check [--track] [--json]` prints the decision for the running hub, verified, and installs nothing | `fleet-core` `service/update/`, `crates/fleet-hub/src/update.rs` | a standalone hub reports `update_available` against a signed fixture channel, `up_to_date` on the newest, its own pin as a rollback, and refuses an untrusted key or a replayed channel (`service/update/tests.rs`) |
| **S4a** ✅ | 4, 5 | migration 079 (desired / observed / events / the signed-document cache `update_docs`); `/update/check` + `/update/report` (`mcp/update_route.rs`); the `updater` token mode (`fleet-hub pair --mode updater`, no tool, no `/events`); `update_status` / `update_admin { pin \| unpin \| refresh }`; the `update.*` settings with `docs/updates.md`; the refresh tick in `fleet-hub serve` (records `hub:self`); `HubUpdateChannel` (S1) | `fleet-core` `mcp/update_route.rs`, `service/update/`, `store/update.rs` | unit + route tests against signed test documents; checked on a real hub |
| **S4b** ✅ (rollouts with S9) | 4, 5 | **built:** `X-Fleet-Client` recorded into `update_observed` on `last_seen_at`'s beat (`fleet_update::client_header`, written there and not to `client_tokens`, so migration 060's auth-epoch trigger needs no exemption); `update:changed` (ids only, kind `update`, hidden from scoped streams); `fleet_health.updates` with `update_required`, `update_failed`, `update_rolled_back`, `rollback_failed`, `channel_stale`; `update_check_for` as `update_status { target }` (one tool fewer on the definition budget). `update:decision` on `/events` (`push_decisions`; delivered to the same streams as `update:changed`, so a scoped caller keeps to its interval); `hub-e2e.sh` section U (a paired desktop; no agent leg). Rollouts and the org-scoped policy rows landed with S9 | `fleet-core` | `hub-e2e.sh` section U: a fake client and a fake agent see `update_available`, `update_required` and `client_too_new` from a channel signed by an e2e key (`FLEET_UPDATE_E2E_KEYS`, `e2e` builds only) |
| **S5** ✅ | 8 | `fleet-hub healthcheck --ready --json`, `fleet-hub backup --to` | `crates/fleet-hub/src/serve.rs`, `fleet-core::store` | a test drives both against a real store |
| **S6** ✅ | 6, 9 | `fleet-updater`: Docker adapter, the state file, the gates, rollback with and without migration; compose `auto-update` profile; image publish | `crates/fleet-updater/`, `deploy/hub/` | a docker e2e (opt-in, like `--hub-e2e`) against a local registry with three images: good, crash-on-start, and migrates-then-unready. All three end in the right state, and the third restores the backup. |
| **S7** ✅ | 10 | desktop: `tauri-plugin-updater` wired to `Decision`; `TAURI_SIGNING_PRIVATE_KEY` = the release key; `update_check` / `update_install` commands + verdict rows; the Updates view; footer shows both versions; contract-gate exemption | `src-tauri`, `src/lib/updates.ts`, `src/components/UpdatesView.svelte` | a standalone desktop updates itself from `nightly`; a paired one from its hub |
| **S8** ✅ | 11 | fleet-mobile: `X-Fleet-Client`, `/update/check`, the Updates row and required screen; Android `PackageInstaller` adapter; release job publishes a versioned APK + sums and dispatches the manifest amendment | fleet-mobile | see its spec |
| **S9** ✅ | 12 | **built:** rollouts (migration 151 `update_rollouts`; waves default `[10, 50, 100]`, auto-advance after `update.rollout_wave_secs` on the decision pusher's five-minute beat, halt at `halt_failure_ratio` with `rollout_paused`; a wave's failure ratio counts in-cohort targets whose last report is a failed install against those that run the release), the maintenance window (`update.window`, UTC, holding only `automatic` components, since an offer a person accepts is theirs to time); per-org policy (migration 152 `update_org_policy`: mode, floor, window and pin per org; `set_policy` / `clear_policy` for the master, `update_policy` for an org admin's own org); the hub-served mirror (`update.mirror`, `/update/artifact/<sha256>`, `target.mirror`, fetched once and checked against the signed sha256; used by the binary target, the desktop and the Android app, each falling back to GitHub); update now (`update_admin update_now`: a pin plus a wake of the hub's updater and the agent hosts' path unit); a `dev` track listing every green main push; the binary target (`fleet_updater::binary` + `systemd`: versioned dirs, a `current` symlink, the unit restarted onto it, gates, rollback with the database) behind `fleet-agent update` (`install --auto-update`) and `fleet-hub update apply` (`fleet-hub-update.timer`). The agent updates from a timer or the update-now path unit, not from a pushed `Desired` frame | `fleet-core` `service::update::{rollout,mirror}`, `fleet-updater` lib, `fleet-agent`, `fleet-hub` | `service::update::tests`, `report_route` (mirror route), `fleet-updater` `binary::tests` (good, never ready, restart in soak, tampered, mirror then GitHub, notify, hub restore) |

**Tests the whole design leans on:**

- the `decide` fixture (Rust; the phone uses it for display strings);
- the contract-gate exemption allowlist test;
- a test that `compat --json` equals the constants;
- `verify-release`'s manifest rule;
- the migration's upgrade test in `store::testgen`;
- the updater e2e in S6.

## 13. Open questions for the owner

Per the repository's rule, a decision-gated slice starts only on the owner's
"yes".

1. **The release signing key.** Generate a minisign key (`minisign -G`),
   store it as the environment-protected secret `RELEASE_SIGNING_KEY`, and
   back it up in the keychain. S2 and S7 need it; S1 can be built against a
   test key. *Recommendation: yes; one key for manifests and Tauri.*
   **Answered yes (2026-09-28), and done:** the key was made and its public
   half merged in #384. The key is made on the owner's machine,
   never in CI or a session: `scripts/release-key.sh` backs it up in the
   keychain, sets the repository secret `RELEASE_SIGNING_KEY` and writes
   the public key into `keys.rs` (`docs/RELEASING.md` → *Update manifest
   and channels*). Moving the secret into a protected environment is the
   owner's option; the jobs read it the same way.
2. **Mobile in the manifest.** Either the fleet-mobile release dispatches a
   signed amendment (§4, recommended: the phone stays built in its own
   repo), or `release.yml` waits for the APK and writes one manifest. The
   latter couples the two runs.
   **Answered (2026-10-09): the signed amendment**, as recommended.
3. **Nightly desktop bundles.** A macOS + Windows + Linux build per `main`
   push costs about 25 runner-minutes on macOS each.
   *Recommendation:* hub image + tarballs per push, desktop bundles once a
   day from the newest green `main` under the same `-dev.N` scheme.
   **Answered (2026-10-09): as recommended.** Built in S2b: per push
   `-dev.N.g<sha>` (at most every two hours), daily
   `-dev.N.desktop.g<sha>`.
4. **Docker socket for the updater.** Mounting `/var/run/docker.sock` is
   root-equivalent on the NAS. The alternative is a host-side
   `fleet-updater` run from a systemd timer, with the same code and no
   container. *Recommendation:* ship both. The compose profile is the
   default, and the docs name the trade-off.
   **Answered (2026-10-09): both**, as recommended. Built in S6: the
   `auto-update` profile and `deploy/hub/fleet-updater.{service,timer}`.
5. **Hub data in a rollback.** Is losing the candidate's writes during the
   validation window (at most `ready_timeout + soak`, about 3.5 min)
   acceptable? The alternative is to put the hub into a read-only
   "validating" mode during the soak (refuse writes with
   `E_HUB_UNAVAILABLE`). That is safer, but a longer outage.
   *Recommendation:* accept the loss, keep the window short, and report it.
   **Answered (2026-10-09): accept the loss**, as recommended; the
   `recovered` report carries `data_restored` and says what was lost.
6. **Default modes.** `notify` everywhere (proposed), or `automatic` for the
   hub once `auto-update` is enabled.
   **Answered (2026-10-09): `notify` everywhere.** Under `notify`,
   `fleet-updater` installs only a pin, a required update or a rollback.
7. **The contract window.** Today the desktop accepts exactly one hub
   contract, so every bump strands paired desktops until they update. U4
   makes that recoverable, because the update itself still flows. Should
   `MIN_HUB_CONTRACT` also trail `CONTRACT_REVISION` by one where the change
   allows it (the agent's "hold MIN for one release" rule)?
   *Recommendation:* decide per bump, and record it in the bump's
   `wire_contract.rs` history line.

## 14. Documentation this lands with

- `docs/updates.md`: the user guide. It covers tracks, modes, the Updates
  page, rollback, and what is lost. It holds the `update.*` settings table.
- `docs/hub.md`: the *Upgrade* section gains "with `fleet-updater`" beside
  `upgrade.sh`, and "Order across the three binaries" becomes the generated
  compatibility table (and its stale "contract 4" goes).
- `docs/RELEASING.md`: covers the manifest, the channel branch, the key and
  `channel-edit.yml` (withdraw / mandatory), and closes T19's decision
  record.
- `CLAUDE.md`: a status line per landed slice.
