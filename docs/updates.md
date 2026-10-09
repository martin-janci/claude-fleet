# Updates

How the fleet updates its own software: the hub, the agents, the desktops
and the phones. Design and rationale:
`docs/superpowers/specs/2026-09-28-update-channel-design.md`.

> **State of the build.** The hub decides and serves updates (slice S4). It
> reads the signed release channel, keeps what each part of the fleet
> reports about itself, and answers `/update/check`.
>
> CI publishes the signed documents (slice S2): each release from 0.4.1
> carries a signed `release-manifest.json`, and the `stable` and `beta`
> channels live on the `update-channels` branch (`docs/RELEASING.md` →
> *Update manifest and channels*). The release key exists and every build
> from 0.4.1 trusts it. The first channel was published with v0.4.1, and
> `stable` and `beta` list every release since. `fleet-hub update check` asks the
> channel directly (slice S3). The Docker hub updates itself through
> `fleet-updater` (slice S6, below): opt-in, and under the default
> `update.hub.mode=notify` it installs only what an operator pins. The
> desktop updates itself (slice S7, below). The phone installs nothing yet
> (S8). A hub that cannot verify a channel still offers nothing. `nightly` is published by
> `nightly.yml` (slice S2b).

## Who decides what

Two authorities, two different questions:

- **The release key decides *what* a release is.** CI signs every
  release's manifest and every track's channel document with it. Every
  component carries the public key compiled in, and checks each signature
  itself.
- **The hub decides *which* signed release, when, and for whom.** It never
  adds an artifact of its own. It picks among the signed releases, and it
  relays the signed documents with its answer so the client can check the
  choice itself. A client refuses a target the publisher withdrew or put
  below its signed minimum. Otherwise it does what the hub says.

A desktop with no hub reads the channel itself (Git mode) and applies its own
`update.*` settings. A phone is always paired, so it always asks its hub.

## Tracks

| track | what it carries |
|-------|-----------------|
| `stable` | `vX.Y.Z` releases |
| `beta` | `-rc.N` release candidates, and every stable release too |
| `nightly` | green `main` commits: the hub image and the agent/hub tarballs as `X.Y.Z-dev.N.g<sha>` (at most one every two hours), and once a day every component, the desktop too, as `X.Y.Z-dev.N.desktop.g<sha>`. A desktop on `nightly` is offered the newest one that carries its bundle. The newest 15 are kept |

## What the hub answers

`POST /update/check` answers one of:

| status | meaning |
|--------|---------|
| `up_to_date` | nothing to do |
| `update_available` | a newer release is offered (`mandatory` when the publisher marked it so) |
| `update_required` | this build can no longer talk to the hub, is withdrawn, is below a minimum, or a mandatory deadline passed |
| `client_too_new` | this build is newer than the hub understands: update the hub |
| `rollback` | the operator pinned an older release |
| `hold` | manual mode, a paused rollout, or a stale channel: something newer exists but is held |
| `unknown` | no verified channel yet, or no artifact for this platform |

The update wire is frozen and outside the contract gate. A desktop or phone
that the hub refuses with `E_HUB_CONTRACT` can still ask what to install.

## Who may ask

| credential | answers for |
|------------|-------------|
| a paired client token | that client (a desktop, a phone) |
| an agent's host token | that agent host |
| an `updater` token (`fleet-hub pair --mode updater`) | the hub itself; it reaches `/update/*` and nothing else |
| the master token | any component (`update_status`, `update_admin`); it never reports |

## Operating it

- **See the fleet.** `update_status` (MCP, read-only) lists every target:
  its reported version and phase, the operator's pin, and what the hub would
  tell it now.
- **Pin a version.** `update_admin { action: pin, component, version,
  target? }` pins a whole component, or one target when `target` is set.
  `action: unpin` removes the pin. A pin below the installed version is a
  rollback. A pin the publisher does not permit (withdrawn, below its signed
  minimum) is held, never served.
