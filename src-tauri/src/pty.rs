use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Hard cap on the un-drained PTY byte buffer (1 MiB). The frontend normally
/// drains every 30-250 ms; this only bites if it stops entirely.
const PTY_BUFFER_CAP: usize = 1 << 20;

/// Smallest PTY the renderer is asked to lay out. A `fit()` result below this
/// (a not-yet-laid-out pane reporting 0×0) is clamped up so tmux never sees a
/// degenerate size.
const MIN_COLS: u16 = 40;
const MIN_ROWS: u16 = 10;

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

    /// Whether a PTY is currently attached (a master is held).
    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.master.is_some()
    }

    /// The single-PTY invariant: installing a new attachment ALWAYS closes
    /// the previous one first (killing its child and dropping its fds), then
    /// takes ownership of the new handles and the new reader's buffer.
    fn install(
        &mut self,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        buffer: Arc<Mutex<Vec<u8>>>,
    ) {
        self.close();
        self.master = Some(master);
        self.writer = Some(writer);
        self.child = Some(child);
        self.buffer = buffer;
    }

    pub(crate) fn close(&mut self) {
        // Terminate the child with SIGKILL BEFORE tearing down the pty fds.
        //
        // The child is the local `tmux attach`, or — for a remote host — the
        // `ssh -tt … tmux attach` that runs it. portable-pty's Child::kill()
        // sends SIGHUP, not SIGKILL. At this point our reader thread still holds
        // a cloned master fd, so the pty is still LIVE when that SIGHUP lands —
        // which lets `ssh -tt` do a *graceful* shutdown and relay a trailing
        // newline down its pty to the remote tmux pane. That stray `\n` is
        // delivered into the attached app's (claude's) input on every detach /
        // session-switch / deselect — the long-standing "new line on switch"
        // bug. SIGKILL gives the child no chance to relay anything; the ssh
        // channel and remote tty then tear down on their own and tmux detaches
        // our client cleanly.
        if let Some(mut child) = self.child.take() {
            match child.process_id() {
                Some(pid) => {
                    let _ = std::process::Command::new("kill")
                        .args(["-KILL", &pid.to_string()])
                        .status();
                }
                // No pid (already exited / unsupported): fall back to SIGHUP.
                None => {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        }
        // Now the child is gone, drop our fds and let the reader thread observe
        // EOF. Clear the buffer so a subsequent open delivers no stale bytes.
        self.writer.take();
        self.master.take();
        if let Ok(mut b) = self.buffer.lock() {
            b.clear();
        }
    }
}

// ---- Pure building blocks (unit-tested below) ----

/// Clamp a requested size to the renderer's minimum. Pixel dimensions are
/// always zero — we size in cells only.
pub(crate) fn clamp_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(MIN_ROWS),
        cols: cols.max(MIN_COLS),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The script run by the remote login shell. We re-export
/// LANG/LC_ALL/COLORTERM/TERM inside the remote shell so the embedded TUI gets
/// proper Unicode glyph rendering even if the remote sshd has AcceptEnv
/// disabled. `session_name` is quoted so it is one inert tmux argument.
pub(crate) fn remote_attach_script(session_name: &str) -> String {
    format!(
        "LANG=${{LANG:-en_US.UTF-8}} LC_ALL=${{LC_ALL:-en_US.UTF-8}} COLORTERM=truecolor TERM=xterm-256color tmux attach -t {}",
        quote(session_name)
    )
}

/// argv (program first) of the process attached to the PTY.
///
/// Local: `tmux attach -t <name>`. Remote: `ssh -tt <mux_opts> -- <host> bash
/// -lc '<script>'`. `mux_opts` is `SshClient::mux_opts` so the attach
/// multiplexes through the SAME ControlMaster — and inherits the SAME
/// keepalive (`ServerAlive*`) — as every other ssh command. Duplicating the
/// option list here is exactly how the wedged-master keepalive fix missed
/// this PTY path once: an attach over a black-holed master produced no
/// output AND never died, so the terminal froze with no way to self-heal.
///
/// CRITICAL: `ssh <host> bash -lc <script>` joins all trailing argv with
/// spaces before sending to the remote sshd, which then re-tokenizes. The
/// whole script is single-quoted so it crosses the ssh boundary as ONE shell
/// word; otherwise the remote bash receives `LANG=...` as its -c argument and
/// never runs tmux attach. (Same fix shape as RemoteTmux::remote_bash.)
pub(crate) fn attach_argv(
    host_alias: &str,
    session_name: &str,
    mux_opts: &[String],
) -> Vec<String> {
    if host_alias == "local" {
        return vec![
            "tmux".into(),
            "attach".into(),
            "-t".into(),
            session_name.into(),
        ];
    }
    let mut argv: Vec<String> = vec!["ssh".into(), "-tt".into()];
    argv.extend(mux_opts.iter().cloned());
    argv.extend([
        // `--` ends ssh option parsing so a host alias can never be
        // interpreted as an option (defence-in-depth; the alias is also
        // validated by the caller).
        "--".into(),
        host_alias.into(),
        "bash".into(),
        "-lc".into(),
        quote(&remote_attach_script(session_name)),
    ]);
    argv
}

/// Environment handed to the attached process, derived from `lookup` (the
/// process environment in production; a map in tests):
///
/// - `PATH` is inherited when set, so `/opt/homebrew/bin` (backfilled by
///   lib.rs at startup) is visible to the spawned tmux;
/// - `TERM=xterm-256color` always;
/// - `LANG` / `LC_ALL` / `LC_CTYPE` are inherited when set AND non-empty
///   (lib.rs backfills these so they are populated even when launched from
///   Finder; without a UTF-8 locale claude and other TUIs detect a degraded
///   terminal and render ASCII fallbacks — `_` instead of `└` / `↑` / `█`);
/// - `COLORTERM=truecolor` always, signalling that 24-bit SGR is supported
///   (our renderer handles it).
pub(crate) fn attach_env(lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(path) = lookup("PATH") {
        env.push(("PATH".into(), path));
    }
    env.push(("TERM".into(), "xterm-256color".into()));
    for var in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Some(val) = lookup(var) {
            if !val.is_empty() {
                env.push((var.into(), val));
            }
        }
    }
    env.push(("COLORTERM".into(), "truecolor".into()));
    env
}

