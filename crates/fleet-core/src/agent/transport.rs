//! `SshExec` over a live agent connection.
//!
//! The service layer above this is unchanged and unaware: it keeps calling the
//! same seven methods. Two places where the agent protocol does not line up
//! with `SshExec` are decided here, and both are visible to a reviewer:
//!
//! 1. **`args` is a command line, not an argv.** `SshExec`'s contract is ssh's:
//!    the args are space-joined and re-tokenised by a shell on the far side,
//!    which is why every caller already passes `["bash", "-lc", &quote(script)]`.
//!    `Exec.argv` is exec'd directly, so the transport re-creates the shell
//!    step: `["bash", "-c", args.join(" ")]`. See [`shell_argv`] for why `-c`
//!    and not `-lc`.
//! 2. **`Upload.mode` has no source.** `SshExec::upload_file` pipes into
//!    `cat > path`, which leaves an existing file's mode alone and gives a new
//!    one the remote umask. The agent creates the file itself and must be told
//!    a mode, so the transport sends the **local** file's mode. See
//!    [`local_mode`].

use super::registry::AgentRegistry;
use crate::ipc_error::{codes, IpcError};
use crate::ssh::{home_from_output, SshClient, SshExec, UPLOAD_WALL_CLOCK};
use dashmap::DashMap;
use fleet_proto::{decode_b64, encode_b64, AgentFrame, HubFrame};
use std::path::Path;
use std::process::{ExitStatus, Output};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// `printenv HOME` budget, matching `SshClient::remote_home`.
const HOME_TIMEOUT: Duration = Duration::from_secs(5);

/// An [`SshExec`] that reaches each host through its connected `fleet-agent`.
pub struct AgentTransport {
    registry: Arc<AgentRegistry>,
    /// Per-host `$HOME`, cached for the life of the process exactly as
    /// `SshClient` caches it. Errors are not cached.
    homes: DashMap<String, String>,
}

impl AgentTransport {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            homes: DashMap::new(),
        }
    }

    /// The registry this transport dispatches through — Task 5's endpoint
    /// registers connections on it, Task 8's `agent_status` reads it.
    pub fn registry(&self) -> &Arc<AgentRegistry> {
        &self.registry
    }

    /// One `exec` round-trip. `wall_clock` bounds it on both sides: the agent
    /// kills the child at `timeout_ms`, and the hub gives up at the same
    /// deadline whatever the agent does.
    async fn exec(
        &self,
        host: &str,
        args: &[&str],
        wall_clock: Duration,
        cap: Option<usize>,
        token: Option<CancellationToken>,
    ) -> Result<Output, IpcError> {
        let id = uuid::Uuid::new_v4().to_string();
        // Every frame this transport builds — here and in `upload_file` below
        // — is one an agent at `proto` 1 already understands, so nothing is
        // gated on `self.registry.negotiated_proto(host)`. The FIRST time
        // that stops being true — a new frame kind, or a changed field
        // meaning, that an agent below some version could not act on — is
        // where a caller here has to check it before sending, refusing (or
        // falling back) rather than handing an old agent something it will
        // only be able to ignore.
        let frame = HubFrame::Exec {
            id: id.clone(),
            argv: shell_argv(args),
            // `SshExec` has no stdin parameter; nothing can set this yet.
            stdin: None,
            timeout_ms: wall_clock.as_millis() as u64,
            cap_bytes: cap.map(|c| c as u64),
        };
        let answer = match token {
            None => self.registry.request(host, frame, wall_clock).await?,
            Some(token) => {
                // Checked before dispatching so the `cancel` below can only
                // ever name a request the agent actually received.
                if token.is_cancelled() {
                    return Err(cancelled(host));
                }
                let call = self.registry.request(host, frame, wall_clock);
                tokio::pin!(call);
                tokio::select! {
                    // Biased like `SshClient::run_child`: a cancel that races
                    // the deadline always wins.
                    biased;
                    _ = token.cancelled() => {
                        // Best effort: the agent may have finished already,
                        // and a cancel for an id it does not know is a no-op.
                        let _ = self.registry.send(host, HubFrame::Cancel { id });
                        return Err(cancelled(host));
                    }
                    answer = &mut call => answer?,
                }
            }
        };
        output_from(host, answer, cap)
    }
}

