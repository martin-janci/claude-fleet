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
> from 0.4.1 trusts it; until 0.4.1 is released there is no channel to
> read, so nothing is offered yet. `fleet-hub update check` asks the
> channel directly (slice S3). `fleet-updater`, the desktop and the phone
> install nothing yet (slices S6–S8), and there is no `nightly` channel
> yet (S2b).

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
| `nightly` | every green `main` commit, as `X.Y.Z-dev.N.g<sha>` (not published yet) |

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
  also re-reads it every `update.check_interval_secs`. A failed read keeps
  the last good copy, and `update_status.last_refresh` says why it failed.
  `FLEET_UPDATE_CHANNEL_URL` points the hub at a mirror. It changes only
  where the documents come from: what is trusted is still the signature.
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

## Settings

| key | default | what it does |
|-----|---------|--------------|
| `update.track` | `stable` | the release track the hub follows for its fleet (a standalone desktop's own) |
| `update.hub.mode` | `notify` | the hub: `manual` (pins only), `notify` (offer), `automatic` (needs `fleet-updater`) |
| `update.agent.mode` | `notify` | the agents: `manual`, `notify` or `automatic` |
| `update.desktop.mode` | `notify` | the desktops: `manual`, `notify` or `automatic` |
| `update.mobile.mode` | `notify` | the phones: `manual` or `notify` (a phone never installs silently) |
| `update.check_interval_secs` | `21600` | seconds between reading the release channel, at least 900 |
