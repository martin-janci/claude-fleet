# Hub, client access, agent and hub-client mode: the live acceptance

The record of the first live acceptance pass asked for in
[#156](https://github.com/martin-janci/claude-fleet/issues/156): the
hub, paired clients, the agent transport and the desktop in hub-client
mode, run against real machines rather than loopback.

The automated half is `scripts/hub-e2e.sh` (91 checks against real
`fleet-hub` and `fleet-agent` binaries, CI's `hub-headless` job). It covers
pairing, `GET /events`, `client list|revoke`, `--local-host false` and the
agent leg on one machine over plaintext loopback. This page records the half
that needs real hardware: a remote hub, TLS, a phone, systemd, a Mac.

**An agent may record evidence; only a person ticks a step that needs one.**
Each row says who observed it and how. A row an agent filled from logs or a
test is marked *agent*; the owner confirms or overturns it.

The hub itself is explained in [hub.md](hub.md).

## Run record

| | |
|---|---|
| Date(s) | 2026-10-07 (first entries, from logs going back to 2026-09-25) |
| Hub version | 0.5.2 (from the desktop's `[hub events] subscribed` line) |
| Hub deployment | published image `ghcr.io/martin-janci/fleet-hub` in Docker on the NAS; TLS terminated in front of it (Cloudflare tunnel → Caddy), not by `--tls cert` |
| Hub URL | `https://fleet.rlt.sk` |
| Desktop | 0.5.2 on macOS, schema 111, hub-client mode, client `mac-desktop` |
| fleet-mobile | not yet recorded |
| Agent host | `claude-fleet-trn` (via `fleet-agent`), not yet exercised for this page |

## Results

Status: **pass**, **fail**, **partial** (part seen, part still open) or
**open** (not run).

### 1. Hub on a remote server from the published image

| Step | Status | Observed by | Notes |
|---|---|---|---|
| Hub runs from the published image on a remote server | pass | agent | `GET /healthz` → `200 fleet-hub ok` through Cloudflare and Caddy; upgraded in place 0.4.7 → 0.5.2 across the logged period |
| Hub with `--tls cert` | open | | The deployment terminates TLS in front of the hub. Needs a hub started with `--tls cert` and a real certificate |
| Copied `state.db`, friendly names backfilled on the hub | open | | `serve` does not run `backfill_friendly_names()`; check the names a migrated database shows |

### 2. Pairing CLI against TLS

| Step | Status | Observed by | Notes |
|---|---|---|---|
| `fleet-hub pair` against a TLS hub (#142, fixed in #162) | open | | Never run against a real certificate |

### 3. Phones

| Step | Status | Observed by | Notes |
|---|---|---|---|
| A phone paired `full` | open | | |
| A phone paired `readonly`: reads work, writes refused | open | | |
| Revoke one while its `/events` stream is open: the stream ends, the next call is 401, the other phone keeps working | open | | `hub-e2e.sh` revokes only between requests |

### 4. Agent host

| Step | Status | Observed by | Notes |
|---|---|---|---|
| `fleet-agent install` on real systemd | open | | |
| Reconnect across a hub restart | open | | |
| A large upload | open | | |
| A cancelled command (`Cancel` on drop, #162) | open | | Unit-tested only |
| `Host` header carries the literal `hub:443` against `allowed_hosts` (#159) | open | | |
| The same host on a loopback hub, no `ssh -R` loop (#141) | open | | |

### 5. Desktop paired to the hub

| Step | Status | Observed by | Notes |
|---|---|---|---|
| Launch in hub-client mode | pass | agent | 234 `[hub events] subscribed` lines in the app log from 2026-09-25 to 2026-10-07, contract 8 |
| macOS keychain arm of `token_store.rs` | pass | agent | Live: every launch logs `hub client token read finished … found=true` in 25–38 ms. Tests: `keychain_tests` in `token_store.rs` run `get` / `set` / `clear` against the real login keychain under a service of their own (see below) |
| Pull the network, reconnect | pass | agent | e.g. 2026-10-02 11:13:13 `connect fleet.rlt.sk:443: Network is unreachable`, subscribed again at 11:13:53; also reconnects through 502 (hub restart on upgrade), 530/1033 (tunnel down), Cloudflare stream resets, and the silence watchdog |
| Backfill after the reconnect | pass | agent | 43 × `the hub replayed the gap; no re-list` |
| Contract skew is refused, not trusted | pass | agent | Both directions seen live: `wire contract is newer than this build understands` (71) and `older than this build requires` (9) |
| Configured hub without a stored token | pass | agent | Starts nothing of its own and says so: `hub: https://fleet.rlt.sk is configured but no client token is stored` |
| Steer a session (send a prompt, kill, re-create the same name: it must not come back, #170/#171) | open | | |
| New session dialog lists a remote host's existing worktrees (#168) | open | | |
| "Cancel creation" is replaced by the explanatory line (#191) | open | | |
| `http://` hub needs `hub.client_plaintext_token` ticked again (#159) | open | | |
| Un-pair (Disconnect, restart, the local fleet comes back) | open | | |

### 6. Local mode

| Step | Status | Observed by | Notes |
|---|---|---|---|
| `pnpm tauri dev` smoke run in local mode | open | | A dev build terminates the installed app: quit it first. Not run by an agent on this Mac, because the installed app is what drives the fleet |

## Running the keychain tests

On a Mac, with the login keychain unlocked:

```bash
cargo fleet-test -- token_store::keychain_tests
```

Each test uses the service `claude-fleet-test-<tag>-<pid>`, never
`claude-fleet`, so the machine's real pairing is not read or replaced, and
each clears its item at the end. First run 2026-10-07 on macOS (Apple
silicon, inside a tmux session): 3 passed, no `claude-fleet-test-*` item left
in the keychain. A locked keychain (a fresh SSH login, for example) fails
them with `keychain … failed`; run `security unlock-keychain` first.

## Found issues

| # | Step | What | Issue |
|---|---|---|---|
| | | | |
