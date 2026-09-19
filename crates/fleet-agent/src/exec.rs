//! What the agent does with a request: run an argv, or write a file.
//!
//! Nothing here knows about sockets. `conn.rs` decodes a frame, hands the
//! request to [`execute`] or [`write_upload`], and encodes what comes back.

use fleet_proto::StreamLimits;
use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// How many requests may run at once. More queue; the hub's own wall clock
/// bounds how long one waits.
pub const MAX_CONCURRENT: usize = 32;

/// How many request ids the agent remembers to refuse a replay. A uuid per
/// request, so this is a few MiB at most.
pub const SEEN_IDS: usize = 65_536;

/// One `exec`.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Duration,
    pub limits: StreamLimits,
    /// Where the child starts. `None` inherits the agent's own directory.
    pub cwd: Option<PathBuf>,
}

/// What one `exec` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutcome {
    /// The exit code; negative for a signal (`-9` for a kill) or `-1` when
    /// the child never started — what `fleet_proto::AgentFrame::Result`
    /// documents.
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

/// Run `req` to completion, its timeout, or `cancel` — whichever is first.
///
/// The child gets its own process group, and a timeout or a cancel kills the
/// whole group: the hub's argv is `bash -c <script>`, so killing only `bash`
/// would leave whatever the script started running, still holding the pipes.
/// (A tmux server escapes this on purpose: it `setsid`s itself.)
///
/// Both streams are drained to EOF even past their limit, so a chatty child
/// finishes rather than blocking on a full pipe; the bytes past the limit are
/// dropped and `truncated` is set. If the child exits but something it left
/// behind keeps a stream open, the agent waits for EOF only until the
/// deadline, then kills the group and reports the child's own exit code.
pub async fn execute(req: ExecRequest, cancel: impl Future<Output = ()>) -> ExecOutcome {
    let Some((program, args)) = req.argv.split_first() else {
        return never_started("empty argv: nothing to run".into());
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(if req.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    if let Some(cwd) = &req.cwd {
        cmd.current_dir(cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return never_started(format!("{program}: {e}")),
    };
    let group = child.id();

    if let (Some(input), Some(mut pipe)) = (req.stdin, child.stdin.take()) {
        // Its own task: a child that never reads stdin must not stall the
        // agent, and dropping the pipe afterwards is what closes it.
        tokio::spawn(async move {
            let _ = pipe.write_all(input.as_bytes()).await;
        });
    }
    let stdout = Captured::new(req.limits.per_stream);
    let stderr = Captured::new(req.limits.per_stream);
    let mut readers = tokio::task::JoinSet::new();
    if let Some(pipe) = child.stdout.take() {
        readers.spawn(drain(pipe, stdout.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.spawn(drain(pipe, stderr.clone()));
    }

    let deadline = tokio::time::Instant::now() + req.timeout;
    tokio::pin!(cancel);
    let exited = tokio::select! {
        status = child.wait() => status.ok(),
        () = tokio::time::sleep_until(deadline) => None,
        () = &mut cancel => None,
    };
    let exit_code = match exited {
        Some(status) => {
            let drained = tokio::select! {
                () = join_all(&mut readers) => true,
                () = tokio::time::sleep_until(deadline) => false,
                () = &mut cancel => false,
            };
            if !drained {
                kill_group(group);
            }
            exit_code(status)
        }
        None => {
            // A child that had already exited when the deadline or the cancel
            // arrived still reports its own code: SIGKILL cannot change a
            // zombie's status, and `wait` reaps the real one.
            kill_group(group);
            let _ = child.start_kill();
            match child.wait().await {
                Ok(status) => exit_code(status),
                Err(_) => -9,
            }
        }
    };
    // The group is dead or finished: what is still in the pipes arrives
    // promptly. A `setsid` descendant holding one is not waited for.
    let _ = tokio::time::timeout(Duration::from_secs(1), join_all(&mut readers)).await;
    readers.abort_all();

    let (stdout, out_cut) = stdout.take();
    let (mut stderr, mut err_cut) = stderr.take();
    // Both streams share one payload past half the ceiling (see
    // `fleet_proto::StreamLimits::combined`); stderr gives way, because
    // stdout is what callers parse.
    let room = req.limits.combined.saturating_sub(stdout.len());
    if stderr.len() > room {
        stderr.truncate(room);
        err_cut = true;
    }
    ExecOutcome {
        exit_code,
        stdout,
        stderr,
        truncated: out_cut || err_cut,
    }
}

fn never_started(why: String) -> ExecOutcome {
    ExecOutcome {
        exit_code: -1,
        stdout: Vec::new(),
        stderr: why.into_bytes(),
        truncated: false,
    }
}

/// An exit code, or the negated signal that ended the child.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -signal;
        }
    }
    -1
}

/// SIGKILL the child's whole process group.
fn kill_group(group: Option<u32>) {
    #[cfg(unix)]
    if let Some(pgid) = group.and_then(|g| i32::try_from(g).ok()) {
        // SAFETY: killpg takes no pointers; a stale pgid is at worst ESRCH.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = group;
}

async fn join_all(set: &mut tokio::task::JoinSet<()>) {
    while set.join_next().await.is_some() {}
}

/// One stream's bytes, up to its limit, shared with the task reading it so
/// that whatever arrived before an abort is kept.
#[derive(Clone)]
struct Captured(Arc<Mutex<(Vec<u8>, bool)>>, usize);

impl Captured {
    fn new(limit: usize) -> Self {
        Self(Arc::new(Mutex::new((Vec::new(), false))), limit)
    }

    fn push(&self, bytes: &[u8]) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let room = self.1.saturating_sub(g.0.len());
        if bytes.len() > room {
            g.1 = true;
        }
        let keep = bytes.len().min(room);
        g.0.extend_from_slice(&bytes[..keep]);
    }

    fn take(&self) -> (Vec<u8>, bool) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (std::mem::take(&mut g.0), g.1)
    }
}

async fn drain(mut pipe: impl AsyncRead + Unpin, into: Captured) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => into.push(&buf[..n]),
        }
    }
}

