use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::shell::quote;
use fleet_core::ssh::SshClient;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;

/// Hard cap on the un-drained PTY byte buffer (1 MiB). The frontend normally
/// drains every 30-250 ms; this only bites if it stops entirely.
const PTY_BUFFER_CAP: usize = 1 << 20;

/// How many un-written input chunks the writer thread may queue. One chunk is
/// a keystroke or a whole paste, so this is generous for a human and still
/// bounded if the PTY stops accepting input (a wedged `ssh -tt`, a child that
/// stopped reading: the tty buffer fills after ~20 KB and `write` blocks for
/// good). Past it, `pty_write` reports `E_PTY_BUSY` instead of blocking.
const PTY_INPUT_QUEUE: usize = 256;

/// How long `PtyParts::teardown` waits for a killed child before handing it
/// to a detached reaper thread. SIGKILL is normally instant; a child stuck in
/// an uninterruptible wait must not stall the caller.
/// How long teardown waits in line for a killed child before handing it to a
/// detached thread. Long enough for a SIGKILLed process, short enough that an
/// async-runtime worker is never meaningfully parked.
const PTY_REAP_INLINE: Duration = Duration::from_millis(50);

const PTY_REAP_TIMEOUT: Duration = Duration::from_secs(2);

/// Smallest PTY the renderer is asked to lay out. A `fit()` result below this
/// (a not-yet-laid-out pane reporting 0×0) is clamped up so tmux never sees a
/// degenerate size.
///
/// These MUST stay equal to the floor in `computeDimensions`
/// (src/lib/TerminalView.svelte). Clamping higher than the renderer does not
/// give the user a bigger terminal — it gives tmux a grid the Screen does not
/// have, so output wraps, scrolls and positions the cursor for the wrong
/// geometry and the top rows (tmux status line included) never appear.
const MIN_COLS: u16 = 10;
const MIN_ROWS: u16 = 2;

/// One active PTY at a time (we render a single terminal pane). Opening a new
/// PTY closes the previous one. Holds the master (for resize), the writer
/// thread's input channel (for keystrokes and pastes) and the child handle
/// (for kill on close); output and liveness live in `PtyShared`, which the
/// reader thread owns a clone of.
///
/// Polling-based transport: the reader thread appends bytes to the shared
/// buffer and the frontend calls `pty_drain` on a short interval (e.g. 30 ms)
/// to swap them out. This avoids the Tauri 2 `emit`/`Channel` from-thread
/// reliability issues observed empirically: emits from the reader thread
/// sometimes silently never reach JS, while emits from the command's own
/// runtime thread always do. Polling has the same on-screen latency (~one
/// frame) and no missing-event class of bugs.
pub struct PtyState {
    master: Option<Box<dyn MasterPty + Send>>,
    /// Input goes to the writer thread through a bounded channel. NOTHING
    /// that can block may live under this mutex (see CLAUDE.md): a PTY whose
    /// child stopped reading blocks `write` forever, and holding the lock
    /// there froze drains, closes and — with sync commands — the whole app.
    input_tx: Option<SyncSender<Vec<u8>>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    shared: Arc<PtyShared>,
}

/// Everything one open shares with its reader thread. A FRESH one per open:
/// a reader that is still winding down owns the previous `Arc` and can
/// neither append to nor flag the PTY that replaced it.
pub(crate) struct PtyShared {
    buffer: Mutex<PtyBuffer>,
    /// Set by the reader thread once the PTY is over (EOF or a read error).
    /// The `[cf]` lines it also writes into the buffer are display text only;
    /// THIS is what the frontend acts on, so output that merely contains that
    /// text can never fake a disconnect.
    exited: AtomicBool,
}

/// Un-drained output plus the "we had to throw output away" latch.
#[derive(Default)]
pub(crate) struct PtyBuffer {
    bytes: Vec<u8>,
    overflowed: bool,
}

impl PtyShared {
    pub(crate) fn new() -> Self {
        Self {
            buffer: Mutex::new(PtyBuffer::default()),
            exited: AtomicBool::new(false),
        }
    }

    /// Append reader output. `false` means the buffer lock is poisoned and
    /// the reader should stop.
    fn append(&self, chunk: &[u8]) -> bool {
        match self.buffer.lock() {
            Ok(mut b) => {
                b.append_capped(chunk, PTY_BUFFER_CAP);
                true
            }
            Err(_) => false,
        }
    }

    /// Append a human-readable `[cf]` line for the user to see. Display text
    /// only — never a signal (see `exited`).
    fn note(&self, line: &str) {
        self.append(line.as_bytes());
    }
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            master: None,
            input_tx: None,
            child: None,
            shared: Arc::new(PtyShared::new()),
        }
    }

    /// Whether a PTY is currently attached (a master is held).
    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.master.is_some()
    }

    /// The single-PTY invariant: installing a new attachment takes ownership
    /// of the new handles and hands the PREVIOUS ones back, so the caller can
    /// tear them down (kill, reap, drop fds — all blocking) after releasing
    /// the state lock.
    #[must_use = "the previous attachment must be torn down off-lock"]
    fn install(
        &mut self,
        master: Box<dyn MasterPty + Send>,
        input_tx: SyncSender<Vec<u8>>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        shared: Arc<PtyShared>,
    ) -> PtyParts {
        let previous = self.take_parts();
        self.master = Some(master);
        self.input_tx = Some(input_tx);
        self.child = Some(child);
        self.shared = shared;
        previous
    }

    /// Detach everything from the state, leaving it closed. Cheap and
    /// non-blocking: the returned parts do the blocking work.
    #[must_use = "the attachment must be torn down off-lock"]
    fn take_parts(&mut self) -> PtyParts {
        PtyParts {
            master: self.master.take(),
            input_tx: self.input_tx.take(),
            child: self.child.take(),
            // Leave the state genuinely closed: after a detach, a drain must
            // not see the dead attachment's buffer or its `exited` flag.
            shared: std::mem::replace(&mut self.shared, Arc::new(PtyShared::new())),
        }
    }
}

