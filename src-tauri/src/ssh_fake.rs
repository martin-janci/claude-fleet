//! `FakeSsh`: a scripted, recording `SshExec` for tests (OPS-8 / W4 F6).
//!
//! Every `run` / `run_cancellable` / `run_bounded_cancellable` /
//! `upload_file` call is appended to a call log — `(host, argv, stdin)` in
//! order — and answered from the matching `Reply`. Rules are
//! `(host filter, matcher, reply)`; the most recently added matching rule
//! wins, so a test can install a broad default first and
//! narrow it later. A command no rule matches gets the `default` reply
//! (exit 0, empty output, unless changed with `set_default`).
//!
//! The fake keeps the contract documented on `SshExec`: an unreachable host
//! is ssh exiting 255 with a connect error on stderr (`Ok(Output)`), not an
//! `Err`; a hang is bounded by the same wall clock the real client applies
//! (`SshClient::default_wall_clock`, `UPLOAD_WALL_CLOCK` for uploads;
//! both overridable with `set_wall_clock` so a
//! test does not wait 30 s) and surfaces as `E_SSH_TIMEOUT`; cancellation
//! wins over both and surfaces as `E_CANCELLED`. `remote_home` runs
//! `printenv HOME` through the log like the real client and caches per host.

use crate::ipc_error::IpcError;
use crate::ssh::{home_from_output, wall_clock_error, SshClient, SshExec, UPLOAD_WALL_CLOCK};
use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// One recorded call. `args` is the argv exactly as the service passed it
/// (for an upload: the single `cat > '<path>'` word); `stdin` is the uploaded
/// file's bytes for `upload_file`, `None` otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub host: String,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
}

impl Call {
    /// The argv space-joined — what ssh hands the remote login shell.
    pub fn command(&self) -> String {
        self.args.join(" ")
    }

    /// For a `bash -lc '<script>'` call: the script with the outer
    /// `shell::quote` undone. `None` for any other argv shape.
    pub fn script(&self) -> Option<String> {
        match self.args.as_slice() {
            [b, l, s] if b == "bash" && l == "-lc" => unquote(s),
            _ => None,
        }
    }