/// Marker appended at open so the user can see in the terminal that the
/// channel is up.
pub(crate) fn attach_banner(session_name: &str, host_alias: &str) -> String {
    format!("\x1b[90m[cf] attached to {session_name}@{host_alias} via polling buffer\x1b[0m\r\n")
}

/// Append `chunk` to the un-drained buffer, dropping the OLDEST bytes so the
/// buffer never exceeds `cap`. Safety valve: if the frontend has stopped
/// draining (backgrounded tab, stalled loop) a busy session could grow this
/// without bound. Losing scrollback is acceptable; OOMing the process is not.
pub(crate) fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    buf.extend_from_slice(chunk);
    if buf.len() > cap {
        let excess = buf.len() - cap;
        buf.drain(0..excess);
    }
}

/// Split drained bytes into the part that can be decoded now and the length
/// of that part. Returns `(text, consumed)` where `consumed <= raw.len()`;
/// the caller pushes `raw[consumed..]` back to the front of the buffer.
///
/// - Valid UTF-8: the whole buffer.
/// - Ends in an INCOMPLETE multi-byte sequence (a chunk boundary fell
///   mid-codepoint): the valid prefix only, so the tail completes on the
///   next drain instead of being lossily replaced with U+FFFD.
/// - A genuine invalid byte mid-stream: lossy-decode the whole buffer (there
///   is nothing to wait for).
pub(crate) fn split_decodable(raw: &[u8]) -> (String, usize) {
    let valid_end = match std::str::from_utf8(raw) {
        Ok(_) => raw.len(),
        // `error_len() == None` means "unexpected end of input" — incomplete.
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => raw.len(),
    };
    (
        String::from_utf8_lossy(&raw[..valid_end]).into_owned(),
        valid_end,
    )
}

// ---- Command handlers ----

#[derive(Deserialize)]
pub struct PtyOpenArgs {
    pub session_name: String,
    pub host_alias: String,
    /// Initial PTY size from the frontend's xterm.js fit().
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn pty_open(
    args: PtyOpenArgs,
    state: State<'_, Mutex<PtyState>>,
    ssh: State<'_, std::sync::Arc<SshClient>>,
) -> Result<(), IpcError> {
    // Validate untrusted IPC input before it reaches `ssh` / `tmux`.
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.session_name)?;

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(clamp_size(args.cols, args.rows))
        .map_err(|e| IpcError::new("E_PTY", format!("openpty: {e}")))?;