/// One attachment's handles, taken out of `PtyState` so they can be torn down
/// with NO lock held.
struct PtyParts {
    master: Option<Box<dyn MasterPty + Send>>,
    input_tx: Option<SyncSender<Vec<u8>>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    shared: Arc<PtyShared>,
}

impl PtyParts {
    /// Kill the child, reap it, then drop the fds. Blocking by nature — it
    /// must never run under `Mutex<PtyState>` or on the main thread.
    fn teardown(self) {
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
        if let Some(mut child) = self.child {
            match child.process_id() {
                // Signal directly: spawning `/bin/kill` is a fork+exec+wait
                // we would otherwise do on the caller's thread.
                //
                // SAFETY: `kill` takes no pointers and cannot trap. The pid is
                // our own child, not yet reaped, so the OS cannot have recycled
                // it. `pid > 1` keeps a bogus 0 (our whole process group) or 1
                // (init) out of the call, as `add_project.rs` does.
                Some(pid) if pid > 1 => unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                },
                // No pid (already exited / unsupported), or an implausible one:
                // fall back to SIGHUP.
                _ => {
                    let _ = child.kill();
                }
            }
            reap(child);
        }
        // Now the child is gone, drop our fds and let the reader thread observe
        // EOF. Dropping the sender ends the writer thread, which drops the
        // writer — so portable-pty's blocking EOF write happens THERE, after
        // the child is dead, not here. Clear the buffer so a subsequent open
        // delivers no stale bytes.
        drop(self.input_tx);
        drop(self.master);
        if let Ok(mut b) = self.shared.buffer.lock() {
            *b = PtyBuffer::default();
        }
    }
}

