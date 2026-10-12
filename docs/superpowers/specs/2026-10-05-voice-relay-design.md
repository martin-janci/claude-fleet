# Voice relay: Claude Code `/voice` on a remote host, microphone in the app

Status: design, approved in conversation 2026-10-05. Phase F1 (desktop) is
specified here; F2 (phone) and F3 (agent-transport hosts) are outlined only.

## Problem

Claude Code's `/voice` records on the machine where `claude` runs. On Linux it
tries, in order: its native capture module (only when `/proc/asound/cards`
lists a card), `arecord`, then SoX `rec`, and reads raw PCM
(S16LE, 16 kHz, mono) from the recorder's stdout. A fleet host is a server
with no sound card, so `/voice` reports "host has no microphone". The
microphone is on the machine the person sits at: the desktop app, or the
phone.

A manual prototype proved the path end to end (2026-10-05): an `arecord`
stand-in on the host reading PCM from `127.0.0.1:4713`, reverse-tunnelled
from a small server on the Mac that runs `sox` only while a connection is
open. `/voice` worked on `claude-fleet-trn`, `-oci` and `mefistos`. This
design moves that into fleet so it needs no tunnels from the Mac and reaches
the phone too.

Claude Code has two voice modes: `hold` (hold space to record) and `tap`
(tap space with the input empty to start, tap again to send). F2 relies on
`tap`.

## Decisions

- **Native `/voice`, not app-side dictation.** Fleet supplies the microphone;
  Claude Code still does the speech-to-text and owns the UX.
- **Relay through the fleet MCP server** (approach 1). The host reaches it
  over the reverse tunnel it already has for hooks (port 4180); the client
  supplies audio to it. One host-side recorder, one route, every client.
  Rejected: a reverse forward on the desktop's terminal ssh (no phone, no
  hub-paired relay) and a staged mix of the two (most of stage one thrown
  away).
- **The microphone opens only while a recording is connected**, and only for
  a session a person has explicitly claimed the microphone for.
- **Phasing:** F1 desktop (standalone and hub-paired) → F2 phone → F3
  agent-transport hosts.

## Architecture (F1)

### Host: the `arecord` stand-in

- Installed by provisioning to `~/.claude-fleet/voice/bin/arecord`
  (fleet-owned, like the hook files). Fleet prefixes that directory to
  `PATH` in the command that starts `claude` in tmux, so it wins over a real
  `/usr/bin/arecord` (seen on `mefistos`) and does not depend on the
  non-interactive shell's `PATH` (seen on `-htz`, `-oci`). A session started
  before provisioning picks it up on its next restart.
- Reads `~/.claude-fleet/voice/voice.env`, written by provisioning: the fleet
  base URL as the host sees it (URL only); the bearer is read with `curl -H
  @~/.claude/fleet-hook.headers`.
- Claude Code calls exactly three forms; the stand-in accepts only these:
  - `arecord --version` → exit 0.
  - `arecord -f S16_LE -r 16000 -c 1 -t raw /dev/null` (probe, killed after
    150 ms; memoised for the life of the `claude` process) → sleep, exit 0,
    regardless of whether a claim exists, so a claim made later still works.
  - `arecord -f S16_LE -r 16000 -c 1 -t raw -q -` → finds its tmux session
    name (`tmux display-message -p -t "$TMUX_PANE" '#S'`, without `-t` when
    `TMUX_PANE` is unset; `TMUX` is inherited), then
    `curl -sN --fail -H @~/.claude/fleet-hook.headers
    "<base>/voice/capture?tmux=<name>"` with stdout passed through.
  - Claude Code ends a recording with SIGTERM to `arecord` and waits for its
    stdout to close, so curl runs in the background under a TERM/INT/HUP/PIPE
    trap that kills it (and removes the stand-in's temp file).
- Fallback: when the fleet request fails before any audio (cannot connect,
  an HTTP error such as 409), it tries the manual `127.0.0.1:4713` path with
  `~/.config/fleet-voice/token` if that file exists, else exits 1 with a
  one-line reason on stderr. A stream cut part-way exits 1 and is never
  continued from the fallback.

### Server: `GET /voice/capture`

- New route in `mcp/` beside `/downloads`, behind `authorize`. Host tokens
  only; person and client tokens are refused.
- Resolves the session by `(caller's host, tmux name)`; a name on another
  host is a 404, never a cross-host lookup.
- Asks `VoiceRegistry` for the claim on that session. None → 409, and no
  microphone is opened. Otherwise starts the claim's source and streams its
  PCM as the response body (`application/octet-stream`, chunked) until the
  client disconnects, the source ends, or `voice.max_capture_secs` passes.
- At most one capture per session at a time; a second gets 409.

### `VoiceRegistry` (fleet-core, `service/voice/`)

- Per session, at most one claim: `{ owner (Caller identity), source,
  claimed_at, last_used_at }`. Last claim wins; a replaced owner's source is
  told so (`VoiceSource::revoked(Replaced)`), and so is an expired one
  (`Expired`, found lazily by the next capture). `/voice/source` closes a
  replaced socket with 4001 "microphone claimed elsewhere" and an expired one
  with 4002 "microphone idle — turn 🎤 on again"; it pings the device every
  20 s so a proxy's idle timeout does not drop a claim. Each ping also
  re-checks the caller: a device revoked or re-bound, or one whose grant no
  longer drives the session, is closed with 4003 (review r04 K3). The owner's own
  release is not a revocation.
- A claim ends on the desktop's `voice_release`, on its source going away
  (websocket closed, desktop detached), or after `voice.claim_ttl_secs`
  unused.
- `VoiceSource` is a trait: `start(sink) -> CaptureHandle`, dropped to stop.
  fleet-core knows nothing about audio devices.
- The desktop emits a local `voice:state` Tauri event: `claimed`,
  `capturing`, `stopped` (a capture ended; the UI goes back to claimed only
  from capturing, since it can follow a release), `released` (with the
  reason when the claim was taken or lapsed), `error`.

### Desktop

- `src-tauri/src/voice.rs` (desktop only, so the hub never links audio
  libraries): `cpal` capture (CoreAudio on macOS, WASAPI on Windows) from the
  default input, downmixed to mono and resampled by linear interpolation to
  16 kHz S16LE, sent in ~100 ms (3,200 B) chunks.
- **Standalone:** the desktop is the MCP server, so it registers an
  in-process `VoiceSource` with the registry directly.
- **Hub-paired:** while a claim is active, `src-tauri/src/backend/` holds a
  websocket to the hub's new `GET /voice/source?session_id=…` (client token;
  `tokio-tungstenite`, already in the tree through axum). The hub's
  `VoiceSource` for that claim sends text `{"start": <capture id>}` /
  `{"stop": …}`; the desktop answers with binary PCM frames between them.
  The websocket carries no audio while idle.