    let mux_opts = if args.host_alias == "local" {
        Vec::new()
    } else {
        ssh.mux_opts(&args.host_alias, std::time::Duration::from_secs(5))
    };
    let argv = attach_argv(&args.host_alias, &args.session_name, &mux_opts);
    let mut cmd = CommandBuilder::new(&argv[0]);
    cmd.args(&argv[1..]);
    for (k, v) in attach_env(|k| std::env::var(k).ok()) {
        cmd.env(k, v);
    }

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

    // A FRESH buffer for each open. The previous PTY's reader thread may
    // still be alive momentarily (kill+wait is best-effort and the thread
    // loops on `read`) — handing the new reader its own buffer means stale
    // bytes from the old session can never bleed into the new screen. The
    // old buffer is orphaned and freed once that thread observes EOF.
    let buffer_for_thread: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let mut s = state
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
        s.install(pair.master, writer, child, Arc::clone(&buffer_for_thread));
    }

    if let Ok(mut b) = buffer_for_thread.lock() {
        b.extend_from_slice(attach_banner(&args.session_name, &args.host_alias).as_bytes());
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
                        append_capped(&mut b, &buf[..n], PTY_BUFFER_CAP);
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
    drain_from(&state)
}

/// Transport-agnostic body of `pty_drain`. Swaps the accumulated bytes out
/// under the locks, then decodes AFTER releasing them — the UTF-8 decode is
/// the bulk of the work and shouldn't block the reader thread (which needs
/// the buffer lock to append). An incomplete trailing multi-byte sequence is
/// pushed back to the FRONT of the buffer (it is chronologically before
/// anything the reader appended since the swap).
fn drain_from(state: &Mutex<PtyState>) -> Result<PtyDrainResult, IpcError> {
    let raw: Vec<u8> = {
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
        std::mem::take(&mut *buf)
    };
    let (data, valid_end) = split_decodable(&raw);
    if valid_end < raw.len() {
        let s = state
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
        let mut buf = s
            .buffer
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "pty buffer poisoned"))?;
        buf.splice(0..0, raw[valid_end..].iter().copied());
    }
    Ok(PtyDrainResult {
        data,
        bytes: valid_end,
    })
}

#[derive(Deserialize)]
pub struct PtyWriteArgs {
    pub data: String,
}

#[tauri::command]
pub fn pty_write(args: PtyWriteArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    write_to(&state, &args.data)
}

/// Transport-agnostic body of `pty_write`: `E_PTY_CLOSED` when nothing is
/// attached, otherwise the bytes are written and flushed to the master.
fn write_to(state: &Mutex<PtyState>, data: &str) -> Result<(), IpcError> {
    let mut s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    let writer = s
        .writer
        .as_mut()
        .ok_or_else(|| IpcError::new("E_PTY_CLOSED", "no PTY open"))?;
    writer
        .write_all(data.as_bytes())
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
    resize_in(&state, args.cols, args.rows)
}