/// Reap a killed child. Teardown runs from Tauri's async runtime, so the
/// in-line wait is capped at `PTY_REAP_INLINE` (a SIGKILLed child needs only
/// milliseconds); anything slower is polled on a detached thread, bounded by
/// `PTY_REAP_TIMEOUT` and then a blocking wait so it cannot linger.
fn reap(mut child: Box<dyn portable_pty::Child + Send + Sync>) {
    // A SIGKILLed child is gone in milliseconds, so wait briefly in line: that
    // keeps "closed means reaped" true for callers (and tests) without parking
    // a runtime worker for anything like `PTY_REAP_TIMEOUT`.
    let inline_deadline = Instant::now() + PTY_REAP_INLINE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {}
        }
        if Instant::now() >= inline_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Still running (uninterruptible wait): finish off-thread so no caller waits.
    std::thread::spawn(move || {
        let deadline = Instant::now() + PTY_REAP_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                let _ = child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

/// Close the current attachment: detach it under the lock, tear it down
/// after releasing it.
pub(crate) fn close_pty(state: &Mutex<PtyState>) {
    let parts = match state.lock() {
        Ok(mut s) => s.take_parts(),
        // Poisoned: nothing safe to take, and the handles are dropped with
        // the state at exit.
        Err(_) => return,
    };
    parts.teardown();
}

/// Own the PTY's writer on a dedicated thread, fed by a bounded channel.
/// `pty_write` only hands a chunk over, so a `write` that blocks — a paste
/// into a session whose child stopped reading fills the tty buffer after
/// ~20 KB and then blocks forever — can no longer stall the IPC thread or
/// hold `Mutex<PtyState>` against drains and closes.
fn spawn_writer(mut writer: Box<dyn Write + Send>, shared: Arc<PtyShared>) -> SyncSender<Vec<u8>> {
    let (tx, rx) = sync_channel::<Vec<u8>>(PTY_INPUT_QUEUE);
    std::thread::spawn(move || {
        while let Ok(chunk) = rx.recv() {
            if let Err(e) = writer.write_all(&chunk).and_then(|_| writer.flush()) {
                shared.note(&format!("\r\n\x1b[31m[cf] writer error: {e}\x1b[0m\r\n"));
                break;
            }
        }
        // Dropping `rx` here disconnects the sender, so the next `pty_write`
        // reports E_PTY_CLOSED instead of queueing into a dead thread.
    });
    tx
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
/// disabled. The target is `crate::tmux::exact_session` so a session that is
/// gone fails the attach instead of prefix-matching a DIFFERENT session (every
/// keystroke would otherwise go to another Claude); it is quoted so it crosses
/// as one inert tmux argument.
pub(crate) fn remote_attach_script(session_name: &str) -> String {
    format!(
        "LANG=${{LANG:-en_US.UTF-8}} LC_ALL=${{LC_ALL:-en_US.UTF-8}} COLORTERM=truecolor TERM=xterm-256color tmux attach -t {}",
        quote(&fleet_core::tmux::exact_session(session_name))
    )
}

/// argv (program first) of the process attached to the PTY.
///
/// Local: `tmux attach -t <name>`. Remote: `ssh -tt <mux_opts> -- <host> bash
/// -lc '<script>'`. `mux_opts` comes from `attach_mux_opts` →
/// `SshClient::mux_opts_for_pty`: its OWN ControlPath (`cm-<host>-tty.sock`)
/// and a gentler keepalive (`ServerAliveInterval=15` × `ServerAliveCountMax=3`
/// ⇒ tolerates a 45s stall), deliberately separate from the probe master's
/// `cm-<host>.sock`. So a probe's `maybe_reset_master` can never take the
/// attached terminal down with it — the reset acts on a different socket
/// entirely.
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
            fleet_core::tmux::exact_session(session_name),
        ];
    }
    let mut argv: Vec<String> = vec![
        "ssh".into(),
        "-tt".into(),
        // `-tt` allocates a tty, which turns ON ssh's own `~` escape
        // character. A `~` typed as the first character of a line (or in a
        // pasted line) is then swallowed by the LOCAL ssh client instead of
        // reaching tmux: `~.` kills the attach outright, `~?` prints ssh's
        // help into the pane, `~B`/`~R`/`~#` do worse. Local attaches have no
        // ssh in the way, so this is the only place the two differ.
        "-o".into(),
        "EscapeChar=none".into(),
        // Bound the INITIAL connect only — never the attached session, which
        // is long-lived by design. Without it a host nothing can dial (an
        // agent-transport host with no SSH route from this machine) hangs on
        // the OS TCP default, ~75s of blank pane, before the UI can say so.
        // A reused ControlMaster connection skips the connect entirely.
        "-o".into(),
        "ConnectTimeout=10".into(),
    ];
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

/// `mux_opts` for the attach site: none for `local` (no ssh in the path),
/// otherwise [`SshClient::mux_opts_for_pty`] — the terminal's own
/// ControlPath and a gentler keepalive, so a probe's master reset (a
/// DIFFERENT ssh call, on the DIFFERENT `cm-<host>.sock`) never takes the
/// user's attached terminal down with it.
pub(crate) fn attach_mux_opts(ssh: &SshClient, host: &str) -> Vec<String> {
    if host == "local" {
        Vec::new()
    } else {
        ssh.mux_opts_for_pty(host, std::time::Duration::from_secs(5))
    }
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

impl PtyBuffer {
    /// Append `chunk`, or — when that would exceed `cap` — drop EVERYTHING
    /// and latch `overflowed` until the next drain reports it.
    ///
    /// Safety valve: if the frontend has stopped draining (backgrounded tab,
    /// stalled loop) a busy session could grow this without bound. Trimming
    /// the oldest bytes instead (what this used to do) cuts at an arbitrary
    /// offset, so the stream resumes mid-escape or mid-codepoint AND loses
    /// the DECSET modes tmux sends once per attach — alt screen, mouse,
    /// bracketed paste, scroll region. The screen is unrecoverable from
    /// there, so say so once and let the frontend re-attach. Dropping while
    /// latched also drops the O(cap) memmove the trim did on every read.
    pub(crate) fn append_capped(&mut self, chunk: &[u8], cap: usize) {
        if self.overflowed {
            return;
        }
        if self.bytes.len() + chunk.len() > cap {
            self.bytes.clear();
            self.overflowed = true;
            return;
        }
        self.bytes.extend_from_slice(chunk);
    }
}

/// How many bytes at the very END of `raw` are an INCOMPLETE multi-byte UTF-8
/// sequence (0-3) — a chunk boundary that fell mid-codepoint. The drain holds
/// those back so the codepoint reassembles on the next read instead of being
/// lossily replaced with U+FFFD.
///
/// It scans back at most 3 bytes for a lead byte and compares the length that
/// lead announces with the bytes that actually follow it. Deliberately
/// independent of anything earlier in the buffer: `from_utf8`'s FIRST error
/// wins, so one genuinely invalid byte in front used to drag the trailing
/// partial codepoint into the lossy decode with it — one 🦀 came out as three
/// U+FFFD. Everything before the suffix is decoded lossily; only the suffix
/// waits.
pub(crate) fn incomplete_suffix_len(raw: &[u8]) -> usize {
    for back in 1..=raw.len().min(3) {
        let need = match raw[raw.len() - back] {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            // A continuation byte: its lead may still be within reach.
            0x80..=0xBF => continue,
            // ASCII, or an invalid lead (0xC0/0xC1, 0xF5..=0xFF): nothing
            // that a later read could complete.
            _ => return 0,
        };
        // `back` bytes are present (the lead plus what follows it).
        return if need > back { back } else { 0 };
    }
    0
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

/// Attach the single global PTY to a session's tmux pane.
///
/// Opens an `ssh … tmux attach` from THIS machine. It carries no remote-mode
/// guard, and that is deliberate: the hub is not in this path at all. The
/// argv is built from the alias and the tmux name the caller passes, the ssh
/// options come from [`attach_mux_opts`] → [`SshClient::mux_opts_for_pty`]
/// (pure string construction), and nothing here reads `state.db` — so a
/// hub-client desktop attaches exactly
/// as a standalone one does, using its own `~/.ssh/config`.
///
/// What it cannot reach is an **agent** host, which has no SSH route from
/// anywhere; `TerminalView` does not offer an attach for one. A host this
/// machine simply lacks a `Host` block for fails in `ssh` with ssh's own
/// message, in the pane, which is more use than a refusal would be.
#[tauri::command(async)]
pub fn pty_open(
    args: PtyOpenArgs,
    state: State<'_, Mutex<PtyState>>,
    ssh: State<'_, std::sync::Arc<SshClient>>,
) -> Result<(), IpcError> {
    // Validate untrusted IPC input before it reaches `ssh` / `tmux`.
    fleet_core::validate::host_alias(&args.host_alias)?;
    fleet_core::validate::tmux_name(&args.session_name)?;

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(clamp_size(args.cols, args.rows))
        .map_err(|e| IpcError::new(codes::E_PTY, format!("openpty: {e}")))?;

    let mux_opts = attach_mux_opts(&ssh, &args.host_alias);
    let argv = attach_argv(&args.host_alias, &args.session_name, &mux_opts);
    let mut cmd = CommandBuilder::new(&argv[0]);
    cmd.args(&argv[1..]);
    for (k, v) in attach_env(|k| std::env::var(k).ok()) {
        cmd.env(k, v);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| IpcError::new(codes::E_PTY, format!("spawn tmux attach: {e}")))?;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| IpcError::new(codes::E_PTY, format!("clone reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| IpcError::new(codes::E_PTY, format!("take writer: {e}")))?;

    // A FRESH buffer for each open. The previous PTY's reader thread may
    // still be alive momentarily (kill+wait is best-effort and the thread
    // loops on `read`) — handing the new reader its own buffer means stale
    // bytes from the old session can never bleed into the new screen. The
    // old buffer is orphaned and freed once that thread observes EOF.
    let shared = Arc::new(PtyShared::new());
    let input_tx = spawn_writer(writer, Arc::clone(&shared));
    let previous = {
        let mut s = state
            .lock()
            .map_err(|_| IpcError::new(codes::E_LOCK, "pty mutex poisoned"))?;
        s.install(pair.master, input_tx, child, Arc::clone(&shared))
    };
    // Kill and reap the attachment we just replaced with the lock released.
    previous.teardown();

    shared.note(&attach_banner(&args.session_name, &args.host_alias));

    // Reader thread: append bytes directly to the buffer the JS side drains.
    // No Tauri events involved — pure shared-state pattern.
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut total = 0usize;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    shared.note(&format!(
                        "\r\n\x1b[33m[cf] PTY EOF after {total} bytes (tmux attach exited)\x1b[0m\r\n"
                    ));
                    break;
                }
                Ok(n) => {
                    total += n;
                    if !shared.append(&buf[..n]) {
                        break;
                    }
                }
                Err(e) => {
                    shared.note(&format!(
                        "\r\n\x1b[31m[cf] reader error after {total} bytes: {e}\x1b[0m\r\n"
                    ));
                    break;
                }
            }
        }
        // Out of band, AFTER the display line: the frontend reacts to this,
        // never to text it happened to find in the session's own output.
        shared.exited.store(true, Ordering::Release);
    });

    Ok(())
}