/// Re-create ssh's remote shell step: the args are one command line, and the
/// far side re-splits it.
///
/// **`bash -c`, not `bash -lc`, and the decisive reason is in this file's own
/// crate:** [`LocalExec::command`](crate::ssh::LocalExec) — the repo's
/// existing model of `SshExec`'s remote-command semantics, the one the
/// `tmux_roundtrip` integration test drives against a real tmux — builds
/// `bash -c <args joined>`. This is the same construction, so it is not a
/// deviation from the codebase's model of ssh; it *is* that model.
///
/// The supporting argument: sshd runs the user's login shell with `-c`, which
/// does *not* source the login profile, and almost every caller in this repo
/// passes its script as `bash -lc '<quoted>'`, so the login shell it wants is
/// the inner one. Adding `-l` here would source the profile a second time,
/// and any output a profile writes would land on the *outer* stdout, silently
/// prepending to the output of every command whose result the service layer
/// parses.
///
/// The callers that pass a bare argv with no inner shell — `commands/upload.rs`'s
/// `["mkdir", "-p", …]` and this file's own `["printenv", "HOME"]` — are the
/// only ones where the two could differ. `printenv HOME` behaves identically
/// under `-c` and under sshd. `mkdir` is resolved from the *agent daemon's*
/// `PATH` rather than a PAM login environment, which `-lc` would have
/// repaired; that is a launch-configuration property of the agent and belongs
/// in its docs, not a reason to source the profile twice.
fn shell_argv(args: &[&str]) -> Vec<String> {
    vec!["bash".to_string(), "-c".to_string(), args.join(" ")]
}

/// Turn the agent's `result` into the `Output` the service layer expects.
fn output_from(host: &str, frame: AgentFrame, cap: Option<usize>) -> Result<Output, IpcError> {
    // `truncated` is knowingly dropped: `SshExec` returns a `std::process::
    // Output` and there is nowhere in it to put the flag. The consequence is
    // real and belongs in the agent's docs — when the hub sends no
    // `cap_bytes` (every `run` / `run_bounded`) an agent that truncates at its
    // *own* ceiling produces output indistinguishable from a quiet command.
    // The capped path is unaffected: callers that set a cap already detect
    // truncation by length, as `account_usage` does.
    let AgentFrame::Result {
        exit_code,
        stdout_b64,
        stderr_b64,
        ..
    } = frame
    else {
        return Err(protocol(
            host,
            format!("expected a result frame, got {frame:?}"),
        ));
    };
    let mut stdout =
        decode_b64(&stdout_b64).map_err(|e| protocol(host, format!("result stdout: {e}")))?;
    let mut stderr =
        decode_b64(&stderr_b64).map_err(|e| protocol(host, format!("result stderr: {e}")))?;
    // The cap is this process's memory bound, so it cannot rest on the agent
    // honouring `cap_bytes` — the SSH path enforces it while reading, and this
    // one enforces it on arrival.
    if let Some(cap) = cap {
        stdout.truncate(cap);
        stderr.truncate(cap);
    }
    Ok(Output {
        status: exit_status(exit_code),
        stdout,
        stderr,
    })
}

/// The agent reports a plain exit code, negative when the child was killed by
/// a signal or never started. Rebuild the `ExitStatus` so `.success()` and
/// `.code()` read the way they do for a locally spawned child.
#[cfg(unix)]
fn exit_status(exit_code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    if exit_code >= 0 {
        // A wait status, not an exit code: the low byte is the signal.
        // Clamped because a shell exit code is a byte, and a bogus value must
        // not wrap round to 0 and read as success.
        ExitStatus::from_raw(exit_code.min(255) << 8)
    } else {
        // `unsigned_abs`, not `-exit_code`: `exit_code` is decoded straight
        // from the agent's JSON, and `-i32::MIN` has no `i32` — it panicked in
        // a debug build (every `cargo test`, `cargo tauri dev` and dev-run
        // hub) and wrapped in release. Signal numbers are a small positive
        // range, so clamping the magnitude is the whole of the fix.
        ExitStatus::from_raw(exit_code.unsigned_abs().min(127) as i32)
    }
}

#[cfg(not(unix))]
fn exit_status(exit_code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(if exit_code >= 0 { exit_code as u32 } else { 1 })
}