/// Transport-agnostic body of `pty_resize`: `E_PTY_CLOSED` when nothing is
/// attached, otherwise the (clamped) size is applied to the master.
fn resize_in(state: &Mutex<PtyState>, cols: u16, rows: u16) -> Result<(), IpcError> {
    let s = state
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty mutex poisoned"))?;
    let master = s
        .master
        .as_ref()
        .ok_or_else(|| IpcError::new("E_PTY_CLOSED", "no PTY open"))?;
    master
        .resize(clamp_size(cols, rows))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mux() -> Vec<String> {
        vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            "ControlPath=/tmp/cm-hetzner.sock".into(),
            "-o".into(),
            "ServerAliveInterval=5".into(),
        ]
    }

    // ---- size clamping ----

    #[test]
    fn clamp_size_enforces_the_minimum_and_zero_pixels() {
        let tiny = clamp_size(0, 0);
        assert_eq!((tiny.cols, tiny.rows), (MIN_COLS, MIN_ROWS));
        let edge = clamp_size(MIN_COLS, MIN_ROWS);
        assert_eq!((edge.cols, edge.rows), (MIN_COLS, MIN_ROWS));
        let big = clamp_size(220, 60);
        assert_eq!((big.cols, big.rows), (220, 60));
        assert_eq!((big.pixel_width, big.pixel_height), (0, 0));
    }

    // ---- argv construction ----

    #[test]
    fn local_attach_is_a_plain_tmux_attach() {
        assert_eq!(
            attach_argv("local", "dev-foo", &mux()),
            vec!["tmux", "attach", "-t", "dev-foo"]
        );
    }

    #[test]
    fn remote_attach_reuses_mux_opts_and_ends_option_parsing_before_the_host() {
        let argv = attach_argv("hetzner", "dev-foo", &mux());
        assert_eq!(&argv[..2], ["ssh", "-tt"]);
        assert_eq!(&argv[2..8], mux().as_slice());
        assert_eq!(&argv[8..12], ["--", "hetzner", "bash", "-lc"]);
        assert_eq!(argv.len(), 13);
    }

    #[test]
    fn remote_script_is_one_quoted_word_that_unquotes_to_the_attach_command() {
        // The last argv element must be a SINGLE shell word: sshd re-joins
        // argv with spaces and the remote bash re-tokenizes. Prove it by
        // letting bash itself unquote it.
        let argv = attach_argv("hetzner", "dev-foo", &mux());
        let script = argv.last().unwrap();
        let out = std::process::Command::new("bash")
            .args(["-c", &format!("printf %s {script}")])
            .output()
            .expect("spawn bash");
        assert!(out.status.success());
        let unquoted = String::from_utf8(out.stdout).unwrap();
        assert_eq!(unquoted, remote_attach_script("dev-foo"));
        assert!(unquoted.ends_with("tmux attach -t 'dev-foo'"));
        for var in [
            "LANG=",
            "LC_ALL=",
            "COLORTERM=truecolor",
            "TERM=xterm-256color",
        ] {
            assert!(unquoted.contains(var), "missing {var} in {unquoted}");
        }
    }

    #[test]
    fn remote_script_neutralises_a_hostile_session_name() {
        // Defence in depth behind validate::tmux_name: even a name full of
        // metacharacters is one inert tmux argument.
        let script = remote_attach_script("x'; rm -rf / #");
        assert!(script.ends_with("tmux attach -t 'x'\\''; rm -rf / #'"));
        // And the outer quoting keeps the whole thing a single word.
        let argv = attach_argv("h", "x'; rm -rf / #", &[]);
        let out = std::process::Command::new("bash")
            .args(["-c", &format!("printf %s {}", argv.last().unwrap())])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), script);
    }

    // ---- environment ----

    fn env_from(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn attach_env_forces_term_and_colorterm_and_inherits_path_and_locale() {
        let vars = env_from(&[
            ("PATH", "/opt/homebrew/bin:/usr/bin"),
            ("LANG", "en_US.UTF-8"),
            ("LC_CTYPE", "C.UTF-8"),
            ("HOME", "/home/x"),
        ]);
        let env = attach_env(|k| vars.get(k).cloned());
        assert_eq!(
            env,
            vec![
                ("PATH".to_string(), "/opt/homebrew/bin:/usr/bin".to_string()),
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("LANG".to_string(), "en_US.UTF-8".to_string()),
                ("LC_CTYPE".to_string(), "C.UTF-8".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
            ]
        );
    }

    #[test]
    fn attach_env_skips_missing_path_and_empty_locale_vars() {
        let vars = env_from(&[("LANG", ""), ("LC_ALL", "")]);
        let env = attach_env(|k| vars.get(k).cloned());
        assert_eq!(
            env,
            vec![
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
            ]
        );
    }

    // ---- output buffering / decoding ----

    #[test]
    fn append_capped_drops_the_oldest_bytes() {
        let mut buf = Vec::new();
        append_capped(&mut buf, b"abcdef", 8);
        assert_eq!(buf, b"abcdef");
        append_capped(&mut buf, b"ghij", 8);
        assert_eq!(buf, b"cdefghij");
        // A single chunk larger than the cap keeps only its tail.
        append_capped(&mut buf, b"0123456789AB", 8);
        assert_eq!(buf, b"456789AB");
    }

    #[test]
    fn split_decodable_takes_everything_when_valid() {
        let (s, n) = split_decodable("héllo 🦀".as_bytes());
        assert_eq!(s, "héllo 🦀");
        assert_eq!(n, "héllo 🦀".len());
        assert_eq!(split_decodable(b""), (String::new(), 0));
    }

    #[test]
    fn split_decodable_holds_back_an_incomplete_trailing_codepoint() {
        let crab = "🦀".as_bytes(); // 4 bytes
        let mut raw = b"ok ".to_vec();
        raw.extend_from_slice(&crab[..2]);
        let (s, n) = split_decodable(&raw);
        assert_eq!(s, "ok ");
        assert_eq!(n, 3, "the partial codepoint must not be consumed");
    }

    #[test]
    fn split_decodable_lossy_decodes_a_genuinely_invalid_byte() {
        let raw = b"a\xffb";
        let (s, n) = split_decodable(raw);
        assert_eq!(s, "a\u{FFFD}b");
        assert_eq!(n, raw.len());
    }

    #[test]
    fn drain_retains_a_split_codepoint_until_the_rest_arrives() {
        let state = Mutex::new(PtyState::new());
        let crab = "🦀".as_bytes();
        // First chunk ends mid-codepoint.
        {
            let s = state.lock().unwrap();
            let mut b = s.buffer.lock().unwrap();
            b.extend_from_slice(b"x");
            b.extend_from_slice(&crab[..3]);
        }
        let first = drain_from(&state).unwrap();
        assert_eq!((first.data.as_str(), first.bytes), ("x", 1));
        // The reader appends the tail plus more; the retained prefix stays in
        // front so the codepoint reassembles in order.
        {
            let s = state.lock().unwrap();
            let mut b = s.buffer.lock().unwrap();
            assert_eq!(&b[..], &crab[..3]);
            b.extend_from_slice(&crab[3..]);
            b.extend_from_slice(b"y");
        }
        let second = drain_from(&state).unwrap();
        assert_eq!((second.data.as_str(), second.bytes), ("🦀y", 5));
        // Empty buffer drains to nothing.
        let third = drain_from(&state).unwrap();
        assert_eq!((third.data.as_str(), third.bytes), ("", 0));
    }

    #[test]
    fn attach_banner_names_session_and_host() {
        let banner = attach_banner("dev-foo", "hetzner");
        assert!(banner.contains("[cf] attached to dev-foo@hetzner"));
        assert!(banner.ends_with("\r\n"));
    }

    // ---- state / single-PTY invariant ----

    #[test]
    fn write_and_resize_report_e_pty_closed_when_nothing_is_attached() {
        let state = Mutex::new(PtyState::new());
        assert!(!state.lock().unwrap().is_open());
        assert_eq!(write_to(&state, "x").unwrap_err().code, "E_PTY_CLOSED");
        assert_eq!(resize_in(&state, 80, 24).unwrap_err().code, "E_PTY_CLOSED");
        // close() on a never-opened state is a no-op.
        state.lock().unwrap().close();
        assert!(!state.lock().unwrap().is_open());
    }

    /// A live attachment's parts plus the child's pid.
    type Sleeper = (
        Box<dyn MasterPty + Send>,
        Box<dyn Write + Send>,
        Box<dyn portable_pty::Child + Send + Sync>,
        u32,
    );

    /// Open a real PTY running `sleep` and return its parts plus the pid.
    fn spawn_sleeper() -> Option<Sleeper> {
        let pair = match native_pty_system().openpty(clamp_size(80, 24)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("SKIP: openpty unavailable here: {e}");
                return None;
            }
        };
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("30");
        let child = pair.slave.spawn_command(cmd).expect("spawn sleep");
        let pid = child.process_id().expect("pid");
        let writer = pair.master.take_writer().expect("writer");
        Some((pair.master, writer, child, pid))
    }

    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn installing_a_second_pty_kills_the_first_and_close_clears_everything() {
        let Some((m1, w1, c1, pid1)) = spawn_sleeper() else {
            return;
        };
        let state = Mutex::new(PtyState::new());
        let buf1 = Arc::new(Mutex::new(b"stale".to_vec()));
        state.lock().unwrap().install(m1, w1, c1, Arc::clone(&buf1));
        assert!(state.lock().unwrap().is_open());
        assert!(alive(pid1));
        // A live attachment accepts writes and resizes.
        write_to(&state, "hello").unwrap();
        resize_in(&state, 100, 30).unwrap();

        let Some((m2, w2, c2, pid2)) = spawn_sleeper() else {
            state.lock().unwrap().close();
            return;
        };
        let buf2 = Arc::new(Mutex::new(Vec::new()));
        state.lock().unwrap().install(m2, w2, c2, Arc::clone(&buf2));
        // Single-PTY invariant: the first child is gone (killed + reaped) and
        // its buffer was cleared; the second is live with its own buffer.
        assert!(!alive(pid1), "first attachment must be killed on re-open");
        assert!(alive(pid2));
        assert!(buf1.lock().unwrap().is_empty());
        assert!(state.lock().unwrap().is_open());

        state.lock().unwrap().close();
        assert!(!alive(pid2));
        assert!(!state.lock().unwrap().is_open());
        assert_eq!(write_to(&state, "x").unwrap_err().code, "E_PTY_CLOSED");
    }
}
