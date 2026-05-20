use crate::ipc_error::IpcError;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::State;

/// One active PTY at a time (we render a single terminal pane). Opening a new
/// PTY closes the previous one. Holds the master (for resize), a writer (for
/// input forwarding), and the child handle (for kill on close). The reader is
/// moved into a background thread that emits chunks via the Tauri Channel
/// supplied at open time.
/// Polling-based PTY transport. The reader thread appends bytes to `buffer`;
/// the frontend calls `pty_drain` on a short interval (e.g. 30 ms) to swap
/// the buffer with an empty Vec and consume the bytes. This avoids the Tauri
/// 2 `emit`/`Channel` from-thread reliability issues observed empirically:
/// emits from the reader thread sometimes silently never reach JS, while
/// emits from the command's main runtime thread always do. Polling has the
/// same on-screen latency (~one frame) and no missing-event class of bugs.
pub struct PtyState {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            master: None,
            writer: None,
            child: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn close(&mut self) {
        // Drop writer first so the slave EOFs; then kill child to make sure
        // the reader thread terminates. Clear the buffer so a subsequent open
        // doesn't deliver stale bytes from the previous session.
        self.writer.take();
        self.master.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(mut b) = self.buffer.lock() {
            b.clear();
        }
    }
}

#[derive(Deserialize)]
pub struct PtyOpenArgs {
    pub session_name: String,
    /// Initial PTY size from the frontend's xterm.js fit().
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn pty_open(args: PtyOpenArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: args.rows.max(10),
            cols: args.cols.max(40),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| IpcError::new("E_PTY", format!("openpty: {e}")))?;

    let mut cmd = CommandBuilder::new("tmux");
    cmd.args(["attach", "-t", &args.session_name]);
    // Inherit PATH that lib.rs already backfilled at startup so /opt/homebrew/bin
    // is visible to the spawned tmux.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TERM", "xterm-256color");
    // Inherit locale env (lib.rs imports/backfills these at startup so they're
    // populated even when launched from Finder). Without UTF-8 locale, claude
    // and other modern TUIs detect a degraded terminal and render ASCII
    // fallbacks (`_` instead of `└` / `↑` / `█` block glyphs).
    for var in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.env(var, val);
            }
        }
    }
    // COLORTERM=truecolor signals to apps (claude, vim, etc.) that they can
    // emit 24-bit SGR sequences. Our renderer supports them already.
    cmd.env("COLORTERM", "truecolor");

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| IpcError::new("E_PTY", format!("spawn tmux attach: {e}")))?;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| IpcError::new("E_PTY", format!("clone reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| IpcError::new("E_PTY", format!("take writer: {e}")))?;

    // Acquire the shared buffer up-front so the reader thread can use it
    // without re-locking the PtyState. The buffer is an Arc<Mutex<Vec<u8>>>
    // so handles can be cloned cheaply across threads.
    let buffer_for_thread = {
        let s = state
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
        Arc::clone(&s.buffer)
    };
    {
        let mut s = state
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
        s.close();
        s.master = Some(pair.master);
        s.writer = Some(writer);
        s.child = Some(child);
        s.buffer = buffer_for_thread.clone();
    }

    // Append a marker so the user can see in xterm that the channel is up.
    if let Ok(mut b) = buffer_for_thread.lock() {
        b.extend_from_slice(
            format!(
                "\x1b[90m[cf] attached to {} via polling buffer\x1b[0m\r\n",
                args.session_name
            )
            .as_bytes(),
        );
    }

    // Reader thread: append bytes directly to the buffer the JS side drains.
    // No Tauri events involved — pure shared-state pattern.
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut total = 0usize;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    if let Ok(mut b) = buffer_for_thread.lock() {
                        b.extend_from_slice(
                            format!(
                                "\r\n\x1b[33m[cf] PTY EOF after {total} bytes (tmux attach exited)\x1b[0m\r\n"
                            )
                            .as_bytes(),
                        );
                    }
                    break;
                }
                Ok(n) => {
                    total += n;
                    if let Ok(mut b) = buffer_for_thread.lock() {
                        b.extend_from_slice(&buf[..n]);
                    } else {
                        break;
                    }
                }
                Err(e) => {
                    if let Ok(mut b) = buffer_for_thread.lock() {
                        b.extend_from_slice(
                            format!(
                                "\r\n\x1b[31m[cf] reader error after {total} bytes: {e}\x1b[0m\r\n"
                            )
                            .as_bytes(),
                        );
                    }
                    break;
                }
            }
        }
    });

    Ok(())
}

#[derive(Serialize)]
pub struct PtyDrainResult {
    /// UTF-8 lossy view of any bytes accumulated since the last drain.
    pub data: String,
    /// How many raw bytes were drained.
    pub bytes: usize,
}

#[tauri::command]
pub fn pty_drain(state: State<'_, Mutex<PtyState>>) -> Result<PtyDrainResult, IpcError> {
    let s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    let mut buf = s
        .buffer
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty buffer poisoned"))?;
    if buf.is_empty() {
        return Ok(PtyDrainResult {
            data: String::new(),
            bytes: 0,
        });
    }
    let bytes = buf.len();
    let data = String::from_utf8_lossy(&buf).into_owned();
    buf.clear();
    Ok(PtyDrainResult { data, bytes })
}

#[derive(Deserialize)]
pub struct PtyWriteArgs {
    pub data: String,
}

#[tauri::command]
pub fn pty_write(args: PtyWriteArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let mut s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    let writer = s
        .writer
        .as_mut()
        .ok_or_else(|| IpcError::new("E_PTY_CLOSED", "no PTY open"))?;
    writer
        .write_all(args.data.as_bytes())
        .map_err(|e| IpcError::new("E_PTY", format!("write: {e}")))?;
    writer
        .flush()
        .map_err(|e| IpcError::new("E_PTY", format!("flush: {e}")))?;
    Ok(())
}

#[derive(Deserialize)]
pub struct PtyResizeArgs {
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn pty_resize(args: PtyResizeArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    let master = s
        .master
        .as_ref()
        .ok_or_else(|| IpcError::new("E_PTY_CLOSED", "no PTY open"))?;
    master
        .resize(PtySize {
            rows: args.rows.max(10),
            cols: args.cols.max(40),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| IpcError::new("E_PTY", format!("resize: {e}")))?;
    Ok(())
}

#[tauri::command]
pub fn pty_close(state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let mut s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    s.close();
    Ok(())
}