/// The mode to create an uploaded file with: the local file's, with group and
/// other **write** cleared.
///
/// `SshExec::upload_file` has no mode argument, so this is a choice, and it
/// makes the agent path *differ* from the SSH path: `cat > path` leaves an
/// existing file's mode untouched and gives a new one `0666 & ~umask`, while
/// the agent always applies what we send. Mirroring the local file is the
/// option that keeps the one case where the mode carries weight correct:
/// `provision::write_host_file_secret` spools the bearer token into a 0600
/// temp file and relies on the remote tmp file already being 0600, so sending
/// the local mode reproduces 0600 where a fixed 0644 would publish the token.
///
/// **The clamp is why it is not a plain mirror.** `commands/upload.rs` uploads
/// the *user's own* file, and Linux mounts exFAT, NTFS and SMB `0777` by
/// default — a file dragged straight off one would land world-writable on the
/// agent host, where SSH's `cat >` under a normal umask gives 0644, and any
/// other user on that host could rewrite a file the session is about to read.
/// `& !0o022` is the same thing a umask of 022 does to the SSH path; it never
/// touches the owner's bits, so 0600 still crosses as 0600. A fleet that
/// wants group-writable uploads (umask 002) does not get them here; that is
/// the deliberate trade.
fn upload_mode(md: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o777 & !0o022
    }
    #[cfg(not(unix))]
    {
        // No unix mode to mirror; 0600 is the safe default — never wider than
        // what the secret path needs.
        let _ = md;
        0o600
    }
}

fn cancelled(host: &str) -> IpcError {
    IpcError::new(
        codes::E_CANCELLED,
        format!("cancelled while running on {host}"),
    )
}

fn protocol(host: &str, why: impl std::fmt::Display) -> IpcError {
    IpcError::new(
        codes::E_AGENT_PROTOCOL,
        format!("agent on {host} broke the protocol: {why}"),
    )
}

#[async_trait::async_trait]
impl SshExec for AgentTransport {
    /// `timeout` is ssh's *connect* timeout; an agent connection is already
    /// open, so only the derived wall clock has any meaning here — the same
    /// one `SshClient::run` applies.
    async fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        self.exec(
            host,
            args,
            SshClient::default_wall_clock(timeout),
            None,
            None,
        )
        .await
    }

    async fn run_bounded(
        &self,
        host: &str,
        args: &[&str],
        _connect_timeout: Duration,
        wall_clock: Duration,
    ) -> Result<Output, IpcError> {
        self.exec(host, args, wall_clock, None, None).await
    }

    async fn run_bounded_capped(
        &self,
        host: &str,
        args: &[&str],
        _connect_timeout: Duration,
        wall_clock: Duration,
        max_output: usize,
    ) -> Result<Output, IpcError> {
        self.exec(host, args, wall_clock, Some(max_output), None)
            .await
    }

    async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        self.exec(
            host,
            args,
            SshClient::default_wall_clock(timeout),
            None,
            Some(token),
        )
        .await
    }

    async fn run_bounded_cancellable(
        &self,
        host: &str,
        args: &[&str],
        _connect_timeout: Duration,
        wall_clock: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        self.exec(host, args, wall_clock, None, Some(token)).await
    }

    /// `timeout` is unused for the same reason as in `run`: there is no
    /// connect step. The wall clock is `UPLOAD_WALL_CLOCK`, exactly as
    /// `SshClient::upload_file` applies it.
    async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        _timeout: Duration,
    ) -> Result<(), IpcError> {
        // One stat, answering both questions, and answering the size one
        // FIRST. One file is one frame, so a file past the payload limit can
        // never be sent — and refusing it from its size means the bytes are
        // never read, never base64'd and never encoded into a throwaway frame
        // just to be measured. At this limit each of those is a ~200 MiB
        // allocation the caller would pay to be told "no".
        let md = std::fs::metadata(local_path).map_err(|e| {
            IpcError::new(
                codes::E_UPLOAD,
                format!("stat {}: {e}", local_path.display()),
            )
        })?;
        if md.len() > fleet_proto::MAX_PAYLOAD_BYTES as u64 {
            return Err(IpcError::new(
                codes::E_UPLOAD,
                format!(
                    "{} is {} MiB: the agent transport carries at most {} MiB in one frame",
                    local_path.display(),
                    md.len() / (1024 * 1024),
                    fleet_proto::MAX_PAYLOAD_BYTES / (1024 * 1024),
                ),
            ));
        }
        // Read before dispatching: a local failure must never reach the wire.
        let bytes = std::fs::read(local_path).map_err(|e| {
            IpcError::new(
                codes::E_UPLOAD,
                format!("open {}: {e}", local_path.display()),
            )
        })?;
        // Same note as `exec`'s `HubFrame::Exec`: nothing here needs gating
        // on `self.registry.negotiated_proto(host)` yet either.
        let frame = HubFrame::Upload {
            id: uuid::Uuid::new_v4().to_string(),
            path: remote_path.to_string(),
            mode: upload_mode(&md),
            bytes_b64: encode_b64(&bytes),
        };
        match self
            .registry
            .request(host, frame, UPLOAD_WALL_CLOCK)
            .await?
        {
            AgentFrame::Result { exit_code: 0, .. } => Ok(()),
            AgentFrame::Result { stderr_b64, .. } => {
                let stderr = decode_b64(&stderr_b64).unwrap_or_default();
                Err(IpcError::new(
                    codes::E_UPLOAD,
                    format!(
                        "upload to {host} failed: {}",
                        String::from_utf8_lossy(&stderr).trim()
                    ),
                ))
            }
            other => Err(protocol(
                host,
                format!("expected a result frame, got {other:?}"),
            )),
        }
    }

    async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
        if let Some(home) = self.homes.get(host) {
            return Ok(home.clone());
        }
        let out = self.run(host, &["printenv", "HOME"], HOME_TIMEOUT).await?;
        let home = home_from_output(host, &out)?;
        self.homes.insert(host.to_string(), home.clone());
        Ok(home)
    }
}

