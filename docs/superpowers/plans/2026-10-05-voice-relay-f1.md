# Voice relay F1 (desktop) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Claude Code's `/voice` in a session on a remote SSH host records from the microphone of the desktop app attached to it — standalone or paired to a hub.

**Architecture:** A fleet-owned `arecord` stand-in on the host streams PCM from `GET /voice/capture` on the fleet MCP server (over the hook tunnel, with the host's token). A process-global `VoiceRegistry` maps a session to the one microphone *claim* for it; a claim is a `VoiceSource` that opens the microphone only while a capture is connected. The standalone desktop registers an in-process `cpal` source; a hub-paired desktop holds a websocket to `GET /voice/source` on the hub, and that connection *is* the claim.

**Tech Stack:** Rust (axum 0.8 + ws, tokio, tokio-tungstenite 0.29 handshake-only, cpal on macOS/Windows), Svelte 5, Vitest, bash.

**Spec:** `docs/superpowers/specs/2026-10-05-voice-relay-design.md` (amended in Task 0 with the three simplifications below).

## Global Constraints

- PCM everywhere: S16LE, 16 000 Hz, 1 channel; chunks of ~100 ms (3 200 B).
- Settings: `voice.enabled` (Bool, default `false`), `voice.max_capture_secs` (Secs, default `300`), `voice.claim_ttl_secs` (Secs, default `1800`, `0` = never expires).
- Audio is never written to disk, the database, or a log. Logs carry start/stop, session id and duration only.
- `/voice/capture`: per-host token only, session resolved on the caller's own host. `/voice/source`: master or a paired client, not `readonly`/`peer`/`updater`, never a host token; the session must be inside the caller's `OrgScope`.
- No microphone without a claim; at most one capture per session at a time.
- Host stand-in path `~/.claude-fleet/voice/bin/arecord`; config `~/.claude-fleet/voice/voice.env`; bearer from the existing `~/.claude/fleet-hook.headers`.
- `cpal` is a dependency only on `cfg(any(target_os = "macos", target_os = "windows"))`; on Linux the desktop reports voice unsupported.
- Every child process via `fleet_core::proc::command`; every interpolated shell value via `crate::shell::quote`; never hold the `Store` guard across `.await`.
- Validation ladder from CLAUDE.md: `cargo fleet-fast-check` while working, `cargo fleet-check` + the task's `cargo fleet-test -- <filter>` before each commit, `cargo fmt --all --check` and `cargo fleet-lint` before each commit.

## Deviations from the spec (applied to the spec in Task 0)

1. No `voice_claim` MCP tool and no `voice_status`: a hub-paired client claims by opening `/voice/source`; closing it releases. The desktop's Tauri commands `voice_claim` / `voice_release` are `SameInBoth` (in-process source standalone, websocket when paired).
2. No `voice:changed` row event in fleet-core: only the desktop knows it is capturing, so `src-tauri` emits a desktop-local Tauri event `voice:state`.
3. The host stand-in reuses `~/.claude/fleet-hook.headers` for its bearer, so `voice.env` carries only the URL. The hub-e2e section is replaced by a fleet-core loopback test (route + websocket) — `scripts/hub-e2e.sh` has no websocket client.

## File map

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/voice/mod.rs` (new) | `VoiceRegistry`, `VoiceSource`, `Capture`, global `registry()` |
| `crates/fleet-core/src/service/voice/tests.rs` (new) | registry unit tests |
| `crates/fleet-core/src/mcp/voice_route.rs` (new) | `handle_capture`, `handle_source`, `WsSource` |
| `crates/fleet-core/src/mcp/voice_route_tests.rs` (new) | loopback HTTP + websocket tests |
| `crates/fleet-core/src/mcp/mod.rs` | register both routes |
| `crates/fleet-core/src/service/settings.rs`, `crates/fleet-core/pages/settings.limits.json`, `src/lib/fleet_settings.ts` | the three settings |
| `tools/voice/arecord` (new) | host stand-in (bash) |
| `scripts/voice-arecord-test.sh` (new) | stand-in tests against a fake server |
| `crates/fleet-core/src/service/provision.rs` | install stand-in + `voice.env`; fingerprint |
| `crates/fleet-core/src/tmux.rs` | `PATH` prefix in `pane_command_with` |
| `src-tauri/src/voice/mod.rs`, `capture.rs`, `resample.rs`, `hub_source.rs` (new) | desktop capture, conversion, hub websocket |
| `src-tauri/src/commands/voice.rs` (new) | `voice_claim`, `voice_release` |
| `src-tauri/src/backend/verdicts.rs`, `src-tauri/src/lib.rs` | rows + registration |
| `src-tauri/Info.plist`, `src-tauri/Entitlements.plist` (new), `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` | permissions, deps |
| `src/lib/voice.ts`, `src/lib/MicToggle.svelte`, `src/lib/MicToggle.test.ts` (new), `src/lib/TerminalView.svelte` | UI |
| `docs/voice.md` (new), `CLAUDE.md` | user guide, orientation |

---

### Task 0: Amend the spec

**Files:** Modify `docs/superpowers/specs/2026-10-05-voice-relay-design.md`

- [ ] **Step 1:** Add a section `## Revisions (plan, 2026-10-05)` at the end listing the three deviations above verbatim, and in the body: replace "MCP tool `voice_claim`" / "`voice_status`" with the `SameInBoth` Tauri commands; replace the `voice:changed` bullet under `VoiceRegistry` with "the desktop emits a local `voice:state` Tauri event"; change the stand-in's config to "`voice.env` (URL only); bearer read with `curl -H @~/.claude/fleet-hook.headers`"; replace the hub-e2e testing bullet with "a fleet-core loopback test drives `/voice/capture` and `/voice/source` together".
- [ ] **Step 2:** Commit: `git add docs/superpowers/specs/2026-10-05-voice-relay-design.md && git commit -m "docs(voice): spec revisions from planning"`

---

### Task 1: `VoiceRegistry`

**Files:**
- Create: `crates/fleet-core/src/service/voice/mod.rs`, `crates/fleet-core/src/service/voice/tests.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (add `pub mod voice;` in alphabetical position)

**Interfaces — Produces:**
```rust
pub const PCM_QUEUE: usize = 64;
pub type PcmTx = tokio::sync::mpsc::Sender<Vec<u8>>;
pub type PcmRx = tokio::sync::mpsc::Receiver<Vec<u8>>;
pub trait VoiceSource: Send + Sync {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String>;
}
pub enum CaptureRefusal { NoClaim, Busy, SourceFailed(String) }   // Debug, PartialEq
pub struct Capture { pub rx: PcmRx, /* private guards */ }
pub struct VoiceRegistry;
impl VoiceRegistry {
    pub fn new() -> Self;
    pub fn claim(&self, session_id: i64, owner: &str, source: Arc<dyn VoiceSource>) -> u64;
    pub fn release(&self, session_id: i64, claim_id: u64) -> bool;
    pub fn owner(&self, session_id: i64) -> Option<String>;
    pub fn begin_capture(self: &Arc<Self>, session_id: i64, ttl: Duration) -> Result<Capture, CaptureRefusal>;
}
pub fn registry() -> &'static Arc<VoiceRegistry>;
```

- [ ] **Step 1: Write the failing tests** in `service/voice/tests.rs`:

```rust
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A source that counts starts and live guards, and pushes one chunk.
#[derive(Default)]
struct Fake { starts: AtomicUsize, live: Arc<AtomicUsize>, fail: bool }
struct Live(Arc<AtomicUsize>);
impl Drop for Live { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); } }
impl VoiceSource for Fake {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        if self.fail { return Err("denied".into()); }
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.live.fetch_add(1, Ordering::SeqCst);
        tx.try_send(vec![1, 2, 3, 4]).unwrap();
        Ok(Box::new(Live(Arc::clone(&self.live))))
    }
}
const TTL: Duration = Duration::from_secs(60);

