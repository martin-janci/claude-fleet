use crate::ipc_error::IpcError;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::State;

/// One active PTY at a time (we render a single terminal pane). Opening a new
/// PTY closes the previous one. Holds the master (for resize), a writer (for
/// input forwarding), and the child handle (for kill on close). The reader is
/// moved into a background thread that emits chunks via the Tauri Channel
/// supplied at open time.
pub struct PtyState {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            master: None,
            writer: None,
            child: None,
        }
    }

    fn close(&mut self) {
        // Drop writer first so the slave EOFs; then kill child to make sure
        // the reader thread terminates.
        self.writer.take();
        self.master.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
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
pub fn pty_open(
    args: PtyOpenArgs,
    on_data: Channel<String>,
    state: State<'_, Mutex<PtyState>>,
) -> Result<(), IpcError> {
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

    // Channel handshake check: visible in xterm immediately if the IPC path
    // works at all. If you never see this line, the Channel<String> on the
    // JS side never got attached.
    let _ = on_data.send("\x1b[90m[cf] channel ready, spawning tmux…\x1b[0m\r\n".to_string());

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

    let _ = on_data.send(format!(
        "\x1b[90m[cf] spawned tmux attach -t {}\x1b[0m\r\n",
        args.session_name
    ));

    // Move reader into a dedicated thread that pumps PTY output through the
    // Tauri channel as UTF-8 lossy strings. xterm.js can ingest these directly.
    // The thread also emits diagnostic markers on entry, EOF, and error so any
    // pipeline break is immediately visible in the terminal pane.
    let reader_channel = on_data.clone();
    std::thread::spawn(move || {
        let _ = reader_channel.send("\x1b[90m[cf] reader thread up\x1b[0m\r\n".to_string());
        let mut buf = [0u8; 4096];
        let mut total = 0usize;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = reader_channel.send(format!(
                        "\r\n\x1b[33m[cf] PTY EOF after {total} bytes (tmux attach exited)\x1b[0m\r\n"
                    ));
                    break;
                }
                Ok(n) => {
                    total += n;
                    let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if reader_channel.send(chunk).is_err() {
                        // Frontend dropped the channel (component unmounted).
                        break;
                    }
                }
                Err(e) => {
                    let _ = reader_channel.send(format!(
                        "\r\n\x1b[31m[cf] reader error after {total} bytes: {e}\x1b[0m\r\n"
                    ));
                    break;
                }
            }
        }
    });

    let mut s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    s.close();
    s.master = Some(pair.master);
    s.writer = Some(writer);
    s.child = Some(child);
    Ok(())
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