/// Where an upload's `path` lands: as given when absolute, under `home` when
/// relative — where `cat > path` would put it, since an ssh remote command
/// starts in `$HOME`. No `~` expansion: the SSH path quotes the path, so it
/// never expands one either.
pub fn resolve_path(home: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        home.join(p)
    }
}

/// Write `bytes` to `path` with exactly `mode`, creating parent directories.
///
/// The mode is applied to the open file BEFORE a byte is written, so an
/// existing wider file is narrowed first — a secret never sits in a
/// world-readable file, not even for an instant — and a new file gets the
/// mode exactly rather than whatever the umask leaves of it. `0600` stays
/// `0600`: the hub uploads its bearer token that way and depends on it.
///
/// Only the permission bits are honoured; setuid, setgid and sticky from the
/// wire are dropped. Like `cat > path`, an existing file is rewritten in
/// place (a symlink is followed), not replaced.
pub fn write_upload(path: &Path, mode: u32, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mode = mode & 0o777;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(mode);
    }
    let mut file = open.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The ids this agent has already been sent, oldest forgotten first.
pub struct SeenIds {
    order: VecDeque<String>,
    set: HashSet<String>,
    capacity: usize,
}

impl SeenIds {
    pub fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::new(),
            set: HashSet::new(),
            capacity,
        }
    }

    /// Record `id`. `false` when it was already seen — a duplicate.
    pub fn insert(&mut self, id: &str) -> bool {
        if self.set.contains(id) {
            return false;
        }
        self.set.insert(id.to_string());
        self.order.push_back(id.to_string());
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{alive, wait_for_pid};
    use std::os::unix::fs::PermissionsExt;

    // Every test here runs on the real clock with a generous bound and never
    // waits on it: a child either finishes on its own, or the test drives the
    // kill itself. The one timeout test runs on the paused clock, where the
    // runtime skips straight to the deadline because the child cannot finish
    // before it.

    /// Far longer than any of these children take; only a hang reaches it.
    const PATIENCE: Duration = Duration::from_secs(60);

    fn req(argv: &[&str]) -> ExecRequest {
        ExecRequest {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            stdin: None,
            timeout: PATIENCE,
            limits: fleet_proto::result_stream_limits(None),
            cwd: None,
        }
    }

    fn never() -> impl Future<Output = ()> {
        std::future::pending()
    }

    #[tokio::test]
    async fn an_argv_runs_and_its_output_and_status_come_back() {
        let out = execute(
            req(&["bash", "-c", "echo out; echo err >&2; exit 3"]),
            never(),
        )
        .await;
        assert_eq!(out.exit_code, 3);
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
        assert!(!out.truncated);
    }

    /// The argv is exec'd, not handed to a shell: a `;` in an argument is
    /// just a byte.
    #[tokio::test]
    async fn the_argv_is_not_re_split_by_a_shell() {
        let out = execute(req(&["printf", "%s", "a; echo b"]), never()).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, b"a; echo b");
    }

    #[tokio::test]
    async fn stdin_is_written_and_closed() {
        let mut r = req(&["cat"]);
        r.stdin = Some("a prompt\n".into());
        let out = execute(r, never()).await;
        assert_eq!(out.stdout, b"a prompt\n");
    }

    /// No stdin means an empty one, not the agent's own: a child that reads
    /// stdin must see EOF rather than hang on the daemon's.
    #[tokio::test]
    async fn no_stdin_is_an_empty_stdin() {
        let out = execute(req(&["cat"]), never()).await;
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
    }

    /// A child's stdin is /dev/null — never the agent's own. Under systemd
    /// the two coincide, but `fleet-agent run` from a terminal would hand
    /// children the TTY from a background process group, and a child that
    /// read it would be stopped (SIGTTIN) until its timeout.
    ///
    /// Deterministic whatever stdin `cargo test` was given: the test re-runs
    /// this binary, filtered to itself, with a PIPE on stdin, and the inner
    /// run asserts what the child's fd 0 is.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_never_inherits_the_agent_s_stdin() {
        const PROBE: &str = "FLEET_AGENT_STDIN_PROBE";
        const NAME: &str = "exec::tests::a_child_never_inherits_the_agent_s_stdin";
        if std::env::var_os(PROBE).is_some() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let out = rt.block_on(execute(req(&["readlink", "/proc/self/fd/0"]), never()));
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/dev/null");
            return;
        }
        let mut inner = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .env(PROBE, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write as _;
        inner
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"the agent's own stdin\n")
            .unwrap();
        let out = inner.wait_with_output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            said.contains("1 passed"),
            "the inner run ran the probe: {said}"
        );
    }

    #[tokio::test]
    async fn the_child_starts_in_the_directory_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = req(&["pwd", "-P"]);
        r.cwd = Some(dir.path().to_path_buf());
        let out = execute(r, never()).await;
        let want = dir.path().canonicalize().unwrap();
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            want.to_str().unwrap()
        );
    }

    #[tokio::test]
    async fn a_command_that_cannot_start_reports_minus_one_and_why() {
        let out = execute(req(&["/nonexistent/fleet-agent-test"]), never()).await;
        assert_eq!(out.exit_code, -1);
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("/nonexistent/fleet-agent-test"),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[tokio::test]
    async fn an_empty_argv_is_refused_without_running_anything() {
        let out = execute(req(&[]), never()).await;
        assert_eq!(out.exit_code, -1);
        assert!(!out.stderr.is_empty());
    }

    /// The cap truncates and flags, and the child is DRAINED past it rather
    /// than left blocked on a full pipe: it exits 0 on its own.
    #[tokio::test]
    async fn the_output_cap_truncates_and_flags_without_blocking_the_child() {
        let mut r = req(&[
            "bash",
            "-c",
            "head -c 1000000 /dev/zero; head -c 1000000 /dev/zero >&2",
        ]);
        r.limits = fleet_proto::result_stream_limits(Some(1000));
        let out = execute(r, never()).await;
        assert_eq!(out.exit_code, 0, "the child ran to completion");
        assert_eq!(out.stdout.len(), 1000);
        assert_eq!(out.stderr.len(), 1000);
        assert!(out.truncated);
    }

    #[tokio::test]
    async fn output_exactly_at_the_cap_is_not_flagged() {
        let mut r = req(&["head", "-c", "1000", "/dev/zero"]);
        r.limits = fleet_proto::result_stream_limits(Some(1000));
        let out = execute(r, never()).await;
        assert_eq!(out.stdout.len(), 1000);
        assert!(!out.truncated);
    }

    /// When both streams together would overrun what the hub will decode,
    /// stderr gives way first: stdout is what callers parse.
    #[tokio::test]
    async fn stderr_gives_way_when_the_streams_share_one_payload() {
        let mut r = req(&[
            "bash",
            "-c",
            "head -c 800 /dev/zero; head -c 800 /dev/zero >&2",
        ]);
        r.limits = StreamLimits {
            per_stream: 1000,
            combined: 1200,
        };
        let out = execute(r, never()).await;
        assert_eq!(out.stdout.len(), 800);
        assert_eq!(out.stderr.len(), 400);
        assert!(out.truncated);
    }

    /// Cancel kills the child AND what it started: the whole process group,
    /// so a `bash -c` wrapper cannot leave its `sleep` behind.
    #[tokio::test]
    async fn cancel_kills_the_child_and_its_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!("sleep 1000 & echo $! > {}; wait", pidfile.display());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn(execute(req(&["bash", "-c", &script]), async move {
            let _ = rx.await;
        }));

        let grandchild = wait_for_pid(&pidfile).await;
        assert!(
            alive(grandchild),
            "the grandchild is running before the cancel"
        );
        tx.send(()).unwrap();

        let out = tokio::time::timeout(PATIENCE, run).await.unwrap().unwrap();
        assert_eq!(out.exit_code, -9, "killed, not exited: {out:?}");
        let deadline = std::time::Instant::now() + PATIENCE;
        while alive(grandchild) {
            assert!(
                std::time::Instant::now() < deadline,
                "the grandchild {grandchild} outlived the cancel"
            );
            tokio::task::yield_now().await;
        }
    }

    /// Paused clock: nothing this child does can finish before its deadline,
    /// so the runtime jumps straight to it.
    #[tokio::test(start_paused = true)]
    async fn a_child_past_its_timeout_is_killed() {
        let mut r = req(&["sleep", "1000"]);
        r.timeout = Duration::from_secs(5);
        let out = execute(r, never()).await;
        assert_eq!(out.exit_code, -9);
    }

    /// A cancel that wins while the child has exited but is not yet reaped
    /// still reports the child's own exit code. The cancel future holds this
    /// single-threaded runtime until the child is a zombie, so the runtime
    /// cannot reap it first: the cancel deterministically wins exactly that
    /// window. (Found by the security review, which also showed an explicit
    /// `try_wait` for this case was redundant: SIGKILL cannot change a
    /// zombie's status, and `wait` reaps the real one.)
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn a_cancel_racing_an_exited_child_keeps_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!("echo $$ > {}; exit 7", pidfile.display());
        let cancel = async move {
            let deadline = std::time::Instant::now() + PATIENCE;
            loop {
                if let Ok(pid) = std::fs::read_to_string(&pidfile)
                    .unwrap_or_default()
                    .trim()
                    .parse::<i32>()
                {
                    let stat =
                        std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                    if stat
                        .rsplit(')')
                        .next()
                        .unwrap_or("")
                        .trim_start()
                        .starts_with('Z')
                    {
                        return;
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the child never exited"
                );
                std::thread::yield_now();
            }
        };
        assert_eq!(
            execute(req(&["bash", "-c", &script]), cancel)
                .await
                .exit_code,
            7
        );
    }

    /// A child that exits while something it started still holds its stdout
    /// keeps its own exit code: the agent stops waiting for EOF when the
    /// deadline or a cancel comes, kills the stragglers, and reports what it
    /// collected. Driven by a cancel the test fires once the child is gone,
    /// not by a deadline: the paused clock would fire the deadline before the
    /// child had exited.
    #[tokio::test]
    async fn a_straggler_holding_stdout_does_not_rewrite_the_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!(
            "echo early; echo $$ > {}; sleep 1000 & exit 4",
            pidfile.display()
        );
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn(execute(req(&["bash", "-c", &script]), async move {
            let _ = rx.await;
        }));
        let child = wait_for_pid(&pidfile).await;
        let deadline = std::time::Instant::now() + PATIENCE;
        while alive(child) {
            assert!(std::time::Instant::now() < deadline, "bash never exited");
            tokio::task::yield_now().await;
        }
        tx.send(()).unwrap();
        let out = tokio::time::timeout(PATIENCE, run).await.unwrap().unwrap();
        assert_eq!(out.exit_code, 4);
        assert_eq!(out.stdout, b"early\n");
    }

    // ── uploads ────────────────────────────────────────────────────────────

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    /// THE GATE on uploads, which carry secrets too (the hub uploads its
    /// bearer token at 0600). A new file gets its mode in the open call, and
    /// an existing one is narrowed BEFORE it is truncated or written — an
    /// order no test of the final file can see, so the source is read.
    #[test]
    fn an_upload_sets_its_mode_before_a_byte_is_written() {
        use crate::test_util::{fn_body, production};
        let body = fn_body(production(include_str!("exec.rs")), "write_upload");
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("write_upload no longer does `{needle}`"))
        };
        assert!(
            at("open.mode(mode)") < at(".open(path)"),
            "the mode goes to the open call"
        );
        let chmod = at("file.set_permissions(");
        assert!(
            chmod < at("file.set_len(0)"),
            "narrowed before it is truncated"
        );
        assert!(
            chmod < at("file.write_all(bytes)"),
            "narrowed before a byte is written"
        );
        assert_eq!(
            body.matches("write_all").count(),
            1,
            "one write, after the chmod"
        );
    }

    #[test]
    fn an_upload_creates_parents_and_the_file_with_exactly_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/token");
        write_upload(&path, 0o600, b"secret").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        assert_eq!(mode_of(&path), 0o600);
    }

    /// An existing file is narrowed BEFORE its new bytes land, so a 0644 file
    /// being replaced by a secret is never world-readable with the secret in
    /// it. The final state is what this test can see.
    #[test]
    fn an_upload_over_an_existing_wider_file_replaces_it_and_narrows_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"a much longer old body").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_upload(&path, 0o600, b"new").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"new",
            "truncated, not overlaid"
        );
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn an_upload_applies_a_wider_mode_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script");
        write_upload(&path, 0o755, b"#!/bin/sh\n").unwrap();
        assert_eq!(mode_of(&path), 0o755);
    }

    /// Only permission bits cross the wire: setuid, setgid and sticky from a
    /// hub are dropped.
    #[test]
    fn an_upload_never_sets_setuid_setgid_or_sticky() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        write_upload(&path, 0o7755, b"x").unwrap();
        assert_eq!(mode_of(&path), 0o755);
    }

    #[test]
    fn a_relative_upload_path_lands_under_home_and_an_absolute_one_as_given() {
        let home = Path::new("/home/someone");
        assert_eq!(
            resolve_path(home, ".claude/settings.json"),
            PathBuf::from("/home/someone/.claude/settings.json")
        );
        assert_eq!(resolve_path(home, "/tmp/x"), PathBuf::from("/tmp/x"));
    }

    // ── replay ─────────────────────────────────────────────────────────────

    #[test]
    fn a_duplicate_id_is_refused() {
        let mut seen = SeenIds::new(8);
        assert!(seen.insert("a"));
        assert!(seen.insert("b"));
        assert!(!seen.insert("a"), "a replayed id");
    }

    #[test]
    fn the_oldest_ids_are_forgotten_past_the_capacity() {
        let mut seen = SeenIds::new(2);
        assert!(seen.insert("a"));
        assert!(seen.insert("b"));
        assert!(seen.insert("c"));
        assert!(seen.insert("a"), "a was the oldest and has been forgotten");
        assert!(!seen.insert("c"), "c is still remembered");
    }
}