- **Re-read the channel now.** `update_admin { action: refresh }`. The hub
  also re-reads it every `update.check_interval_secs`, and at once when
  `update.track` or `update.check_interval_secs` changes. A failed read
  keeps the last good copy, is logged at `warn`, and
  `update_status.last_refresh` says why it failed.
  `FLEET_UPDATE_CHANNEL_URL` points the hub at a mirror. It changes only
  where the documents come from: what is trusted is still the signature.
- **The transition log.** Every phase a target reports is also kept in
  `update_events` (90 days, the newest 200 per target) for the rollout
  view of slice S4b. Nothing reads it out yet: no tool or route exposes it.
- **Ask the channel from the hub box.** `fleet-hub update check` reads the
  published channel itself (Git mode), verifies it against the release key,
  and says what this hub build should run under its own `update.*`
  settings and pin. It needs no running hub, no token and no network but
  GitHub, and installs nothing:

  ```bash
  docker compose exec fleet-hub fleet-hub update check
  # fleet-hub 0.4.1 (linux-x86_64, oci) on stable: update_available
  #   why    newer_recommended — 0.4.2 is available.
  #   target 0.4.2
  #   image  ghcr.io/martin-janci/fleet-hub@sha256:…
  #   signed release manifest and channel #7 verified against the release key
  #   mode   notify (update.hub.mode)
  ```

  `--track beta` reads another track; `--json` prints the whole decision,
  including the signed documents it rests on. It exits 1 when there is no
  answer: no channel published yet, a document no trusted key signed, or
  GitHub unreachable.

## The desktop updates itself

The app checks about 15 seconds after it starts and then on the interval
the decision names (`update.check_interval_secs`, at least an hour):

- **paired**, it asks its hub (`POST /update/check`, as `client:<id>`) under
  the fleet's `update.desktop.mode` and pins. That route is outside the hub's
  wire contract on purpose: a desktop too old or too new for its hub's
  contract — the one that most needs an update — still learns what to install;
- **standalone**, it reads the published channel itself (Git mode) under its
  own Settings → Updates rows.

Either way the target is verified against the release key before anything
else happens: the signed channel lists it, the signed manifest carries a
bundle for this platform, and the publisher has not withdrawn it.

| platform | what it does |
|---|---|
| macOS | replaces the `.app` from the signed `.app.tar.gz`, restarts |
| Windows | runs the signed NSIS installer, restarts |
| Linux, started as an AppImage | replaces the AppImage, restarts |
| Linux, installed from the `.deb` | offers the `.deb` to download; a person installs it |

A banner says what is on offer: *"claude-fleet 0.5.5 is available — restart
to update"*, which can be dismissed for the session, or *"Update required…"*
when the hub refuses this build or a mandatory release is past its deadline,
which cannot. `notify` waits for that button, `automatic` installs at the
next launch, and `manual` offers only what an operator pins
(`update_admin { action: pin, component: desktop, version }`).

The install is `tauri-plugin-updater`'s. It downloads the bundle and checks
its minisign signature a second time, against the same release key. The
signature is not a file of its own: CI signs each updater bundle with the
release key, and the signature travels inside the signed manifest
(`tauri_signature`). Its trusted comment names the version, and the app
requires that (`requireSignedVersion`), so an older bundle's signature
cannot pass for a newer one's. The plugin is handed the verified decision
from a one-shot `127.0.0.1` listener inside the app, and has no update
endpoint of its own. That loopback URL is why
`dangerousInsecureTransportProtocol` is on; the bundle itself comes over
`https`.

Paired, the hub sees every step (`downloading` … `installing`, then
`validating` and `success` from the relaunched app, or `failed`). Every
request a paired desktop makes also names its build
(`X-Fleet-Client: desktop/0.5.5 (macos-aarch64; build 1a2b3c4; contract 14-14)`),
so the hub knows what each desktop runs before it ever asks. A desktop
never installs an older build by itself: a `rollback` decision is shown,
not applied.