#[derive(Serialize)]
pub struct PtyDrainResult {
    /// UTF-8 lossy view of any bytes accumulated since the last drain.
    pub data: String,
    /// How many raw bytes were drained.
    pub bytes: usize,
    /// The PTY is gone (the reader saw EOF or a read error). Sticky, and
    /// only reported once every remaining byte has been handed over — so it
    /// can arrive with `bytes == 0`.
    pub eof: bool,
    /// Output was dropped: the un-drained buffer hit its cap. Reported once,
    /// then cleared. The screen cannot be repaired from the stream (the lost
    /// bytes include mode switches tmux sends only once), so the frontend
    /// must re-attach.
    pub overflowed: bool,
}

#[tauri::command(async)]
pub fn pty_drain(state: State<'_, Mutex<PtyState>>) -> Result<PtyDrainResult, IpcError> {
    drain_from(&state)
}

/// Transport-agnostic body of `pty_drain`. Swaps the accumulated bytes out
/// under the buffer lock, leaving an incomplete trailing multi-byte sequence
/// BEHIND in that same buffer (it is chronologically before anything the
/// reader appends next), and decodes AFTER releasing the lock — the UTF-8
/// decode is the bulk of the work and shouldn't block the reader thread.
///
/// One lock acquisition, one buffer: the earlier shape re-locked to push the
/// tail back, which a concurrent `install()` could turn into "the old
/// session's bytes land in front of the new session's first output".
fn drain_from(state: &Mutex<PtyState>) -> Result<PtyDrainResult, IpcError> {
    let (raw, overflowed, eof) = {
        let s = state
            .lock()
            .map_err(|_| IpcError::new(codes::E_LOCK, "pty mutex poisoned"))?;
        let exited = s.shared.exited.load(Ordering::Acquire);
        let mut buf = s
            .shared
            .buffer
            .lock()
            .map_err(|_| IpcError::new(codes::E_LOCK, "pty buffer poisoned"))?;
        let overflowed = std::mem::take(&mut buf.overflowed);
        // Once the reader is done nothing can complete a partial codepoint,
        // so hand the bytes over as they are.
        let keep = if exited {
            buf.bytes.len()
        } else {
            buf.bytes.len() - incomplete_suffix_len(&buf.bytes)
        };
        let tail = buf.bytes.split_off(keep);
        let raw = std::mem::replace(&mut buf.bytes, tail);
        // EOF only after the last byte has been delivered, so the `[cf]`
        // line is on screen before the frontend reacts to it.
        let eof = exited && raw.is_empty();
        (raw, overflowed, eof)
    };
    Ok(PtyDrainResult {
        bytes: raw.len(),
        data: String::from_utf8_lossy(&raw).into_owned(),
        eof,
        overflowed,
    })
}

#[derive(Deserialize)]
pub struct PtyWriteArgs {
    pub data: String,
}