    /// The uploaded bytes as text (`upload_file` calls only).
    pub fn stdin_str(&self) -> Option<String> {
        self.stdin
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

/// Undo `shell::quote`: strip the outer single quotes and collapse `'\''`.
/// `None` when `s` is not a single quoted word.
pub fn unquote(s: &str) -> Option<String> {
    let inner = s.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace("'\\''", "'"))
}

/// How a matched command answers.
#[derive(Clone, Debug)]
pub enum Reply {
    /// The child ran and exited with `code`, producing the given streams.
    Exit {
        code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// ssh could not reach the host: exit 255 with the connect error on
    /// stderr, exactly what `SshClient::run` returns for a down host.
    Unreachable,
    /// The child never answers: sleeps `for_` (default: an hour) and only
    /// the wall clock / a cancellation gets the caller out.
    Hang { for_: Duration },
    /// `spawn` itself failed (no `ssh` binary, fd exhaustion): `Err(E_SSH)`
    /// — or `E_UPLOAD` for an upload — with `message`.
    SpawnError { message: String },
}

impl Reply {
    pub fn ok(stdout: &str) -> Self {
        Self::Exit {
            code: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    pub fn fail(code: i32, stderr: &str) -> Self {
        Self::Exit {
            code,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    pub fn hang() -> Self {
        Self::Hang {
            for_: Duration::from_secs(3600),
        }
    }
}

/// Which commands a rule applies to. `Prefix` / `Contains` / `Regex` run on
/// the space-joined argv (see `Call::command`), i.e. the string the remote
/// shell would see; `Script` / `ScriptContains` run on the unquoted body of
/// a `bash -lc '<script>'` call (see `Call::script`) and never match any
/// other argv shape.
#[derive(Clone, Debug)]
pub enum Match {
    Any,
    Prefix(String),
    Contains(String),
    Regex(regex::Regex),
    Script(String),
    ScriptContains(String),
}

impl Match {
    pub fn prefix(s: &str) -> Self {
        Self::Prefix(s.to_string())
    }

    pub fn contains(s: &str) -> Self {
        Self::Contains(s.to_string())
    }

    pub fn regex(re: &str) -> Self {
        Self::Regex(regex::Regex::new(re).expect("valid test regex"))
    }

    pub fn script(s: &str) -> Self {
        Self::Script(s.to_string())
    }

    pub fn script_contains(s: &str) -> Self {
        Self::ScriptContains(s.to_string())
    }

    fn matches(&self, call: &Call) -> bool {
        let command = call.command();
        match self {
            Self::Any => true,
            Self::Prefix(p) => command.starts_with(p.as_str()),
            Self::Contains(c) => command.contains(c.as_str()),
            Self::Regex(re) => re.is_match(&command),
            Self::Script(s) => call.script().as_deref() == Some(s.as_str()),
            Self::ScriptContains(s) => call.script().is_some_and(|sc| sc.contains(s.as_str())),
        }
    }
}

#[derive(Clone, Debug)]
struct Rule {
    /// `None` = every host.
    host: Option<String>,
    matcher: Match,
    reply: Reply,
}

#[derive(Default)]
struct State {
    rules: Vec<Rule>,
    calls: Vec<Call>,
    default: Option<Reply>,
    homes: HashMap<String, String>,
    wall_clock: Option<Duration>,
}

/// Cheaply cloneable; clones share the rules and the call log, so a test can
/// hand one clone to the code under test and inspect the other.
#[derive(Clone, Default)]
pub struct FakeSsh {
    state: Arc<Mutex<State>>,
}

impl FakeSsh {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ── scripting ──────────────────────────────────────────────────────────

    /// Answer `matcher` on every host with `reply`. Later rules win.
    pub fn on(&self, matcher: Match, reply: Reply) -> &Self {
        self.lock().rules.push(Rule {
            host: None,
            matcher,
            reply,
        });
        self
    }

    /// Answer `matcher` on `host` only with `reply`. Later rules win.
    pub fn on_host(&self, host: &str, matcher: Match, reply: Reply) -> &Self {
        self.lock().rules.push(Rule {
            host: Some(host.to_string()),
            matcher,
            reply,
        });
        self
    }

    /// Every command on `host` fails like ssh does for a down host.
    pub fn unreachable(&self, host: &str) -> &Self {
        self.on_host(host, Match::Any, Reply::Unreachable)
    }

    /// Every command on `host` hangs until the wall clock fires.
    pub fn hanging(&self, host: &str) -> &Self {
        self.on_host(host, Match::Any, Reply::hang())
    }

    /// Reply for commands no rule matches (default: exit 0, no output).
    pub fn set_default(&self, reply: Reply) -> &Self {
        self.lock().default = Some(reply);
        self
    }

    /// Override the wall clock applied to hanging commands (the real client
    /// uses `SshClient::default_wall_clock(timeout)`, 30 s at least).
    pub fn set_wall_clock(&self, wall_clock: Duration) -> &Self {
        self.lock().wall_clock = Some(wall_clock);
        self
    }

    /// Script `printenv HOME` on every host to answer `home`.
    pub fn with_home(&self, home: &str) -> &Self {
        self.on(
            Match::prefix("printenv HOME"),
            Reply::ok(&format!("{home}\n")),
        )
    }

    // ── inspection ─────────────────────────────────────────────────────────

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// Calls made against `host`, in order.
    pub fn calls_for(&self, host: &str) -> Vec<Call> {
        self.lock()
            .calls
            .iter()
            .filter(|c| c.host == host)
            .cloned()
            .collect()
    }

    /// The space-joined argv of every call, in order.
    pub fn commands(&self) -> Vec<String> {
        self.lock().calls.iter().map(Call::command).collect()
    }

    /// Forget the recorded calls (rules and the home cache stay).
    pub fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    // ── execution ──────────────────────────────────────────────────────────

    fn record_and_resolve(&self, host: &str, args: &[&str], stdin: Option<Vec<u8>>) -> Reply {
        let call = Call {
            host: host.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            stdin,
        };
        let mut st = self.lock();
        let reply = st
            .rules
            .iter()
            .rev()
            .find(|r| r.host.as_deref().is_none_or(|h| h == host) && r.matcher.matches(&call))
            .map(|r| r.reply.clone())
            .or_else(|| st.default.clone())
            .unwrap_or_else(|| Reply::ok(""));
        st.calls.push(call);
        reply
    }

    /// The test override from `set_wall_clock`, else `base` — the bound the
    /// real client would apply to this kind of call.
    fn wall_clock_or(&self, base: Duration) -> Duration {
        self.lock().wall_clock.unwrap_or(base)
    }

    async fn execute(
        &self,
        host: &str,
        args: &[&str],
        stdin: Option<Vec<u8>>,
        wall_clock: Duration,
        token: Option<CancellationToken>,
        spawn_code: &str,
    ) -> Result<Output, IpcError> {
        let reply = self.record_and_resolve(host, args, stdin);
        match reply {
            Reply::Exit {
                code,
                stdout,
                stderr,
            } => Ok(Output {
                status: ExitStatus::from_raw(code << 8),
                stdout,
                stderr,
            }),
            Reply::Unreachable => Ok(Output {
                status: ExitStatus::from_raw(255 << 8),
                stdout: Vec::new(),
                stderr: format!("ssh: connect to host {host} port 22: No route to host\r\n")
                    .into_bytes(),
            }),
            Reply::SpawnError { message } => Err(IpcError::new(
                spawn_code,
                format!("ssh spawn {host}: {message}"),
            )),
            Reply::Hang { for_ } => {
                let cancelled = async {
                    match token {
                        Some(t) => t.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    biased;
                    _ = cancelled => Err(IpcError::new("E_CANCELLED", format!("ssh {host} cancelled"))),
                    _ = tokio::time::sleep(wall_clock) => Err(wall_clock_error(host, wall_clock, false)),
                    _ = tokio::time::sleep(for_) => Ok(Output {
                        status: ExitStatus::from_raw(0),
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    }),
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl SshExec for FakeSsh {
    async fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        self.execute(
            host,
            args,
            None,
            self.wall_clock_or(SshClient::default_wall_clock(timeout)),
            None,
            "E_SSH",
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
        // `set_wall_clock` still overrides, exactly like `run`; the caller's
        // `connect_timeout` plays no role for a fake — there is nothing to
        // time a connect against.
        self.execute(
            host,
            args,
            None,
            self.wall_clock_or(wall_clock),
            Some(token),
            "E_SSH",
        )
        .await
    }

    async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        self.execute(
            host,
            args,
            None,
            self.wall_clock_or(SshClient::default_wall_clock(timeout)),
            Some(token),
            "E_SSH",
        )
        .await
    }

    async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        _timeout: Duration,
    ) -> Result<(), IpcError> {
        let bytes = std::fs::read(local_path).map_err(|e| {
            IpcError::new("E_UPLOAD", format!("open {}: {e}", local_path.display()))
        })?;
        let remote_cmd = format!("cat > {}", crate::shell::quote(remote_path));
        let out = self
            .execute(
                host,
                &[remote_cmd.as_str()],
                Some(bytes),
                // Same bound as `SshClient::upload_file`, not the probe one.
                self.wall_clock_or(UPLOAD_WALL_CLOCK),
                None,
                "E_UPLOAD",
            )
            .await?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_UPLOAD",
                format!(
                    "upload to {host} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(())
    }

    async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
        if let Some(home) = self.lock().homes.get(host) {
            return Ok(home.clone());
        }
        let out = self
            .run(host, &["printenv", "HOME"], Duration::from_secs(5))
            .await?;
        let home = home_from_output(host, &out)?;
        self.lock().homes.insert(host.to_string(), home.clone());
        Ok(home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_calls_in_order_and_answers_last_matching_rule() {
        let fake = FakeSsh::new();
        fake.on(Match::prefix("bash -lc"), Reply::ok("broad\n"))
            .on(Match::contains("tmux -V"), Reply::ok("tmux 3.4\n"))
            .on_host("beta", Match::Any, Reply::fail(1, "nope"));
        let a = fake
            .run(
                "alpha",
                &["bash", "-lc", "'tmux -V'"],
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert!(a.status.success());
        assert_eq!(String::from_utf8_lossy(&a.stdout), "tmux 3.4\n");
        let b = fake
            .run("alpha", &["bash", "-lc", "'other'"], Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&b.stdout), "broad\n");
        let c = fake
            .run(
                "beta",
                &["bash", "-lc", "'tmux -V'"],
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(c.status.code(), Some(1));
        assert_eq!(String::from_utf8_lossy(&c.stderr), "nope");
        // Unmatched → default exit 0.
        let d = fake
            .run("gamma", &["true"], Duration::from_secs(1))
            .await
            .unwrap();
        assert!(d.status.success() && d.stdout.is_empty());

        let cmds = fake.commands();
        assert_eq!(
            cmds,
            vec![
                "bash -lc 'tmux -V'",
                "bash -lc 'other'",
                "bash -lc 'tmux -V'",
                "true"
            ]
        );
        assert_eq!(fake.calls_for("beta").len(), 1);
        assert_eq!(fake.calls()[0].script().as_deref(), Some("tmux -V"));
        assert!(fake.calls()[3].script().is_none());
    }

    #[tokio::test]
    async fn unreachable_is_exit_255_not_err() {
        let fake = FakeSsh::new();
        fake.unreachable("down");
        let out = fake
            .run("down", &["true"], Duration::from_secs(1))
            .await
            .expect("Ok(Output), like the real client");
        assert_eq!(out.status.code(), Some(255));
        assert!(String::from_utf8_lossy(&out.stderr).contains("connect to host down"));
        // upload → E_UPLOAD; remote_home → E_SSH.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let up = fake
            .upload_file("down", tmp.path(), "/x", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(up.code, "E_UPLOAD");
        let home = fake.remote_home("down").await.unwrap_err();
        assert_eq!(home.code, "E_SSH");
    }

    #[tokio::test]
    async fn hang_times_out_under_the_wall_clock_and_cancels_first() {
        let fake = FakeSsh::new();
        fake.hanging("slow")
            .set_wall_clock(Duration::from_millis(50));
        let start = std::time::Instant::now();
        let e = fake
            .run("slow", &["true"], Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(e.code, "E_SSH_TIMEOUT");
        assert!(start.elapsed() < Duration::from_secs(2));

        let token = CancellationToken::new();
        token.cancel();
        let e = fake
            .run_cancellable("slow", &["true"], Duration::from_secs(1), token)
            .await
            .unwrap_err();
        assert_eq!(e.code, "E_CANCELLED");
    }

    #[tokio::test]
    async fn upload_records_stdin_and_home_is_cached() {
        let fake = FakeSsh::new();
        fake.with_home("/home/fake");
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "secret").unwrap();
        fake.upload_file("h", tmp.path(), "/home/fake/x y", Duration::from_secs(1))
            .await
            .unwrap();
        let calls = fake.calls();
        assert_eq!(calls[0].command(), "cat > '/home/fake/x y'");
        assert_eq!(calls[0].stdin_str().as_deref(), Some("secret"));
        assert_eq!(fake.remote_home("h").await.unwrap(), "/home/fake");
        assert_eq!(fake.remote_home("h").await.unwrap(), "/home/fake");
        assert_eq!(
            fake.commands()
                .iter()
                .filter(|c| c.as_str() == "printenv HOME")
                .count(),
            1,
            "second remote_home is served from the cache"
        );
    }

    #[tokio::test]
    async fn default_reply_regex_rules_spawn_errors_and_clear() {
        let fake = FakeSsh::new();
        fake.set_default(Reply::fail(127, "command not found"));
        fake.on(
            Match::regex(r"^tmux (list|kill)-session"),
            Reply::ok(
                "matched
",
            ),
        )
        .on_host(
            "nossh",
            Match::Any,
            Reply::SpawnError {
                message: "No such file or directory (os error 2)".into(),
            },
        );
        let d = fake
            .run("h", &["whatever"], Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(d.status.code(), Some(127), "unmatched → configured default");
        let r = fake
            .run(
                "h",
                &["tmux", "kill-session", "-t", "x"],
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&r.stdout), "matched\n");
        let e = fake
            .run("nossh", &["true"], Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(e.code, "E_SSH");
        assert!(e.message.contains("ssh spawn nossh"));
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let e = fake
            .upload_file("nossh", tmp.path(), "/x", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(e.code, "E_UPLOAD", "spawn failure keeps the upload code");
        assert_eq!(fake.calls().len(), 4);
        fake.clear_calls();
        assert!(fake.calls().is_empty());
        // Rules survive a clear.
        let r = fake
            .run("h", &["tmux", "list-sessions"], Duration::from_secs(1))
            .await
            .unwrap();
        assert!(r.status.success());
    }

    #[test]
    fn unquote_reverses_quote() {
        for s in ["", "plain", "it's", "a 'b' \"c\" $d `e`\nnew"] {
            assert_eq!(unquote(&crate::shell::quote(s)).as_deref(), Some(s));
        }
        assert!(unquote("bare").is_none());
    }
}
