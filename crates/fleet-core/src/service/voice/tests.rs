use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A source that counts starts and live guards, and pushes one chunk.
#[derive(Default)]
struct Fake {
    starts: AtomicUsize,
    live: Arc<AtomicUsize>,
    fail: bool,
    revocations: std::sync::Mutex<Vec<RevokeReason>>,
}
struct Live(Arc<AtomicUsize>);
impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl VoiceSource for Fake {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        if self.fail {
            return Err("denied".into());
        }
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.live.fetch_add(1, Ordering::SeqCst);
        tx.try_send(vec![1, 2, 3, 4]).unwrap();
        Ok(Box::new(Live(Arc::clone(&self.live))))
    }
    fn revoked(&self, reason: RevokeReason) {
        self.revocations.lock().unwrap().push(reason);
    }
}
impl Fake {
    fn revocations(&self) -> Vec<RevokeReason> {
        self.revocations.lock().unwrap().clone()
    }
}
const TTL: Duration = Duration::from_secs(60);

#[tokio::test]
async fn no_claim_means_no_capture_and_no_microphone() {
    let reg = Arc::new(VoiceRegistry::new());
    assert_eq!(
        reg.begin_capture(7, TTL).err(),
        Some(CaptureRefusal::NoClaim)
    );
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
    reg.claim(
        7,
        "master",
        Arc::new(Fake {
            fail: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        reg.begin_capture(7, TTL).err(),
        Some(CaptureRefusal::SourceFailed("denied".into()))
    );
    // not left busy
    assert_eq!(
        reg.begin_capture(7, TTL).err(),
        Some(CaptureRefusal::SourceFailed("denied".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn an_unused_claim_expires_after_the_ttl_and_zero_means_never() {
    let reg = Arc::new(VoiceRegistry::new());
    reg.claim(7, "master", Arc::new(Fake::default()));
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(
        reg.begin_capture(7, TTL).err(),
        Some(CaptureRefusal::NoClaim)
    );
    assert_eq!(reg.owner(7), None);
    reg.claim(8, "master", Arc::new(Fake::default()));
    tokio::time::advance(Duration::from_secs(10_000)).await;
    assert!(reg.begin_capture(8, Duration::ZERO).is_ok());
}

#[tokio::test]
async fn replacing_the_claim_revokes_a_live_capture_and_allows_a_new_one() {
    let reg = Arc::new(VoiceRegistry::new());
    let a = Arc::new(Fake::default());
    reg.claim(7, "client:phone", a.clone());
    let c1 = reg.begin_capture(7, TTL).unwrap();
    let revoked = c1.revoked();
    assert!(!revoked.is_cancelled());
    let b = Arc::new(Fake::default());
    reg.claim(7, "master", b.clone());
    assert!(revoked.is_cancelled());
    // The revoked capture no longer holds the session: a new one may start.
    let c2 = reg.begin_capture(7, TTL).unwrap();
    assert_eq!(b.starts.load(Ordering::SeqCst), 1);
    // Dropping the old capture must not clear the new one's hold.
    drop(c1);
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::Busy));
    drop(c2);
    assert!(reg.begin_capture(7, TTL).is_ok());
}

#[tokio::test]
async fn releasing_revokes_a_live_capture() {
    let reg = Arc::new(VoiceRegistry::new());
    let id = reg.claim(7, "master", Arc::new(Fake::default()));
    let cap = reg.begin_capture(7, TTL).unwrap();
    let revoked = cap.revoked();
    assert!(reg.release(7, id));
    assert!(revoked.is_cancelled());
    assert_eq!(
        reg.begin_capture(7, TTL).err(),
        Some(CaptureRefusal::NoClaim)
    );
}

#[tokio::test(start_paused = true)]
async fn a_busy_claim_older_than_the_ttl_stays_busy() {
    let reg = Arc::new(VoiceRegistry::new());
    reg.claim(7, "master", Arc::new(Fake::default()));
    let cap = reg.begin_capture(7, TTL).unwrap();
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(reg.begin_capture(7, TTL).err(), Some(CaptureRefusal::Busy));
    assert_eq!(reg.owner(7).as_deref(), Some("master"));
    drop(cap);
    // Dropping refreshed last_used, so the claim is fresh again.
    assert!(reg.begin_capture(7, TTL).is_ok());
}

#[tokio::test]
async fn a_replaced_claim_is_told_it_was_replaced() {
    let reg = Arc::new(VoiceRegistry::new());
    let a = Arc::new(Fake::default());
    let b = Arc::new(Fake::default());
    reg.claim(7, "client:phone", a.clone());
    reg.claim(7, "master", b.clone());
    assert_eq!(a.revocations(), [RevokeReason::Replaced]);
    assert!(b.revocations().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_expired_claim_is_told_so_on_the_next_capture() {
    let reg = Arc::new(VoiceRegistry::new());
    let a = Arc::new(Fake::default());
    reg.claim(7, "master", a.clone());
    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(
        a.revocations().is_empty(),
        "lazy: nothing until a capture asks"
    );
    assert_eq!(
        reg.begin_capture(7, TTL).err(),
        Some(CaptureRefusal::NoClaim)
    );
    assert_eq!(a.revocations(), [RevokeReason::Expired]);
}

#[tokio::test]
async fn the_owners_own_release_is_not_a_revocation() {
    let reg = Arc::new(VoiceRegistry::new());
    let a = Arc::new(Fake::default());
    let id = reg.claim(7, "master", a.clone());
    assert!(reg.release(7, id));
    assert!(a.revocations().is_empty());
}

#[test]
fn each_revocation_has_its_own_close_code_and_words() {
    assert_eq!(RevokeReason::Replaced.close_code(), 4001);
    assert_eq!(
        RevokeReason::Replaced.text(),
        "microphone claimed elsewhere"
    );
    assert_eq!(RevokeReason::Expired.close_code(), 4002);
    assert_eq!(
        RevokeReason::Expired.text(),
        "microphone idle — turn 🎤 on again"
    );
    for r in [RevokeReason::Replaced, RevokeReason::Expired] {
        assert_eq!(RevokeReason::from_close_code(r.close_code()), Some(r));
    }
    assert_eq!(RevokeReason::from_close_code(1000), None);
}
