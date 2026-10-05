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
        Self {
            claims: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Claim>> {
        self.claims.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Make `source` the session's microphone, replacing any earlier claim.
    pub fn claim(&self, session_id: i64, owner: &str, source: Arc<dyn VoiceSource>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().insert(
            session_id,
            Claim {
                id,
                owner: owner.to_string(),
                source,
                last_used: Instant::now(),
                busy: false,
            },
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
        let busy = BusyGuard {
            reg: Arc::clone(self),
            session_id,
            claim_id,
        };
        let (tx, rx) = tokio::sync::mpsc::channel(PCM_QUEUE);
        match source.start(tx) {
            Ok(guard) => {
                tracing::info!(session_id, "[voice] capture started");
                Ok(Capture {
                    rx,
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