#[tokio::test]
async fn no_claim_means_no_capture_and_no_microphone() {
    let reg = Arc::new(VoiceRegistry::new());
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::NoClaim));
}

#[tokio::test]
async fn a_capture_reads_the_source_and_dropping_it_closes_the_microphone() {
    let reg = Arc::new(VoiceRegistry::new());
    let src = Arc::new(Fake::default());
    reg.claim(7, "master", src.clone());
    let mut cap = reg.begin_capture(7, TTL).unwrap();
    assert_eq!(cap.rx.recv().await, Some(vec![1, 2, 3, 4]));
    assert_eq!(src.live.load(Ordering::SeqCst), 1);
    drop(cap);
    assert_eq!(src.live.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn one_capture_per_session_at_a_time() {
    let reg = Arc::new(VoiceRegistry::new());
    reg.claim(7, "master", Arc::new(Fake::default()));
    let first = reg.begin_capture(7, TTL).unwrap();
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::Busy));
    drop(first);
    assert!(reg.begin_capture(7, TTL).is_ok());
}

#[tokio::test]
async fn the_last_claim_wins_and_a_stale_release_is_ignored() {
    let reg = Arc::new(VoiceRegistry::new());
    let a = reg.claim(7, "client:phone", Arc::new(Fake::default()));
    let b = reg.claim(7, "master", Arc::new(Fake::default()));
    assert!(!reg.release(7, a));
    assert_eq!(reg.owner(7).as_deref(), Some("master"));
    assert!(reg.release(7, b));
    assert_eq!(reg.owner(7), None);
}

#[tokio::test]
async fn a_failed_source_frees_the_session() {
    let reg = Arc::new(VoiceRegistry::new());
    reg.claim(7, "master", Arc::new(Fake { fail: true, ..Default::default() }));
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::SourceFailed("denied".into())));
    // not left busy
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::SourceFailed("denied".into())));
}

#[tokio::test(start_paused = true)]
async fn an_unused_claim_expires_after_the_ttl_and_zero_means_never() {
    let reg = Arc::new(VoiceRegistry::new());
    reg.claim(7, "master", Arc::new(Fake::default()));
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::NoClaim));
    assert_eq!(reg.owner(7), None);
    reg.claim(8, "master", Arc::new(Fake::default()));
    tokio::time::advance(Duration::from_secs(10_000)).await;
    assert!(reg.begin_capture(8, Duration::ZERO).is_ok());
}
```

- [ ] **Step 2: Run, expect a compile failure** (`VoiceRegistry` undefined): `cargo fleet-test -- service::voice`
- [ ] **Step 3: Implement** `service/voice/mod.rs`:

```rust
//! The microphone relay's one piece of shared state: which source supplies
//! the audio for a session's Claude Code `/voice`. See
//! docs/superpowers/specs/2026-10-05-voice-relay-design.md.
//!
//! A claim is made by a person's action (the desktop's 🎤, a client's
//! `/voice/source` websocket) and holds a `VoiceSource`. The source opens
//! the microphone only inside `start`, and closes it when the guard it
//! returned is dropped — which `Capture` does when the host's recorder
//! hangs up. Audio passes through in memory only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::time::Instant;

pub const PCM_QUEUE: usize = 64;
pub type PcmTx = tokio::sync::mpsc::Sender<Vec<u8>>;
pub type PcmRx = tokio::sync::mpsc::Receiver<Vec<u8>>;

/// Something that can open a microphone: S16LE, 16 kHz, mono into `tx`
/// until the returned guard is dropped.
pub trait VoiceSource: Send + Sync {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureRefusal {
    NoClaim,
    Busy,
    SourceFailed(String),
}

struct Claim {
    id: u64,
    owner: String,
    source: Arc<dyn VoiceSource>,
    last_used: Instant,
    busy: bool,
}

pub struct VoiceRegistry {
    claims: Mutex<HashMap<i64, Claim>>,
    next: AtomicU64,
}

impl Default for VoiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// An open capture. Dropping it stops the source and frees the session.
pub struct Capture {
    pub rx: PcmRx,
    _source: Box<dyn Send>,
    _busy: BusyGuard,
}

struct BusyGuard {
    reg: Arc<VoiceRegistry>,
    session_id: i64,
    claim_id: u64,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut claims = self.reg.claims.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = claims.get_mut(&self.session_id) {
            if c.id == self.claim_id {
                c.busy = false;
                c.last_used = Instant::now();
            }
        }
    }
}

