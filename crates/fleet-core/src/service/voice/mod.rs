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
use tokio_util::sync::CancellationToken;

pub const PCM_QUEUE: usize = 64;
pub type PcmTx = tokio::sync::mpsc::Sender<Vec<u8>>;
pub type PcmRx = tokio::sync::mpsc::Receiver<Vec<u8>>;

/// Something that can open a microphone: S16LE, 16 kHz, mono into `tx`
/// until the returned guard is dropped.
pub trait VoiceSource: Send + Sync {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String>;

    /// The registry dropped this source's claim without its owner releasing
    /// it, for `reason`. Called outside the registry's lock, at most once. A
    /// source tells its person here (the 🎤 turns off, saying why). The
    /// owner's own `release` is not a revocation: it already knows.
    fn revoked(&self, _reason: RevokeReason) {}
}

/// Why a claim ended without its owner releasing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeReason {
    /// Another device (or window) claimed the session's microphone.
    Replaced,
    /// Unused for longer than `voice.claim_ttl_secs`; found by the next
    /// capture that asked for it.
    Expired,
}

impl RevokeReason {
    /// What the person is told.
    pub fn text(self) -> &'static str {
        match self {
            RevokeReason::Replaced => "microphone claimed elsewhere",
            RevokeReason::Expired => "microphone idle — turn 🎤 on again",
        }
    }

    /// The close code `/voice/source` ends a revoked socket with.
    pub fn close_code(self) -> u16 {
        match self {
            RevokeReason::Replaced => 4001,
            RevokeReason::Expired => 4002,
        }
    }

    /// The reason a `/voice/source` close code stands for, if any.
    pub fn from_close_code(code: u16) -> Option<Self> {
        [RevokeReason::Replaced, RevokeReason::Expired]
            .into_iter()
            .find(|r| r.close_code() == code)
    }
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
}

/// A live capture of a session, whichever claim started it.
struct LiveCapture {
    id: u64,
    revoked: CancellationToken,
}

#[derive(Default)]
struct State {
    claims: HashMap<i64, Claim>,
    captures: HashMap<i64, LiveCapture>,
}

impl State {
    /// End the session's live capture, if any: its holder sees `revoked`.
    fn revoke_capture(&mut self, session_id: i64) {
        if let Some(live) = self.captures.remove(&session_id) {
            live.revoked.cancel();
        }
    }
}

pub struct VoiceRegistry {
    state: Mutex<State>,
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
    revoked: CancellationToken,
    _source: Box<dyn Send>,
    _busy: BusyGuard,
}

impl Capture {
    /// Cancelled when the claim that supplies this capture is replaced or
    /// released. The route that streams `rx` must end the stream (and drop
    /// the capture) when it fires, so no microphone stays open without a claim.
    pub fn revoked(&self) -> CancellationToken {
        self.revoked.clone()
    }
}

struct BusyGuard {
    reg: Arc<VoiceRegistry>,
    session_id: i64,
    capture_id: u64,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut st = self.reg.lock();
        // Only this capture's own entry: a replacement may have a newer one.
        if st
            .captures
            .get(&self.session_id)
            .is_some_and(|l| l.id == self.capture_id)
        {
            st.captures.remove(&self.session_id);
        }
        if let Some(c) = st.claims.get_mut(&self.session_id) {
            c.last_used = Instant::now();
        }
    }
}

impl VoiceRegistry {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            next: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Make `source` the session's microphone, replacing any earlier claim
    /// and revoking a capture the earlier claim has open. The earlier
    /// claim's source is told (`RevokeReason::Replaced`).
    pub fn claim(&self, session_id: i64, owner: &str, source: Arc<dyn VoiceSource>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let replaced = {
            let mut st = self.lock();
            st.revoke_capture(session_id);
            st.claims.insert(
                session_id,
                Claim {
                    id,
                    owner: owner.to_string(),
                    source,
                    last_used: Instant::now(),
                },
            )
        };
        tracing::info!(session_id, owner, "[voice] microphone claimed");
        if let Some(old) = replaced {
            old.source.revoked(RevokeReason::Replaced);
        }
        id
    }

    /// Release `claim_id` (revoking a live capture); a claim that was already
    /// replaced is left alone.
    pub fn release(&self, session_id: i64, claim_id: u64) -> bool {
        let mut st = self.lock();
        if st.claims.get(&session_id).is_some_and(|c| c.id == claim_id) {
            st.claims.remove(&session_id);
            st.revoke_capture(session_id);
            tracing::info!(session_id, "[voice] microphone released");
            true
        } else {
            false
        }
    }

    pub fn owner(&self, session_id: i64) -> Option<String> {
        self.lock().claims.get(&session_id).map(|c| c.owner.clone())
    }

    /// Start the session's source. `ttl` of zero never expires a claim; a
    /// claim with a live capture never expires. An expired claim is dropped
    /// here, when a capture finds it, and its source told
    /// (`RevokeReason::Expired`).
    pub fn begin_capture(
        self: &Arc<Self>,
        session_id: i64,
        ttl: Duration,
    ) -> Result<Capture, CaptureRefusal> {
        let (source, capture_id, revoked) = {
            let mut st = self.lock();
            let State { claims, captures } = &mut *st;
            let Some(c) = claims.get(&session_id) else {
                return Err(CaptureRefusal::NoClaim);
            };
            if captures.contains_key(&session_id) {
                return Err(CaptureRefusal::Busy);
            }
            if !ttl.is_zero() && c.last_used.elapsed() > ttl {
                let expired = claims.remove(&session_id);
                drop(st);
                tracing::info!(session_id, "[voice] microphone claim expired");
                if let Some(old) = expired {
                    old.source.revoked(RevokeReason::Expired);
                }
                return Err(CaptureRefusal::NoClaim);
            }
            let capture_id = self.next.fetch_add(1, Ordering::Relaxed);
            let revoked = CancellationToken::new();
            captures.insert(
                session_id,
                LiveCapture {
                    id: capture_id,
                    revoked: revoked.clone(),
                },
            );
            (Arc::clone(&c.source), capture_id, revoked)
        };
        // The guard exists before `start` so a failure frees the session.
        let busy = BusyGuard {
            reg: Arc::clone(self),
            session_id,
            capture_id,
        };
        let (tx, rx) = tokio::sync::mpsc::channel(PCM_QUEUE);
        match source.start(tx) {
            Ok(guard) => {
                tracing::info!(session_id, "[voice] capture started");
                Ok(Capture {
                    rx,
                    revoked,
                    _source: guard,
                    _busy: busy,
                })
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
