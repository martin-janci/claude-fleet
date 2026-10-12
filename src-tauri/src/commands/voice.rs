//! Tauri IPC wrappers for the voice relay's claim (`crate::voice`): this
//! machine's microphone becomes the one a session's Claude Code `/voice`
//! records from.
//!
//! The microphone is this machine's in both modes. Standalone the claim is
//! registered with the embedded server's registry; paired, the desktop holds
//! the hub's `/voice/source` open, which the hub refuses (and this command
//! reports) while `voice.enabled` is off there.

use crate::backend::FleetBackend;
use crate::voice::{cpal_source, Emit, VoiceState, STATE_EVENT};
use fleet_core::ipc_error::{codes, lock, IpcError};
use fleet_core::service::settings;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

fn emitter(app: &AppHandle) -> Emit {
    let app = app.clone();
    Arc::new(move |v| {
        let _ = tauri::Emitter::emit(&app, STATE_EVENT, v);
    })
}

/// Make this machine's microphone the session's, releasing any earlier claim.
#[tauri::command]
pub async fn voice_claim(
    session_id: i64,
    app: AppHandle,
    backend: State<'_, Arc<FleetBackend>>,
    voice: State<'_, VoiceState>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    if !crate::voice::capture::supported() {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "voice relay needs the macOS or Windows app",
        ));
    }
    let emit = emitter(&app);
    let source = cpal_source(session_id, Arc::clone(&emit));
    match backend.hub() {
        Some(hub) => {
            let cfg = hub.config();
            voice.claim_hub(&cfg.base_url, &cfg.token, session_id, source, &emit);
        }
        None => {
            let enabled = {
                let s = lock(&store)?;
                settings::get_bool(&s, settings::VOICE_ENABLED)
            };
            if !enabled {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "turn on Settings → Limits → Voice first",
                ));
            }
            voice.claim_local(session_id, source, &emit);
        }
    }
    Ok(())
}

/// Release this machine's claim, whichever session it is on.
#[tauri::command]
pub async fn voice_release(app: AppHandle, voice: State<'_, VoiceState>) -> Result<(), IpcError> {
    voice.release(&emitter(&app));
    Ok(())
}
