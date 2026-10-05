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

## Use it

1. In the session, run `/voice` (or `/voice tap` for tap mode).
2. In the terminal header, switch on the 🎤 button. It follows the session you
   are attached to; one session at a time holds the microphone.
3. Hold space and speak; release to finish. The red dot shows only while the
   microphone is open.

On macOS the first recording asks for microphone permission for claude-fleet. If
you denied it, allow it under System Settings -> Privacy & Security ->
Microphone. The microphone indicator turns off when you release.

## Privacy and limits

- The microphone opens only while Claude Code is recording for a session you
  turned 🎤 on for. Nothing is stored on the host, the hub or the app.
- Only that host's own token can ask for audio, and only for a session with a
  claim.
- Turning 🎤 on for another session, or off, ends a live capture.
- `voice.max_capture_secs` (default 300): longest single capture; `0` means no
  limit.
- `voice.claim_ttl_secs` (default 1800): a 🎤 claim lapses after this long;
  `0` means never.

## Troubleshooting

`claude` shows the reason `arecord` printed.

| Message | Meaning and fix |
|---|---|
| HTTP 409 | No claim for this session, or the microphone is busy. Switch 🎤 on for it (or off and on). |
| HTTP 403 | The relay is off. Turn on `voice.enabled`. |
| HTTP 404 | The fleet server does not know this tmux session. |
| HTTP 502 | The microphone failed to open: check the OS permission and the input device. |
| "no fleet voice config" (or a connection error) | The host was not re-provisioned, or the hub is unreachable from it. Re-provision, or use the manual relay below. |
| `/voice` says no recorder, or ignores the relay | The session started before re-provisioning. Restart it. |

### Manual relay fallback

If the fleet path is unavailable, the stand-in falls back to a relay you run
yourself: a microphone server reverse-tunnelled to `127.0.0.1:4713` on the host
(override with `FLEET_VOICE_FALLBACK_PORT`), authenticated by the token in
`~/.config/fleet-voice/token` (sent as the first line).

## Not yet

- Hosts reached through `fleet-agent` (the 🎤 is disabled there).
- The phone as the microphone.
- The Linux desktop (the app records on macOS and Windows only).
