//! The desktop's microphone as a `VoiceSource`: the default input through
//! cpal, opened in `start` and closed when the returned guard drops.

use fleet_core::service::voice::{PcmTx, VoiceSource};

pub fn supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

pub struct CpalSource {
    /// Told about start / stop / error for the UI (`voice:state`). Unread
    /// where there is no capture.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub on_state: std::sync::Arc<dyn Fn(&'static str, Option<String>) + Send + Sync>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl VoiceSource for CpalSource {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        use super::resample::Converter;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use std::sync::mpsc::TryRecvError;
        use std::sync::{Arc, Mutex};
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let on_state = self.on_state.clone();
        // cpal's Stream is !Send on macOS: it lives on its own thread until
        // the guard's sender drops.
        std::thread::Builder::new()
            .name("voice-capture".into())
            .spawn(move || {
                // `Ok(None)`: `start` gave up waiting, so the device is never played.
                let run = || -> Result<Option<cpal::Stream>, String> {
                    let dev = cpal::default_host()
                        .default_input_device()
                        .ok_or("no microphone found")?;
                    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
                    // macOS and WASAPI default to f32; nothing else is converted.
                    if cfg.sample_format() != cpal::SampleFormat::F32 {
                        return Err(format!(
                            "unsupported microphone format: {}",
                            cfg.sample_format()
                        ));
                    }
                    let mut conv = Converter::new(cfg.sample_rate().0, cfg.channels());
                    let mut buf = Vec::with_capacity(3_200);
                    // The only sender: a stream error takes it, which ends the
                    // capture at once instead of leaving the recorder in silence.
                    let sink = Arc::new(Mutex::new(Some(tx)));
                    let err_sink = Arc::clone(&sink);
                    let err_state = on_state.clone();
                    let stream = dev
                        .build_input_stream(
                            &cfg.config(),
                            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                                buf.extend(conv.push(data));
                                if buf.len() >= 3_200 {
                                    // Contended only while an error takes the sender.
                                    if let Ok(sink) = sink.try_lock() {
                                        if let Some(tx) = sink.as_ref() {
                                            let _ = tx.try_send(std::mem::take(&mut buf));
                                        }
                                    }
                                }
                            },
                            move |e| {
                                if let Ok(mut sink) = err_sink.lock() {
                                    sink.take();
                                }
                                err_state("error", Some(e.to_string()));
                            },
                            None,
                        )
                        .map_err(|e| e.to_string())?;
                    if let Err(TryRecvError::Disconnected) = stop_rx.try_recv() {
                        return Ok(None);
                    }
                    stream.play().map_err(|e| e.to_string())?;
                    Ok(Some(stream))
                };
                match run() {
                    Ok(Some(stream)) => {
                        if ready_tx.send(Ok(())).is_err() {
                            // `start` already returned its timeout error.
                            drop(stream);
                            return;
                        }
                        on_state("capturing", None);
                        let _ = stop_rx.recv();
                        drop(stream);
                        on_state("claimed", None);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        on_state("error", Some(e.clone()));
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "the microphone did not open".to_string())??;
        Ok(Box::new(stop_tx))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl VoiceSource for CpalSource {
    fn start(&self, _tx: PcmTx) -> Result<Box<dyn Send>, String> {
        Err("voice relay is not supported on this platform".into())
    }
}
