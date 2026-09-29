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
> *Update manifest and channels*). The owner's release key exists, and
> every build from 0.4.1 trusts it, so a hub offers what the signed channel
> lists; a hub that cannot verify a channel still offers nothing.
> `fleet-updater`, the desktop and the phone install nothing yet (slices
> S3, S6–S8): on a standalone desktop the Settings → Updates rows have no
> effect until S3. There is no `nightly` channel yet (S2b).

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
| `nightly` | every green `main` commit, as `X.Y.Z-dev.N.g<sha>` (not published yet, and not selectable in `update.track` until S2b) |

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

## Settings

| key | default | what it does |
|-----|---------|--------------|
| `update.track` | `stable` | the release track the hub follows for its fleet (a standalone desktop's own) |
| `update.hub.mode` | `notify` | the hub: `manual` (pins only), `notify` (offer), `automatic` (needs `fleet-updater`) |
| `update.agent.mode` | `notify` | the agents: `manual`, `notify` or `automatic` |
| `update.desktop.mode` | `notify` | the desktops: `manual`, `notify` or `automatic` |
| `update.mobile.mode` | `notify` | the phones: `manual` or `notify` (a phone never installs silently) |
| `update.check_interval_secs` | `21600` | seconds between reading the release channel, at least 900 |
