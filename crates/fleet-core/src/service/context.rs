//! Context-size refresh after a turn (spec §1.5).

use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};

/// Read the tail of the session's current transcript and store its context
/// size. Best-effort and off the hook's response path.
// Task 4 fills this in; until then the Stop / StopFailure hooks spawn a no-op.
pub async fn refresh_context(_store: Arc<Mutex<Store>>, _ssh: Arc<SshClient>, _session_id: i64) {}