impl VoiceRegistry {
    pub fn new() -> Self {
        Self { claims: Mutex::new(HashMap::new()), next: AtomicU64::new(1) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Claim>> {
        self.claims.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Make `source` the session's microphone, replacing any earlier claim.
    pub fn claim(&self, session_id: i64, owner: &str, source: Arc<dyn VoiceSource>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().insert(
            session_id,
            Claim { id, owner: owner.to_string(), source, last_used: Instant::now(), busy: false },
        );
        tracing::info!(session_id, owner, "[voice] microphone claimed");
        id
    }

    /// Release `claim_id`; a claim that was already replaced is left alone.
    pub fn release(&self, session_id: i64, claim_id: u64) -> bool {
        let mut claims = self.lock();
        if claims.get(&session_id).is_some_and(|c| c.id == claim_id) {
            claims.remove(&session_id);
            tracing::info!(session_id, "[voice] microphone released");
            true
        } else {
            false
        }
    }

    pub fn owner(&self, session_id: i64) -> Option<String> {
        self.lock().get(&session_id).map(|c| c.owner.clone())
    }

    /// Start the session's source. `ttl` of zero never expires a claim.
    pub fn begin_capture(
        self: &Arc<Self>,
        session_id: i64,
        ttl: Duration,
    ) -> Result<Capture, CaptureRefusal> {
        let (source, claim_id) = {
            let mut claims = self.lock();
            let Some(c) = claims.get_mut(&session_id) else {
                return Err(CaptureRefusal::NoClaim);
            };
            if !ttl.is_zero() && !c.busy && c.last_used.elapsed() > ttl {
                claims.remove(&session_id);
                return Err(CaptureRefusal::NoClaim);
            }
            if c.busy {
                return Err(CaptureRefusal::Busy);
            }
            c.busy = true;
            (Arc::clone(&c.source), c.id)
        };
        // The guard exists before `start` so a failure frees the session.
        let busy = BusyGuard { reg: Arc::clone(self), session_id, claim_id };
        let (tx, rx) = tokio::sync::mpsc::channel(PCM_QUEUE);
        match source.start(tx) {
            Ok(guard) => {
                tracing::info!(session_id, "[voice] capture started");
                Ok(Capture { rx, _source: guard, _busy: busy })
            }
            Err(e) => Err(CaptureRefusal::SourceFailed(e)),
        }
    }
}

static REGISTRY: LazyLock<Arc<VoiceRegistry>> = LazyLock::new(|| Arc::new(VoiceRegistry::new()));

/// The process's registry, shared by the routes and the desktop's source.
pub fn registry() -> &'static Arc<VoiceRegistry> {
    &REGISTRY
}

#[cfg(test)]
mod tests;
```

- [ ] **Step 4: Run** `cargo fleet-test -- service::voice` — expect 6 passed.
- [ ] **Step 5:** `cargo fmt --all && cargo fleet-lint`, then commit: `git add crates/fleet-core/src/service/voice crates/fleet-core/src/service/mod.rs && git commit -m "feat(voice): VoiceRegistry — one microphone claim per session"`

---

### Task 2: Settings

**Files:** Modify `crates/fleet-core/src/service/settings.rs`, `crates/fleet-core/pages/settings.limits.json`, `src/lib/fleet_settings.ts`; regenerate docs.

**Interfaces — Produces:** `pub const VOICE_ENABLED: &str = "voice.enabled"; pub const VOICE_MAX_CAPTURE_SECS: &str = "voice.max_capture_secs"; pub const VOICE_CLAIM_TTL_SECS: &str = "voice.claim_ttl_secs";`

- [ ] **Step 1:** Add the constants next to `DOWNLOADS_KEEP_SECS` and these rows after the downloads rows in `SPECS`:

```rust
    Spec::new(
        VOICE_ENABLED,
        "false",
        Kind::Bool,
        "Voice relay",
        "Let Claude Code's /voice on a host record from the microphone of the app attached to it. The microphone opens only while you record, for a session you turned 🎤 on for.",
    ),
    Spec::new(
        VOICE_MAX_CAPTURE_SECS,
        "300",
        Kind::Secs,
        "Voice: longest recording",
        "A recording longer than this is cut off.",
    )
    .unit(Unit::Seconds),
    Spec::new(
        VOICE_CLAIM_TTL_SECS,
        "1800",
        Kind::Secs,
        "Voice: microphone idle release",
        "A session's 🎤 turns itself off after this long without a recording.",
    )
    .unit(Unit::Seconds)
    .zero("never"),
```

- [ ] **Step 2:** In `settings.limits.json`, after the Downloads section, add:

```json
    {
      "title": "Voice",
      "items": [
        { "type": "field", "key": "voice.enabled" },
        { "type": "field", "key": "voice.max_capture_secs", "when": { "key": "voice.enabled", "truthy": true } },
        { "type": "field", "key": "voice.claim_ttl_secs", "when": { "key": "voice.enabled", "truthy": true } }
      ]
    },
```

- [ ] **Step 3:** In `src/lib/fleet_settings.ts` add to `SETTING_KEYS`: `voiceEnabled: 'voice.enabled', voiceMaxCaptureSecs: 'voice.max_capture_secs', voiceClaimTtlSecs: 'voice.claim_ttl_secs',` and to `SETTING_DEFAULTS`: `'voice.enabled': 'false', 'voice.max_capture_secs': '300', 'voice.claim_ttl_secs': '1800',`.
- [ ] **Step 4:** Run `cargo fleet-test -- settings` and `cargo fleet-test -- pages` — expect the docs-current tests to FAIL only on stale generated docs.
- [ ] **Step 5:** Regenerate: `REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current && REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`, then re-run both filters — all pass; `pnpm exec vitest run src/lib/fleet_settings` passes.
- [ ] **Step 6:** Commit settings.rs, the page, fleet_settings.ts and every regenerated file (`git status` lists them): `git commit -m "feat(voice): voice.* settings (relay off by default)"`

---

### Task 3: `GET /voice/capture` and `GET /voice/source`

**Files:**
- Create: `crates/fleet-core/src/mcp/voice_route.rs`, `crates/fleet-core/src/mcp/voice_route_tests.rs`
- Modify: `crates/fleet-core/src/mcp/mod.rs` (`mod voice_route;`; two routes merged beside `/downloads/{id}`, before the `authorize` layer, using the existing `report_state.clone()`)

**Interfaces — Consumes:** Task 1 (`registry()`, `VoiceSource`, `CaptureRefusal`, `PcmTx`), Task 2 constants, `ReportState::store()`, `Caller`, `refuses_peer`, `Store::get_session(tmux, host)`, `Store::get_session_by_id(id)`, `validate::tmux_name_lookup`, `OrgScope::sees_row`.
**Produces:** routes `GET /voice/capture?tmux=<name>` (200 `application/octet-stream` PCM | 403 | 404 | 409 | 502) and `GET /voice/source?session_id=<id>` (websocket; server→client text `{"start":<n>}` / `{"stop":<n>}`; client→server binary PCM).

- [ ] **Step 1: Write the failing tests** in `voice_route_tests.rs` (wired with `#[cfg(test)] #[path = "voice_route_tests.rs"] mod tests;` at the end of `voice_route.rs`). Use the existing `test_app` in `mcp/mod.rs` and its helpers for minting a host token and a client token — read `mcp/mod.rs:533-569` and an existing route test (`grep -rn "test_app(" crates/fleet-core/src/mcp`) and reuse its fixture (store with host `h-a`, session `s1` running on `h-a`, `voice.enabled=true`). Tests:

```rust
// 1. capture_refuses_a_person_token            → master bearer → 403
// 2. capture_without_voice_enabled_is_403      → voice.enabled=false → 403
// 3. capture_of_another_hosts_session_is_404   → host token of h-b, tmux=s1 → 404
// 4. capture_without_a_claim_is_409            → host h-a, tmux=s1, no claim → 409, body mentions "🎤"
// 5. capture_streams_the_claimed_source        → registry().claim(id, "test", Fake that pushes [9;3200] every 10ms);
//                                                GET → 200, read ≥ 6400 bytes, all 9s; drop the response;
//                                                within 1 s the Fake's live count is 0
// 6. source_refuses_host_and_readonly_tokens   → upgrade with host token → 403; readonly client → 403
// 7. source_claims_and_relays_end_to_end       → connect tokio_tungstenite (dev-dep) to /voice/source?session_id=<id>
//                                                with a full client token; poll registry().owner(id) == Some("client:<name>");
//                                                start GET /voice/capture as h-a in a task; ws receives Text {"start":N};
//                                                send two Binary frames of 3200×[5]; the capture body yields 6400 bytes of 5;
//                                                drop the capture → ws receives Text {"stop":N};
//                                                close the ws → owner(id) becomes None within 1 s
// 8. source_for_a_session_outside_the_org_scope_is_404 → org-bound client of another org → 404
```
Use a unique session id per test (the registry is process-global) and `registry().release` in teardown.

- [ ] **Step 2: Run** `cargo fleet-test -- mcp::voice_route` — expect compile failure.
- [ ] **Step 3: Implement** `voice_route.rs`:

```rust
//! The microphone relay's two routes. `/voice/capture` is the host side: the
//! `arecord` stand-in reads a session's audio. `/voice/source` is the client
//! side: a person's device that holds the socket open is that session's
//! microphone. Spec: docs/superpowers/specs/2026-10-05-voice-relay-design.md.

use super::auth::{refuses_peer, Caller, TokenMode};
use super::report_route::ReportState;
use crate::service::settings;
use crate::service::voice::{registry, CaptureRefusal, PcmTx, VoiceSource};
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const FRAME_CAP: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct CaptureQuery {
    pub tmux: String,
}

pub async fn handle_capture(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<CaptureQuery>,
) -> Response {
    if let Some(refused) = refuses_peer(&caller) {
        return refused;
    }
    let Some(host) = caller.host_alias.clone() else {
        return (StatusCode::FORBIDDEN, "only a host's recorder captures\n").into_response();
    };
    if crate::validate::tmux_name_lookup(&q.tmux).is_err() {
        return (StatusCode::BAD_REQUEST, "bad tmux name\n").into_response();
    }
    let (session_id, max, ttl) = {
        let Ok(s) = state.store().lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if !settings::get_bool(&s, settings::VOICE_ENABLED) {
            return (StatusCode::FORBIDDEN, "voice relay is off (Settings → Limits → Voice)\n")
                .into_response();
        }
        let row = match s.get_session(&q.tmux, &host) {
            Ok(Some(r)) => r,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such session on this host\n").into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        (
            row.id,
            Duration::from_secs(settings::get_secs(&s, settings::VOICE_MAX_CAPTURE_SECS)),
            Duration::from_secs(settings::get_secs(&s, settings::VOICE_CLAIM_TTL_SECS)),
        )
    };
    let capture = match registry().begin_capture(session_id, ttl) {
        Ok(c) => c,
        Err(CaptureRefusal::NoClaim) => {
            return (StatusCode::CONFLICT, "no microphone for this session — turn on 🎤 in the app\n")
                .into_response()
        }
        Err(CaptureRefusal::Busy) => {
            return (StatusCode::CONFLICT, "this session is already recording\n").into_response()
        }
        Err(CaptureRefusal::SourceFailed(e)) => {
            tracing::warn!(session_id, error = %e, "[voice] source failed to start");
            return (StatusCode::BAD_GATEWAY, format!("the microphone did not start: {e}\n"))
                .into_response();
        }
    };
    let deadline = tokio::time::Instant::now() + max;
    let started = std::time::Instant::now();
    // The stream owns the capture: when the recorder hangs up, hyper drops
    // the body, the capture drops, and the microphone closes.
    let body = futures_util::stream::unfold(Some(capture), move |cap| async move {
        let mut cap = cap?;
        match tokio::time::timeout_at(deadline, cap.rx.recv()).await {
            Ok(Some(chunk)) => Some((Ok::<_, std::io::Error>(chunk), Some(cap))),
            _ => {
                tracing::info!(session_id, secs = started.elapsed().as_secs(), "[voice] capture ended");
                None
            }
        }
    });
    let mut resp = Response::new(Body::from_stream(body));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[derive(Deserialize)]
pub struct SourceQuery {
    pub session_id: i64,
}

enum SourceCmd {
    Start { capture: u64, tx: PcmTx },
    Stop { capture: u64 },
}

/// A claim held by a websocket: `start` asks the device to open its
/// microphone; the returned guard asks it to close it.
struct WsSource {
    cmds: tokio::sync::mpsc::Sender<SourceCmd>,
    next: AtomicU64,
}

struct StopOnDrop {
    cmds: tokio::sync::mpsc::Sender<SourceCmd>,
    capture: u64,
}

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.cmds.try_send(SourceCmd::Stop { capture: self.capture });
    }
}

impl VoiceSource for WsSource {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        let capture = self.next.fetch_add(1, Ordering::Relaxed);
        self.cmds
            .try_send(SourceCmd::Start { capture, tx })
            .map_err(|_| "the device's connection is gone".to_string())?;
        Ok(Box::new(StopOnDrop { cmds: self.cmds.clone(), capture }))
    }
}

pub async fn handle_source(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<SourceQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(refused) = refuses_peer(&caller) {
        return refused;
    }
    if caller.host_alias.is_some() {
        return (StatusCode::FORBIDDEN, "a host's token records; a person's device supplies the microphone\n")
            .into_response();
    }
    if caller.mode == TokenMode::Readonly {
        return (StatusCode::FORBIDDEN, "a readonly token cannot supply a microphone\n").into_response();
    }
    {
        let Ok(s) = state.store().lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if !settings::get_bool(&s, settings::VOICE_ENABLED) {
            return (StatusCode::FORBIDDEN, "voice relay is off\n").into_response();
        }
        let scope = match caller.org_scope(&s) {
            Ok(sc) => sc,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let row = s.get_session_by_id(q.session_id).ok().flatten();
        if !row.as_ref().is_some_and(|r| scope.sees_row(r)) {
            return (StatusCode::NOT_FOUND, "no such session\n").into_response();
        }
    }
    let owner = caller.label();
    let session_id = q.session_id;
    ws.max_frame_size(FRAME_CAP)
        .max_message_size(FRAME_CAP)
        .on_upgrade(move |socket| serve_source(socket, session_id, owner))
}

async fn serve_source(socket: WebSocket, session_id: i64, owner: String) {
    let (mut sink, mut stream) = socket.split();
    let (cmds, mut cmd_rx) = tokio::sync::mpsc::channel(4);
    let source = Arc::new(WsSource { cmds, next: AtomicU64::new(1) });
    let claim_id = registry().claim(session_id, &owner, source);
    let mut live: Option<(u64, PcmTx)> = None;
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(SourceCmd::Start { capture, tx }) => {
                    live = Some((capture, tx));
                    if sink.send(Message::Text(format!("{{\"start\":{capture}}}").into())).await.is_err() { break; }
                }
                Some(SourceCmd::Stop { capture }) => {
                    if live.as_ref().is_some_and(|(c, _)| *c == capture) { live = None; }
                    if sink.send(Message::Text(format!("{{\"stop\":{capture}}}").into())).await.is_err() { break; }
                }
                None => break,
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(pcm))) => {
                    if let Some((_, tx)) = &live {
                        // Full queue: drop the chunk rather than buffer audio.
                        let _ = tx.try_send(pcm.to_vec());
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    registry().release(session_id, claim_id);
}
```

- [ ] **Step 4:** Register in `build_app` (`mcp/mod.rs`, beside the downloads merge, before `.layer(... authorize)`):

```rust
        .merge(
            axum::Router::new()
                .route("/voice/capture", axum::routing::get(voice_route::handle_capture))
                .route("/voice/source", axum::routing::get(voice_route::handle_source))
                .with_state(report_state.clone()),
        )
```
- [ ] **Step 5: Run** `cargo fleet-test -- mcp::voice_route` — 8 pass. Then `cargo fleet-test -- mcp::` to confirm no route regressions.
- [ ] **Step 6:** fmt + lint; commit: `git commit -m "feat(voice): /voice/capture and /voice/source relay routes"`

---

### Task 4: Host stand-in `arecord`

**Files:** Create `tools/voice/arecord`, `scripts/voice-arecord-test.sh`.

- [ ] **Step 1: Write the test** `scripts/voice-arecord-test.sh` (bash, `set -euo pipefail`): it creates a temp `HOME` with `~/.claude/fleet-hook.headers` = `Authorization: Bearer good`, and `~/.claude-fleet/voice/voice.env` = `FLEET_VOICE_URL=http://127.0.0.1:$PORT`; starts a Python stdlib HTTP server on `$PORT` that answers `GET /voice/capture?tmux=t1` with 32 000 bytes of `\x07` when the bearer is `good` and 409 otherwise; puts a fake `tmux` on `PATH` that prints `t1` for `display-message -p '#S'`; sets `TMUX=/tmp/x,1,0`. Assertions:
  1. `arecord --version` exits 0.
  2. `arecord -f S16_LE -r 16000 -c 1 -t raw /dev/null` exits 0 within 2 s.
  3. `arecord -f S16_LE -r 16000 -c 1 -t raw -q - | wc -c` = 32000.
  4. with the headers file holding `Bearer bad` and no `~/.config/fleet-voice/token`: exit status 1 and stderr contains `turn on 🎤`.
  5. fallback: bad bearer, plus a `nc`-free Python TCP server on `FLEET_VOICE_FALLBACK_PORT` that checks the first line equals the token in `~/.config/fleet-voice/token` and then sends 3 200 bytes → 3200 bytes read.
  6. `arecord -l` exits 2.
- [ ] **Step 2: Run** `bash scripts/voice-arecord-test.sh` — fails (no stand-in).
- [ ] **Step 3: Implement** `tools/voice/arecord`:

```bash
#!/usr/bin/env bash
# claude-fleet voice relay: an `arecord` stand-in for Claude Code's /voice on a
# host with no sound card. Installed by fleet provisioning; do not edit here.
# Streams the microphone of the app attached to this session from the fleet
# server (GET /voice/capture), with this host's hook token.
#
# Claude Code calls exactly:
#   arecord --version                                  presence check
#   arecord -f S16_LE -r 16000 -c 1 -t raw /dev/null   probe (memoised per claude process)
#   arecord -f S16_LE -r 16000 -c 1 -t raw -q -        record to stdout
set -u
ENV_FILE="$HOME/.claude-fleet/voice/voice.env"
HEADERS="$HOME/.claude/fleet-hook.headers"
FALLBACK_PORT="${FLEET_VOICE_FALLBACK_PORT:-4713}"
FALLBACK_TOKEN="$HOME/.config/fleet-voice/token"

case "${1:-}" in
  --version) echo "arecord: claude-fleet voice relay"; exit 0 ;;
esac
last="${!#}"
if [ "$last" = "/dev/null" ]; then
  # Succeed regardless: the probe is memoised for the life of `claude`, and a
  # microphone turned on later must still work.
  sleep 1; exit 0
fi
if [ "$last" != "-" ]; then
  echo "arecord (claude-fleet): only raw capture to stdout is supported" >&2; exit 2
fi

reason="no fleet voice config"
if [ -r "$ENV_FILE" ] && [ -r "$HEADERS" ]; then
  # shellcheck disable=SC1090
  . "$ENV_FILE"
  name="$(tmux display-message -p '#S' 2>/dev/null || true)"
  if [ -n "${FLEET_VOICE_URL:-}" ] && [ -n "$name" ]; then
    err="$(mktemp)"
    # --fail: on an HTTP error curl writes nothing to stdout (PCM only there)
    # and names the status on stderr.
    if curl -sSN --fail -H @"$HEADERS" -G \
         --data-urlencode "tmux=$name" "$FLEET_VOICE_URL/voice/capture" 2>"$err"; then
      rm -f "$err"; exit 0
    fi
    reason="$(tr -d '\r' <"$err" | tail -n 1)"; rm -f "$err"
    case "$reason" in
      *409*) reason="$reason — turn on 🎤 for this session in claude-fleet" ;;
      *403*) reason="$reason — voice relay is off in claude-fleet settings" ;;
    esac
  fi
fi

# The manual relay (a reverse-tunnelled microphone server), if set up.
if [ -r "$FALLBACK_TOKEN" ] && { exec 3<>"/dev/tcp/127.0.0.1/$FALLBACK_PORT"; } 2>/dev/null; then
  printf '%s\n' "$(cat "$FALLBACK_TOKEN")" >&3
  exec cat <&3
fi
echo "arecord (claude-fleet): $reason" >&2
exit 1
```
- [ ] **Step 4:** `chmod +x tools/voice/arecord`; run `bash scripts/voice-arecord-test.sh` — all 6 pass; `shellcheck tools/voice/arecord` if available.
- [ ] **Step 5:** Commit: `git add tools/voice scripts/voice-arecord-test.sh && git commit -m "feat(voice): host arecord stand-in for /voice"`

---

### Task 5: Provision the stand-in and put it on `claude`'s PATH

**Files:** Modify `crates/fleet-core/src/service/provision.rs`, `crates/fleet-core/src/tmux.rs`.

**Interfaces — Produces:** `pub const VOICE_DIR: &str = "~/.claude-fleet/voice";` and `async fn provision_voice(ssh: &dyn SshExec, host: &str, base: &HubBase) -> Result<(), IpcError>` in provision.rs; `tmux::VOICE_PATH_PREFIX`.

- [ ] **Step 1: Failing tests.**
  - In provision.rs tests (beside the existing provision fakes; `grep -n "fn provision_one_with" -A3` and the test module's fake `SshExec` that records scripts): `provision_writes_the_voice_stand_in_and_env` — after `provision_one_with(&fake, "h-a", &HubBase::loopback(4180), "tok", false)`, the fake saw a write to `~/.claude-fleet/voice/bin/arecord` whose content equals `include_str!` of the stand-in (`VOICE_ARECORD`), a `chmod 755` of it, and a write of `~/.claude-fleet/voice/voice.env` with exactly `FLEET_VOICE_URL='http://127.0.0.1:4180'\n`.
  - Update `fingerprint_is_stable_and_covers_skills_claude_md_the_hook_shape_and_ag` to include `{VOICE_ARECORD}` in its recomputed format string (it will fail until Step 3).
  - In tmux.rs tests: `pane_command_puts_the_voice_bin_first_on_path` — `pane_command_for(None, "s")` starts with `VOICE_PATH_PREFIX`, and `VOICE_PATH_PREFIX == "PATH=\"$HOME/.claude-fleet/voice/bin:$PATH\"; "`. Adjust the existing tests that `.find(CL_FALLBACK)` from index 0 to search after the prefix.
- [ ] **Step 2: Run** `cargo fleet-test -- service::provision` and `cargo fleet-test -- tmux::` — new tests fail.
- [ ] **Step 3: Implement.** provision.rs:

```rust
/// The `/voice` recorder stand-in (docs/voice.md). In the fingerprint, so a
/// change re-provisions stale hosts.
pub const VOICE_ARECORD: &str = include_str!("../../../../tools/voice/arecord");
pub const VOICE_DIR: &str = "~/.claude-fleet/voice";

async fn provision_voice(ssh: &dyn SshExec, host: &str, base: &HubBase) -> Result<(), IpcError> {
    let bin = format!("{VOICE_DIR}/bin");
    let path = format!("{bin}/arecord");
    write_host_file(ssh, host, &bin, &path, VOICE_ARECORD).await?;
    crate::ssh::run_shell(ssh, host, &format!("chmod 755 {path}"), PROVISION_TIMEOUT).await?;
    let env = format!("FLEET_VOICE_URL={}\n", crate::shell::quote(&base.url));
    write_host_file(ssh, host, VOICE_DIR, &format!("{VOICE_DIR}/voice.env"), &env).await
}
```
  (`path` contains `~`, which must stay unquoted to expand; it is a constant, not user input. Check `run_shell`'s exact signature/return type with `graft skeleton crates/fleet-core/src/ssh.rs` and adapt the `?`.) Call `provision_voice(ssh, host, base).await?` in `provision_one_with` right after `provision_hook(...)`, and in `provision_content_only` after its hook step. Add `\u{0}{VOICE_ARECORD}` to `fingerprint()`'s format string. tmux.rs:

```rust
/// Puts fleet's `/voice` recorder ahead of any real `arecord` (docs/voice.md).
pub(crate) const VOICE_PATH_PREFIX: &str = "PATH=\"$HOME/.claude-fleet/voice/bin:$PATH\"; ";
```
  and in `pane_command_with`, prefix both `format!` results: `format!("{VOICE_PATH_PREFIX}{CL_FALLBACK} cl --resume …")`.
- [ ] **Step 4: Run** both filters, then `cargo fleet-test -- sessions::` (pane-command callers) — all pass.
- [ ] **Step 5:** fmt + lint + `cargo fleet-check`; commit: `git commit -m "feat(voice): provision the recorder stand-in and prefix claude's PATH"`

---

### Task 6: Desktop audio capture (cpal) and conversion

**Files:**
- Create: `src-tauri/src/voice/mod.rs`, `src-tauri/src/voice/resample.rs`, `src-tauri/src/voice/capture.rs`
- Create: `src-tauri/Info.plist`, `src-tauri/Entitlements.plist`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/lib.rs` (`mod voice;`)

**Interfaces — Produces:**
```rust
// resample.rs
pub struct Converter { /* in_rate, channels, pos, carry */ }
impl Converter {
    pub fn new(in_rate: u32, channels: u16) -> Self;
    /// Interleaved f32 in → S16LE mono 16 kHz bytes out.
    pub fn push(&mut self, input: &[f32]) -> Vec<u8>;
}
// capture.rs
pub struct CpalSource;                       // impl fleet_core::service::voice::VoiceSource
pub fn supported() -> bool;                  // false on Linux
```

- [ ] **Step 1: Failing tests** in `resample.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn samples(b: &[u8]) -> Vec<i16> { b.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect() }

    #[test]
    fn mono_16k_passes_through() {
        let mut c = Converter::new(16_000, 1);
        assert_eq!(samples(&c.push(&[0.0, 0.5, -0.5, 1.0])), vec![0, 16383, -16383, 32767]);
    }
    #[test]
    fn stereo_is_averaged() {
        let mut c = Converter::new(16_000, 2);
        assert_eq!(samples(&c.push(&[1.0, 0.0, -1.0, -1.0])), vec![16383, -32767]);
    }
    #[test]
    fn a_48k_second_becomes_a_16k_second_across_chunk_boundaries() {
        let mut c = Converter::new(48_000, 1);
        let mut n = 0;
        for _ in 0..10 { n += c.push(&vec![0.25; 4_800]).len() / 2; }
        assert!((15_999..=16_001).contains(&n), "{n}");
    }
    #[test]
    fn out_of_range_input_is_clamped() {
        let mut c = Converter::new(16_000, 1);
        assert_eq!(samples(&c.push(&[2.0, -2.0])), vec![32767, -32767]);
    }
}
```
- [ ] **Step 2: Run** `cargo fleet-test -- voice::resample` — compile failure.
- [ ] **Step 3: Implement** `resample.rs`:

```rust
//! Device audio → the relay's PCM: average the channels, resample to 16 kHz
//! by linear interpolation (plenty for speech), S16LE.

pub const OUT_RATE: u32 = 16_000;

pub struct Converter {
    step: f64,      // input frames per output frame
    pos: f64,       // position of the next output frame, in input frames
    prev: f32,      // last input frame of the previous chunk
    channels: usize,
}

impl Converter {
    pub fn new(in_rate: u32, channels: u16) -> Self {
        Self { step: in_rate as f64 / OUT_RATE as f64, pos: 0.0, prev: 0.0, channels: channels.max(1) as usize }
    }

    pub fn push(&mut self, input: &[f32]) -> Vec<u8> {
        let mono: Vec<f32> = input
            .chunks(self.channels)
            .map(|f| f.iter().sum::<f32>() / f.len() as f32)
            .collect();
        let mut out = Vec::with_capacity((mono.len() as f64 / self.step) as usize * 2 + 2);
        // Frame -1 is `prev`, so interpolation spans the chunk boundary.
        let at = |i: isize| if i < 0 { self.prev } else { mono[i as usize] };
        while self.pos < mono.len() as f64 - 1.0 + 1e-9 {
            let i = self.pos.floor() as isize;
            let frac = (self.pos - i as f64) as f32;
            let a = at(i);
            let b = if (i + 1) < mono.len() as isize { at(i + 1) } else { a };
            let v = (a + (b - a) * frac).clamp(-1.0, 1.0);
            out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
            self.pos += self.step;
        }
        if let Some(&last) = mono.last() {
            self.prev = last;
        }
        self.pos -= mono.len() as f64;
        out
    }
}
```
  Run the tests; if the boundary test is off by more than one sample, fix the loop bound (the contract is the test, not this sketch).
- [ ] **Step 4:** `Cargo.toml` (src-tauri):

```toml
[target.'cfg(any(target_os = "macos", target_os = "windows"))'.dependencies]
cpal = "0.15"
```
  Run `cargo deny check` (licenses: cpal is Apache-2.0; if a transitive crate is flagged, stop and report).
- [ ] **Step 5: Implement** `capture.rs`:

```rust
//! The desktop's microphone as a `VoiceSource`: the default input through
//! cpal, opened in `start` and closed when the returned guard drops.

use super::resample::Converter;
use fleet_core::service::voice::{PcmTx, VoiceSource};

pub fn supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

pub struct CpalSource {
    /// Told about start / stop / error for the UI (`voice:state`).
    pub on_state: std::sync::Arc<dyn Fn(&'static str, Option<String>) + Send + Sync>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl VoiceSource for CpalSource {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let on_state = self.on_state.clone();
        // cpal's Stream is !Send on macOS: it lives on its own thread until
        // the guard's sender drops.
        std::thread::Builder::new().name("voice-capture".into()).spawn(move || {
            let run = || -> Result<cpal::Stream, String> {
                let dev = cpal::default_host().default_input_device().ok_or("no microphone found")?;
                let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
                let mut conv = Converter::new(cfg.sample_rate().0, cfg.channels());
                let mut buf = Vec::with_capacity(3_200);
                let err_state = on_state.clone();
                let stream = dev.build_input_stream(
                    &cfg.config(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        buf.extend(conv.push(data));
                        if buf.len() >= 3_200 {
                            let _ = tx.try_send(std::mem::take(&mut buf));
                        }
                    },
                    move |e| err_state("error", Some(e.to_string())),
                    None,
                ).map_err(|e| e.to_string())?;
                stream.play().map_err(|e| e.to_string())?;
                Ok(stream)
            };
            match run() {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    on_state("capturing", None);
                    let _ = stop_rx.recv();
                    drop(stream);
                    on_state("claimed", None);
                }
                Err(e) => {
                    on_state("error", Some(e.clone()));
                    let _ = ready_tx.send(Err(e));
                }
            }
        }).map_err(|e| e.to_string())?;
        ready_rx.recv_timeout(std::time::Duration::from_secs(5)).map_err(|_| "the microphone did not open".to_string())??;
        Ok(Box::new(stop_tx))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl VoiceSource for CpalSource {
    fn start(&self, _tx: PcmTx) -> Result<Box<dyn Send>, String> {
        Err("voice relay is not supported on this platform".into())
    }
}
```
  Only `f32` input is handled; if `default_input_config().sample_format()` is not `F32`, return `Err("unsupported microphone format: …")` (check before `build_input_stream`). macOS and WASAPI default to f32.
- [ ] **Step 6:** Permissions. Create `src-tauri/Info.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>NSMicrophoneUsageDescription</key>
  <string>claude-fleet streams your microphone to Claude Code's /voice on the host you are attached to, only while you record.</string>
</dict>
</plist>
```
  (Tauri 2 merges `src-tauri/Info.plist` into the bundle automatically.) Create `src-tauri/Entitlements.plist` with `com.apple.security.device.audio-input` = `<true/>` and set `"macOS": { "hardenedRuntime": false, "entitlements": "Entitlements.plist" }` in `tauri.conf.json`.
- [ ] **Step 7:** `cargo fleet-test -- voice::` passes; `cargo fleet-check` passes on Linux (cpal absent there). Commit: `git commit -m "feat(voice): desktop microphone capture (cpal) and PCM conversion"`

---

### Task 7: Desktop claim commands — standalone and hub-paired

**Files:**
- Create: `src-tauri/src/voice/hub_source.rs`, `src-tauri/src/commands/voice.rs`
- Modify: `src-tauri/src/voice/mod.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs` (manage `voice::VoiceState`, register commands), `src-tauri/src/backend/verdicts.rs`, `src-tauri/Cargo.toml`

**Interfaces — Consumes:** Task 1 registry, Task 6 `CpalSource`/`supported()`, `FleetBackend::hub()`, `HubBackend::config() -> &RemoteConfig { base_url, token, .. }`, `fleet_core::http_client::{Endpoint, connect}`.
**Produces:**
```rust
// voice/mod.rs
pub struct VoiceState { inner: Mutex<Option<Active>> }       // managed by Tauri
enum Active { Local { session_id: i64, claim_id: u64 }, Hub { session_id: i64, stop: tokio_util::sync::CancellationToken } }
pub const STATE_EVENT: &str = "voice:state";                  // payload { session_id, state: "claimed"|"capturing"|"released"|"error", error?: string }
// commands/voice.rs
#[tauri::command] pub async fn voice_claim(session_id: i64, …) -> Result<(), IpcError>;
#[tauri::command] pub async fn voice_release(…) -> Result<(), IpcError>;
```

- [ ] **Step 1: Failing tests.**
  - `backend/tests_routing.rs` will fail once the commands are registered without verdict rows — that is the first red.
  - In `voice/mod.rs`: `claiming_a_second_session_releases_the_first` — with a fake `VoiceSource`, `VoiceState::claim_local(1, src)` then `claim_local(2, src)`: `registry().owner(1) == None`, `owner(2) == Some("desktop")`; `release()` clears 2.
  - In `hub_source.rs`: `the_upgrade_request_targets_voice_source_with_the_bearer` — `upgrade_request("https://hub.example:8443/prefix", "tok", 42)` returns a request whose URI is `wss://hub.example:8443/prefix/voice/source?session_id=42` and whose `Authorization` header is `Bearer tok`. A full websocket round trip is covered by Task 3's loopback test on the server side and by the manual check in Task 9.
- [ ] **Step 2:** Add to `src-tauri/Cargo.toml` `[dependencies]`: `tokio-tungstenite = { version = "0.29", default-features = false, features = ["handshake"] }` and `futures-util = { version = "0.3", default-features = false, features = ["std", "sink"] }` (same versions as `Cargo.lock`; confirm no new crate appears in `git diff Cargo.lock` beyond these entries).
- [ ] **Step 3: Implement** `voice/mod.rs` (claim/release, one active claim, emits `STATE_EVENT` via `tauri::Emitter::emit(&app, STATE_EVENT, json!({...}))`; `claim_local` calls `fleet_core::service::voice::registry().claim(session_id, "desktop", Arc::new(CpalSource{ on_state }))`; `release` calls `registry().release(...)` or cancels the hub token), and `hub_source.rs`:

```rust
//! A hub-paired desktop's microphone: hold `/voice/source` open on the hub
//! (that is the claim), answer `{"start":n}` by opening cpal and sending
//! binary PCM, `{"stop":n}` by closing it.

use fleet_core::service::voice::{VoiceSource, PCM_QUEUE};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http, Message};

pub fn upgrade_request(base_url: &str, token: &str, session_id: i64) -> Result<http::Request<()>, String> {
    let ws = if let Some(rest) = base_url.strip_prefix("https://") { format!("wss://{rest}") }
        else if let Some(rest) = base_url.strip_prefix("http://") { format!("ws://{rest}") }
        else { return Err(format!("not a hub address: {base_url}")) };
    let mut req = format!("{ws}/voice/source?session_id={session_id}")
        .into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert(http::header::AUTHORIZATION,
        format!("Bearer {token}").parse().map_err(|_| "bad token".to_string())?);
    Ok(req)
}

pub async fn run(
    base_url: String, token: String, session_id: i64,
    source: std::sync::Arc<dyn VoiceSource>,
    stop: tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    let at = fleet_core::http_client::Endpoint::parse(&base_url)?;
    let io = fleet_core::http_client::connect(&at).await?;
    let (ws, _) = tokio_tungstenite::client_async(upgrade_request(&base_url, &token, session_id)?, io)
        .await.map_err(|e| format!("the hub refused the microphone: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let (pcm_tx, mut pcm_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PCM_QUEUE);
    let mut guard: Option<Box<dyn Send>> = None;
    loop {
        tokio::select! {
            _ = stop.cancelled() => { let _ = sink.send(Message::Close(None)).await; return Ok(()); }
            Some(pcm) = pcm_rx.recv(), if guard.is_some() => {
                sink.send(Message::Binary(pcm.into())).await.map_err(|e| e.to_string())?;
            }
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                    if v.get("start").is_some() { guard = Some(source.start(pcm_tx.clone())?); }
                    else if v.get("stop").is_some() { guard = None; }
                }
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(e)) => return Err(e.to_string()),
                Some(Ok(_)) => {}
            },
        }
    }
}
```
  Before writing, confirm `Endpoint::parse` accepts the `https://…` base and that `connect` returns a stream implementing `AsyncRead + AsyncWrite + Unpin` (it is `Box<dyn Duplex>`; see `crates/fleet-core/src/net/conn.rs:11`). If `start` fails, send `Message::Close` and return the error so the UI shows it.
- [ ] **Step 4:** `commands/voice.rs`:

```rust
#[tauri::command]
pub async fn voice_claim(
    session_id: i64,
    app: tauri::AppHandle,
    backend: State<'_, Arc<FleetBackend>>,
    voice: State<'_, crate::voice::VoiceState>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    if !crate::voice::capture::supported() {
        return Err(IpcError::new(codes::E_UNSUPPORTED, "voice relay needs the macOS or Windows app"));
    }
    match backend.hub() {
        Some(hub) => voice.claim_hub(&app, hub.config(), session_id),   // spawns hub_source::run
        None => {
            let enabled = { let s = lock(&store)?; fleet_core::service::settings::get_bool(&s, fleet_core::service::settings::VOICE_ENABLED) };
            if !enabled { return Err(IpcError::new(codes::E_FORBIDDEN, "turn on Settings → Limits → Voice first")); }
            voice.claim_local(&app, session_id)
        }
    }
}

#[tauri::command]
pub async fn voice_release(voice: State<'_, crate::voice::VoiceState>, app: tauri::AppHandle) -> Result<(), IpcError> {
    voice.release(&app);
    Ok(())
}
```
  (Use the error-code constants that exist — `grep -n "pub const E_" crates/fleet-core/src/ipc_error.rs`; if there is no `E_UNSUPPORTED`, use the closest existing one, e.g. `E_LOCAL_ONLY`'s sibling for "not available", rather than adding a code.) Register both in `lib.rs`'s `generate_handler!`; `app.manage(crate::voice::VoiceState::default())`.
- [ ] **Step 5:** `verdicts.rs` rows:

```rust
    (
        "voice_claim",
        Verdict::SameInBoth {
            why: "the microphone is this machine's: standalone it is registered with the \
                  embedded server; paired, the desktop holds /voice/source open on the hub",
        },
    ),
    (
        "voice_release",
        Verdict::SameInBoth {
            why: "releases whichever claim voice_claim made on this machine",
        },
    ),
```
  Then `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen`.
- [ ] **Step 6: Run** `cargo fleet-test -- voice::`, `cargo fleet-test -- backend::` — pass; `pnpm exec vitest run src/lib/hub_verdicts.test.ts` passes.
- [ ] **Step 7:** fmt + lint + `cargo fleet-check`; commit: `git commit -m "feat(voice): desktop voice_claim — in-process standalone, /voice/source when paired"`

---

### Task 8: 🎤 in the terminal header

**Files:** Create `src/lib/voice.ts`, `src/lib/MicToggle.svelte`, `src/lib/MicToggle.test.ts`; Modify `src/lib/TerminalView.svelte`.

**Interfaces — Consumes:** commands `voice_claim { sessionId }`, `voice_release`; event `voice:state` `{ session_id: number, state: 'claimed'|'capturing'|'released'|'error', error?: string }`; `SessionRow`, `hostByAlias`.
**Produces:** `voice.ts` exports `voiceState` (writable `{ sessionId: number | null; state: 'off'|'claimed'|'capturing'|'error'; error: string | null; tipShown: boolean }`), `claimVoice(sessionId)`, `releaseVoice()`, `startVoiceEvents(): Promise<UnlistenFn>`.

- [ ] **Step 1: Failing test** `MicToggle.test.ts` (pattern of `TransferChip.test.ts`):

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));
import { invoke } from '@tauri-apps/api/core';
import MicToggle from './MicToggle.svelte';
import { voiceState, resetVoiceForTest } from './voice';

const sess = { id: 5, host_alias: 'alpha', tmux_name: 's' } as any;

beforeEach(() => { vi.mocked(invoke).mockReset(); vi.mocked(invoke).mockResolvedValue(undefined); resetVoiceForTest(); });

describe('MicToggle', () => {
  it('is disabled on an agent host', () => {
    render(MicToggle, { props: { session: sess, transport: 'agent' } });
    expect((screen.getByTestId('mic-toggle') as HTMLButtonElement).disabled).toBe(true);
  });
  it('claims the microphone for the session and shows the tip once', async () => {
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    expect(invoke).toHaveBeenCalledWith('voice_claim', { sessionId: 5 });
    expect(get(voiceState)).toMatchObject({ sessionId: 5, state: 'claimed' });
    expect(screen.getByTestId('mic-tip').textContent).toContain('/voice');
  });
  it('a second click releases', async () => {
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    expect(invoke).toHaveBeenLastCalledWith('voice_release', undefined);
    expect(get(voiceState).state).toBe('off');
  });
  it('shows the error from a refused claim', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'turn on Settings → Limits → Voice first' });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    expect(screen.getByTestId('mic-toggle').getAttribute('title')).toContain('Settings → Limits → Voice');
  });
  it('shows a red dot while capturing', () => {
    voiceState.set({ sessionId: 5, state: 'capturing', error: null, tipShown: true });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    expect(screen.getByTestId('mic-live')).toBeTruthy();
  });
});
```
- [ ] **Step 2: Run** `pnpm exec vitest run src/lib/MicToggle.test.ts` — fails.
- [ ] **Step 3: Implement** `voice.ts` (wrappers via `invokeCmd` from `result.ts`; `claimVoice` sets `{sessionId, state:'claimed'}` on ok and `{state:'error', error: message}` on error; `releaseVoice` sets `off`; `startVoiceEvents` listens to `voice:state` with `listen` and maps `released` → `off`; `resetVoiceForTest`), and `MicToggle.svelte`:

```svelte
<script lang="ts">
  import type { SessionRow } from './sessions';
  import { voiceState, claimVoice, releaseVoice } from './voice';
  let { session, transport }: { session: SessionRow; transport: 'ssh' | 'agent' } = $props();
  const mine = $derived($voiceState.sessionId === session.id);
  const on = $derived(mine && ($voiceState.state === 'claimed' || $voiceState.state === 'capturing'));
  const title = $derived(
    transport === 'agent' ? 'Voice needs an SSH host (agent hosts come later)'
    : mine && $voiceState.state === 'error' ? ($voiceState.error ?? 'Microphone error')
    : on ? 'Microphone on for this session — click to turn off'
    : 'Use this computer\'s microphone for /voice in this session',
  );
  async function toggle() { on ? await releaseVoice() : await claimVoice(session.id); }