#[cfg(test)]
impl AgentTransport {
    /// Test-only: proves a failed `remote_home` is not cached.
    pub(crate) fn home_cache_len(&self) -> usize {
        self.homes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::fake::{answer_exit, answer_with, custom, silent, FakeAgent};
    use crate::agent::registry::AgentRegistry;
    use crate::ipc_error::codes;
    use crate::shell::quote;
    use crate::ssh::{SshClient, SshExec};
    use fleet_proto::{encode_b64, AgentFrame, HubFrame};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    fn setup(policy: crate::agent::fake::Policy) -> (AgentTransport, FakeAgent) {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", policy);
        (AgentTransport::new(reg), agent)
    }

    /// A `result` frame carrying `exit_code` and nothing else.
    fn result_frame(exit_code: i32) -> AgentFrame {
        AgentFrame::Result {
            id: "id".into(),
            exit_code,
            stdout_b64: encode_b64(b""),
            stderr_b64: encode_b64(b""),
            truncated: false,
        }
    }

    /// Destructure an `exec` frame, failing the test on anything else.
    fn as_exec(frame: HubFrame) -> (String, Vec<String>, Option<String>, u64, Option<u64>) {
        match frame {
            HubFrame::Exec {
                id,
                argv,
                stdin,
                timeout_ms,
                cap_bytes,
            } => (id, argv, stdin, timeout_ms, cap_bytes),
            other => panic!("expected an exec frame, got {other:?}"),
        }
    }

    // ── the argv trap ───────────────────────────────────────────────────────

    /// `SshExec`'s `args` are **not** an argv: ssh joins them with spaces and
    /// the remote shell re-tokenises, which is why every caller already
    /// `shell::quote`s its script. Handing them over as `Exec.argv` would exec
    /// the quoting literally. The transport must re-create the shell step.
    #[tokio::test]
    async fn run_hands_the_agent_a_shell_command_not_the_callers_argv() {
        let (t, agent) = setup(answer_exit(0));
        let script = "echo 'hi there' > /tmp/x && tmux ls";
        let quoted = quote(script);
        t.run("laptop", &["bash", "-lc", &quoted], Duration::from_secs(5))
            .await
            .unwrap();

        let (id, argv, stdin, timeout_ms, cap) = as_exec(agent.only_frame());
        assert!(!id.is_empty(), "every request carries an id");
        assert_eq!(
            argv,
            vec![
                "bash".to_string(),
                "-c".to_string(),
                format!("bash -lc {quoted}"),
            ],
            "the args are joined into one command line for a shell to re-split"
        );
        assert_eq!(stdin, None, "SshExec has no stdin parameter");
        assert_eq!(
            timeout_ms,
            SshClient::default_wall_clock(Duration::from_secs(5)).as_millis() as u64,
            "the agent gets the same wall clock the SSH path derives"
        );
        assert_eq!(cap, None);
    }

    /// The same quoting, seen from the agent's side: a POSIX shell given the
    /// command line we send re-tokenises it into exactly the argv ssh's
    /// remote login shell would have produced.
    #[tokio::test]
    async fn the_command_line_re_splits_into_the_script_the_caller_wrote() {
        let (t, agent) = setup(answer_exit(0));
        let script = "printf '%s\\n' \"a b\" 'c'\"'\"'d'";
        t.run(
            "laptop",
            &["bash", "-lc", &quote(script)],
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let (_, argv, ..) = as_exec(agent.only_frame());

        // Run the real thing: `bash -c '<our command line>'` must end up
        // running the caller's script verbatim.
        let out = std::process::Command::new(&argv[0])
            .arg(&argv[1])
            .arg(&argv[2])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a b\nc'd\n");
    }

    // ── result mapping ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_maps_the_result_frame_onto_an_output() {
        // Non-UTF-8 bytes on both streams: they must survive base64 intact.
        let (t, _agent) = setup(answer_with(3, b"\xff\xfe out", b"\x00 err"));
        let out = t
            .run("laptop", &["bash", "-lc", "x"], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert!(!out.status.success());
        assert_eq!(out.stdout, b"\xff\xfe out");
        assert_eq!(out.stderr, b"\x00 err");
    }

    #[tokio::test]
    async fn a_negative_exit_code_is_a_failed_status_with_no_code() {
        let (t, _agent) = setup(answer_exit(-1));
        let out = t
            .run("laptop", &["bash", "-lc", "x"], Duration::from_secs(5))
            .await
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(
            out.status.code(),
            None,
            "killed by a signal, like a local child"
        );
    }

    #[tokio::test]
    async fn a_pong_answering_an_exec_is_a_protocol_error() {
        let (t, _agent) = setup(custom(|f: &HubFrame| match f {
            HubFrame::Exec { id, .. } => Some(AgentFrame::Pong { id: id.clone() }),
            _ => None,
        }));
        let err = t
            .run("laptop", &["bash", "-lc", "x"], Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_PROTOCOL);
    }

    #[tokio::test]
    async fn an_undecodable_stream_is_a_protocol_error() {
        let (t, _agent) = setup(custom(|f: &HubFrame| match f {
            HubFrame::Exec { id, .. } => Some(AgentFrame::Result {
                id: id.clone(),
                exit_code: 0,
                stdout_b64: "not base64 !!".into(),
                stderr_b64: encode_b64(b""),
                truncated: false,
            }),
            _ => None,
        }));
        let err = t
            .run("laptop", &["bash", "-lc", "x"], Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_PROTOCOL);
    }

    // ── the bounded / capped variants ───────────────────────────────────────

    #[tokio::test]
    async fn run_bounded_sends_the_wall_clock_not_the_connect_timeout() {
        let (t, agent) = setup(answer_exit(0));
        t.run_bounded(
            "laptop",
            &["bash", "-lc", "x"],
            Duration::from_secs(5),
            Duration::from_secs(600),
        )
        .await
        .unwrap();
        let (_, _, _, timeout_ms, _) = as_exec(agent.only_frame());
        assert_eq!(timeout_ms, 600_000, "an agent call has no connect step");
    }

    #[tokio::test]
    async fn run_bounded_capped_asks_the_agent_to_cap_each_stream() {
        let (t, agent) = setup(answer_exit(0));
        t.run_bounded_capped(
            "laptop",
            &["bash", "-lc", "x"],
            Duration::from_secs(5),
            Duration::from_secs(60),
            4_096,
        )
        .await
        .unwrap();
        let (_, _, _, timeout_ms, cap) = as_exec(agent.only_frame());
        assert_eq!(timeout_ms, 60_000);
        assert_eq!(cap, Some(4_096));
    }

    /// The cap is the app's memory bound, so it cannot depend on the agent
    /// honouring it — the SSH path enforces it while reading, and this one
    /// enforces it on arrival.
    #[tokio::test]
    async fn an_agent_that_ignores_the_cap_is_still_capped_on_arrival() {
        let (t, _agent) = setup(answer_with(0, b"0123456789", b"abcdefghij"));
        let out = t
            .run_bounded_capped(
                "laptop",
                &["bash", "-lc", "x"],
                Duration::from_secs(5),
                Duration::from_secs(60),
                4,
            )
            .await
            .unwrap();
        assert_eq!(out.stdout, b"0123");
        assert_eq!(out.stderr, b"abcd");
    }

    // ── remote_home ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn remote_home_asks_the_agent_and_caches_the_answer() {
        let (t, agent) = setup(answer_with(0, b"/home/dev\n", b""));
        assert_eq!(t.remote_home("laptop").await.unwrap(), "/home/dev");
        let (_, argv, _, timeout_ms, _) = as_exec(agent.only_frame());
        assert_eq!(argv, vec!["bash", "-c", "printenv HOME"]);
        assert_eq!(
            timeout_ms,
            SshClient::default_wall_clock(Duration::from_secs(5)).as_millis() as u64
        );

        assert_eq!(t.remote_home("laptop").await.unwrap(), "/home/dev");
        assert_eq!(
            agent.sent().len(),
            1,
            "the second call is served from the cache, like SshClient's"
        );
    }

    #[tokio::test]
    async fn remote_home_reports_a_failure_rather_than_guessing() {
        let (t, _agent) = setup(answer_with(1, b"", b"printenv: not found"));
        let err = t.remote_home("laptop").await.unwrap_err();
        assert!(!err.message.is_empty());
        assert!(
            t.home_cache_len() == 0,
            "a failure is never cached — a transient error must retry"
        );
    }

    // ── upload ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn upload_file_sends_the_bytes_the_path_and_the_local_mode() {
        let (t, agent) = setup(answer_exit(0));
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("hook.sh");
        std::fs::write(&local, b"#!/bin/sh\nexit 0\n").unwrap();
        set_mode(&local, 0o600);

        t.upload_file(
            "laptop",
            &local,
            "/home/dev/.claude/hook.sh",
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        match agent.only_frame() {
            HubFrame::Upload {
                id,
                path,
                mode,
                bytes_b64,
            } => {
                assert!(!id.is_empty());
                assert_eq!(path, "/home/dev/.claude/hook.sh");
                assert_eq!(mode, 0o600, "the local file's mode travels with it");
                assert_eq!(
                    fleet_proto::decode_b64(&bytes_b64).unwrap(),
                    b"#!/bin/sh\nexit 0\n"
                );
            }
            other => panic!("expected an upload frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn upload_file_carries_an_executable_mode_through() {
        let (t, agent) = setup(answer_exit(0));
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("script");
        std::fs::write(&local, b"x").unwrap();
        set_mode(&local, 0o755);
        t.upload_file("laptop", &local, "/tmp/script", Duration::from_secs(5))
            .await
            .unwrap();
        match agent.only_frame() {
            HubFrame::Upload { mode, .. } => assert_eq!(mode, 0o755),
            other => panic!("expected an upload frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn upload_file_reports_a_non_zero_result_as_e_upload() {
        let (t, _agent) = setup(answer_with(1, b"", b"permission denied"));
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("f");
        std::fs::write(&local, b"x").unwrap();
        let err = t
            .upload_file("laptop", &local, "/root/f", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_UPLOAD);
        assert!(
            err.message.contains("permission denied"),
            "the agent's stderr reaches the caller: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn a_missing_local_file_is_e_upload_and_never_reaches_the_agent() {
        let (t, agent) = setup(answer_exit(0));
        let err = t
            .upload_file(
                "laptop",
                std::path::Path::new("/nonexistent/nope"),
                "/tmp/x",
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_UPLOAD);
        assert!(agent.sent().is_empty());
    }

    /// A file too big for one frame must be refused with a message that names
    /// the limit, not silently truncated and not left to blow up the socket.
    ///
    /// **And it must be refused from the file's size alone, before the bytes
    /// are read.** The file here is sparse *and* mode 0000: if the transport
    /// reached `std::fs::read` first it would fail with a permission error
    /// instead of the limit, and on the way it would allocate the whole
    /// oversize file just to throw it away.
    #[tokio::test]
    async fn upload_file_refuses_an_oversize_file_without_reading_it() {
        let (t, agent) = setup(answer_exit(0));
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("big");
        // Sparse (`set_len`, not `write`): the bytes read back as zeros and
        // nothing of this size reaches the disk, which keeps the test off the
        // machine's back — other tests in this binary are timing-sensitive.
        std::fs::File::create(&local)
            .unwrap()
            .set_len(fleet_proto::MAX_PAYLOAD_BYTES as u64 + 1)
            .unwrap();
        set_mode(&local, 0o000);
        let err = t
            .upload_file("laptop", &local, "/tmp/big", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_UPLOAD);
        assert!(
            err.message.contains("200 MiB"),
            "the message names the payload limit: {}",
            err.message
        );
        assert!(
            agent.sent().is_empty(),
            "nothing oversized reaches the wire"
        );
        // Undo 0000 so the tempdir can be cleaned up.
        set_mode(&local, 0o600);
    }

    /// The limit is not a number somebody picked: it is what the application
    /// says it moves. `move.max_transcript_mb` defaults to 200 MiB and
    /// `move_session` checks a transcript's real size against it before the
    /// copy, so a payload limit below it breaks "Move to host…" for any real
    /// session on an agent host — which is exactly what 16 MiB did.
    #[test]
    fn the_frame_cap_covers_the_transcript_the_fleet_moves() {
        let transcript =
            crate::service::move_session::DEFAULT_MAX_TRANSCRIPT_MB as usize * 1024 * 1024;
        assert!(
            fleet_proto::MAX_PAYLOAD_BYTES >= transcript,
            "one frame must carry a default-cap transcript: payload limit {} < {transcript}",
            fleet_proto::MAX_PAYLOAD_BYTES
        );
        // …and the frame cap must leave room for base64 plus the envelope, or
        // the payload limit is a promise the codec cannot keep.
        assert!(
            fleet_proto::MAX_FRAME_BYTES > fleet_proto::base64_len(transcript),
            "frame cap {} does not fit {} base64 bytes",
            fleet_proto::MAX_FRAME_BYTES,
            fleet_proto::base64_len(transcript)
        );
    }

    /// A hostile or buggy agent can put any `i32` in `exit_code`; `i32::MIN`
    /// has no negation, so negating it panicked in a debug build (which is
    /// every `cargo test`, `cargo tauri dev` and dev-run `fleet-hub`) and
    /// wrapped in release. It must read as a failure, like any other signal.
    #[test]
    fn a_hostile_exit_code_does_not_panic() {
        for code in [i32::MIN, i32::MIN + 1, -1, -128, i32::MAX] {
            let out = output_from("laptop", result_frame(code), None).expect("decodes");
            assert!(!out.status.success(), "{code} must not read as success");
        }
    }

    /// The drag-drop upload (`commands/upload.rs`) carries the user's own
    /// file, and Linux mounts exFAT/NTFS/SMB `0777` by default. Mirroring
    /// that mode would land the file world-writable on the agent host, where
    /// SSH's `cat >` under a normal umask gives 0644 — another user on that
    /// host could rewrite a file the session is about to read.
    #[tokio::test]
    async fn an_upload_never_carries_group_or_other_write() {
        for (local, sent) in [(0o777, 0o755), (0o666, 0o644), (0o664, 0o644)] {
            let (t, agent) = setup(answer_exit(0));
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f");
            std::fs::write(&path, b"x").unwrap();
            set_mode(&path, local);
            t.upload_file("laptop", &path, "/tmp/f", Duration::from_secs(5))
                .await
                .unwrap();
            match agent.only_frame() {
                HubFrame::Upload { mode, .. } => {
                    assert_eq!(mode, sent, "local {local:o} must upload as {sent:o}")
                }
                other => panic!("expected an upload, got {other:?}"),
            }
        }
    }

    /// The owner's own bits are never widened *or* narrowed: the bearer-token
    /// case depends on 0600 crossing unchanged.
    #[tokio::test]
    async fn the_clamp_leaves_a_private_file_private() {
        let (t, agent) = setup(answer_exit(0));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, b"secret").unwrap();
        set_mode(&path, 0o600);
        t.upload_file("laptop", &path, "/tmp/token", Duration::from_secs(5))
            .await
            .unwrap();
        match agent.only_frame() {
            HubFrame::Upload { mode, .. } => assert_eq!(mode, 0o600),
            other => panic!("expected an upload, got {other:?}"),
        }
    }

    // ── cancellation ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_cancelled_call_sends_cancel_for_its_own_id_and_returns_e_cancelled() {
        let (t, agent) = setup(silent());
        let token = CancellationToken::new();
        let t = Arc::new(t);
        let call = tokio::spawn({
            let t = Arc::clone(&t);
            let token = token.clone();
            async move {
                t.run_cancellable(
                    "laptop",
                    &["bash", "-lc", "x"],
                    Duration::from_secs(5),
                    token,
                )
                .await
            }
        });
        agent.wait_until_sent(1).await;
        token.cancel();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, codes::E_CANCELLED);

        agent.wait_until_sent(2).await;
        let sent = agent.sent();
        let (exec_id, ..) = as_exec(sent[0].clone());
        match &sent[1] {
            HubFrame::Cancel { id } => assert_eq!(
                *id, exec_id,
                "the cancel names the request it is cancelling"
            ),
            other => panic!("expected a cancel frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_bounded_cancellable_is_cancellable_too() {
        let (t, agent) = setup(silent());
        let token = CancellationToken::new();
        let t = Arc::new(t);
        let call = tokio::spawn({
            let t = Arc::clone(&t);
            let token = token.clone();
            async move {
                t.run_bounded_cancellable(
                    "laptop",
                    &["bash", "-lc", "x"],
                    Duration::from_secs(5),
                    Duration::from_secs(600),
                    token,
                )
                .await
            }
        });
        agent.wait_until_sent(1).await;
        token.cancel();
        assert_eq!(call.await.unwrap().unwrap_err().code, codes::E_CANCELLED);
        agent.wait_until_sent(2).await;
        assert!(matches!(agent.sent()[1], HubFrame::Cancel { .. }));
    }

    // ── offline and timeout ─────────────────────────────────────────────────

    /// Every method, not just `run`: an agent host with nothing connected must
    /// fail now. The budget here is 60 s; anything approaching it is a hang.
    #[tokio::test]
    async fn every_method_is_offline_immediately_with_no_agent() {
        let t = AgentTransport::new(AgentRegistry::new());
        let long = Duration::from_secs(60);
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("f");
        std::fs::write(&local, b"x").unwrap();

        let started = Instant::now();
        for code in [
            t.run("ghost", &["bash", "-lc", "x"], long)
                .await
                .unwrap_err()
                .code,
            t.run_bounded("ghost", &["bash", "-lc", "x"], long, long)
                .await
                .unwrap_err()
                .code,
            t.run_bounded_capped("ghost", &["bash", "-lc", "x"], long, long, 10)
                .await
                .unwrap_err()
                .code,
            t.run_cancellable(
                "ghost",
                &["bash", "-lc", "x"],
                long,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
            t.run_bounded_cancellable(
                "ghost",
                &["bash", "-lc", "x"],
                long,
                long,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
            t.upload_file("ghost", &local, "/tmp/x", long)
                .await
                .unwrap_err()
                .code,
            t.remote_home("ghost").await.unwrap_err().code,
        ] {
            assert_eq!(code, codes::E_AGENT_OFFLINE);
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "seven offline calls took {:?} — they are waiting, not failing",
            started.elapsed()
        );
    }

    // `start_paused`: virtual time reaches the deadline without this test
    // holding a thread for 80 real milliseconds next to the timing-sensitive
    // tests elsewhere in this binary.
    #[tokio::test(start_paused = true)]
    async fn a_silent_agent_times_out() {
        let (t, _agent) = setup(silent());
        let err = t
            .run_bounded(
                "laptop",
                &["bash", "-lc", "x"],
                Duration::from_millis(10),
                Duration::from_millis(80),
            )
            .await
            .unwrap_err();
        // The SAME code every other `SshExec` returns for a blown wall clock
        // (`ssh::wall_clock_error`). A code of its own looked harmless and was
        // not: `account_usage::classify_run` reads an unrecognised code as
        // "nothing ran on that host" and re-fires the Anthropic API request
        // against the next one.
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
    }

    fn set_mode(path: &std::path::Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = (path, mode);
    }
}
