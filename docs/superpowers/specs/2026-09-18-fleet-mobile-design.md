# fleet-mobile: the phone client

**Date:** 2026-09-18
**Status:** Approved design, awaiting implementation plan
**Scope:** a new repository, `martin-janci/fleet-mobile` — a Kotlin Multiplatform app with a
shared Compose Multiplatform UI for Android and iOS, talking to a `fleet-hub`
over its existing HTTP API. No change to `claude-fleet`.

## Why

Sub-projects 1 and 2 put the fleet behind one always-on hub reachable over
HTTPS, with a credential a phone can hold, a live event stream, and the
conversation available as a tool. This is the client those were built for: see
every session across every machine from a phone, read what an agent is doing,
and steer it.

This is sub-project 3 of five. It is the original ask.

## Goals

- **One UI for both platforms.** Compose Multiplatform, so screens are written
  once. Platform code is limited to what genuinely differs: secure storage,
  camera, notifications, lifecycle.
- **Pair by scanning.** Point the camera at the QR code `fleet-hub pair`
  prints, or paste the code. The phone ends up holding its own token.
- **See the fleet.** Sessions grouped by host and project, with status,
  what each is doing, and which need a human.
- **Read and steer.** A session's conversation as structured turns, and a
  prompt box that sends to it.
- **Stay live.** The event stream drives the list; no pull-to-refresh ritual,
  though a manual refresh exists.

## Non-goals

- A terminal emulator. The desktop app owns the PTY; a phone shows the
  conversation, not the raw pane.
- Editing files or reviewing diffs. Read-only repository views may come later.
- Host administration. The hub refuses a phone every fleet-admin tool, so the
  app does not pretend to offer provisioning, adding or removing hosts.
- Push notifications while the app is closed. That needs a vendor service and
  a hub-side sender; it is its own sub-project.
- Offline editing or a local mirror of fleet state. The app caches what it has
  seen so a cold start has something to draw, and refetches.

## Architecture

```
fleet-mobile/
  shared/                     Kotlin Multiplatform library
    commonMain/               models, HubClient, repositories, view models, Compose UI
    commonTest/               everything testable without a device
    androidMain/ iosMain/     expect/actual: secure storage, camera, platform bits
  androidApp/                 thin Android host: one Activity, Compose entry point
  iosApp/                     thin SwiftUI host embedding the shared UI
```

- **Language and UI:** Kotlin Multiplatform with Compose Multiplatform. The
  Android app is Jetpack Compose; iOS renders the same composables.
- **Networking:** Ktor client (OkHttp engine on Android, Darwin on iOS) with
  `kotlinx.serialization`. Two shapes: JSON-RPC `tools/call` over `POST /mcp`
  for everything, and an SSE subscription on `GET /events`.
- **State:** a `FleetRepository` owns an in-memory snapshot of sessions, hosts
  and projects, applies event frames to it, and exposes `StateFlow`s. Screens
  observe; nothing polls except a bounded refresh on resume.
- **Storage:** the hub URL and token live in platform secure storage —
  `EncryptedSharedPreferences` on Android, the Keychain on iOS — behind one
  `expect` interface. Nothing else is persisted except a small cache of the
  last session list, so a cold start draws something immediately.

### Talking to the hub

One client type wraps the three shapes the hub offers:

| Call | Shape |
|---|---|
| Pair | `POST /pair` with the scanned code; returns the token and the hub's own base URL |
| Anything else | `POST /mcp`, JSON-RPC `tools/call`, SSE-framed reply — take the `data:` line |
| Live updates | `GET /events`, SSE, `event:` name and a JSON payload per frame |

Every request carries `Authorization: Bearer <token>`. The tools the app uses:
`list_sessions`, `list_hosts`, `list_projects`, `session_conversation`,
`send_prompt`, `capture_session`, `session_history`, `wait_for_session`,
`fleet_health`. The app never calls a tool the hub would refuse a client, so a
refusal is a bug, not a flow.

### Screens

1. **Pair** — when no token is stored: a camera view with a manual-entry
   fallback, then a success screen naming the hub.
2. **Sessions** — the home screen. Grouped by host, then project. Each row:
   name, status, and the one-line activity the hub already exposes. A filter
   for "needs attention" (blocked or stuck).
3. **Session** — the conversation as turns, newest at the bottom; a prompt box;
   the session's status and host; actions limited to what a client may do.
4. **Hosts** — reachability, the Claude and tmux versions, and the session
   count per host.
5. **Settings** — which hub, which client name, a way to forget the token
   (which does not revoke it; revocation is the operator's, from the terminal),
   and the app's own version.

### Events

The app subscribes on resume and drops the subscription on background. A
`ready` frame carries the hub's version; a `lagged` frame or any disconnect
triggers one full refetch and a resubscribe with backoff. Frames are applied
to the snapshot by id: session rows replace, `session:killed` removes, host and
project rows replace.

### Failure and refusal

- No network, or the hub unreachable: the last snapshot stays on screen with a
  banner; actions are disabled rather than hidden.
- `401`: the token is gone or revoked. The app drops it and returns to Pair
  with an explanation, since only the operator can issue another.
- `403`: the host name is not what the hub expects. Shown verbatim with the
  hub URL, because that is a configuration problem the operator must fix.
- A tool answering `isError` shows the `E_*` code and message as-is. These are
  written for a person and hiding them would be worse.

## Testing

- **Shared, on the JVM:** the JSON-RPC envelope (including the SSE `data:`
  framing the hub returns), event-frame application to the snapshot, the
  pairing flow against a fake client, the refusal paths (401, 403, `isError`),
  and reconnect with backoff. A fake `HubClient` backed by recorded responses,
  so no network is needed.
- **Android instrumentation:** deliberately minimal — secure storage round
  trip and the camera permission path, since those are the platform pieces.
- **Manual, on device:** pair with a real hub, watch a session update live,
  send a prompt and see the reply arrive.

## Build and CI

Gradle with the Kotlin Multiplatform and Compose plugins; the Android
application module builds a debug APK. iOS is configured but builds only on a
Mac with Xcode, so CI builds the shared library and the Android app, and the
iOS target is the developer's own `xcodebuild` on the Mac. GitHub Actions runs
`./gradlew :shared:jvmTest :androidApp:assembleDebug` on Linux.

## Open questions settled here

1. **Compose Multiplatform for iOS rather than SwiftUI.** The ask was one
   shared view; Compose on iOS is stable enough for a tool used by its author,
   and a SwiftUI rewrite of five screens is the alternative.
2. **No hub administration in the app**, because the hub refuses it anyway.
3. **The app never stores the master token**, only a paired client token, so a
   lost phone is revoked from the terminal without touching anything else.