/// Deliberately NOT `(async)`: `write_to` only takes the state lock briefly and
/// `try_send`s, so it cannot block the caller, and Tauri's sync dispatch keeps
/// keystrokes in the order they were typed. An async command would hand each
/// call to a separate runtime task, letting two fast keystrokes race.
#[tauri::command]
pub fn pty_write(args: PtyWriteArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    write_to(&state, &args.data)
}

/// Transport-agnostic body of `pty_write`. Hands the bytes to the writer
/// thread and returns immediately: `E_PTY_CLOSED` when nothing is attached
/// (or the writer thread died on an I/O error), `E_PTY_BUSY` when the input
/// queue is full — the PTY is not draining our input, and blocking here
/// would freeze the terminal instead of just this keystroke.
fn write_to(state: &Mutex<PtyState>, data: &str) -> Result<(), IpcError> {
    let tx = {
        let s = state
            .lock()
            .map_err(|_| IpcError::new(codes::E_LOCK, "pty mutex poisoned"))?;
        s.input_tx
            .as_ref()
            .ok_or_else(|| IpcError::new(codes::E_PTY_CLOSED, "no PTY open"))?
            .clone()
    };
    match tx.try_send(data.as_bytes().to_vec()) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(IpcError::new(
            "E_PTY_BUSY",
            "terminal is not accepting input",
        )),
        Err(TrySendError::Disconnected(_)) => {
            Err(IpcError::new(codes::E_PTY_CLOSED, "PTY input closed"))
        }
    }
}

#[derive(Deserialize)]
pub struct PtyResizeArgs {
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command(async)]
pub fn pty_resize(args: PtyResizeArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    resize_in(&state, args.cols, args.rows)
}

/// Transport-agnostic body of `pty_resize`: `E_PTY_CLOSED` when nothing is
/// attached, otherwise the (clamped) size is applied to the master.
fn resize_in(state: &Mutex<PtyState>, cols: u16, rows: u16) -> Result<(), IpcError> {
    let s = state
        .lock()
        .map_err(|_| IpcError::new(codes::E_LOCK, "pty mutex poisoned"))?;
    let master = s
        .master
        .as_ref()
        .ok_or_else(|| IpcError::new(codes::E_PTY_CLOSED, "no PTY open"))?;
    master
        .resize(clamp_size(cols, rows))
        .map_err(|e| IpcError::new(codes::E_PTY, format!("resize: {e}")))?;
    Ok(())
}