- **macOS permission:** `NSMicrophoneUsageDescription` through
  `bundle.macOS.infoPlist` ("claude-fleet streams your microphone to Claude
  Code's /voice on the host you are attached to, only while you record."),
  and the `com.apple.security.device.audio-input` entitlement so a future
  hardened runtime keeps working. The prompt appears on the first capture.
  A dev build (`pnpm tauri dev`) is attributed to the launching terminal.
- **UI:** a 🎤 toggle in the terminal header (`TerminalView.svelte`, beside
  the Transfer chip): grey off, highlighted when claimed, a red dot while
  capturing (from `voice:state`). Disabled with a tooltip when the host is
  agent-transport, not provisioned for voice, or `voice.enabled` is off.
  The claim follows the attached session (only one PTY is attached at a
  time). First use shows: "Run `/voice` in the session, then hold space."
  Fleet never runs `/voice` itself.
- **Commands:** `voice_claim { session_id, on }` and `voice_release { session_id }`
  are `SameInBoth` Tauri commands.

### Data flow

```
🎤 on → voice_claim{session_id, on:true} (desktop command) → registry: claim(session, source)
/voice, hold space → claude spawns arecord (stand-in)
  → curl -N <base>/voice/capture?tmux=<name>   (host token, over the tunnel)
  → registry: claim? → source.start()          (microphone opens)
  ← PCM 32 KB/s ← Claude Code transcribes
release space → claude kills arecord → connection drops
  → CaptureHandle dropped → source stops      (microphone closes)
```

## Security

- `/voice/capture`: a host token, for a session on that host only.
- `voice_claim` is a desktop command (standalone and hub-paired). `/voice/source`
  is the only hub-side claim surface (master or paired `full` device, for a
  session inside its `OrgScope`; readonly, peer and host tokens are refused).
  Rows in `mcp/tools/tests_isolation.rs`.
- No claim, no microphone. A claim exists only through a person's action.
- Limits: `voice.max_capture_secs` (default 300), one capture per session,
  `voice.claim_ttl_secs` (default 1800).
- `voice.enabled` (default **off**). All `voice.*` settings are `SPECS` rows
  with a field on a settings page; docs regenerated.
- Audio is never stored: no disk, no database, no log. The log records
  start, stop, session and duration only.

## Errors

| Case | What happens |
|---|---|
| No claim / fleet down | stand-in tries the 4713 fallback, else exit 1 with a reason; Claude Code reports a failed recording |
| macOS denies the microphone | `voice:state` error; header shows "Microphone denied — System Settings → Privacy → Microphone"; capture response closed |
| hub ↔ desktop websocket drops mid-capture | capture closed, claim released |
| capture exceeds the limit | server closes the response |
| second capture on the same session | 409 |

## Testing

- Rust: `VoiceRegistry` (claim, last wins, TTL, release on source drop);
  `/voice/capture` auth (foreign host, unknown tmux name, no claim, busy,
  limit); downmix/resample over fixed inputs; a fleet-core loopback test
  drives `/voice/capture` and `/voice/source` together.
- Stand-in: a shell test against a fake server: `--version`, probe, stream,
  rejected token, 4713 fallback.
- Frontend: the 🎤 toggle's states (Vitest).
- Manual: real `cpal` capture on a Mac (not in CI).

## Later phases (outline)

- **F2, phone (fleet-mobile):** a 🎤 button on the session screen. It claims
  the microphone, then sends one Space key to the pane (Claude Code in
  `tap` mode starts recording), streams PCM over `/voice/source`, and sends
  Space again to stop and send. Needs `Space` in `send_prompt`'s key
  allowlist (or a `voice_tap` tool) and `RECORD_AUDIO` /
  `NSMicrophoneUsageDescription` in the app. The session must be in `tap`
  mode; the phone says so when it is not.
- **F3, agent-transport hosts:** a stream frame pair in `fleet-proto`
  (proto bump) so the agent can serve `/voice/capture` locally and carry it
  over its websocket.

## Revisions (plan, 2026-10-05)

1. No `voice_claim` MCP tool and no `voice_status`: a hub-paired client claims by opening `/voice/source`; closing it releases. The desktop's Tauri commands `voice_claim` / `voice_release` are `SameInBoth` (in-process source standalone, websocket when paired).
2. No `voice:changed` row event in fleet-core: only the desktop knows it is capturing, so `src-tauri` emits a desktop-local Tauri event `voice:state`.
3. The host stand-in reuses `~/.claude/fleet-hook.headers` for its bearer, so `voice.env` carries only the URL. The hub-e2e section is replaced by a fleet-core loopback test (route + websocket) — `scripts/hub-e2e.sh` has no websocket client.