## The phone

A phone is always paired, so its hub decides for it, as for a desktop
(`client:<id>`, `update.mobile.mode`: `manual` or `notify`; a phone never
installs silently). Its build comes from fleet-mobile's own release, which
runs after this one, so it reaches the manifest as a signed **amendment**:
`android-amendment.yml` signs it with the release key and lists it beside
the release on every track. An amendment can only add a component the
release does not have; the hub verifies it on every read, like the rest of
its cache, and a release without one offers a phone nothing. The phone
checks the APK's sha256 against the decision and its signing certificate
against its own before handing it to Android's installer, which asks the
person to confirm. See fleet-mobile's
`docs/superpowers/specs/2026-09-28-mobile-update-adapter.md`.

## fleet-updater: the Docker hub updates itself

`fleet-updater` is a small sidecar for the compose deployment
(`crates/fleet-updater`, image `ghcr.io/martin-janci/fleet-updater`). It
asks the hub what the hub itself should run, and when the answer is an
install it carries it through and checks the result. It is off unless you
opt in; `deploy/hub/upgrade.sh` stays the manual path.

### What it installs, and when

It asks `/update/check` as `hub:self` (an `updater` token) on start and then
every `next_check_secs` the hub names (`update.check_interval_secs`). It
installs when the decision is:

- `update_required`: below the signed or the policy minimum, withdrawn, or
  a mandatory release past its deadline;