</script>

<button class="mic" class:on data-testid="mic-toggle" disabled={transport === 'agent'} {title} onclick={toggle}>
  🎤{#if mine && $voiceState.state === 'capturing'}<span class="live" data-testid="mic-live"></span>{/if}
</button>
{#if on && !$voiceState.tipShown}
  <span class="tip" data-testid="mic-tip">Run <code>/voice</code> in the session, then hold space.</span>
{/if}

<style>
  .mic { background: none; border: 1px solid transparent; border-radius: 4px; cursor: pointer; opacity: 0.5; position: relative; }
  .mic.on { opacity: 1; border-color: var(--accent, #6aa0ff); }
  .mic:disabled { cursor: not-allowed; opacity: 0.25; }
  .live { position: absolute; top: 1px; right: 1px; width: 6px; height: 6px; border-radius: 50%; background: #e5484d; }
  .tip { font-size: 11px; opacity: 0.8; margin-left: 4px; }
</style>
```
  `tipShown` flips to true on the first `capturing` event (so the tip stays until the first real recording). In `TerminalView.svelte`, import `MicToggle` and add `<MicToggle session={$selectedSession} transport={selectedSessionHostTransport} />` right after `<TransferChip … />`; add an `$effect` that, when `$selectedSession?.id` changes while `$voiceState.state !== 'off'` and the id differs, calls `claimVoice($selectedSession.id)` (the claim follows the attached session) and calls `releaseVoice()` in `closeTerm()`. Call `startVoiceEvents()` once from the app's existing event setup (where `subscribeToRowEvents` is started — `grep -n "subscribeToRowEvents(" src`).
- [ ] **Step 4: Run** `pnpm exec vitest run src/lib/MicToggle.test.ts src/lib/TerminalView` and `pnpm check` — pass.
- [ ] **Step 5:** Commit: `git commit -m "feat(voice): 🎤 toggle in the terminal header"`

---

### Task 9: Docs, orientation, full validation, manual check

**Files:** Create `docs/voice.md`; Modify `CLAUDE.md`.

- [ ] **Step 1:** `docs/voice.md` — user guide: what it does; turn on Settings → Limits → Voice; re-provision hosts (`fleet-hub provision --host <alias> --content-only` or the desktop's re-provision) and restart sessions started before (PATH); in a session `/voice` (or `/voice tap`), then 🎤 in the terminal header, then hold space; macOS permission prompt on first recording and where to fix a denial; privacy (microphone only while recording, nothing stored); limits; troubleshooting table (409 → 🎤 off; 403 → setting off; "no microphone tunnel" → manual fallback); not yet: agent hosts, phone, Linux desktop. Mention the manual 4713 fallback.
- [ ] **Step 2:** `CLAUDE.md` — one paragraph under Architecture: "**Voice relay F1** (spec `docs/superpowers/specs/2026-10-05-voice-relay-design.md`, plan `…/plans/2026-10-05-voice-relay-f1.md`): `service/voice` `VoiceRegistry` (one claim per session, process-global), `mcp/voice_route.rs` (`/voice/capture` host token only; `/voice/source` websocket = a client's claim), host stand-in `tools/voice/arecord` provisioned to `~/.claude-fleet/voice/bin` and put first on `claude`'s PATH by `tmux::VOICE_PATH_PREFIX`; desktop `src-tauri/src/voice/` (cpal, macOS/Windows only) and the 🎤 `MicToggle`. `voice.enabled` off by default. Audio is never stored."
- [ ] **Step 3:** Full ladder: `cargo fmt --all --check && cargo fleet-lint && cargo test --workspace && pnpm test && bash scripts/voice-arecord-test.sh`. Re-run known flakes alone before blaming the change.
- [ ] **Step 4: Manual (owner, Mac):** build the desktop, enable the setting, re-provision `claude-fleet-oci`, restart a session there, turn 🎤 on, `/voice`, hold space, speak; confirm the macOS prompt names claude-fleet, the transcript appears, the red dot shows only while held, and the mic indicator turns off on release. Repeat paired to a hub.
- [ ] **Step 5:** Commit: `git commit -m "docs(voice): user guide and orientation"`