#[tauri::command(async)]
pub fn pty_close(state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    close_pty(&state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn bash_available() -> bool {
        std::process::Command::new("bash")
            .args(["-c", "true"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

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

    #[test]
    fn clamp_size_leaves_the_renderers_smallest_grid_alone() {
        // The backend minimum MUST equal `computeDimensions`'s floor in
        // src/lib/TerminalView.svelte. A bigger floor here silently gives
        // tmux a larger grid than the Screen holds: output wraps and scrolls
        // for the wrong geometry and the top rows (status line included) are
        // never rendered.
        assert_eq!(
            (MIN_COLS, MIN_ROWS),
            (10, 2),
            "must match computeDimensions"
        );
        let smallest = clamp_size(10, 2);
        assert_eq!((smallest.cols, smallest.rows), (10, 2));
    }

    // ---- argv construction ----

    #[test]
    fn local_attach_targets_the_session_exactly() {
        // `=` disables tmux's prefix / fnmatch lookup: attaching to a dead
        // `dev-foo` must fail, not land in `dev-foo--feat-x`.
        assert_eq!(
            attach_argv("local", "dev-foo", &mux()),
            vec!["tmux", "attach", "-t", "=dev-foo"]
        );
    }

    #[test]
    fn remote_attach_reuses_mux_opts_and_ends_option_parsing_before_the_host() {
        let argv = attach_argv("hetzner", "dev-foo", &mux());
        assert_eq!(
            &argv[..6],
            [
                "ssh",
                "-tt",
                "-o",
                "EscapeChar=none",
                "-o",
                "ConnectTimeout=10"
            ]
        );
        assert_eq!(&argv[6..12], mux().as_slice());
        assert_eq!(&argv[12..16], ["--", "hetzner", "bash", "-lc"]);
        assert_eq!(argv.len(), 17);
    }

    #[test]
    fn remote_attach_bounds_the_initial_connect() {
        // Without this, a host nothing can dial hangs on the OS TCP default
        // (~75s) with a blank pane before the UI can explain itself. It
        // bounds only the INITIAL connect, never the attached session, and a
        // reused ControlMaster connection skips it entirely.
        let argv = attach_argv("hetzner", "dev-foo", &[]);
        let at = argv
            .iter()
            .position(|a| a == "ConnectTimeout=10")
            .expect("ssh attach carries a connect timeout");
        assert_eq!(argv[at - 1], "-o");
        assert!(at < argv.iter().position(|a| a == "--").unwrap());
        // A local attach runs tmux directly — no ssh, nothing to bound.
        assert!(!attach_argv("local", "dev-foo", &[])
            .iter()
            .any(|a| a.contains("ConnectTimeout")));
    }

    #[test]
    fn remote_attach_disables_the_ssh_escape_character() {
        // With a tty (`-tt`) ssh enables its `~` escape: a `~` typed first on
        // a line is eaten by the LOCAL ssh client, and `~.` tears the attach
        // down. The pane must see both characters instead.
        let argv = attach_argv("hetzner", "dev-foo", &[]);
        let esc = argv.iter().position(|a| a == "EscapeChar=none").unwrap();
        assert_eq!(argv[esc - 1], "-o");
        assert!(esc < argv.iter().position(|a| a == "--").unwrap());
        // Local attaches run tmux directly — no ssh, nothing to disable.
        assert!(!attach_argv("local", "dev-foo", &[])
            .iter()
            .any(|a| a.contains("EscapeChar")));
    }

    #[test]
    fn remote_script_is_one_quoted_word_that_unquotes_to_the_attach_command() {
        // The last argv element must be a SINGLE shell word: sshd re-joins
        // argv with spaces and the remote bash re-tokenizes. Prove it by
        // letting bash itself unquote it.
        if !bash_available() {
            eprintln!("SKIP remote_script_is_one_quoted_word_that_unquotes_to_the_attach_command: bash not on PATH");
            return;
        }
        let argv = attach_argv("hetzner", "dev-foo", &mux());
        let script = argv.last().unwrap();
        let out = std::process::Command::new("bash")
            .args(["-c", &format!("printf %s {script}")])
            .output()
            .expect("spawn bash");
        assert!(out.status.success());
        let unquoted = String::from_utf8(out.stdout).unwrap();
        assert_eq!(unquoted, remote_attach_script("dev-foo"));
        assert!(unquoted.ends_with("tmux attach -t '=dev-foo'"));
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
        assert!(script.ends_with("tmux attach -t '=x'\\''; rm -rf / #'"));
        // And the outer quoting keeps the whole thing a single word.
        if !bash_available() {
            eprintln!("SKIP remote_script_neutralises_a_hostile_session_name: bash not on PATH");
            return;
        }
        let argv = attach_argv("h", "x'; rm -rf / #", &[]);
        let out = std::process::Command::new("bash")
            .args(["-c", &format!("printf %s {}", argv.last().unwrap())])
            .output()
            .expect("spawn bash");
        assert_eq!(String::from_utf8(out.stdout).unwrap(), script);
    }

    #[test]
    fn attach_mux_opts_gives_local_nothing_and_remote_the_pty_socket() {
        assert!(attach_mux_opts(&SshClient::new(), "local").is_empty());
        let opts = attach_mux_opts(&SshClient::new(), "h").join(" ");
        assert!(
            opts.contains("cm-h-tty.sock"),
            "attach must use its own ControlPath, not the probe's: {opts}"
        );
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

    /// The shared output state of the (only) open, for tests that feed bytes
    /// in the way the reader thread does.
    fn shared_of(state: &Mutex<PtyState>) -> Arc<PtyShared> {
        Arc::clone(&state.lock().unwrap().shared)
    }

    #[test]
    fn an_overflow_drops_everything_and_latches_until_the_next_drain() {
        let mut buf = PtyBuffer::default();
        buf.append_capped(b"abcdef", 8);
        assert_eq!(buf.bytes, b"abcdef");
        assert!(!buf.overflowed);
        // Trimming the oldest bytes would cut mid-escape / mid-codepoint and
        // lose mode switches tmux never resends: drop the lot instead.
        buf.append_capped(b"ghij", 8);
        assert!(buf.bytes.is_empty(), "no partial data survives");
        assert!(buf.overflowed);
        // Still overflowed: nothing accumulates until a drain reports it.
        buf.append_capped(b"kl", 8);
        assert!(buf.bytes.is_empty());
        // A single chunk larger than the cap overflows the same way.
        let mut buf = PtyBuffer::default();
        buf.append_capped(b"0123456789AB", 8);
        assert!(buf.bytes.is_empty());
        assert!(buf.overflowed);
    }

    #[test]
    fn drain_reports_an_overflow_once_then_accumulates_again() {
        let state = Mutex::new(PtyState::new());
        let shared = shared_of(&state);
        shared.append(&vec![b'x'; PTY_BUFFER_CAP + 1]);
        let first = drain_from(&state).unwrap();
        assert!(first.overflowed);
        assert_eq!((first.data.as_str(), first.bytes), ("", 0));
        shared.append(b"fresh");
        let second = drain_from(&state).unwrap();
        assert!(!second.overflowed, "reported once");
        assert_eq!(second.data, "fresh", "accumulation resumes after a drain");
    }

    #[test]
    fn drain_reports_eof_out_of_band_after_the_last_bytes() {
        let state = Mutex::new(PtyState::new());
        let shared = shared_of(&state);
        // The marker TEXT in ordinary session output means nothing: grepping
        // this repo inside an attached pane must not look like a disconnect.
        shared.note("[cf] PTY EOF after 0 bytes (tmux attach exited)");
        let first = drain_from(&state).unwrap();
        assert!(first.data.contains("[cf] PTY EOF"));
        assert!(!first.eof, "output text is not a signal");

        // The reader flags the real thing after writing its display line.
        shared.note("bye");
        shared.exited.store(true, Ordering::Release);
        let second = drain_from(&state).unwrap();
        assert_eq!(second.data, "bye");
        assert!(!second.eof, "remaining bytes are delivered first");
        let third = drain_from(&state).unwrap();
        assert_eq!((third.data.as_str(), third.bytes), ("", 0));
        assert!(third.eof, "reported with zero new bytes");
        assert!(drain_from(&state).unwrap().eof, "sticky");
    }

    #[test]
    fn a_dead_readers_eof_never_flags_the_pty_that_replaced_it() {
        let state = Mutex::new(PtyState::new());
        let old = shared_of(&state);
        old.exited.store(true, Ordering::Release);
        assert!(drain_from(&state).unwrap().eof);
        // A re-open installs a fresh shared state; the old reader thread
        // keeps writing to (and flagging) the Arc nobody drains any more.
        state.lock().unwrap().shared = Arc::new(PtyShared::new());
        old.note("stale");
        old.exited.store(true, Ordering::Release);
        let after = drain_from(&state).unwrap();
        assert!(!after.eof);
        assert_eq!(after.data, "");
    }

    #[test]
    fn nothing_is_held_back_from_complete_input() {
        assert_eq!(incomplete_suffix_len(b""), 0);
        assert_eq!(incomplete_suffix_len("héllo 🦀".as_bytes()), 0);
        // A genuinely invalid byte is not an incomplete sequence: there is
        // nothing to wait for, so it is decoded (lossily) right away.
        assert_eq!(incomplete_suffix_len(b"a\xffb"), 0);
        assert_eq!(incomplete_suffix_len(b"a\xff"), 0);
        // Orphan continuation bytes with no lead in reach.
        assert_eq!(incomplete_suffix_len(b"x\x80\x80\x80"), 0);
    }

    #[test]
    fn an_incomplete_trailing_codepoint_is_held_back_whole() {
        let crab = "🦀".as_bytes(); // 4 bytes: F0 9F A6 80
        for cut in 1..4 {
            let mut raw = b"ok ".to_vec();
            raw.extend_from_slice(&crab[..cut]);
            assert_eq!(incomplete_suffix_len(&raw), cut, "cut after {cut} bytes");
        }
        // 2- and 3-byte sequences too.
        assert_eq!(incomplete_suffix_len("é".as_bytes()), 0);
        assert_eq!(incomplete_suffix_len(&"é".as_bytes()[..1]), 1);
        assert_eq!(incomplete_suffix_len(&"中".as_bytes()[..2]), 2);
        // An invalid byte BEFORE the partial tail changes nothing.
        let mut raw = b"a\xffb ".to_vec();
        raw.extend_from_slice(&crab[..2]);
        assert_eq!(incomplete_suffix_len(&raw), 2);
    }

    #[test]
    fn drain_retains_a_split_codepoint_until_the_rest_arrives() {
        let state = Mutex::new(PtyState::new());
        let crab = "🦀".as_bytes();
        // First chunk ends mid-codepoint.
        {
            let s = state.lock().unwrap();
            let mut b = s.shared.buffer.lock().unwrap();
            b.bytes.extend_from_slice(b"x");
            b.bytes.extend_from_slice(&crab[..3]);
        }
        let first = drain_from(&state).unwrap();
        assert_eq!((first.data.as_str(), first.bytes), ("x", 1));
        // The reader appends the tail plus more; the retained prefix stays in
        // front so the codepoint reassembles in order.
        {
            let s = state.lock().unwrap();
            let mut b = s.shared.buffer.lock().unwrap();
            assert_eq!(&b.bytes[..], &crab[..3]);
            b.bytes.extend_from_slice(&crab[3..]);
            b.bytes.extend_from_slice(b"y");
        }
        let second = drain_from(&state).unwrap();
        assert_eq!((second.data.as_str(), second.bytes), ("🦀y", 5));
        // Empty buffer drains to nothing.
        let third = drain_from(&state).unwrap();
        assert_eq!((third.data.as_str(), third.bytes), ("", 0));
    }

    #[test]
    fn an_invalid_byte_does_not_make_a_split_codepoint_lossy() {
        // An invalid byte EARLIER in the buffer must not drag the trailing
        // partial codepoint through the lossy decode with it: it would turn
        // one 🦀 into three U+FFFD (one here, two on the next drain).
        let state = Mutex::new(PtyState::new());
        let crab = "🦀".as_bytes();
        {
            let s = state.lock().unwrap();
            let mut b = s.shared.buffer.lock().unwrap();
            b.bytes.extend_from_slice(b"a\xffb ");
            b.bytes.extend_from_slice(&crab[..2]);
        }
        let first = drain_from(&state).unwrap();
        assert_eq!((first.data.as_str(), first.bytes), ("a\u{FFFD}b ", 4));
        {
            let s = state.lock().unwrap();
            let mut b = s.shared.buffer.lock().unwrap();
            b.bytes.extend_from_slice(&crab[2..]);
            b.bytes.extend_from_slice(b"z");
        }
        let second = drain_from(&state).unwrap();
        assert_eq!(second.data, "🦀z");
    }

    #[test]
    fn a_held_back_tail_stays_in_the_buffer_it_came_from() {
        // The split happens under the SAME lock as the take, so a re-open
        // swapping in a fresh buffer between two drains can never move the
        // old session's partial codepoint in front of the new session's
        // first bytes.
        let state = Mutex::new(PtyState::new());
        let crab = "🦀".as_bytes();
        let old = shared_of(&state);
        old.append(&crab[..2]);
        let first = drain_from(&state).unwrap();
        assert_eq!((first.data.as_str(), first.bytes), ("", 0));
        assert_eq!(
            &old.buffer.lock().unwrap().bytes[..],
            &crab[..2],
            "tail stays put"
        );

        let fresh = Arc::new(PtyShared::new());
        fresh.note("banner");
        state.lock().unwrap().shared = Arc::clone(&fresh);
        let second = drain_from(&state).unwrap();
        assert_eq!(second.data, "banner", "no stale bytes in front");
        assert_eq!(&old.buffer.lock().unwrap().bytes[..], &crab[..2]);
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
        // Closing a never-opened state is a no-op.
        close_pty(&state);
        assert!(!state.lock().unwrap().is_open());
    }

    /// A writer that blocks inside `write` until the test releases it — what
    /// a PTY does once its child stops reading and the tty buffer is full.
    struct BlockedSink(std::sync::mpsc::Receiver<()>);

    impl Write for BlockedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // Returns as soon as the test drops the sender.
            let _ = self.0.recv();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A writer that fails every write, like a master whose child is gone.
    struct FailingSink;

    impl Write for FailingSink {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn input_never_blocks_the_caller_and_reports_e_pty_busy_when_the_queue_fills() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let state = Mutex::new(PtyState::new());
        let shared = shared_of(&state);
        state.lock().unwrap().input_tx = Some(spawn_writer(Box::new(BlockedSink(blocked)), shared));

        let start = Instant::now();
        let mut busy = 0usize;
        for _ in 0..(PTY_INPUT_QUEUE * 2) {
            if let Err(e) = write_to(&state, "x") {
                assert_eq!(e.code, "E_PTY_BUSY");
                busy += 1;
            }
        }
        assert!(busy > 0, "the input queue must be bounded");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "writes must not block on a stuck PTY: {:?}",
            start.elapsed()
        );
        // And the state was never held while the writer was stuck.
        assert!(state.try_lock().is_ok(), "state lock stays free");
        drop(release);
    }

    #[test]
    fn a_writer_error_closes_the_input_channel_and_says_so_on_screen() {
        let state = Mutex::new(PtyState::new());
        let shared = shared_of(&state);
        state.lock().unwrap().input_tx =
            Some(spawn_writer(Box::new(FailingSink), Arc::clone(&shared)));

        // The first chunk is queued; the thread then fails and exits, so
        // every later write reports the PTY closed.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match write_to(&state, "x") {
                Err(e) if e.code == "E_PTY_CLOSED" => break,
                _ => {
                    assert!(Instant::now() < deadline, "writer thread never closed");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        let buf = shared.buffer.lock().unwrap();
        assert!(
            String::from_utf8_lossy(&buf.bytes).contains("[cf] writer error"),
            "the user is told, as display text"
        );
    }

    /// A live attachment's parts plus the child's pid.
    type Sleeper = (
        Box<dyn MasterPty + Send>,
        Box<dyn Write + Send>,
        Box<dyn portable_pty::Child + Send + Sync>,
        u32,
    );

    /// `spawn_sleeper` with its writer already on a writer thread.
    fn install_sleeper(state: &Mutex<PtyState>) -> Option<(u32, Arc<PtyShared>)> {
        let (master, writer, child, pid) = spawn_sleeper()?;
        let shared = Arc::new(PtyShared::new());
        let input_tx = spawn_writer(writer, Arc::clone(&shared));
        // Drop the guard BEFORE tearing down: teardown kills and reaps, which
        // must never run under the state lock (see the CLAUDE.md convention).
        let previous = {
            let mut s = state.lock().unwrap();
            s.install(master, input_tx, child, Arc::clone(&shared))
        };
        previous.teardown();
        Some((pid, shared))
    }

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

    /// Kills the sleeper on drop so a failed assertion never leaves a
    /// `sleep 30` behind. Killing an already-reaped pid is a harmless ESRCH.
    struct KillOnDrop(u32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &self.0.to_string()])
                .status();
        }
    }

    /// `alive` with a deadline. Teardown signals the child while the caller
    /// waits, but reaping finishes on a detached thread once the inline budget
    /// is spent (`PTY_REAP_INLINE`), and `kill -0` still succeeds for a zombie.
    /// "Gone" is therefore eventually-true; how fast depends on the machine,
    /// which is what made this flaky on the macOS runner.
    fn died(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
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
        let state = Mutex::new(PtyState::new());
        let Some((pid1, sh1)) = install_sleeper(&state) else {
            return;
        };
        let _guard1 = KillOnDrop(pid1);
        sh1.note("stale");
        assert!(state.lock().unwrap().is_open());
        assert!(alive(pid1));
        // A live attachment accepts writes and resizes.
        write_to(&state, "hello").unwrap();
        resize_in(&state, 100, 30).unwrap();

        let Some((pid2, _sh2)) = install_sleeper(&state) else {
            close_pty(&state);
            return;
        };
        let _guard2 = KillOnDrop(pid2);
        // Single-PTY invariant: the first child is gone (killed + reaped) and
        // its buffer was cleared; the second is live with its own buffer.
        assert!(died(pid1), "first attachment must be killed on re-open");
        assert!(alive(pid2));
        assert!(sh1.buffer.lock().unwrap().bytes.is_empty());
        assert!(state.lock().unwrap().is_open());

        close_pty(&state);
        assert!(died(pid2));
        assert!(!state.lock().unwrap().is_open());
        assert_eq!(write_to(&state, "x").unwrap_err().code, "E_PTY_CLOSED");
    }

    #[test]
    fn the_child_is_killed_and_reaped_with_the_state_lock_released() {
        let state = Mutex::new(PtyState::new());
        let Some((pid, _shared)) = install_sleeper(&state) else {
            return;
        };
        let _guard = KillOnDrop(pid);
        // Detaching is all that happens under the lock...
        let parts = state.lock().unwrap().take_parts();
        assert!(alive(pid), "still running: nothing has been killed yet");
        assert!(
            state.try_lock().is_ok(),
            "the state is free while the child is torn down"
        );
        assert_eq!(write_to(&state, "x").unwrap_err().code, "E_PTY_CLOSED");
        // ...the kill + reap happens here, off-lock.
        parts.teardown();
        assert!(died(pid));
    }
}
