# Voice relay

Claude Code's `/voice` records from a microphone on the machine it runs on. A
host in a datacentre has none. The voice relay streams the microphone of the
claude-fleet app you are attached with to the session's host, so you can talk
to a remote Claude Code session.

How it works in one line: on the host, a stand-in `arecord` (put first on
`claude`'s `PATH` by fleet) asks the fleet server for audio; the app attached to
that session opens its microphone and streams it back. Audio is never stored.

Design: `docs/superpowers/specs/2026-10-05-voice-relay-design.md`.

## Turn it on

1. **Settings -> Limits -> Voice**: switch on `voice.enabled` (off by default).
2. **Re-provision the host** so it gets the stand-in: `fleet-hub provision
   --host <alias> --content-only` (or re-provision it from the desktop). This
   installs `~/.claude-fleet/voice/bin/arecord` and
   `~/.claude-fleet/voice/voice.env` on the host.
3. **Restart sessions started before** the re-provisioning: the `PATH` is set
   when the session starts.
4. **Standalone desktop only** (not paired to a hub): the host's recorder
   reaches the desktop through its Control API, the embedded MCP server
   (Settings -> Control API (MCP)). It must be running, and reachable from the
   host (the same path the hooks use), or every recording fails to connect.

## Use it

1. In the session, run `/voice` (or `/voice tap` for tap mode).
2. In the terminal header, switch on the 🎤 button. It follows the session you
   are attached to; one session at a time holds the microphone.
3. Hold space and speak; release to finish. The red dot shows only while the
   microphone is open.

On macOS the first recording asks for microphone permission for claude-fleet.
The microphone has 5 s to open, and a permission prompt you have not answered
by then fails that first recording (HTTP 502): answer it and record again. If
you denied it, macOS hands the app a silent microphone, so recordings arrive
empty rather than failing: allow it under System Settings -> Privacy &
Security -> Microphone. The microphone indicator turns off when you release.

## Privacy and limits

- The microphone opens only while Claude Code is recording for a session you
  turned 🎤 on for. Nothing is stored on the host, the hub or the app.
- Only that host's own token can ask for audio, and only for a session with a
  claim.
- Turning 🎤 on for another session, or off, ends a live capture.
- `voice.max_capture_secs` (default 300): longest single capture; `0` means no
  limit.
- `voice.claim_ttl_secs` (default 1800): a session's 🎤 turns itself off after
  this long without a recording; `0` means never. The lapse is noticed by the
  next recording, which fails with HTTP 409 while the 🎤 turns off saying
  "microphone idle — turn 🎤 on again".

## Troubleshooting

`claude` shows the reason `arecord` printed.

| Message | Meaning and fix |
|---|---|
| HTTP 409 | No claim for this session, or the microphone is busy. Switch 🎤 on for it (or off and on). |
| HTTP 403 "voice relay is off" | The relay is off. Turn on `voice.enabled`. |
| HTTP 403 "only a host's recorder captures" | The bearer in `~/.claude/fleet-hook.headers` is not this host's own token (a person's or client's token was put there). Re-provision the host. |
| HTTP 400 "bad tmux name" | The recorder's tmux session has a name fleet never gives a session (characters it refuses), so `claude` is running in a tmux session fleet does not manage. Use `/voice` in a fleet session. |
| HTTP 404 | The fleet server does not know this tmux session. |
| HTTP 502 | The microphone failed to open: check the OS permission and the input device. |
| "no fleet voice config" (or a connection error) | The host was not re-provisioned, or the hub is unreachable from it. Re-provision, or use the manual relay below. |
| `/voice` says no recorder, or ignores the relay | The session started before re-provisioning. Restart it. |
| 🎤 turned off: "microphone claimed elsewhere" | Another app or device switched on the microphone for this session. Switch 🎤 on again to take it back. |
| 🎤 turned off: "microphone idle — turn 🎤 on again" | The claim lapsed (`voice.claim_ttl_secs`). Switch it on again. |

### Manual relay fallback

If the fleet path is unavailable (it cannot be reached, or answers with an HTTP
error before any audio), the stand-in falls back to a relay you run yourself; a
recording cut off part-way is never continued from it. The relay is a
microphone server reverse-tunnelled to `127.0.0.1:4713` on the host (override
with `FLEET_VOICE_FALLBACK_PORT`), authenticated by the token in
`~/.config/fleet-voice/token` (sent as the first line).

## Not yet

- Hosts reached through `fleet-agent` (the 🎤 is disabled there).
- The phone as the microphone.
- The Linux desktop (the app records on macOS and Windows only).