- `rollback`: an operator pinned a version below what runs;
- an operator's pin above what runs (`update_admin { action: pin,
  component: hub, version }`): this is how a person says "install it" under
  `notify`;
- anything `update_available`, only when `update.hub.mode` is `automatic`.

Under the default `notify` it reports `available` once per version and
waits. `manual` holds everything but pins.

Before anything is touched it verifies the target against the release key
(the signed channel and manifest the hub relays, `verify_target`), pulls the
image **by the digest in that signed manifest**, and checks that the pulled
image's `RepoDigests` names it. The hub cannot make it run anything the
publisher did not sign.

### One update, step by step

| phase | what happens |
|---|---|
| `downloading` | `docker pull <image>@<digest>` |
| `verifying` | the pulled image carries that digest |
| `ready` | `fleet-hub backup --prefix pre-<version> --json` in the running hub; the copy must be visible through the updater's mount of the data volume, or it stops here |
| `installing` | the hub container is stopped and removed, and created again with the same name, labels, env, volumes, networks and restart policy on the new image (the image's own defaults — `ENV`, `CMD`, `HEALTHCHECK` — come from the new image) |
| `validating` | the health gates below |
| `success` | it is kept; the previous build's config and image id stay in the state file for the next rollback |

Every step is reported to the hub (`update_status`, `fleet_health.updates`).
Reports the hub cannot take while it restarts are kept in the state file
and sent in order once it answers.

**The health gates.** Within `FLEET_UPDATER_READY_TIMEOUT_SECS` (90 s) the
new container must be running and not restarting, `healthy` by the image's
`HEALTHCHECK`, and `fleet-hub healthcheck --ready --json` (run through
`docker exec`, nothing on the network) must say ready, live and fresh, with
the version, commit and build id of the signed manifest, on the image id
just pulled. Then it must stay so for `FLEET_UPDATER_SOAK_SECS` (120 s),
with no more than one new failed reconcile pass.

### Rollback, and what is lost

A candidate that fails the gates is stopped, its last 500 log lines are
kept in the state volume (`failed-<version>-<time>.log`), and the version
goes on the updater's bad list so it is never retried by itself. Then:

- **It did not migrate the database** (its reported schema, or its
  manifest's, is the one the backup has): the previous build is started
  again on the same data. Nothing is lost. Reported `recovered`.
- **It migrated it** (or nothing says it did not): the previous build would
  refuse that database, so `state.db*` is moved aside to
  `failed-<version>-<time>/` in the data volume (never deleted), the
  pre-update backup is copied in its place, and the previous build is
  started on it. Reported `recovered` with `data_restored: true`. **What the
  candidate wrote while it was being validated — at most the ready timeout
  plus the soak, about 3.5 minutes — is lost**, and so is anything written
  between the backup and the stop (seconds).
- **The previous build does not come back either**: `rollback_failed`. The
  updater stops the hub, leaves every container and file where it is, and
  does nothing more until an operator has sorted it out and run
  `docker compose run --rm updater clear`. It never loops.

After a `success` there is no automatic rollback. Going back is an
operator's pin to the older version.

### Turning it on

```bash
cd /opt/fleet-hub                                   # the compose directory
docker compose exec fleet-hub fleet-hub pair --name updater --mode updater
# it prints https://<your hub>/pair#<code>; redeem it into the updater's volume:
docker compose --profile auto-update run --rm updater pair 'https://<your hub>/pair#<code>'
docker compose --profile auto-update up -d
docker compose logs -f updater
```

The code works once and expires in 10 minutes. The token lands in the
updater's state volume (`token`, mode 0600); `FLEET_UPDATER_TOKEN` in
`fleet-updater.env` overrides it. `fleet-updater.env.example` lists the
other knobs, all optional. The `updater` token reaches `/update/*` and
nothing else; `fleet-hub client revoke updater` takes it away. The service mounts
`/var/run/docker.sock` — **root on that machine** — the hub's data volume
(for the backup and the restore) and its own state volume
(`/var/lib/fleet-updater/state.json`, written atomically).

**Without a long-running container holding the socket**: install
`deploy/hub/fleet-updater.service` and `fleet-updater.timer` (set
`WorkingDirectory` to the compose directory) and do not start the profile's
service. The timer runs one pass, `docker compose --profile auto-update run
--rm updater once`, every 6 hours; the socket is mounted only for that run.

**After it has updated the hub**, the version pinned in
`docker-compose.yml` (or `FLEET_HUB_TAG` in `.env` behind a proxy) is no
longer what runs. Set it to what `docker compose run --rm updater status`
reports (`current.version`) before your next `docker compose up -d`, or
compose recreates the hub on the older build — on a database the newer one
may have migrated, which the older build refuses to open.

**A Docker host with no hub above it** (`FLEET_UPDATER_STANDALONE=1`): the
updater reads the published channel itself, verifies it the same way, and
decides with `FLEET_UPDATER_TRACK`, `FLEET_UPDATER_MODE` and
`FLEET_UPDATER_PIN` from its environment.

### Commands and environment

```text
fleet-updater [run]   the loop (the compose service)
fleet-updater once    one pass; exits 1 if it failed or rolled back
fleet-updater status  the state file: what runs, the previous build, the bad list, queued reports
fleet-updater clear   after a rollback_failed is sorted out: clears the phase and the bad list
fleet-updater pair <url-or-code>   redeem `fleet-hub pair --mode updater`'s code; keeps the token
```

| variable | default | what it does |
|---|---|---|
| `FLEET_UPDATER_TOKEN` / `_TOKEN_FILE` | `<state dir>/token` (from `fleet-updater pair`) | the `updater` token |
| `FLEET_UPDATER_HUB_URL` | `http://fleet-hub:4180` | where the hub answers |
| `FLEET_UPDATER_HUB_HOST` | the host of `FLEET_HUB_PUBLIC_URL` | the `Host:` it sends: a public hub accepts only its own names (`fleet-hub.env` is read for this) |
| `FLEET_UPDATER_HUB_CONTAINER` | the compose service `fleet-hub` of its own project | the container it updates |
| `FLEET_UPDATER_HUB_SERVICE` | `fleet-hub` | the compose service to look for |
| `FLEET_UPDATER_HUB_DATA` | `/hub-data` | where the hub's data volume is mounted in the updater |
| `FLEET_UPDATER_STATE_DIR` | `/var/lib/fleet-updater` | the state file, the replay guard, failed logs |
| `FLEET_UPDATER_READY_TIMEOUT_SECS` | `90` | time to become ready |
| `FLEET_UPDATER_SOAK_SECS` | `120` | time to stay ready |
| `FLEET_UPDATER_KEEP_BACKUPS` | `3` | `pre-*.db` backups kept in the data volume |
| `FLEET_UPDATER_INTERVAL_SECS` | `21600` | the longest wait between checks |
| `FLEET_UPDATER_STANDALONE` | off | read the channel instead of a hub |
| `FLEET_UPDATER_TRACK` / `_MODE` / `_PIN` | `stable` / `notify` / — | the standalone policy |
| `DOCKER_HOST` | `unix:///var/run/docker.sock` | the Docker socket (`unix://` only) |

`scripts/updater-e2e.sh` (`scripts/ci-local.sh --updater-e2e`, and CI's
hub-headless job) drives a real Docker daemon through a good image, one that
crashes on start and one that migrates and never gets ready.

## What the hub knows without being asked

- **`X-Fleet-Client`.** A client names its build on every request:

  ```
  X-Fleet-Client: desktop/0.4.1 (macos-aarch64; build 1a2b3c4; contract 5-6)
  ```

  The hub records it as the client's observed version, at most once a
  minute (the same beat as its "last seen"), so the dashboard knows what a
  desktop or a phone runs before it ever calls `/update/check`. Only a
  paired client token is recorded, only for a client component, and only
  what the header says about the build: the phase and the last error stay
  the client's own reports. A missing or garbled header records nothing.
- **What needs a person.** `fleet_health.updates` carries the channel's
  state (`fresh`, `stale`, `none`) and one entry per target that needs a
  person: `update_required` (the hub would refuse it until it updates),
  `update_failed`, `update_rolled_back` and `rollback_failed` (from its
  reported phase), plus `channel_stale` when the verified channel is past
  its signed expiry.
- **Why.** `update_status { target: "client:3" }` is one target's whole
  decision: what it would be offered, why, and whether it is mandatory.
- **Changes.** `/events` carries `update:changed` (ids only) when a
  target's reported build or phase, a pin, or the verified channel
  changes. A per-host token and an org-bound client never receive it; they
  read their own row through `update_status`.
- **Decisions.** `/events` also carries `update:decision` (`{target, status,
  version}`) when what the hub would tell a target moves — an operator's
  pin, a newly verified channel, an `update.*` setting — so a desktop checks
  again within seconds instead of at its next interval. A target seen for
  the first time is not pushed: it was just told by its own check. The same
  streams receive it as `update:changed`; `fleet-updater` (whose token
  reaches `/update/*` only) keeps to its interval.
- **Decisions.** `/events` also carries `update:decision` (`{target, status,
  version}`) when what the hub would tell a target moves — an operator's
  pin, a newly verified channel, an `update.*` setting — so a desktop checks
  again within seconds instead of at its next interval. A target seen for
  the first time is not pushed: it was just told by its own check. The same
  streams receive it as `update:changed`; `fleet-updater` (whose token
  reaches `/update/*` only) keeps to its interval.

## Settings

| key | default | what it does |
|-----|---------|--------------|
| `update.track` | `stable` | the release track the hub follows for its fleet (a standalone desktop's own) |
| `update.hub.mode` | `notify` | the hub: `manual` (pins only), `notify` (offer), `automatic` (needs `fleet-updater`) |
| `update.agent.mode` | `notify` | the agents: `manual`, `notify` or `automatic` |
| `update.desktop.mode` | `notify` | the desktops: `manual`, `notify` or `automatic` |
| `update.mobile.mode` | `notify` | the phones: `manual` or `notify` (a phone never installs silently) |
| `update.check_interval_secs` | `21600` | seconds between reading the release channel, at least 900 |
