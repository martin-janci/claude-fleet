use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use async_trait::async_trait;
use serde::Serialize;
use std::path::PathBuf;

use crate::ssh::{SshClient, SshExec};
use std::sync::Arc;

/// tmux target for an EXACT session name. A bare `-t NAME` is a *lookup*:
/// tmux tries an exact match, then a unique PREFIX, then an fnmatch pattern —
/// so an operation aimed at a dead `dev-foo` silently lands on
/// `dev-foo--feat-x`. The `=` prefix disables both fallbacks (verified
/// against tmux 3.6a: `has-session -t api` succeeds against `api-review`,
/// `has-session -t '=api'` fails with "can't find session").
pub fn exact_session(name: &str) -> String {
    format!("={name}")
}

/// The most shell terminals one session keeps (redesign step 5.3).
pub const MAX_SHELL_TERMINALS: u32 = 9;

/// The tmux session that holds shell terminal `n` of `session` (redesign
/// step 5.3): `<session>--sh<n>`. A terminal is a tmux session of its own,
/// never a window or pane of the agent's session, so `exact_pane(session)`
/// always lands on the agent and never on a shell.
pub fn shell_terminal_name(session: &str, n: u32) -> String {
    format!("{session}--sh{n}")
}

/// `Some((session, n))` when `name` is a shell terminal's tmux session
/// ([`shell_terminal_name`]): the suffix `--sh` and a number 1..=99 with no
/// leading zero, after a non-empty session name. PURE.
pub fn parse_shell_terminal_name(name: &str) -> Option<(&str, u32)> {
    let at = name.rfind("--sh")?;
    let (session, digits) = (&name[..at], &name[at + 4..]);
    if session.is_empty()
        || digits.is_empty()
        || digits.len() > 2
        || digits.starts_with('0')
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((session, digits.parse().ok()?))
}

/// Whether `name` is a shell terminal, which reconcile, discover and every
/// session list leave out: a terminal belongs to its session and is never a
/// session row of its own (no ghost, no Lost and found entry).
pub fn is_shell_terminal_name(name: &str) -> bool {
    parse_shell_terminal_name(name).is_some()
}

/// Every tmux session name on the server, one per line; nothing when no
/// server runs. [`shell_terminals_in`] picks a session's terminals out.
pub const LIST_SESSION_NAMES_SCRIPT: &str =
    "tmux list-sessions -F '#{session_name}' 2>/dev/null; true";

/// The terminal numbers of `session` in [`LIST_SESSION_NAMES_SCRIPT`]'s
/// output, ascending. PURE.
pub fn shell_terminals_in(session: &str, names: &str) -> Vec<u32> {
    let mut out: Vec<u32> = names
        .lines()
        .filter_map(|l| parse_shell_terminal_name(l.trim()))
        .filter(|(s, _)| *s == session)
        .map(|(_, n)| n)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Every tmux session on the server with what runs in the front of its
/// active pane (`pane_current_command`: the shell when it is idle, `node`
/// while `pnpm dev` runs), one `<command>|<name>` per line; nothing when no
/// server runs. [`shell_terminal_commands_in`] picks a session's terminals
/// out. The command goes first because a session name is the part that
/// must survive whole.
pub const LIST_SESSION_COMMANDS_SCRIPT: &str =
    "tmux list-sessions -F '#{pane_current_command}|#{session_name}' 2>/dev/null; true";

/// The terminals of `session` in [`LIST_SESSION_COMMANDS_SCRIPT`]'s output,
/// ascending, each with what runs in it (`None` when tmux printed nothing).
/// A `|` in the command or the name is resolved by trying each split until
/// the rest names one of `session`'s terminals. PURE.
pub fn shell_terminal_commands_in(session: &str, lines: &str) -> Vec<(u32, Option<String>)> {
    let mut out: Vec<(u32, Option<String>)> = lines
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            line.match_indices('|').find_map(|(at, _)| {
                let (s, n) = parse_shell_terminal_name(&line[at + 1..])?;
                (s == session).then(|| {
                    let cmd = line[..at].trim();
                    (n, (!cmd.is_empty()).then(|| cmd.to_string()))
                })
            })
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out.dedup_by_key(|(n, _)| *n);
    out
}

/// Exit code [`open_shell_terminal_script`] ends with when the agent's own
/// tmux session is gone: a terminal is only ever opened beside a live one.
pub const SHELL_TERMINAL_NO_SESSION: i32 = 3;

/// Open terminal `n` of `session` unless it is already open: a tmux session
/// of its own, started in the agent pane's current directory (`$HOME` when
/// that is gone, or when `home` asks for it — the strip's "New terminal opens
/// on" picker), running the same respawning login shell a shell session
/// does, so `exit` in it gives a fresh prompt rather than closing it.
pub fn open_shell_terminal_script(session: &str, n: u32, home: bool) -> String {
    let sh = shell_terminal_name(session, n);
    let cwd = if home {
        "cwd=\"$HOME\"; ".to_string()
    } else {
        format!(
            "cwd=$(tmux display-message -p -t {pane} '#{{pane_current_path}}' 2>/dev/null); \
             [ -d \"$cwd\" ] || cwd=\"$HOME\"; ",
            pane = quote(&exact_pane(session)),
        )
    };
    format!(
        "tmux has-session -t {agent} 2>/dev/null || exit {gone}; {cwd}\
         tmux has-session -t {exact} 2>/dev/null || tmux new-session -d -s {name} -c \"$cwd\" \
         -e COLORTERM=truecolor -e TERM=xterm-256color -e \"LANG=${{LANG:-en_US.UTF-8}}\" \
         -e \"PATH=$PATH\" {cmd}",
        agent = quote(&exact_session(session)),
        gone = SHELL_TERMINAL_NO_SESSION,
        exact = quote(&exact_session(&sh)),
        name = quote(&sh),
        cmd = quote(&shell_pane_command(None)),
    )
}

/// Close terminal `n` of `session`. One already gone is not an error.
pub fn close_shell_terminal_script(session: &str, n: u32) -> String {
    format!(
        "tmux kill-session -t {} 2>/dev/null; true",
        quote(&exact_session(&shell_terminal_name(session, n)))
    )
}

/// The `case` pattern matching every terminal of `session`: the name is
/// quoted, so it matches literally, and the suffix is `--sh` and 1..=99.
fn shell_terminal_case(session: &str) -> String {
    let q = quote(session);
    format!("{q}--sh[1-9]|{q}--sh[1-9][0-9]")
}

/// Close every terminal of `session`: a killed session takes its terminals
/// with it.
pub fn close_all_shell_terminals_script(session: &str) -> String {
    format!(
        "tmux list-sessions -F '#{{session_name}}' 2>/dev/null | while IFS= read -r s; do \
         case \"$s\" in {pat}) tmux kill-session -t \"=$s\";; esac; done; true",
        pat = shell_terminal_case(session),
    )
}

/// Rename every terminal of `old` to the same terminal of `new`: a renamed
/// session keeps its terminals.
pub fn rename_shell_terminals_script(old: &str, new: &str) -> String {
    format!(
        "tmux list-sessions -F '#{{session_name}}' 2>/dev/null | while IFS= read -r s; do \
         case \"$s\" in {pat}) tmux rename-session -t \"=$s\" {new}\"${{s#{old}}}\";; esac; done; true",
        pat = shell_terminal_case(old),
        new = quote(new),
        old = quote(old),
    )
}

/// The same, for commands taking a *pane* target (`capture-pane`,
/// `respawn-pane`). tmux only accepts `=` in the session part of a pane
/// target, so the trailing `:` (current window, active pane) is required:
/// `-t '=NAME'` fails there with "can't find pane" (tmux 3.6a).
pub fn exact_pane(name: &str) -> String {
    format!("={name}:")
}

/// Backend-agnostic tmux operations. Implementations differ only in how
/// the `tmux` binary is invoked: locally or wrapped in `ssh <host>`.
#[async_trait]
pub trait TmuxExec: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError>;
    /// `pane_cmd` is the shell command tmux runs as the pane's initial
    /// process — `pane_command_for()` for a Claude session, `shell_pane_command()`
    /// for a plain shell session.
    async fn new_session(
        &self,
        name: &str,
        working_dir: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError>;
    async fn kill_session(&self, name: &str) -> Result<(), IpcError>;
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError>;
    async fn restart_session(&self, name: &str, pane_cmd: &str) -> Result<(), IpcError>;
    /// `restart_session` with an explicit start directory (`respawn-pane -k
    /// -c <cwd>`), for a pane whose cwd was deleted or recreated. The default
    /// ignores `cwd` so test doubles need not implement it.
    async fn respawn_pane_in(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        let _ = cwd;
        self.restart_session(name, pane_cmd).await
    }
    /// Run one shell script against this host's tmux server: `bash -c`
    /// locally, the same transport as every other call remotely. Used by
    /// shell terminals (step 5.3), whose open / close / list are scripts of
    /// their own ([`open_shell_terminal_script`] and friends). The default
    /// refuses, so a test double never runs anything it was not built for.
    async fn run_script(&self, script: &str) -> Result<std::process::Output, IpcError> {
        let _ = script;
        Err(IpcError::new(
            codes::E_TMUX,
            "this tmux executor runs no scripts",
        ))
    }
    async fn capture_pane(&self, name: &str) -> Result<String, IpcError>;
    /// Capture the pane plus `lines` rows of scrollback history.
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError>;
    /// `claude agents --json` on the host. `None` when the host could not be
    /// asked (ssh failure, timeout, non-zero exit): the caller must not treat
    /// it as "no agents" — that is what ghosts every background row.
    async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>>;
    /// `sessionId → transcript mtime (unix s)` for the given ids; ids that are not
    /// valid Claude session ids are skipped. `Some(map)` when the call succeeded
    /// (possibly empty: no transcript found, or no valid id to ask about);
    /// `None` on any failure (spawn error, non-zero exit, timeout), so callers
    /// can tell "unknown" apart from "no transcript".
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        let _ = ids;
        Some(std::collections::HashMap::new())
    }
    /// The Claude account this host is currently logged into, read from its
    /// `~/.claude.json` `oauthAccount`. `None` means "could not tell" (ssh
    /// failure, file missing/unparseable, logged out, or an executor that
    /// does not implement it) — reconcile then leaves the host's stored
    /// account link untouched; it never clears it. Only `Some` with a uuid
    /// can relink a host (see `service::hosts::sync_host_account`).
    ///
    /// The default is `None`: `LocalTmux` keeps it, because `local` is
    /// synced by `service::hosts::sync_local_account` (an injectable-home
    /// file read, exercised by its own tests) — implementing it here too
    /// would probe `local` twice per pass. `RemoteTmux` overrides it.
    async fn read_oauth_account(&self) -> Option<crate::service::hosts::OauthAccount> {
        None
    }
    /// This host's boot identity (kernel boot id + tmux server pid), read
    /// once per reconcile probe. `None` means "could not tell" (ssh failure,
    /// transport timeout, unparseable output, or an executor that does not
    /// implement it) — it must NEVER be read as "no tmux server", because a
    /// later pass treats `Some(HostIdentity { tmux_server_pid: None, .. })`
    /// as exactly that and marks every session on the host lost. The default
    /// is `None`; `LocalTmux` and `RemoteTmux` both override it.
    async fn host_identity(&self) -> Option<HostIdentity> {
        None
    }

    /// `tmux -V` + `claude --version` on the host. `None` = could not tell
    /// (or an executor that does not implement it); the stored versions
    /// are then left alone. `LocalTmux` and `RemoteTmux` override it.
    async fn host_versions(&self) -> Option<HostVersions> {
        None
    }

    /// One health sample (disk, load, memory, uptime) from the host. `None`
    /// = could not tell (or an executor that does not implement it); the
    /// stored sample is then left alone. `LocalTmux` and `RemoteTmux`
    /// override it.
    async fn host_health(&self) -> Option<HostHealthSample> {
        None
    }

    /// How long an empty command takes to come back from the host, in ms:
    /// the "SSH · 18 ms" on the Hosts page (Orbit Fleet 4.6). `None` = not
    /// a remote host, or it did not answer. Only `RemoteTmux` overrides it.
    async fn round_trip_ms(&self) -> Option<i64> {
        None
    }

    /// The host's Claude login profiles (`~/.claude-profiles/<name>`,
    /// docs/accounts.md), each with the account its `.claude.json` is
    /// logged into. `None` = could not tell (or an executor that does not
    /// implement it): the stored list is then left alone. `Some(vec![])` =
    /// the host has none. `LocalTmux` and `RemoteTmux` override it.
    async fn read_profiles(&self) -> Option<Vec<HostProfile>> {
        None
    }

    /// Everything a reconcile pass needs from the host. The default composes
    /// the per-call methods (local tmux, test fakes); `RemoteTmux` overrides
    /// it with one script so a pass costs one round trip, not 5 + N.
    /// `want_versions`: ask for the versions section this pass (see
    /// `service::sessions::versions_due`).
    async fn probe_snapshot(&self, tail_lines: u32, want_versions: bool) -> ProbeSnapshot {
        let identity = self.host_identity().await;
        let versions = if want_versions {
            self.host_versions().await
        } else {
            None
        };
        let sessions = self.list_sessions().await;
        let account = if sessions.is_ok() {
            self.read_oauth_account().await
        } else {
            None
        };
        let health = if sessions.is_ok() {
            self.host_health().await
        } else {
            None
        };
        let profiles = if sessions.is_ok() {
            self.read_profiles().await
        } else {
            None
        };
        let mut pane_tails = std::collections::HashMap::new();
        if let Ok(live) = &sessions {
            for s in live {
                if let Ok(tail) = self.capture_pane_scrollback(&s.name, tail_lines).await {
                    pane_tails.insert(s.name.clone(), tail);
                }
            }
        }
        ProbeSnapshot {
            identity,
            sessions,
            account,
            pane_tails,
            versions,
            health,
            profiles,
        }
    }
}

/// Shell script printing `<sessionId>\t<mtime>` for the first
/// `$HOME/.claude/projects/*/<sessionId>.jsonl` found per id. `date -r <file>
/// +%s` behaves the same on GNU and BSD. Every id is validated as a Claude
/// session id and shell-quoted; `None` when no id survives validation.
pub fn transcript_mtimes_script(ids: &[String]) -> Option<String> {
    let valid: Vec<String> = ids
        .iter()
        .filter(|id| crate::validate::claude_session_id(id).is_ok())
        .map(|id| quote(id))
        .collect();
    if valid.is_empty() {
        return None;
    }
    Some(format!(
        "for id in {}; do for f in \"$HOME\"/.claude/projects/*/\"$id\".jsonl; do \
         if [ -f \"$f\" ]; then printf '%s\\t%s\\n' \"$id\" \"$(date -r \"$f\" +%s)\"; break; fi; \
         done; done",
        valid.join(" ")
    ))
}

/// Parse [`transcript_mtimes_script`] output. Lines that are not exactly
/// `<valid session id>\t<i64>` (login-shell noise, a failed `date`) are
/// ignored.
pub fn parse_mtimes(stdout: &str) -> std::collections::HashMap<String, i64> {
    stdout
        .lines()
        .filter_map(|line| {
            let (id, mtime) = line.split_once('\t')?;
            if mtime.contains('\t') || crate::validate::claude_session_id(id).is_err() {
                return None;
            }
            Some((id.to_string(), mtime.trim().parse::<i64>().ok()?))
        })
        .collect()
}

/// Section delimiter of the batched probe. A pane line that starts with it
/// is escaped by the script (one leading space), so the parser never
/// mistakes pane text for a section.
pub const PROBE_DELIM: &str = "---FLEET:";

/// Everything a reconcile pass reads from a host, in ONE round trip.
#[derive(Debug)]
pub struct ProbeSnapshot {
    pub identity: Option<HostIdentity>,
    pub sessions: Result<Vec<TmuxSession>, IpcError>,
    pub account: Option<crate::service::hosts::OauthAccount>,
    /// Pane text per live session, exactly `capture-pane -S -<n> -p`.
    pub pane_tails: std::collections::HashMap<String, String>,
    /// The versions section, when this pass asked for it and it parsed.
    pub versions: Option<HostVersions>,
    /// The health section (every pass), when the host answered it.
    pub health: Option<HostHealthSample>,
    /// The host's login profiles ([`TmuxExec::read_profiles`]); `None` =
    /// could not tell.
    pub profiles: Option<Vec<HostProfile>>,
}

/// One Claude login profile on a host: `~/.claude-profiles/<name>` and the
/// account its `.claude.json` is logged into (`None`: not logged in yet, or
/// unreadable).
#[derive(Debug, Clone)]
pub struct HostProfile {
    pub name: String,
    pub account: Option<crate::service::hosts::OauthAccount>,
}

/// Shell section listing the host's login profiles: one
/// `@@P\t<name>\t<oauthAccount json>` line per `~/.claude-profiles/<name>`
/// directory whose name is a valid profile name (anything else is skipped,
/// never echoed into a line a parser splits on tabs). The account is read
/// by the same script as the host's own, with `CLAUDE_CONFIG_DIR` pointed
/// at the profile, and squeezed onto one line.
pub(crate) fn profiles_script() -> String {
    format!(
        "for d in \"$HOME\"/.claude-profiles/*/; do [ -d \"$d\" ] || continue; n=$(basename \"$d\"); \
         case \"$n\" in ''|[!A-Za-z0-9]*|*[!A-Za-z0-9_-]*) continue;; esac; \
         printf '@@P\\t%s\\t%s\\n' \"$n\" \"$(CLAUDE_CONFIG_DIR=\"${{d%/}}\"; {} | tr -d '\\n')\"; done",
        crate::service::hosts::OAUTH_ACCOUNT_SCRIPT
    )
}

/// Parse [`profiles_script`] output (or the `profiles` probe section).
/// Lines that are not `@@P` lines, and names that do not validate, are
/// dropped.
pub(crate) fn parse_profiles(body: &str) -> Vec<HostProfile> {
    let mut out: Vec<HostProfile> = body
        .lines()
        .filter_map(|l| l.strip_prefix("@@P\t"))
        .filter_map(|rest| {
            let (name, json) = rest.split_once('\t').unwrap_or((rest, ""));
            crate::validate::claude_profile(name).ok()?;
            Some(HostProfile {
                name: name.to_string(),
                account: crate::service::hosts::parse_oauth_account(json),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// `local`'s login profiles, read from `<home>/.claude-profiles` directly
/// (blocking fs reads: call off the async worker). `None` when the
/// directory exists but cannot be listed; a missing directory is no
/// profiles.
pub(crate) fn read_local_profiles(home: &std::path::Path) -> Option<Vec<HostProfile>> {
    let dir = home.join(".claude-profiles");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut out: Vec<HostProfile> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            crate::validate::claude_profile(&name).ok()?;
            let account = match crate::service::hosts::probe_local_account_in(&e.path()) {
                crate::service::hosts::LocalAccountProbe::LoggedIn(a) => Some(a),
                _ => None,
            };
            Some(HostProfile { name, account })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

/// Shell script listing the most recently modified Claude transcripts under
/// `$CLAUDE_CONFIG_DIR/projects/*/*.jsonl` (`$HOME/.claude` when unset; a
/// `*/subagents/*` path is skipped),
/// newest first, up to `limit` (clamped to `1..=500` here — `limit` is
/// then a bare integer in the script, so it needs no shell quoting). For
/// each transcript kept it prints `@@F\t<mtime>\t<claude session id>`
/// (the id is the file's basename with `.jsonl` stripped), then
/// `@@L\t<line>` carrying the last line of that file (within its last 4 MiB,
/// truncated to 8192 bytes) that mentions `"cwd"` — the transcript line that
/// carries the session's `cwd`/`gitBranch`. [`crate::service::sessions::parse_discover_output`]
/// turns this into [`crate::service::sessions::TranscriptProbe`]s.
///
/// Before any of that it prints one boot-time line: `bootsec=<epoch>` when
/// `/proc/uptime` is readable (Linux), else `bootraw=<kern.boottime output>`
/// (macOS, via `sysctl`) — the same parser turns either into a boot epoch.
///
/// The mtime is GNU `stat -c %Y`, else BSD `stat -f %m`, else `date -r FILE
/// +%s` (review r18: `date -r FILE` alone is not portable — an older BSD
/// `date -r` takes seconds, not a file); a file none of them can date is
/// skipped rather than sorted on garbage. Runs unmodified on the Linux and
/// macOS hosts fleet targets.
pub fn discover_transcripts_script(limit: usize) -> String {
    let limit = limit.clamp(1, 500);
    format!(
        "now=$(date +%s)\n\
if [ -r /proc/uptime ]; then printf 'bootsec=%s\\n' \"$(( now - $(cut -d. -f1 /proc/uptime) ))\"; else printf 'bootraw=%s\\n' \"$(sysctl -n kern.boottime 2>/dev/null)\"; fi\n\
d=\"${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}\"\n\
for f in \"$d\"/projects/*/*.jsonl; do [ -f \"$f\" ] || continue; case \"$f\" in */subagents/*) continue;; esac; m=$(stat -c %Y -- \"$f\" 2>/dev/null || stat -f %m -- \"$f\" 2>/dev/null || date -r \"$f\" +%s 2>/dev/null); case \"$m\" in ''|*[!0-9]*) continue;; esac; printf '%s\\t%s\\n' \"$m\" \"$f\"; done | sort -rn | head -n {limit} | while IFS=\"$(printf '\\t')\" read -r m f; do\n\
  printf '@@F\\t%s\\t%s\\n' \"$m\" \"$(basename \"$f\" .jsonl)\"\n\
  printf '@@L\\t%s\\n' \"$(tail -c 4194304 \"$f\" | grep -a '\"cwd\"' | tail -n 1 | cut -c1-8192)\"\n\
done"
    )
}

/// A host's boot identity, read once per reconcile probe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostIdentity {
    /// Kernel boot id — `/proc/sys/kernel/random/boot_id` (the HOST's, even
    /// inside a container), else `sysctl -n kern.bootsessionuuid` (macOS): a
    /// UUID stable for the whole boot. `None` when neither produced anything.
    /// Deliberately NOT `kern.boottime` or `uptime -s`: both render their
    /// timestamp in the local timezone, so the same boot prints a different
    /// string under a different `TZ` (or across a DST change, or after NTP
    /// steps the clock) — a later task reads any change in this string as a
    /// reboot and marks every session on the host lost, so a
    /// timezone-flavored "identity" is worse than none. Do not reintroduce
    /// them.
    pub boot_id: Option<String>,
    /// Pid of the tmux server. `None` ⇒ no tmux server is running.
    pub tmux_server_pid: Option<i64>,
}

/// Prints `boot=<id>`, then `tmuxrc=<tmux's own exit code>` and
/// `tmuxout=<first line of tmux's combined stdout+stderr>`.
///
/// This deliberately does NOT decide "no server" in shell: every tmux
/// failure looks the same to a naive `2>/dev/null` — missing binary (exit
/// 127, e.g. a login profile edit or a `brew`/`apt upgrade tmux` mid-relink
/// dropping tmux off `PATH`), a client/server protocol mismatch after an
/// upgrade (the new client can't talk to the still-running old server),
/// socket permission errors, or an actually-dead server — and only the last
/// one means the server (and every session on it) is gone. So the script
/// only *surfaces* tmux's raw exit code and output; `parse_host_identity`
/// classifies it in Rust, via the same [`is_no_server_running`] the session
/// lister already trusts, instead of guessing here.
///
/// `tmux list-sessions -F '#{pid}'` (not `display-message`) because it
/// needs no attached client, matching what `list_local_sessions` /
/// `RemoteTmux::list_sessions` already run.
///
/// The boot id source is deliberately just `/proc/sys/kernel/random/boot_id`
/// (Linux) falling back to `sysctl -n kern.bootsessionuuid` (macOS) — both
/// are per-boot identifiers with no wall-clock rendering. `kern.boottime`
/// and `uptime -s` were rejected: both print a local-time timestamp, so the
/// same boot yields a different "boot id" under a different `TZ`, across a
/// DST change, or after NTP steps the clock, and a spurious change is read
/// downstream as a reboot that marks every session on the host lost.
pub const HOST_IDENTITY_SCRIPT: &str = "printf 'boot=%s\\n' \"$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || sysctl -n kern.bootsessionuuid 2>/dev/null)\"; out=$(tmux list-sessions -F '#{pid}' 2>&1); rc=$?; printf 'tmuxrc=%s\\n' \"$rc\"; printf 'tmuxout=%s\\n' \"$(printf '%s' \"$out\" | head -n 1)\"";

/// The versions the fleet shows for a host, read from the host itself.
/// `None` = the binary answered nothing (not on `PATH`, or the section was
/// not asked for this pass) — the stored value is then kept, never blanked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostVersions {
    /// `2.1.282` (the first word of `claude --version`).
    pub claude_version: Option<String>,
    /// `3.6a` (`tmux -V` without the `tmux ` prefix).
    pub tmux_version: Option<String>,
}

/// Prints `tmuxv=<tmux -V>` and `claudev=<first line of claude --version>`.
/// `claude --version` is a node start (0.3–1 s), so the reconcile pass asks
/// for this section only when the stored stamp is older than
/// [`crate::service::sessions::VERSIONS_REFRESH_SECS`].
pub const HOST_VERSIONS_SCRIPT: &str = "printf 'tmuxv=%s\\n' \"$(tmux -V 2>/dev/null)\"; printf 'claudev=%s\\n' \"$(claude --version 2>/dev/null | head -n 1)\"";

/// Parse [`HOST_VERSIONS_SCRIPT`] output. Missing or empty lines are
/// `None`; nothing here is ever an error.
pub fn parse_host_versions(stdout: &str) -> HostVersions {
    let mut v = HostVersions::default();
    for line in stdout.lines() {
        if let Some(t) = line.strip_prefix("tmuxv=") {
            v.tmux_version = crate::service::hosts::parse_tmux_version(t.trim());
        } else if let Some(c) = line.strip_prefix("claudev=") {
            v.claude_version = crate::service::hosts::parse_claude_version(c.trim());
        }
    }
    v
}

/// One health sample of a host (host identity & health, task 2). Every
/// field `None` when the host could not answer that line; nothing here is
/// ever an error.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostHealthSample {
    pub disk_home_free_kb: Option<i64>,
    pub disk_home_total_kb: Option<i64>,
    pub disk_tmp_free_kb: Option<i64>,
    pub load_1m: Option<f64>,
    pub mem_avail_kb: Option<i64>,
    pub uptime_secs: Option<i64>,
    /// Online CPUs (`getconf _NPROCESSORS_ONLN`, else `hw.ncpu`).
    pub cpu_count: Option<i64>,
    /// Physical memory in kB (`MemTotal`, else `hw.memsize` / 1024).
    pub mem_total_kb: Option<i64>,
    /// Boot time, unix seconds, as the host states it (`btime` in
    /// `/proc/stat`, else `kern.boottime`). Steady between passes, unlike
    /// now − uptime, so a change means the host rebooted.
    pub boot_at: Option<i64>,
    /// Round trip of an empty command to the host, in ms (Orbit Fleet 4.6).
    /// Not read by the script: the probe times [`TmuxExec::round_trip_ms`]
    /// and fills it in. `None` for `local` and whenever it was not timed.
    pub latency_ms: Option<i64>,
    /// Which [`AUTH_OVERRIDE_VARS`] are set, by NAME only, in the probing
    /// shell or the tmux server's global environment (which every new pane
    /// inherits). Any of them outranks the host's `/login`, so a session
    /// started there bills that credential rather than the account fleet
    /// shows. `None`: the host could not tell (an older agent, a failed
    /// line); `Some(empty)`: none set.
    pub auth_overrides: Option<Vec<String>>,
    /// Which of [`crate::service::host_check::AGENT_BINARIES`] are on the
    /// probing shell's `PATH`, in that order (Orbit Fleet 12.4: the New
    /// session picker enables an agent only where it can run). `None`: the
    /// host could not tell; `Some(empty)`: none found.
    pub agents_on_path: Option<Vec<String>>,
}

/// The variables that outrank a Claude Code `/login` subscription
/// credential (code.claude.com/docs/en/authentication, "Authentication
/// precedence"), in that order. Only their names ever leave the host.
pub const AUTH_OVERRIDE_VARS: [&str; 7] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_PROFILE",
];

/// `df -Pk` of `$HOME` and `${TMPDIR:-/tmp}` (POSIX output, one line each,
/// header dropped), the load average (`/proc/loadavg` on Linux, `sysctl -n
/// vm.loadavg` on macOS — `{ 1.62 1.80 1.91 }`), available memory in kB
/// (`MemAvailable` on Linux; empty on macOS, whose `vm_stat` has no single
/// equivalent), and the uptime in seconds (`/proc/uptime`, else derived
/// from `kern.boottime`, `{ sec = N, usec = M } <date>`; the sed anchors on
/// `{ sec = ` because `usec = ` also contains `sec = `), then the online CPU
/// count, total memory in kB and the boot epoch (Orbit Fleet 4.6: `cpus=`,
/// `memtotal=`, `bootat=`, Linux first, macOS `sysctl` second). Every command is `2>/dev/null` with an empty
/// value on failure, so a missing tool degrades one field, never the probe.
/// `authenv=` lists which [`AUTH_OVERRIDE_VARS`] hold a non-empty value in
/// this shell or in `tmux show-environment -g`: `grep` matches whole lines
/// inside the pipe and `cut` keeps only the name, so no value is printed.
/// `agents=` lists which agent CLIs `command -v` finds (Orbit Fleet 12.4;
/// the names are [`crate::service::host_check::AGENT_BINARIES`]).
pub const HOST_HEALTH_SCRIPT: &str = "printf 'dfhome=%s\\n' \"$(df -Pk \"$HOME\" 2>/dev/null | tail -n 1)\"; \
printf 'dftmp=%s\\n' \"$(df -Pk \"${TMPDIR:-/tmp}\" 2>/dev/null | tail -n 1)\"; \
printf 'load=%s\\n' \"$(cat /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null)\"; \
printf 'memkb=%s\\n' \"$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo 2>/dev/null)\"; \
printf 'uptime=%s\\n' \"$(cut -d. -f1 /proc/uptime 2>/dev/null || { b=$(sysctl -n kern.boottime 2>/dev/null | sed -n 's/.*{ sec = \\([0-9]*\\),.*/\\1/p'); [ -n \"$b\" ] && echo $(( $(date +%s) - b )); })\"; \
printf 'cpus=%s\\n' \"$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null)\"; \
printf 'memtotal=%s\\n' \"$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || { m=$(sysctl -n hw.memsize 2>/dev/null); [ -n \"$m\" ] && echo $(( m / 1024 )); })\"; \
printf 'bootat=%s\\n' \"$(awk '/^btime / {print $2}' /proc/stat 2>/dev/null || sysctl -n kern.boottime 2>/dev/null | sed -n 's/.*{ sec = \\([0-9]*\\),.*/\\1/p')\"; \
printf 'authenv=%s\\n' \"$({ env; tmux show-environment -g 2>/dev/null; } | grep -E '^(CLAUDE_CODE_USE_BEDROCK|CLAUDE_CODE_USE_VERTEX|CLAUDE_CODE_USE_FOUNDRY|ANTHROPIC_AUTH_TOKEN|ANTHROPIC_API_KEY|CLAUDE_CODE_OAUTH_TOKEN|ANTHROPIC_PROFILE)=.' | cut -d= -f1 | sort -u | tr '\\n' ' ')\"; \
printf 'agents=%s\\n' \"$(for a in claude codex agy gemini; do command -v \"$a\" >/dev/null 2>&1 && printf '%s ' \"$a\"; done)\"";

/// Second field of a `df -Pk` data line is total kB, fourth is available kB.
fn df_kb(line: &str) -> (Option<i64>, Option<i64>) {
    let f: Vec<&str> = line.split_whitespace().collect();
    let total = f.get(1).and_then(|v| v.parse().ok());
    let avail = f.get(3).and_then(|v| v.parse().ok());
    (total, avail)
}

/// Parse [`HOST_HEALTH_SCRIPT`] output.
pub fn parse_host_health(stdout: &str) -> HostHealthSample {
    let mut h = HostHealthSample::default();
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("dfhome=") {
            let (total, avail) = df_kb(v);
            h.disk_home_total_kb = total;
            h.disk_home_free_kb = avail;
        } else if let Some(v) = line.strip_prefix("dftmp=") {
            h.disk_tmp_free_kb = df_kb(v).1;
        } else if let Some(v) = line.strip_prefix("load=") {
            h.load_1m = v
                .split(|c: char| c.is_whitespace() || c == '{' || c == '}')
                .find(|s| !s.is_empty())
                .and_then(|s| s.parse().ok());
        } else if let Some(v) = line.strip_prefix("memkb=") {
            h.mem_avail_kb = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("uptime=") {
            h.uptime_secs = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("cpus=") {
            h.cpu_count = v.trim().parse().ok().filter(|n: &i64| *n > 0);
        } else if let Some(v) = line.strip_prefix("memtotal=") {
            h.mem_total_kb = v.trim().parse().ok().filter(|n: &i64| *n > 0);
        } else if let Some(v) = line.strip_prefix("bootat=") {
            h.boot_at = v.trim().parse().ok().filter(|n: &i64| *n > 0);
        } else if let Some(v) = line.strip_prefix("authenv=") {
            // In precedence order, and only names fleet asked about.
            let set: Vec<&str> = v.split_whitespace().collect();
            h.auth_overrides = Some(
                AUTH_OVERRIDE_VARS
                    .iter()
                    .filter(|n| set.contains(n))
                    .map(|n| n.to_string())
                    .collect(),
            );
        } else if let Some(v) = line.strip_prefix("agents=") {
            // In the checklist's order, and only names fleet asked about.
            let found: Vec<&str> = v.split_whitespace().collect();
            h.agents_on_path = Some(
                crate::service::host_check::AGENT_BINARIES
                    .iter()
                    .filter(|a| found.contains(a))
                    .map(|a| a.to_string())
                    .collect(),
            );
        }
    }
    h
}

/// Parse [`HOST_IDENTITY_SCRIPT`] output. Requires BOTH a `tmuxrc=` line
/// (tmux's exit code) and a `tmuxout=` line (the first line of its combined
/// stdout+stderr) — missing either, or an unparseable `tmuxrc`, is output we
/// cannot trust → outer `None`. Then:
/// - `tmuxrc == 0`: the output must parse as the server pid. Anything else
///   (empty — e.g. a live server holding zero sessions — or non-numeric) means
///   we cannot tell the pid, so outer `None`, never a guess.
/// - `tmuxrc != 0` and [`is_no_server_running`] matches the output: the ONLY
///   path to `tmux_server_pid: None` — the server is confirmed gone.
/// - `tmuxrc != 0` and anything else (missing binary, protocol mismatch,
///   permission error, …): untrustworthy, so outer `None`. An untrusted
///   "no server" would mark every session on the host lost.
pub fn parse_host_identity(stdout: &str) -> Option<HostIdentity> {
    let mut boot_id = None;
    let mut rc_line: Option<&str> = None;
    let mut out_line: Option<&str> = None;
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("boot=") {
            let v = v.trim();
            if !v.is_empty() {
                boot_id = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("tmuxrc=") {
            rc_line = Some(v.trim());
        } else if let Some(v) = line.strip_prefix("tmuxout=") {
            out_line = Some(v.trim());
        }
    }
    let rc: i32 = rc_line?.parse().ok()?;
    let out = out_line?;
    let tmux_server_pid = if rc == 0 {
        Some(out.parse::<i64>().ok()?)
    } else if is_no_server_running(out) {
        None
    } else {
        return None;
    };
    Some(HostIdentity {
        boot_id,
        tmux_server_pid,
    })
}

/// tmux (and `claude` / `bash`) on this machine, the `local` host. Every
/// method refuses first when this is a hub with `hub.local_host=false`: the
/// fallible ones return `E_NOTFOUND`, the infallible ones their "nothing
/// known" value, so no construction site can spawn on a disabled `local`.
pub struct LocalTmux;

fn local_allowed() -> Result<(), IpcError> {
    crate::service::hub::ensure_local_allowed(crate::service::projects::LOCAL_HOST)
}

#[async_trait]
impl TmuxExec for LocalTmux {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        local_allowed()?;
        list_local_sessions().await
    }
    async fn new_session(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        local_allowed()?;
        new_session(name, cwd, pane_cmd).await
    }
    async fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        local_allowed()?;
        kill_session(name).await
    }
    async fn run_script(&self, script: &str) -> Result<std::process::Output, IpcError> {
        local_allowed()?;
        crate::proc::command("bash")
            .args(["-c", script])
            .output()
            .await
            .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn bash: {e}")))
    }
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        local_allowed()?;
        rename_session(old, new).await
    }
    async fn restart_session(&self, name: &str, pane_cmd: &str) -> Result<(), IpcError> {
        local_allowed()?;
        restart_session(name, pane_cmd).await
    }
    async fn respawn_pane_in(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        local_allowed()?;
        respawn_pane_in(name, cwd, pane_cmd).await
    }
    async fn capture_pane(&self, name: &str) -> Result<String, IpcError> {
        local_allowed()?;
        let output = crate::proc::command("tmux")
            .args(["capture-pane", "-t", &exact_pane(name), "-p"])
            .output()
            .await
            .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError> {
        local_allowed()?;
        let start = scrollback_start(lines);
        // Bounded like the remote path (30 s): the Conversation tab's probe
        // polls this every 2 s, and a wedged local tmux server must not
        // hang it.
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::proc::command("tmux")
                .args(["capture-pane", "-t", &exact_pane(name), "-S", &start, "-p"])
                .output(),
        )
        .await
        .map_err(|_| IpcError::new(codes::E_TIMEOUT, "tmux capture-pane timed out"))?
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Ok(String::new())
        }
    }
    async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> {
        local_allowed().ok()?;
        let output = crate::proc::command("claude")
            .args(["agents", "--json"])
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(crate::claude_agents::parse_claude_agents_json(
            &String::from_utf8_lossy(&output.stdout),
        ))
    }
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        local_allowed().ok()?;
        let Some(script) = transcript_mtimes_script(ids) else {
            return Some(std::collections::HashMap::new());
        };
        // No login shell: the script needs only `$HOME`, which is inherited.
        match crate::proc::command("bash")
            .args(["-c", &script])
            .output()
            .await
        {
            Ok(o) if o.status.success() => Some(parse_mtimes(&String::from_utf8_lossy(&o.stdout))),
            _ => None,
        }
    }
    async fn host_identity(&self) -> Option<HostIdentity> {
        local_allowed().ok()?;
        let out = crate::proc::command("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())?;
        parse_host_identity(&String::from_utf8_lossy(&out.stdout))
    }
    async fn host_versions(&self) -> Option<HostVersions> {
        local_allowed().ok()?;
        let out = crate::proc::command("bash")
            .args(["-c", HOST_VERSIONS_SCRIPT])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some(parse_host_versions(&String::from_utf8_lossy(&out.stdout)))
    }
    async fn host_health(&self) -> Option<HostHealthSample> {
        local_allowed().ok()?;
        let out = crate::proc::command("bash")
            .args(["-c", HOST_HEALTH_SCRIPT])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some(parse_host_health(&String::from_utf8_lossy(&out.stdout)))
    }
    async fn read_profiles(&self) -> Option<Vec<HostProfile>> {
        local_allowed().ok()?;
        let home = crate::service::hosts::local_home_dir();
        tokio::task::spawn_blocking(move || read_local_profiles(&home))
            .await
            .ok()
            .flatten()
    }
}

/// tmux over an `SshExec`. Generic (defaulting to the production client) so
/// the exact scripts it builds can be exercised against a scripted fake or
/// the local `bash -c` executor without a real host; every construction site
/// still just writes `RemoteTmux { client: Arc::clone(ssh), host }`.
pub struct RemoteTmux<C: SshExec = Arc<SshClient>> {
    pub client: C,
    pub host: String,
}

/// Cap on the combined stdout captured for one tmux call — a runaway pane
/// capture or a chatty script must not let the app buffer unbounded remote
/// output.
pub const TMUX_OUTPUT_CAP: usize = 8 * 1024 * 1024;

impl<C: SshExec> RemoteTmux<C> {
    /// One tmux call on the host. With a resolved toolchain: `sh -c 'export
    /// PATH=<login PATH>; <script>'` — no login shell spawned per call.
    /// Without one: `bash -lc '<script>'` as before, so the remote user's
    /// login env (PATH, LANG, etc.) is still sourced. sshd may have
    /// `AcceptEnv` disabled which would silently drop SendEnv vars; the
    /// login shell route is portable.
    ///
    /// CRITICAL: `ssh <host> bash -lc <script>` (or `sh -c <script>`) joins
    /// ALL trailing argv with spaces before sending to the remote sshd. The
    /// remote shell then re-tokenizes, so any spaces in `<script>` would
    /// break `bash -c` (it would get just the first token as the script and
    /// everything else as positional args). We therefore single-quote the
    /// WHOLE script via `quote` so it crosses the ssh boundary as one shell
    /// word. `quote` already escapes the embedded `'` characters used by
    /// per-arg quoting inside `script`.
    ///
    /// The 10s here is ssh's `ConnectTimeout` only. `SshClient::run` bounds
    /// the whole command by `default_wall_clock(10s)` = 30s on top, so a tmux
    /// command that hangs after connect (wedged ControlMaster) surfaces as
    /// `E_SSH_TIMEOUT` instead of blocking the caller forever. Output is
    /// capped at [`TMUX_OUTPUT_CAP`].
    async fn remote_sh(&self, script: &str) -> Result<std::process::Output, IpcError> {
        let connect = std::time::Duration::from_secs(10);
        let args: Vec<String> = match self.client.toolchain(&self.host).await {
            Some(tc) => vec![
                "sh".into(),
                "-c".into(),
                quote(&format!("export PATH={}; {script}", quote(&tc.path))),
            ],
            None => vec!["bash".into(), "-lc".into(), quote(script)],
        };
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        self.client
            .run_bounded_capped(
                &self.host,
                &argv,
                connect,
                SshClient::default_wall_clock(connect),
                TMUX_OUTPUT_CAP,
            )
            .await
    }
}

const SESSIONS_FORMAT: &str = "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}|#{pane_id}";

/// The batched probe: identity, the session list with its exit code, the
/// oauth account, then one pane capture per live session, each behind a
/// `---FLEET:` line. Ends with `---FLEET:end` so a capped or cut output is
/// recognisable. Session and pane lines starting with the delimiter get one
/// leading space (`sed 's/^---FLEET/ &/'`) so they cannot open a section —
/// a live session literally named `---FLEET:evil` (or a pane whose tail
/// happens to start with the marker) must never forge a section boundary.
/// `parse_probe_snapshot` undoes the escape on the sessions section before
/// parsing it (pane tails keep the leading space verbatim: it's just
/// display text there).
pub fn probe_snapshot_script(tail_lines: u32, want_versions: bool) -> String {
    let start = scrollback_start(tail_lines);
    // The versions section costs a `claude --version` node start, so it is
    // only in the script on a pass that is due for it.
    let versions = if want_versions {
        format!("printf '%s\\n' '---FLEET:versions'; {HOST_VERSIONS_SCRIPT}; ")
    } else {
        String::new()
    };
    format!(
        "printf '%s\\n' '---FLEET:identity'; {HOST_IDENTITY_SCRIPT}; \
         {versions}\
         printf '%s\\n' '---FLEET:health'; {HOST_HEALTH_SCRIPT}; \
         printf '%s\\n' '---FLEET:sessions'; out=$(tmux list-sessions -F '{SESSIONS_FORMAT}' 2>&1); rc=$?; printf 'rc=%s\\n' \"$rc\"; printf '%s\\n' \"$out\" | sed 's/^---FLEET/ &/'; \
         printf '%s\\n' '---FLEET:account'; {}; \
         printf '%s\\n' '---FLEET:profiles'; {}; \
         printf '%s\\n' '---FLEET:panes'; \
         tmux list-sessions -F '#{{session_name}}' 2>/dev/null | while IFS= read -r s; do case \"$s\" in *--sh[1-9]|*--sh[1-9][0-9]) continue;; esac; printf '%s\\n' \"---FLEET:pane $s\"; tmux capture-pane -t \"=$s:\" -S {start} -p 2>/dev/null | sed 's/^---FLEET/ &/'; done; \
         printf '%s\\n' '---FLEET:end'",
        crate::service::hosts::OAUTH_ACCOUNT_SCRIPT,
        profiles_script()
    )
}

/// Undo the `sed 's/^---FLEET/ &/'` escape `probe_snapshot_script` applies
/// to a session (or pane) line that would otherwise open a section: strip
/// exactly one leading space from any line starting with ` ---FLEET`.
fn unescape_delim_lines(body: &str) -> String {
    body.lines()
        .map(|l| {
            l.strip_prefix(' ')
                .filter(|r| r.starts_with("---FLEET"))
                .unwrap_or(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse [`probe_snapshot_script`] output. `Err` only when the text is not
/// a probe at all (ssh's own error, or a capped/cut output without the end
/// marker); a section that is present but unusable degrades to that
/// section's "unknown" value, except the session list, whose garbage is an
/// `Err` inside the snapshot exactly as `list_sessions` reports it.
pub fn parse_probe_snapshot(stdout: &str) -> Result<ProbeSnapshot, IpcError> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        if let Some(name) = line.strip_prefix(PROBE_DELIM) {
            sections.push((name.to_string(), String::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !sections.iter().any(|(n, _)| n == "end") {
        return Err(IpcError::new(
            codes::E_TMUX,
            format!(
                "probe output truncated or not a probe: {}",
                stdout.trim().lines().next().unwrap_or("")
            ),
        ));
    }
    let section = |name: &str| {
        sections
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.as_str())
    };
    let identity = section("identity").and_then(parse_host_identity);
    let sessions = match section("sessions") {
        None => Err(IpcError::new(
            codes::E_TMUX,
            "probe output has no sessions section",
        )),
        Some(body) => {
            let mut lines = body.lines();
            let rc: Option<i32> = lines
                .next()
                .and_then(|l| l.strip_prefix("rc="))
                .and_then(|v| v.trim().parse().ok());
            let combined: String = unescape_delim_lines(&lines.collect::<Vec<_>>().join("\n"));
            match rc {
                Some(0) => {
                    if combined.trim().is_empty() {
                        Ok(Vec::new())
                    } else {
                        parse_sessions_checked(&combined)
                    }
                }
                Some(_) if is_no_server_running(&combined) => Ok(Vec::new()),
                Some(_) => Err(IpcError::new(codes::E_TMUX, combined.trim())),
                None => Err(IpcError::new(
                    codes::E_TMUX,
                    "probe output has no sessions exit code",
                )),
            }
        }
    };
    let account =
        section("account").and_then(|b| crate::service::hosts::parse_oauth_account(b.trim()));
    let versions = section("versions").map(parse_host_versions);
    let health = section("health").map(parse_host_health);
    let profiles = section("profiles").map(parse_profiles);
    let mut pane_tails = std::collections::HashMap::new();
    for (name, body) in &sections {
        if let Some(pane) = name.strip_prefix("pane ") {
            pane_tails.insert(pane.to_string(), body.trim_end_matches('\n').to_string());
        }
    }
    Ok(ProbeSnapshot {
        identity,
        sessions,
        account,
        pane_tails,
        versions,
        health,
        profiles,
    })
}

#[async_trait]
impl<C: SshExec> TmuxExec for RemoteTmux<C> {
    async fn round_trip_ms(&self) -> Option<i64> {
        // `true` straight on the ssh command line, not through
        // `remote_sh`'s `bash -lc`: a login shell's profile would be timed
        // as network.
        let budget = std::time::Duration::from_secs(5);
        let start = std::time::Instant::now();
        self.client
            .run_bounded_capped(&self.host, &["true"], budget, budget, 1024)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some(i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX))
    }
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        let script = format!("tmux list-sessions -F '{SESSIONS_FORMAT}' 2>&1");
        let output = self.remote_sh(&script).await?;
        let combined = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return parse_sessions_checked(&combined);
        }
        if is_no_server_running(&combined) {
            return Ok(Vec::new());
        }
        Err(IpcError::new(codes::E_TMUX, combined.trim()))
    }

    async fn new_session(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        // Build the `tmux new-session` command identically to LocalTmux but
        // shell-escape arguments since we're sending a single script string.
        let mut script = String::from("tmux new-session -d");
        script.push_str(&format!(" -s {}", quote(name)));
        script.push_str(&format!(" -c {}", quote(&cwd.to_string_lossy())));
        // Forward env explicitly — remote sshd typically doesn't pass LANG.
        script.push_str(" -e COLORTERM=truecolor -e TERM=xterm-256color");
        script.push_str(&format!(
            " -e LANG={}",
            quote(&std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()))
        ));
        if let Some(tc) = self.client.toolchain(&self.host).await {
            script.push_str(&format!(" -e PATH={}", quote(&tc.path)));
        }
        script.push(' ');
        script.push_str(&quote(pane_cmd));
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn run_script(&self, script: &str) -> Result<std::process::Output, IpcError> {
        self.remote_sh(script).await
    }

    async fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        let script = format!("tmux kill-session -t {}", quote(&exact_session(name)));
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        let trimmed = new.trim();
        if trimmed.is_empty() {
            return Err(IpcError::new(
                codes::E_TMUX,
                "new session name must not be empty",
            ));
        }
        if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':' || c == '#') {
            return Err(IpcError::new(
                codes::E_TMUX,
                "tmux session name must not contain whitespace, `.`, `:` or `#`",
            ));
        }
        if trimmed == old {
            return Ok(());
        }
        let script = format!(
            "tmux rename-session -t {} {}",
            quote(&exact_session(old)),
            quote(trimmed)
        );
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn restart_session(&self, name: &str, pane_cmd: &str) -> Result<(), IpcError> {
        let script = format!(
            "tmux respawn-pane -k -t {} {}",
            quote(&exact_pane(name)),
            quote(pane_cmd)
        );
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn respawn_pane_in(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        let script = respawn_pane_in_script(name, cwd, pane_cmd);
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn capture_pane(&self, name: &str) -> Result<String, IpcError> {
        let script = format!("tmux capture-pane -t {} -p", quote(&exact_pane(name)));
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError> {
        let start = scrollback_start(lines);
        let script = format!(
            "tmux capture-pane -t {} -S {} -p",
            quote(&exact_pane(name)),
            quote(&start),
        );
        let output = self.remote_sh(&script).await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
    async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> {
        let output = self
            .remote_sh("claude agents --json 2>/dev/null")
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(crate::claude_agents::parse_claude_agents_json(
            &String::from_utf8_lossy(&output.stdout),
        ))
    }
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        let Some(script) = transcript_mtimes_script(ids) else {
            return Some(std::collections::HashMap::new());
        };
        // `remote_sh` bounds the call (its timeout surfaces as `Err`); an
        // unreachable host is ssh exiting 255 — both are failures, not "no
        // transcript".
        match self.remote_sh(&script).await {
            Ok(o) if o.status.success() => Some(parse_mtimes(&String::from_utf8_lossy(&o.stdout))),
            _ => None,
        }
    }
    async fn read_oauth_account(&self) -> Option<crate::service::hosts::OauthAccount> {
        // Same script section `add_host`'s probe runs, on its own. The
        // script itself never fails (`|| true`), so a non-zero exit is ssh
        // (unreachable / timeout) — "could not tell", not "logged out".
        let output = self
            .remote_sh(crate::service::hosts::OAUTH_ACCOUNT_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        crate::service::hosts::parse_oauth_account(&String::from_utf8_lossy(&output.stdout))
    }
    async fn host_identity(&self) -> Option<HostIdentity> {
        let out = self
            .remote_sh(HOST_IDENTITY_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        parse_host_identity(&String::from_utf8_lossy(&out.stdout))
    }
    async fn host_versions(&self) -> Option<HostVersions> {
        let out = self
            .remote_sh(HOST_VERSIONS_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some(parse_host_versions(&String::from_utf8_lossy(&out.stdout)))
    }
    async fn host_health(&self) -> Option<HostHealthSample> {
        let out = self
            .remote_sh(HOST_HEALTH_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some(parse_host_health(&String::from_utf8_lossy(&out.stdout)))
    }
    async fn read_profiles(&self) -> Option<Vec<HostProfile>> {
        let out = self
            .remote_sh(&profiles_script())
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some(parse_profiles(&String::from_utf8_lossy(&out.stdout)))
    }
    async fn probe_snapshot(&self, tail_lines: u32, want_versions: bool) -> ProbeSnapshot {
        let script = probe_snapshot_script(tail_lines, want_versions);
        match self.remote_sh(&script).await {
            Err(e) => ProbeSnapshot {
                identity: None,
                sessions: Err(e),
                account: None,
                pane_tails: Default::default(),
                versions: None,
                health: None,
                profiles: None,
            },
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                match parse_probe_snapshot(&text) {
                    Ok(snap) => snap,
                    Err(e) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let why = if out.status.success() {
                            e.message
                        } else {
                            format!("{} ({})", stderr.trim(), e.message)
                        };
                        ProbeSnapshot {
                            identity: None,
                            sessions: Err(IpcError::new(codes::E_SSH, why)),
                            account: None,
                            pane_tails: Default::default(),
                            versions: None,
                            health: None,
                            profiles: None,
                        }
                    }
                }
            }
        }
    }
}

/// A key `send_prompt { keys }` may press. Closed on purpose: tmux's key
/// names are a small language of their own and "press whatever you like"
/// would be a second way to type into a pane, unmarked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Escape,
    /// Moves a multi-select question's cursor to its next option, and from
    /// the last one onto the dialog's `Submit` row (where a further `Tab` does
    /// nothing), so a client can reach Submit without knowing the cursor.
    Tab,
    CtrlC,
    /// The four arrows: a select dialog's cursor, the REPL's history and
    /// its line editing. The phone's key bar (redesign 14.14).
    Up,
    Down,
    Left,
    Right,
    /// Shift-Tab (tmux `BTab`): Claude Code's mode switch.
    BackTab,
    /// Ctrl plus a letter from [`CtrlKey`]'s closed list: the REPL's own
    /// shortcuts (C-r history, C-o transcript, C-l clear, C-u / C-k / C-w
    /// line editing, …). `C-c` stays [`NamedKey::CtrlC`].
    Ctrl(CtrlKey),
    /// One of `1`..`9` — the keystroke that answers a numbered permission /
    /// question dialog. Typing the ordinal as *text* would not do: the text
    /// path pastes through `paste-buffer -p`, and the REPL has bracketed
    /// paste on (DECSET 2004), so the pane receives
    /// `ESC [ 2 0 0 ~ 3 ESC [ 2 0 1 ~` — the first key the dialog sees is
    /// ESC, which cancels it. `send-keys 3` delivers one raw `3`.
    Digit(DigitKey),
}

/// An ordinal a dialog can be answered with: `1`..`9`, and nothing else.
///
/// A dialog may carry up to `PENDING_OPTIONS_MAX` (16) options, but the REPL
/// has no keystroke for a two-digit ordinal — there is no way to press "10"
/// that a select dialog will not read as "1". So option 10 and up simply has
/// no key, and [`new`](Self::new) is the only way in: the field is private,
/// which is what makes the `1..=9` indexing in [`NamedKey::tmux_name`] sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DigitKey(u8);

const DIGIT_NAMES: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// A Ctrl+letter a pane may be sent: every letter but the ones that are
/// another key under a second name or that stop the pane rather than the
/// REPL. Left out: `C-c` (it is [`NamedKey::CtrlC`]), `C-i` / `C-j` / `C-m`
/// (Tab and Enter under other names, which would side-step the rules those
/// keys carry), `C-s` / `C-q` (XON/XOFF: `C-s` can freeze a terminal until a
/// `C-q` nobody knows to send) and `C-z` (suspends the foreground process,
/// Claude Code with it, to a shell prompt). Private field, so
/// [`new`](Self::new) is the only way in and every value is in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtrlKey(usize);

/// The tmux names of the Ctrl keys [`CtrlKey`] admits, in alphabet order.
pub const CTRL_NAMES: [&str; 19] = [
    "C-a", "C-b", "C-d", "C-e", "C-f", "C-g", "C-h", "C-k", "C-l", "C-n", "C-o", "C-p", "C-r",
    "C-t", "C-u", "C-v", "C-w", "C-x", "C-y",
];

impl CtrlKey {
    /// `Some` for a name in [`CTRL_NAMES`], exactly as spelled there.
    pub fn new(name: &str) -> Option<Self> {
        CTRL_NAMES.iter().position(|n| *n == name).map(Self)
    }

    pub fn tmux_name(self) -> &'static str {
        CTRL_NAMES[self.0]
    }
}

impl DigitKey {
    /// `Some` for `1..=9`, `None` for anything else.
    pub fn new(n: u8) -> Option<Self> {
        (1..=9).contains(&n).then_some(Self(n))
    }

    /// The ordinal, for a caller that needs the number back.
    pub fn get(self) -> u8 {
        self.0
    }
}

impl NamedKey {
    /// Every accepted value, in the words a refusal shows the caller. Both
    /// `send_prompt` paths (the service function and the MCP tool) print
    /// this, so the message can never fall behind [`parse`](Self::parse).
    pub const VOCABULARY: &'static str = "Enter, Escape, Tab, BTab, Up, Down, Left, Right, C-c, \
         C-a/b/d/e/f/g/h/k/l/n/o/p/r/t/u/v/w/x/y or a digit 1-9";

    /// Every key name [`parse`](Self::parse) accepts, digits included: what
    /// the `keys` argument's schema enumerates, so a client can tell a hub
    /// that takes the arrows and Ctrl keys from one that does not.
    pub fn all_names() -> Vec<&'static str> {
        let mut v = vec![
            "Enter", "Escape", "Tab", "BTab", "Up", "Down", "Left", "Right", "C-c",
        ];
        v.extend(CTRL_NAMES);
        v.extend(DIGIT_NAMES);
        v
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Enter" => Some(Self::Enter),
            "Escape" => Some(Self::Escape),
            "Tab" => Some(Self::Tab),
            "C-c" => Some(Self::CtrlC),
            "Up" => Some(Self::Up),
            "Down" => Some(Self::Down),
            "Left" => Some(Self::Left),
            "Right" => Some(Self::Right),
            "BTab" => Some(Self::BackTab),
            _ if s.starts_with("C-") => CtrlKey::new(s).map(Self::Ctrl),
            // Exactly one ASCII digit. `str::parse::<u8>` would accept
            // "+1", " 1" and "007"; a dialog answer must be the literal
            // keystroke or nothing.
            _ => match s.as_bytes() {
                [b @ b'0'..=b'9'] => DigitKey::new(b - b'0').map(Self::Digit),
                _ => None,
            },
        }
    }
    pub fn tmux_name(self) -> &'static str {
        match self {
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::CtrlC => "C-c",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::BackTab => "BTab",
            Self::Ctrl(k) => k.tmux_name(),
            // Sound by construction: `DigitKey`'s field is private and
            // `DigitKey::new` admits only 1..=9.
            Self::Digit(d) => DIGIT_NAMES[(d.get() - 1) as usize],
        }
    }
}

/// `tmux send-keys -t '=<session>:' <Key>` — one named key, no literal text.
/// The target is an EXACT pane target (`exact_pane`, the same one
/// `service::sessions::prompt::build_send_script` falls back to for text): a bare
/// `-t NAME` is a *lookup* that falls back to a unique prefix or an fnmatch
/// pattern, so a key aimed at a dead/renamed session could otherwise land on
/// an unrelated one whose name merely starts with it.
pub fn send_named_key(tmux_name: &str, key: NamedKey) -> String {
    format!(
        "tmux send-keys -t {} {}",
        quote(&exact_pane(tmux_name)),
        key.tmux_name()
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TmuxSession {
    pub name: String,
    pub created: i64,
    pub last_activity: i64,
    pub attached: bool,
    pub path: PathBuf,
    /// The session's active pane (`%N`), used to bind hook calls to the row.
    /// `None` from an old format / a test fixture.
    pub pane_id: Option<String>,
}

/// Lists tmux sessions on the local host. Returns an empty Vec (not an error)
/// when the tmux server isn't running.
pub async fn list_local_sessions() -> Result<Vec<TmuxSession>, IpcError> {
    let output = crate::proc::command("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}|#{pane_id}",
        ])
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            parse_sessions_checked(&stdout)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if is_no_server_running(&stderr) {
                Ok(Vec::new())
            } else {
                Err(IpcError::new(codes::E_TMUX, stderr.trim()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(IpcError::new(
            codes::E_TMUX,
            "tmux binary not found on PATH",
        )),
        Err(e) => Err(IpcError::new(
            codes::E_TMUX,
            format!("spawn tmux failed: {e}"),
        )),
    }
}

/// True for any tmux stderr that means "no server is running on this socket"
/// — i.e. an empty `list_local_sessions()` return rather than an error.
///
/// Variants observed:
/// - "no server running on /tmp/tmux-501/default"  (server was started then exited)
/// - "error connecting to /private/tmp/tmux-501/default (No such file or directory)"
///   (no server has ever been started — the socket file doesn't exist)
fn is_no_server_running(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("no server running")
        || (s.contains("error connecting to") && s.contains("no such file or directory"))
}

fn parse_sessions(input: &str) -> Vec<TmuxSession> {
    input
        .lines()
        .filter_map(|line| {
            // Destructure the fixed format off the split iterator — no
            // per-line `Vec` allocation. The optional 6th field is the pane
            // id (`%N`); anything else there, or a 7th field, means the line
            // is malformed (a `|` inside a name or path); reject it.
            let mut it = line.split('|');
            let name = it.next()?;
            let created = it.next()?.parse::<i64>().ok()?;
            let last_activity = it.next()?.parse::<i64>().ok()?;
            let attached_int = it.next()?.parse::<i64>().ok()?;
            let path = it.next()?;
            let pane_id = match it.next() {
                None => None,
                Some(p) if p.starts_with('%') => Some(p.to_string()),
                Some(_) => return None,
            };
            if it.next().is_some() {
                return None;
            }
            Some(TmuxSession {
                name: name.to_string(),
                created,
                last_activity,
                attached: attached_int > 0,
                path: PathBuf::from(path),
                pane_id,
            })
        })
        .collect()
}

/// `parse_sessions` for a SUCCESSFUL `list-sessions`, refusing output that
/// has content but not one parseable line (a tmux wrapper/alias, a login
/// banner, a format mismatch). Treating that as "no sessions" would ghost,
/// then delete, every row on the host; an `E_TMUX` makes the reconcile count
/// the host unreachable for this pass instead. Blank output and "no server
/// running" still mean zero sessions.
#[cfg(test)]
pub(crate) fn parse_sessions_for_test(input: &str) -> Result<Vec<TmuxSession>, IpcError> {
    parse_sessions_checked(input)
}

fn parse_sessions_checked(input: &str) -> Result<Vec<TmuxSession>, IpcError> {
    let mut sessions = parse_sessions(input);
    let mut lines = input.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return Ok(sessions);
    };
    if !sessions.is_empty() || is_no_server_running(input) {
        // Shell terminals (step 5.3) leave AFTER the "nothing parsed"
        // check: a host running only terminals parsed fine, it just has no
        // session for the list.
        sessions.retain(|s| !is_shell_terminal_name(&s.name));
        return Ok(sessions);
    }
    let sample: String = first.chars().take(80).collect();
    Err(IpcError::new(
        codes::E_TMUX,
        format!("unparseable tmux list-sessions output: {sample:?}"),
    ))
}

/// tmux `-S` start offset for `lines` rows of scrollback (a negative count).
pub(crate) fn scrollback_start(lines: u32) -> String {
    format!("-{lines}")
}

/// Inline stand-in for `cl`, defined only when no `cl` is on PATH. The pane
/// runs in the environment of the tmux CLIENT that created the session —
/// for the hub that is a non-interactive `bash -lc` over SSH — so a `cl`
/// that only exists in interactive shells (a zsh alias, a `.zshrc`-only
/// PATH entry) is simply not there. POSIX `sh` syntax: tmux runs the pane
/// command under `default-shell -c`, whatever that shell is.
///
/// Preference order: the user's own `cl`; else fleet's provisioned `ag`
/// launcher at its absolute install path (F2 — not bare `ag`, which may be
/// The Silver Searcher, and `~/.local/bin` may be missing from the cached
/// toolchain PATH), as `ag claude --yolo`; else plain
/// `claude --dangerously-skip-permissions`. All three take the same flags
/// (`--resume`, `--session-id`, `--continue`, `--name`, `--model`,
/// `--effort`), so the chain below is identical whichever one runs.
pub(crate) const CL_FALLBACK: &str = r#"if ! command -v cl >/dev/null 2>&1; then if [ -x "$HOME/.local/share/ag/ag" ]; then cl() { "$HOME/.local/share/ag/ag" claude --yolo "$@"; }; else cl() { claude --dangerously-skip-permissions "$@"; }; fi; fi;"#;

/// Makes `$CLAUDE_CONFIG_DIR` a credential profile that shares everything
/// with `~/.claude` except the login (docs/accounts.md). Each visible entry
/// of `~/.claude` (`projects/`, `settings.json` with fleet's hooks, skills,
/// …) is symlinked in unless the profile already has its own; the dotfiles
/// are not, so `.credentials.json` and the profile's `.claude.json` (its
/// `/login` account) stay the profile's own. Fleet reads transcripts from
/// `~/.claude/projects` and a resumed session must find its transcript
/// under any profile, so `projects/` is created first. Idempotent: run on
/// every launch, it also links an entry `~/.claude` gained since. Run by
/// `/bin/sh`, so a zsh pane shell's no-match glob error cannot abort it.
pub const PROFILE_LINKS: &str = r#"umask 077; mkdir -p "$HOME/.claude/projects" "$CLAUDE_CONFIG_DIR" || exit 0; for f in "$HOME"/.claude/*; do [ -e "$f" ] || continue; b="${f##*/}"; [ -e "$CLAUDE_CONFIG_DIR/$b" ] || [ -L "$CLAUDE_CONFIG_DIR/$b" ] || ln -s "$f" "$CLAUDE_CONFIG_DIR/$b"; done"#;

/// Puts fleet's `/voice` recorder ahead of any real `arecord` (docs/voice.md).
pub(crate) const VOICE_PATH_PREFIX: &str = "PATH=\"$HOME/.claude-fleet/voice/bin:$PATH\"; ";

/// The pane command for a Claude ("work"/"review") session. With a known
/// session id: resume it, else create it under that id, else a bare `cl` — an
/// idempotent create-or-resume. Without an id (legacy rows): today's
/// most-recent-for-cwd behavior. The id is single-quoted; externally-supplied
/// ids should be validated with `validate::claude_session_id` before being
/// passed in (minted ids are safe by construction).
///
/// Every `cl` also gets `--name <tmux_name>`, so `claude agents` lists the
/// session under its tmux name and reconcile pairs it with its agent BY NAME
/// (authoritative) instead of inferring it from the cwd, which is ambiguous
/// once two sessions share a directory.
///
/// The command starts with [`VOICE_PATH_PREFIX`] (fleet's `/voice`
/// recorder first on PATH), then [`CL_FALLBACK`]: the user's own `cl` wins
/// when it is on PATH, otherwise fleet's provisioned `ag` launcher, else an
/// inline `claude --dangerously-skip-permissions`, stands in. Without it a
/// host whose `cl` lives only in interactive shells printed "command not
/// found: cl" and dropped straight to the login shell.
pub fn pane_command_for(claude_session_id: Option<&str>, tmux_name: &str) -> String {
    pane_command_with(claude_session_id, tmux_name, &ClaudeLaunch::default())
}

/// Per-session `claude` launch options a new session was asked for. Values
/// must already have passed `validate::claude_model` / `validate::effort_level`;
/// they are shell-quoted here all the same.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClaudeLaunch {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The credential profile the session runs under: `CLAUDE_CONFIG_DIR`
    /// is `~/.claude-profiles/<profile>` ([`PROFILE_LINKS`]). `None` = the
    /// host's own login. Must have passed `validate::claude_profile`.
    pub profile: Option<String>,
}

impl ClaudeLaunch {
    /// Stored launch options, each kept only when it still validates: a
    /// value read back from the DB never reaches the shell unchecked (an
    /// invalid one degrades to the host's default, like a bad claude id).
    pub fn checked(model: Option<String>, effort: Option<String>, profile: Option<String>) -> Self {
        Self {
            model: model.filter(|m| crate::validate::claude_model(m).is_ok()),
            effort: effort.filter(|e| crate::validate::effort_level(e).is_ok()),
            profile: profile.filter(|p| crate::validate::claude_profile(p).is_ok()),
        }
    }

    /// `export CLAUDE_CONFIG_DIR=…; <PROFILE_LINKS>` for a session with a
    /// profile; empty otherwise. Exported, not `-e` on the tmux session, so
    /// a restart (`respawn-pane`, which takes no environment), a repair or
    /// a move rebuilds it from the stored profile like `--model`.
    fn profile_prefix(&self) -> String {
        match self.profile.as_deref() {
            Some(p) => format!(
                "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"{}; /bin/sh -c {} 2>/dev/null; ",
                crate::shell::quote(p),
                crate::shell::quote(PROFILE_LINKS),
            ),
            None => String::new(),
        }
    }

    /// ` --model 'm' --effort 'e'`, each only when set; empty otherwise.
    fn flags(&self) -> String {
        let mut out = String::new();
        if let Some(m) = self.model.as_deref() {
            out.push_str(&format!(" --model {}", crate::shell::quote(m)));
        }
        if let Some(e) = self.effort.as_deref() {
            out.push_str(&format!(" --effort {}", crate::shell::quote(e)));
        }
        out
    }
}

/// [`pane_command_for`] with launch options: every `cl` in the chain gets
/// the same `--model` / `--effort`, so which branch runs makes no difference.
pub fn pane_command_with(
    claude_session_id: Option<&str>,
    tmux_name: &str,
    launch: &ClaudeLaunch,
) -> String {
    let tail = "exec ${SHELL:-/bin/zsh} -l";
    let name = format!(
        "--name {}{}",
        crate::shell::quote(tmux_name),
        launch.flags()
    );
    let profile = launch.profile_prefix();
    match claude_session_id {
        Some(id) => {
            let id = crate::shell::quote(id);
            format!(
                "{VOICE_PATH_PREFIX}{profile}{CL_FALLBACK} cl --resume {id} {name} 2>/dev/null || cl --session-id {id} {name} || cl {name}; {tail}"
            )
        }
        None => format!(
            "{VOICE_PATH_PREFIX}{profile}{CL_FALLBACK} cl --continue {name} || cl {name}; {tail}"
        ),
    }
}

/// Pane command for a plain shell session (`kind = "shell"`). Runs an
/// interactive login shell in a loop so the pane — and therefore the tmux
/// session — survives the user typing `exit`; a fresh shell respawns instead.
///
/// When `start_command` is given, it runs first (in the session's cwd, with
/// the env tmux injected via `-e`), its exit code is printed, and then the
/// pane drops to the respawning interactive shell — so the output stays
/// visible. `start_command` is the user's raw text; the remote transport
/// layer (`quote`) escapes the whole pane command as one shell word.
pub fn shell_pane_command(start_command: Option<&str>) -> String {
    let respawn = "while :; do ${SHELL:-/bin/zsh} -l; done";
    match start_command {
        Some(cmd) if !cmd.trim().is_empty() => {
            format!("{{ {cmd}; }}; printf '\\n[exit %s]\\n' \"$?\"; {respawn}")
        }
        _ => respawn.to_string(),
    }
}

pub async fn new_session(
    name: &str,
    working_dir: &std::path::Path,
    pane_cmd: &str,
) -> Result<(), IpcError> {
    // Push env into the session explicitly via `-e KEY=VAL`. This matters
    // because the tmux SERVER may already be running with stale env (e.g.
    // started before claude-fleet imported the user's locale from their
    // login shell). `-e` overrides the server env for processes started
    // in this session, so the spawned `cl`/`bash` reliably sees UTF-8.
    let mut cmd = crate::proc::command("tmux");
    cmd.args([
        "new-session",
        "-d",
        "-s",
        name,
        "-c",
        &working_dir.to_string_lossy(),
    ]);
    for var in ["LANG", "LC_ALL", "LC_CTYPE", "PATH"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.args(["-e", &format!("{var}={val}")]);
            }
        }
    }
    cmd.args(["-e", "COLORTERM=truecolor", "-e", "TERM=xterm-256color"]);
    cmd.arg(pane_cmd);
    let output = cmd
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// Rename an existing tmux session. New name must follow tmux's naming rules
/// (no `.`, `:`, or whitespace; non-empty). Validation here keeps the caller
/// from getting a cryptic tmux error.
pub async fn rename_session(old: &str, new: &str) -> Result<(), IpcError> {
    let trimmed = new.trim();
    if trimmed.is_empty() {
        return Err(IpcError::new(
            codes::E_TMUX,
            "new session name must not be empty",
        ));
    }
    if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':' || c == '#') {
        return Err(IpcError::new(
            codes::E_TMUX,
            "tmux session name must not contain whitespace, `.`, `:` or `#`",
        ));
    }
    if trimmed == old {
        return Ok(()); // no-op
    }
    let output = crate::proc::command("tmux")
        .args(["rename-session", "-t", &exact_session(old), trimmed])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// Restart the pane's process by killing claude (or whatever's running) and
/// respawning with the same command the session was created with. Uses
/// `respawn-pane -k` so we don't need to know if claude is currently running
/// or already dropped to shell.
pub async fn restart_session(name: &str, pane_cmd: &str) -> Result<(), IpcError> {
    let output = crate::proc::command("tmux")
        .args(["respawn-pane", "-k", "-t", &exact_pane(name), pane_cmd])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// The remote form of [`respawn_pane_in`]: one shell word per argument.
pub(crate) fn respawn_pane_in_script(name: &str, cwd: &std::path::Path, pane_cmd: &str) -> String {
    format!(
        "tmux respawn-pane -k -c {} -t {} {}",
        quote(&cwd.to_string_lossy()),
        quote(&exact_pane(name)),
        quote(pane_cmd)
    )
}

/// `restart_session` with an explicit start directory. Used by the workspace
/// repair path when the pane's cwd was deleted (or just recreated under it —
/// a process keeps the dead inode as its cwd until it is respawned).
pub async fn respawn_pane_in(
    name: &str,
    cwd: &std::path::Path,
    pane_cmd: &str,
) -> Result<(), IpcError> {
    let output = crate::proc::command("tmux")
        .args([
            "respawn-pane",
            "-k",
            "-c",
            &cwd.to_string_lossy(),
            "-t",
            &exact_pane(name),
            pane_cmd,
        ])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

pub async fn kill_session(name: &str) -> Result<(), IpcError> {
    let output = crate::proc::command("tmux")
        .args(["kill-session", "-t", &exact_session(name)])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// Writing a fake executable that a test is about to run, without the
/// `ETXTBSY` race that turned `main` red twice (code-review round 20, F1).
///
/// `fs::write` + `set_permissions` + exec is not safe in a multi-threaded
/// test binary: another test's `fork()` landing between the `open(O_WRONLY)`
/// and the `close()` inherits the write fd, and an exec of that inode then
/// fails with "Text file busy". `O_CLOEXEC` does not help — the fd is closed
/// *at* exec, which is exactly when the kernel checks for writers.
///
/// [`write_exec`] closes the window in three steps, and the third is the one
/// that actually closes it:
///
///  1. the body goes to a sibling path and is `rename`d into place, so the
///     final name never names a half-written or not-yet-`chmod`ded file;
///  2. the `File` is dropped — closed — *before* the rename, so this thread
///     holds no writer by the time the final name exists;
///  3. the file is then exec'd once with `--fleet-probe`, retrying while that
///     exec reports `ETXTBSY`. Steps 1–2 alone do NOT close the race:
///     `rename` keeps the inode, so a child forked during the write still
///     holds a writer on the very inode the final name now points at. A
///     *successful* exec is the only available proof that no writer is left —
///     and none can appear afterwards, because nothing opens the file again.
///
/// Every body written through this module must therefore start with
/// [`PROBE_GUARD`], which answers the probe before any of the body's own side
/// effects run (several fakes count their invocations, and a test asserts the
/// exact count).
///
/// Only Linux enforces this on `exec` (the inode's writer count); Darwin does
/// not, so the flake never reproduces on a developer's Mac and both red runs
/// were on the Linux runners (`rust (ubuntu-24.04)` and `hub-headless`). A
/// green local suite says nothing about it — step 3 is what makes it safe by
/// construction rather than by luck.
///
/// Lives in `tmux.rs` rather than in a module of its own so the crate's other
/// fakes can share it without adding a file to the crate root: `ssh.rs`,
/// `claude_cli.rs`, `service/account_usage.rs`, `service/add_project.rs` and
/// `service/move_session/carry.rs` all write their exec'd stubs through it.
/// Any new fake the test process itself spawns belongs here too.
///
/// Unix only: the fakes are `sh` scripts exec'd by path, so a test that needs
/// one is a Unix test.
#[cfg(unix)]
#[cfg(test)]
pub(crate) mod fake_exec {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// The shell line that answers [`write_exec`]'s probe and nothing else.
    /// No test passes `--fleet-probe` itself.
    pub(crate) const PROBE_GUARD: &str = "case \"$1\" in --fleet-probe) exit 0;; esac\n";

    /// `ETXTBSY` — 26 on both Linux and macOS, the only two platforms this
    /// crate's tests run on.
    const ETXTBSY: i32 = 26;

    /// Write `body` to `dir/name` as a 0755 file that is safe to exec
    /// immediately, and hand back its path. `body` must carry
    /// [`PROBE_GUARD`] ahead of anything with a side effect.
    pub(crate) fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        assert!(
            body.contains(PROBE_GUARD),
            "a fake exec must carry the probe guard: {body}"
        );
        let path = dir.join(name);
        let tmp = dir.join(format!(".{name}.fleet-tmp"));
        {
            let mut f = std::fs::File::create(&tmp).expect("create the fake");
            f.write_all(body.as_bytes()).expect("write the fake");
            f.set_permissions(std::fs::Permissions::from_mode(0o755))
                .expect("chmod the fake");
            f.sync_all().expect("fsync the fake");
            // Explicit, because it is load-bearing: the handle is closed
            // here, before the rename below publishes the name.
            drop(f);
        }
        std::fs::rename(&tmp, &path).expect("publish the fake");
        wait_until_executable(&path);
        path
    }

    /// Exec `path` with `--fleet-probe` until the exec is not refused with
    /// `ETXTBSY` any more. Returning means the inode has no writer left.
    fn wait_until_executable(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match std::process::Command::new(path)
                .arg("--fleet-probe")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(_) => return,
                Err(e) if e.raw_os_error() == Some(ETXTBSY) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{} never became executable: {e}", path.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_text(
        sessions_rc: i32,
        sessions: &str,
        account: &str,
        panes: &[(&str, &str)],
    ) -> String {
        let mut s = String::new();
        s.push_str("---FLEET:identity\nboot=abc-123\ntmuxrc=0\ntmuxout=4242\n");
        s.push_str(&format!(
            "---FLEET:sessions\nrc={sessions_rc}\n{sessions}\n"
        ));
        s.push_str(&format!("---FLEET:account\n{account}\n"));
        s.push_str("---FLEET:panes\n");
        for (name, tail) in panes {
            s.push_str(&format!("---FLEET:pane {name}\n{tail}\n"));
        }
        s.push_str("---FLEET:end\n");
        s
    }

    #[test]
    fn probe_snapshot_parses_every_section() {
        let text = snapshot_text(
            0,
            "dev-a|1700000000|1700000100|0|/home/u/p|%3\ndev-b|1700000000|1700000200|1|/home/u/q|%7",
            r#"{"accountUuid":"u-1","emailAddress":"a@b.c"}"#,
            &[("dev-a", "❯ \n? for shortcuts"), ("dev-b", "Thinking… (3s · esc to interrupt)")],
        );
        let snap = parse_probe_snapshot(&text).unwrap();
        let sessions = snap.sessions.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev-a");
        assert_eq!(snap.identity.unwrap().tmux_server_pid, Some(4242));
        assert_eq!(snap.account.unwrap().uuid.as_deref(), Some("u-1"));
        assert_eq!(
            snap.pane_tails["dev-b"],
            "Thinking… (3s · esc to interrupt)"
        );
        assert_eq!(snap.pane_tails.len(), 2);
    }

    #[test]
    fn probe_snapshot_reads_no_server_running_as_zero_sessions() {
        let text = snapshot_text(1, "no server running on /tmp/tmux-501/default", "{}", &[]);
        let snap = parse_probe_snapshot(&text).unwrap();
        assert_eq!(snap.sessions.unwrap().len(), 0);
        assert!(snap.account.is_none());
        assert!(snap.pane_tails.is_empty());
    }

    #[test]
    fn probe_snapshot_refuses_garbage_and_truncation() {
        let text = snapshot_text(0, "this is not a session line", "{}", &[]);
        assert!(
            parse_probe_snapshot(&text).unwrap().sessions.is_err(),
            "garbage must not read as zero sessions"
        );
        let mut truncated = snapshot_text(0, "", "{}", &[]);
        truncated.truncate(truncated.len() - "---FLEET:end\n".len());
        let e = parse_probe_snapshot(&truncated).unwrap_err();
        assert_eq!(e.code, "E_TMUX");
        assert!(e.message.contains("truncated"), "{}", e.message);
        let e =
            parse_probe_snapshot("ssh: connect to host h port 22: No route to host").unwrap_err();
        assert_eq!(e.code, "E_TMUX");
    }

    #[test]
    fn probe_snapshot_keeps_an_escaped_delimiter_inside_a_pane() {
        let text = snapshot_text(
            0,
            "dev-a|1|2|0|/p|%1",
            "{}",
            &[("dev-a", " ---FLEET:panes is just text\nline2")],
        );
        let snap = parse_probe_snapshot(&text).unwrap();
        assert_eq!(
            snap.pane_tails["dev-a"],
            " ---FLEET:panes is just text\nline2"
        );
    }

    #[test]
    fn probe_snapshot_unescapes_a_delimiter_named_session() {
        // A live session literally named `---FLEET:evil` comes back from
        // the script with the sed escape already applied (one leading
        // space) — the parser must undo it before handing the line to
        // `parse_sessions_checked`, and the section boundary that follows
        // must still be found (the escape is what makes that possible in
        // the first place).
        let text = snapshot_text(
            0,
            " ---FLEET:evil|1|2|0|/p|%9",
            r#"{"accountUuid":"u-9"}"#,
            &[],
        );
        let snap = parse_probe_snapshot(&text).unwrap();
        let sessions = snap.sessions.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "---FLEET:evil");
        assert_eq!(
            snap.account.unwrap().uuid.as_deref(),
            Some("u-9"),
            "the section boundary after the escaped session line must still be found"
        );
    }

    #[test]
    fn probe_snapshot_script_has_every_section_and_escapes_pane_lines() {
        let s = probe_snapshot_script(8, true);
        for section in [
            "---FLEET:identity",
            "---FLEET:versions",
            "---FLEET:health",
            "---FLEET:sessions",
            "---FLEET:account",
            "---FLEET:panes",
            "---FLEET:end",
        ] {
            assert!(
                s.contains(&format!("printf '%s\\n' '{section}'")),
                "{section} missing in {s}"
            );
        }
        assert!(s.contains(HOST_IDENTITY_SCRIPT));
        assert!(s.contains(HOST_VERSIONS_SCRIPT));
        assert!(s.contains(HOST_HEALTH_SCRIPT));
        assert!(s.contains(crate::service::hosts::OAUTH_ACCOUNT_SCRIPT));
        // A pass that is not due for versions leaves the section (and its
        // `claude --version` node start) out entirely.
        let without = probe_snapshot_script(50, false);
        assert!(!without.contains("---FLEET:versions"), "{without}");
        assert!(!without.contains(HOST_VERSIONS_SCRIPT), "{without}");
        assert!(
            s.contains(
                "tmux capture-pane -t \"=$s:\" -S -8 -p 2>/dev/null | sed 's/^---FLEET/ &/'"
            ),
            "{s}"
        );
        // A live session literally named `---FLEET:evil` must not forge a
        // section boundary either — the session list is escaped the same
        // way the pane captures are.
        assert!(
            s.contains("printf '%s\\n' \"$out\" | sed 's/^---FLEET/ &/'"),
            "{s}"
        );
        assert!(s.contains("while IFS= read -r s; do"), "{s}");
    }

    #[test]
    fn scrollback_start_is_negative_lines() {
        assert_eq!(scrollback_start(120), "-120");
        assert_eq!(scrollback_start(0), "-0");
    }

    #[test]
    fn parse_two_sessions() {
        let input = "dev-foo|1716000000|1716100000|1|/repos/foo\ndev-bar|1716000100|1716200000|0|/repos/bar\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev-foo");
        assert!(sessions[0].attached);
        assert_eq!(sessions[0].path, PathBuf::from("/repos/foo"));
        assert_eq!(sessions[1].name, "dev-bar");
        assert!(!sessions[1].attached);
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let input = "good|1716000000|1716100000|1|/x\nbad-line-without-pipes\nempty||||\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "good");
    }

    #[test]
    fn parse_sessions_reads_an_optional_pane_id() {
        let out = "a|1|2|0|/w|%7\nb|1|2|1|/x\n";
        let s = parse_sessions(out);
        assert_eq!(s[0].pane_id.as_deref(), Some("%7"));
        assert_eq!(s[1].pane_id, None);
        // A 7th field is still a malformed line (a `|` in the name).
        assert!(parse_sessions("a|b|1|2|0|/w|%7").is_empty());
        // A 6th field that is not a pane id is a `|` inside the path.
        assert!(parse_sessions("a|1|2|0|/w|x").is_empty());
    }

    #[test]
    fn parse_empty_input() {
        assert!(parse_sessions("").is_empty());
    }

    #[test]
    fn checked_parse_accepts_blank_and_no_server_as_zero_sessions() {
        for blank in ["", "\n", "  \n\n"] {
            assert!(
                parse_sessions_checked(blank).unwrap().is_empty(),
                "{blank:?}"
            );
        }
        assert!(
            parse_sessions_checked("no server running on /tmp/tmux-1000/default\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn checked_parse_keeps_good_lines_among_noise() {
        let out = parse_sessions_checked("warning: x\ngood|1|2|0|/x\n").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "good");
    }

    #[test]
    fn checked_parse_rejects_content_with_no_parseable_line() {
        let err = parse_sessions_checked("Welcome to delta!\nsession list unavailable: ???\n")
            .unwrap_err();
        assert_eq!(err.code, "E_TMUX");
        assert!(err.message.contains("Welcome to delta!"), "{}", err.message);
        let long = "x".repeat(500);
        let err = parse_sessions_checked(&long).unwrap_err();
        assert!(err.message.len() < 200, "sample is truncated");
    }

    #[test]
    fn detects_no_server_running_classic() {
        assert!(is_no_server_running(
            "no server running on /tmp/tmux-501/default\n"
        ));
    }

    #[test]
    fn detects_socket_file_missing_macos() {
        // What `tmux list-sessions` actually prints on macOS when no server
        // was ever started — the user-reported bug.
        assert!(is_no_server_running(
            "error connecting to /private/tmp/tmux-501/default (No such file or directory)\n"
        ));
    }

    #[test]
    fn detects_socket_file_missing_case_insensitive() {
        assert!(is_no_server_running(
            "Error connecting to /tmp/tmux-501/default (No such file or directory)"
        ));
    }

    #[test]
    fn does_not_swallow_unrelated_errors() {
        assert!(!is_no_server_running("can't find session: dev-foo"));
        assert!(!is_no_server_running("ambiguous option"));
        assert!(!is_no_server_running(""));
    }

    #[test]
    fn pane_command_for_none_falls_back_to_shell_after_claude_exits() {
        let cmd = pane_command_for(None, "dev-x");
        // The semicolon (NOT `||`) after the second `cl` is the whole point:
        // it makes the shell always continue to the exec regardless of `cl`'s
        // exit status. Regression test that the next person who edits this
        // doesn't accidentally use `||` and resurrect the "session dies on
        // /exit" bug.
        assert!(
            cmd.contains("cl --continue --name 'dev-x' || cl --name 'dev-x';"),
            "got: {cmd}"
        );
        assert!(cmd.contains("exec ${SHELL:-/bin/zsh}"), "got: {cmd}");
    }

    #[test]
    fn shell_pane_command_respawns_shell_so_session_survives_exit() {
        let cmd = shell_pane_command(None);
        // The loop is the point: a shell session must NOT die when the user
        // types `exit` — a fresh login shell respawns instead.
        assert!(cmd.contains("while :;"), "got: {cmd}");
        assert!(cmd.contains("${SHELL:-/bin/zsh}"), "got: {cmd}");
    }

    #[test]
    fn shell_pane_command_runs_start_command_then_keeps_pane_alive() {
        let cmd = shell_pane_command(Some("cargo test"));
        // Start command runs, its exit code is printed, then the pane drops
        // to the same respawning shell so the output stays on screen.
        assert!(cmd.contains("{ cargo test; }"), "got: {cmd}");
        assert!(cmd.contains("[exit %s]"), "got: {cmd}");
        assert!(cmd.contains("while :;"), "got: {cmd}");
        // Blank / whitespace-only start command is treated as "no command".
        assert_eq!(shell_pane_command(Some("  ")), shell_pane_command(None));
    }

    #[tokio::test]
    async fn rename_rejects_whitespace_dots_colons_and_empty() {
        // Can't actually run tmux in unit tests; just exercise the validation
        // path. tmux command is never reached.
        assert!(rename_session("a", "").await.is_err());
        assert!(rename_session("a", "   ").await.is_err());
        assert!(rename_session("a", "has space").await.is_err());
        assert!(rename_session("a", "has.dot").await.is_err());
        assert!(rename_session("a", "has:colon").await.is_err());
    }

    #[test]
    fn pane_command_quotes_a_session_id_through_shq() {
        // An id is a UUID in practice, but the command must not depend on
        // that: a quote in it stays inside one shell word.
        let cmd = pane_command_for(Some("a'b"), "dev-x");
        let q = crate::shell::quote("a'b");
        assert!(
            cmd.contains(&format!("cl --resume {q} --name 'dev-x'")),
            "got: {cmd}"
        );
        assert!(
            cmd.contains(&format!("cl --session-id {q} --name 'dev-x'")),
            "got: {cmd}"
        );
        assert!(!cmd.contains("'a'b'"), "hand-quoted id: {cmd}");
    }

    #[test]
    fn pane_command_for_resumes_or_creates_with_id() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let cmd = pane_command_for(Some(id), "dev-x");
        assert!(
            cmd.contains(&format!("cl --resume '{id}' --name 'dev-x'")),
            "got: {cmd}"
        );
        assert!(
            cmd.contains(&format!("cl --session-id '{id}' --name 'dev-x'")),
            "got: {cmd}"
        );
        assert!(
            cmd.contains("|| cl --name 'dev-x';"),
            "bare fallback missing: {cmd}"
        );
        assert!(cmd.contains("exec ${SHELL"), "got: {cmd}");
    }

    #[test]
    fn respawn_pane_in_script_quotes_cwd_name_and_command() {
        let s = respawn_pane_in_script(
            "dev-x",
            std::path::Path::new("/re po/it's"),
            "cl; exec $SHELL",
        );
        assert_eq!(
            s,
            "tmux respawn-pane -k -c '/re po/it'\\''s' -t '=dev-x:' 'cl; exec $SHELL'"
        );
    }

    #[test]
    fn exact_targets_carry_the_no_lookup_prefix() {
        // Verified against tmux 3.6a: a SESSION target takes `=NAME`, a PANE
        // target needs the trailing `:` — `-t '=NAME'` fails there with
        // "can't find pane".
        assert_eq!(exact_session("dev-foo"), "=dev-foo");
        assert_eq!(exact_pane("dev-foo"), "=dev-foo:");
    }

    #[test]
    fn pane_command_for_defines_cl_fallback_before_first_use() {
        // Regression: `cl` is a convenience from the user's dotfiles. On macOS
        // it exists only in interactive zsh, and a tmux session created by the
        // hub over SSH inherits the ssh client's non-interactive PATH, where
        // `cl` is missing — the pane printed "command not found: cl" twice
        // and dropped to a login shell. The pane command must therefore carry
        // its own fallback (the documented `~/bin/cl` wrapper, inline).
        for cmd in [
            pane_command_for(None, "dev-x"),
            pane_command_for(Some("550e8400-e29b-41d4-a716-446655440000"), "dev-x"),
        ] {
            let def = cmd
                .find(CL_FALLBACK)
                .unwrap_or_else(|| panic!("fallback definition missing: {cmd}"));
            let first_use = cmd.find("cl --").expect("no cl invocation");
            assert!(
                def < first_use,
                "fallback must precede the first `cl`: {cmd}"
            );
            assert!(
                cmd.contains("claude --dangerously-skip-permissions \"$@\""),
                "fallback must forward all args to claude: {cmd}"
            );
        }
    }

    /// Run a pane command under a real shell with a fake `claude` on PATH and
    /// a fake `$SHELL` for the trailing `exec`, returning the argv lines the
    /// fake `claude` recorded. `with_cl` also puts a fake `cl` on PATH.
    #[cfg(unix)]
    fn run_pane_command(shell: &str, cmd: &str, with_cl: bool) -> Vec<String> {
        run_pane_command_opts(shell, cmd, with_cl, false)
    }

    /// [`run_pane_command`], optionally with a fake provisioned launcher at
    /// `$HOME/.local/share/ag/ag` (`$HOME` is the temp dir). Like the fake
    /// `claude`, it records its argv and fails a `--resume` so the chain
    /// has to reach `--session-id`.
    #[cfg(unix)]
    fn run_pane_command_opts(shell: &str, cmd: &str, with_cl: bool, with_ag: bool) -> Vec<String> {
        use super::fake_exec::{write_exec, PROBE_GUARD};
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        // Never `fs::write` + `set_permissions` here: see `fake_exec`'s doc
        // comment — that shape is what made this very test fail with an
        // empty argv log on CI (round 20, F1).
        let write = |name: &str, body: String| {
            write_exec(dir.path(), name, &body);
        };
        // `--resume` fails (no such conversation) so the chain has to reach
        // `--session-id`; everything else succeeds.
        write(
            "claude",
            format!(
                "#!/bin/sh\n{PROBE_GUARD}printf 'claude %s\\n' \"$*\" >> '{}'\ncase \"$*\" in *--resume*) exit 1;; esac\nexit 0\n",
                log.display()
            ),
        );
        if with_cl {
            write(
                "cl",
                format!(
                    "#!/bin/sh\n{PROBE_GUARD}printf 'cl %s\\n' \"$*\" >> '{}'\nexit 0\n",
                    log.display()
                ),
            );
        }
        if with_ag {
            let ag_dir = dir.path().join(".local/share/ag");
            std::fs::create_dir_all(&ag_dir).unwrap();
            write_exec(
                &ag_dir,
                "ag",
                &format!(
                    "#!/bin/sh\n{PROBE_GUARD}printf 'ag %s\\n' \"$*\" >> '{}'\ncase \"$*\" in *--resume*) exit 1;; esac\nexit 0\n",
                    log.display()
                ),
            );
        }
        // The trailing `exec ${SHELL:-/bin/zsh} -l` must terminate, not hang.
        write("fake-shell", format!("#!/bin/sh\n{PROBE_GUARD}exit 0\n"));
        let status = std::process::Command::new(shell)
            .arg("-c")
            .arg(cmd)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .env("HOME", dir.path())
            .env("SHELL", dir.path().join("fake-shell"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        // NOT `unwrap_or_default`: an absent or empty log means the fake
        // `claude` never ran at all, and the pane command's own
        // `a || b || c; exec $SHELL` shape hides that — every `cl`/`claude`
        // attempt can fail (126 on an ETXTBSY exec) and the trailing `exec
        // fake-shell` still exits 0, so the assert above passes and the
        // caller compares against an empty vec. Name the real cause here
        // instead of letting it masquerade as a fallback-logic bug.
        let logged = std::fs::read_to_string(&log).unwrap_or_else(|e| {
            panic!(
                "{shell}: no argv log at {} ({e}) — the fake `claude`/`cl` never ran, \
                 so this says nothing about the fallback chain",
                log.display()
            )
        });
        assert!(
            !logged.trim().is_empty(),
            "{shell}: the argv log at {} is empty — the fake `claude`/`cl` never ran, \
             so this says nothing about the fallback chain",
            log.display()
        );
        logged.lines().map(str::to_string).collect()
    }

    #[cfg(unix)]
    fn available_shells() -> Vec<&'static str> {
        ["/bin/sh", "bash", "zsh"]
            .into_iter()
            .filter(|s| {
                std::process::Command::new(s)
                    .arg("-c")
                    .arg("exit 0")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|st| st.success())
                    .unwrap_or(false)
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn pane_command_launches_claude_when_cl_is_not_on_path() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        for shell in available_shells() {
            let argv = run_pane_command(shell, &pane_command_for(Some(id), "dev-x"), false);
            assert_eq!(
                argv,
                vec![
                    format!("claude --dangerously-skip-permissions --resume {id} --name dev-x"),
                    format!("claude --dangerously-skip-permissions --session-id {id} --name dev-x"),
                ],
                "{shell}"
            );
            let argv = run_pane_command(shell, &pane_command_for(None, "dev-x"), false);
            assert_eq!(
                argv,
                vec!["claude --dangerously-skip-permissions --continue --name dev-x".to_string()],
                "{shell}"
            );
        }
    }

    /// F2: a host fleet provisioned has `ag` at `$HOME/.local/share/ag/ag`;
    /// with no `cl` on PATH the pane command goes through it, with `--yolo`
    /// (the same bypass as the plain-claude fallback).
    #[cfg(unix)]
    #[test]
    fn pane_command_uses_the_provisioned_ag_when_cl_is_missing() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        for shell in available_shells() {
            let argv =
                run_pane_command_opts(shell, &pane_command_for(Some(id), "dev-x"), false, true);
            assert_eq!(
                argv,
                vec![
                    format!("ag claude --yolo --resume {id} --name dev-x"),
                    format!("ag claude --yolo --session-id {id} --name dev-x"),
                ],
                "{shell}"
            );
            let argv = run_pane_command_opts(shell, &pane_command_for(None, "dev-x"), false, true);
            assert_eq!(
                argv,
                vec!["ag claude --yolo --continue --name dev-x".to_string()],
                "{shell}"
            );
        }
    }

    /// The user's own `cl` still wins over a provisioned `ag`.
    #[cfg(unix)]
    #[test]
    fn pane_command_prefers_the_users_cl_over_a_provisioned_ag() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        for shell in available_shells() {
            let argv =
                run_pane_command_opts(shell, &pane_command_for(Some(id), "dev-x"), true, true);
            assert_eq!(
                argv,
                vec![format!("cl --resume {id} --name dev-x")],
                "{shell}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn pane_command_passes_model_and_effort_to_claude() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let launch = ClaudeLaunch {
            model: Some("sonnet[1m]".into()),
            effort: Some("high".into()),
            profile: None,
        };
        for shell in available_shells() {
            let argv =
                run_pane_command(shell, &pane_command_with(Some(id), "dev-x", &launch), true);
            assert_eq!(
                argv,
                vec![format!(
                    "cl --resume {id} --name dev-x --model sonnet[1m] --effort high"
                )],
                "{shell}"
            );
        }
    }

    /// A profile session runs `cl` with `CLAUDE_CONFIG_DIR` at
    /// `~/.claude-profiles/<name>`, which shares `~/.claude`'s visible
    /// entries (transcripts, settings with fleet's hooks) but never its
    /// login, and keeps whatever the profile already has of its own.
    #[cfg(unix)]
    #[test]
    fn a_profile_session_runs_under_its_config_dir_and_shares_all_but_the_login() {
        use super::fake_exec::{write_exec, PROBE_GUARD};
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let launch = ClaudeLaunch {
            profile: Some("work".into()),
            ..ClaudeLaunch::default()
        };
        let cmd = pane_command_with(Some(id), "dev-x", &launch);
        for shell in available_shells() {
            let home = tempfile::tempdir().unwrap();
            let h = home.path();
            let log = h.join("cl.log");
            write_exec(
                h,
                "cl",
                &format!(
                    "#!/bin/sh\n{PROBE_GUARD}printf '%s %s\\n' \"$CLAUDE_CONFIG_DIR\" \"$*\" >> '{}'\nexit 0\n",
                    log.display()
                ),
            );
            write_exec(
                h,
                "fake-shell",
                &format!("#!/bin/sh\n{PROBE_GUARD}exit 0\n"),
            );
            let claude = h.join(".claude");
            std::fs::create_dir_all(claude.join("skills")).unwrap();
            std::fs::write(claude.join("settings.json"), "{\"hooks\":{}}").unwrap();
            std::fs::write(claude.join(".credentials.json"), "host-login").unwrap();
            std::fs::write(claude.join("CLAUDE.md"), "shared").unwrap();
            let profile = h.join(".claude-profiles/work");
            std::fs::create_dir_all(&profile).unwrap();
            std::fs::write(profile.join("CLAUDE.md"), "the profile's own").unwrap();
            let out = std::process::Command::new(shell)
                .arg("-c")
                .arg(&cmd)
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", h.display()))
                .env("HOME", h)
                .env("SHELL", h.join("fake-shell"))
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let logged = std::fs::read_to_string(&log).unwrap();
            assert_eq!(
                logged.trim(),
                format!(
                    "{}/.claude-profiles/work --resume {id} --name dev-x",
                    h.display()
                ),
                "{shell}"
            );
            for shared in ["projects", "settings.json", "skills"] {
                let link = std::fs::read_link(profile.join(shared))
                    .unwrap_or_else(|e| panic!("{shell}: {shared} not linked: {e}"));
                assert_eq!(link, claude.join(shared), "{shell}");
            }
            assert!(claude.join("projects").is_dir(), "{shell}");
            assert!(!profile.join(".credentials.json").exists(), "{shell}");
            assert_eq!(
                std::fs::read_to_string(profile.join("CLAUDE.md")).unwrap(),
                "the profile's own",
                "{shell}"
            );
        }
    }

    /// The profiles section lists each valid `~/.claude-profiles/<name>`
    /// with its login, skips a name that is not a profile name, and reports
    /// a profile with no login yet as such.
    #[cfg(unix)]
    #[test]
    fn the_profiles_script_lists_each_profile_and_its_login() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().join(".claude-profiles");
        std::fs::create_dir_all(p.join("work")).unwrap();
        std::fs::write(
            p.join("work/.claude.json"),
            r#"{"oauthAccount":{"accountUuid":"acc-w","emailAddress":"w@x.com"},"other":1}"#,
        )
        .unwrap();
        std::fs::create_dir_all(p.join("fresh")).unwrap();
        std::fs::create_dir_all(p.join("bad name")).unwrap();
        std::fs::create_dir_all(p.join("-dash")).unwrap();
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(profiles_script())
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got = parse_profiles(&String::from_utf8_lossy(&out.stdout));
        let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["fresh", "work"]);
        assert!(got[0].account.is_none());
        let w = got[1].account.as_ref().unwrap();
        assert_eq!(w.uuid.as_deref(), Some("acc-w"));
        assert_eq!(w.email.as_deref(), Some("w@x.com"));

        let local = read_local_profiles(home.path()).unwrap();
        let names: Vec<&str> = local.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["fresh", "work"]);
        assert_eq!(
            local[1].account.as_ref().unwrap().uuid.as_deref(),
            Some("acc-w")
        );
        assert_eq!(
            read_local_profiles(&home.path().join("nowhere"))
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_probe_reports_its_profiles_section() {
        let text = "---FLEET:sessions\nrc=0\n---FLEET:profiles\n@@P\twork\t{\"accountUuid\":\"a\"}\n@@P\tx y\t\n---FLEET:end\n";
        let snap = parse_probe_snapshot(text).unwrap();
        let profiles = snap.profiles.unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "work");
        let old = parse_probe_snapshot("---FLEET:sessions\nrc=0\n---FLEET:end\n").unwrap();
        assert!(
            old.profiles.is_none(),
            "no section = could not tell, never 'none'"
        );
        assert!(probe_snapshot_script(10, false).contains("---FLEET:profiles"));
    }

    #[test]
    fn a_session_without_a_profile_leaves_the_config_dir_alone() {
        let cmd = pane_command_with(None, "dev-x", &ClaudeLaunch::default());
        assert!(!cmd.contains("CLAUDE_CONFIG_DIR"), "{cmd}");
        let bad = ClaudeLaunch::checked(None, None, Some("../../etc".into()));
        assert_eq!(
            bad.profile, None,
            "a stored profile that no longer validates is dropped"
        );
    }

    #[cfg(unix)]
    #[test]
    fn pane_command_prefers_the_users_cl_when_present() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        for shell in available_shells() {
            let argv = run_pane_command(shell, &pane_command_for(Some(id), "dev-x"), true);
            assert_eq!(
                argv,
                vec![format!("cl --resume {id} --name dev-x")],
                "{shell}"
            );
        }
    }

    /// Voice relay F1: fleet's `arecord` stand-in comes ahead of any real one
    /// on `claude`'s PATH, whichever branch of the chain runs.
    #[test]
    fn pane_command_puts_the_voice_bin_first_on_path() {
        assert_eq!(
            VOICE_PATH_PREFIX,
            "PATH=\"$HOME/.claude-fleet/voice/bin:$PATH\"; "
        );
        for cmd in [
            pane_command_for(None, "s"),
            pane_command_for(Some("550e8400-e29b-41d4-a716-446655440000"), "s"),
        ] {
            assert!(cmd.starts_with(VOICE_PATH_PREFIX), "got: {cmd}");
            assert_eq!(
                cmd[VOICE_PATH_PREFIX.len()..].find(CL_FALLBACK),
                Some(0),
                "the fallback follows the prefix: {cmd}"
            );
        }
    }

    #[test]
    fn pane_command_for_none_uses_continue() {
        let cmd = pane_command_for(None, "dev-x");
        assert!(cmd.contains("cl --continue --name 'dev-x'"), "got: {cmd}");
        assert!(!cmd.contains("--session-id"), "got: {cmd}");
    }

    #[test]
    fn pane_command_for_quotes_the_session_name() {
        let cmd = pane_command_for(None, "it's$(x)");
        assert!(cmd.contains("--name 'it'\\''s$(x)'"), "got: {cmd}");
    }

    #[test]
    fn mtimes_script_quotes_ids_and_skips_invalid() {
        let ids = vec![
            "44366faf-ae97-426a-91cd-beaf3c74f1d7".to_string(),
            "'; rm -rf / #".to_string(),
        ];
        let s = crate::tmux::transcript_mtimes_script(&ids).unwrap();
        assert!(s.contains("'44366faf-ae97-426a-91cd-beaf3c74f1d7'"));
        assert!(!s.contains("rm -rf"));
        assert!(s.contains("date -r"));
        assert!(crate::tmux::transcript_mtimes_script(&["bad".into()]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn mtimes_script_runs_against_a_real_projects_tree() {
        // The script itself, through `bash -c` with a throwaway $HOME: finds a
        // transcript in any project dir and skips ids with no transcript.
        let home = tempfile::tempdir().unwrap();
        let proj = home.path().join(".claude/projects/-Users-u-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let found = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let missing = "0b8e2f41-9d3c-4a7e-b1f0-6c5d4e3a2b19";
        std::fs::write(proj.join(format!("{found}.jsonl")), "{}\n").unwrap();
        let script = transcript_mtimes_script(&[found.to_string(), missing.to_string()]).unwrap();
        let out = std::process::Command::new("bash")
            .args(["-c", &script])
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let m = parse_mtimes(&String::from_utf8_lossy(&out.stdout));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(
            (now - m[found]).abs() < 120,
            "mtime {} vs now {now}",
            m[found]
        );
    }

    #[test]
    fn discover_script_clamps_limit() {
        assert!(discover_transcripts_script(0).contains("head -n 1"));
        assert!(discover_transcripts_script(10_000).contains("head -n 500"));
        assert!(discover_transcripts_script(50).contains("head -n 50"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_script_runs_under_local_bash() {
        use crate::service::sessions::parse_discover_output;

        let home = tempfile::tempdir().unwrap();
        let uuid1 = "11111111-1111-1111-1111-111111111111";
        let uuid3 = "33333333-3333-3333-3333-333333333333";
        let uuid4 = "44444444-4444-4444-4444-444444444444";

        // p1/<uuid1>.jsonl: two lines, the last carrying cwd/gitBranch.
        let p1 = home.path().join(".claude/projects/p1");
        std::fs::create_dir_all(&p1).unwrap();
        std::fs::write(
            p1.join(format!("{uuid1}.jsonl")),
            format!(
                "{{\"parentUuid\":null,\"sessionId\":\"{uuid1}\",\"message\":{{\"content\":\"hi\"}}}}\n\
                 {{\"parentUuid\":\"x\",\"cwd\":\"/w/a\",\"sessionId\":\"{uuid1}\",\"gitBranch\":\"main\",\"message\":{{\"content\":\"there\"}}}}\n"
            ),
        )
        .unwrap();

        // projects/subagents/<uuid3>.jsonl: its path contains "/subagents/",
        // and — unlike a deeper nesting — it IS reachable by the top-level
        // `*/*.jsonl` glob (first star = "subagents" itself), so this
        // actually exercises the script's `case "$f" in */subagents/*)
        // continue;;` guard rather than being excluded by path depth alone.
        let sub = home.path().join(".claude/projects/subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join(format!("{uuid3}.jsonl")),
            format!("{{\"cwd\":\"/w/sub\",\"sessionId\":\"{uuid3}\",\"gitBranch\":\"sub\"}}\n"),
        )
        .unwrap();

        // p2/<uuid4>.jsonl: the last cwd-bearing line is followed by a 20 KB
        // line with no "cwd" in it, exercising the `tail -n 1` over
        // `grep -a '"cwd"'` against trailing noise.
        let p2 = home.path().join(".claude/projects/p2");
        std::fs::create_dir_all(&p2).unwrap();
        let big = "x".repeat(20_000);
        std::fs::write(
            p2.join(format!("{uuid4}.jsonl")),
            format!(
                "{{\"cwd\":\"/w/b\",\"sessionId\":\"{uuid4}\",\"gitBranch\":\"dev\",\"message\":{{\"content\":\"hi\"}}}}\n\
                 {{\"sessionId\":\"{uuid4}\",\"message\":{{\"content\":\"{big}\"}}}}\n"
            ),
        )
        .unwrap();

        // Distinct, deterministic mtimes (no `filetime` dev-dep): `touch -t`.
        // uuid1 newest, uuid4 older — both well within the limit either way.
        let touch = |path: &std::path::Path, stamp: &str| {
            let status = std::process::Command::new("touch")
                .args(["-t", stamp])
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success(), "touch -t {stamp} {path:?}");
        };
        touch(&p1.join(format!("{uuid1}.jsonl")), "202509190200");
        touch(&p2.join(format!("{uuid4}.jsonl")), "202509190100");

        let script = discover_transcripts_script(10);
        let out = std::process::Command::new("bash")
            .args(["-c", &script])
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        // `cut -c` is bytewise, so decode leniently rather than assuming
        // valid UTF-8 — matches how a real caller must treat this output.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let (boot, probes) = parse_discover_output(&stdout);

        assert!(boot.is_some(), "{stdout}");
        assert_eq!(probes.len(), 2, "{probes:?}\nraw:\n{stdout}");
        let by_id: std::collections::HashMap<&str, _> = probes
            .iter()
            .map(|p| (p.claude_session_id.as_str(), p))
            .collect();
        assert!(!by_id.contains_key(uuid3), "{probes:?}");

        let p1_probe = by_id.get(uuid1).expect("uuid1 present");
        assert_eq!(p1_probe.cwd.as_deref(), Some("/w/a"));
        assert_eq!(p1_probe.git_branch.as_deref(), Some("main"));

        let p2_probe = by_id.get(uuid4).expect("uuid4 present");
        assert_eq!(p2_probe.cwd.as_deref(), Some("/w/b"));
        assert_eq!(p2_probe.git_branch.as_deref(), Some("dev"));

        // `limit` keeps the NEWEST transcripts: at 1, only uuid1 (touched
        // an hour after uuid4) comes back.
        let out = std::process::Command::new("bash")
            .args(["-c", &discover_transcripts_script(1)])
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let (_boot, probes) = parse_discover_output(&stdout);
        let ids: Vec<&str> = probes
            .iter()
            .map(|p| p.claude_session_id.as_str())
            .collect();
        assert_eq!(ids, vec![uuid1], "raw:\n{stdout}");
    }

    /// Review r18: a host whose Claude keeps its state under
    /// `CLAUDE_CONFIG_DIR` has its transcripts found there. Unix only, as
    /// the other script tests here: the script runs on a host's bash, and
    /// Windows runners' `bash` is the WSL stub.
    #[cfg(unix)]
    #[test]
    fn discover_transcripts_script_honours_claude_config_dir() {
        let home = tempfile::tempdir().unwrap();
        let conf = tempfile::tempdir().unwrap();
        let proj = conf.path().join("projects").join("-w-c");
        std::fs::create_dir_all(&proj).unwrap();
        let id = "33333333-3333-4333-8333-333333333333";
        std::fs::write(
            proj.join(format!("{id}.jsonl")),
            "{\"cwd\":\"/w/c\",\"gitBranch\":\"main\"}\n",
        )
        .unwrap();
        let out = std::process::Command::new("bash")
            .args(["-c", &discover_transcripts_script(10)])
            .env("HOME", home.path())
            .env("CLAUDE_CONFIG_DIR", conf.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let (_boot, probes) = crate::service::sessions::parse_discover_output(&stdout);
        assert_eq!(probes.len(), 1, "raw:\n{stdout}");
        assert_eq!(probes[0].claude_session_id, id);
        assert!(probes[0].mtime > 0, "dated: {stdout}");
        assert_eq!(probes[0].cwd.as_deref(), Some("/w/c"));
    }

    #[test]
    fn parse_host_identity_reads_both_fields() {
        let id = parse_host_identity("boot=abc-123\ntmuxrc=0\ntmuxout=4242\n").unwrap();
        assert_eq!(id.boot_id.as_deref(), Some("abc-123"));
        assert_eq!(id.tmux_server_pid, Some(4242));
    }

    #[test]
    fn parse_host_versions_reads_both_lines_and_tolerates_a_missing_binary() {
        let v = parse_host_versions("tmuxv=tmux 3.6a\nclaudev=2.1.282 (Claude Code)\n");
        assert_eq!(v.tmux_version.as_deref(), Some("3.6a"));
        assert_eq!(v.claude_version.as_deref(), Some("2.1.282"));
        // `claude` not on PATH: the line is empty, the field is unknown, the
        // other one still parses.
        let v = parse_host_versions("tmuxv=tmux 3.3a\nclaudev=\n");
        assert_eq!(v.tmux_version.as_deref(), Some("3.3a"));
        assert_eq!(v.claude_version, None);
        assert_eq!(parse_host_versions(""), HostVersions::default());
    }

    #[test]
    fn parse_host_health_reads_linux_and_macos_shapes() {
        let linux = "dfhome=/dev/sda1 157286400 150000000 3600000 98% /home\n\
                     dftmp=tmpfs 8000000 2100000 5900000 27% /tmp\n\
                     load=5.25 4.10 3.90 1/900 12345\n\
                     memkb=1234567\n\
                     uptime=12441600\n\
                     cpus=16\n\
                     memtotal=65842312\n\
                     bootat=1778000000\n";
        let h = parse_host_health(linux);
        assert_eq!(h.cpu_count, Some(16));
        assert_eq!(h.mem_total_kb, Some(65_842_312));
        assert_eq!(h.boot_at, Some(1_778_000_000));
        assert_eq!(h.latency_ms, None, "the script never reports latency");
        assert_eq!(h.disk_home_total_kb, Some(157_286_400));
        assert_eq!(h.disk_home_free_kb, Some(3_600_000));
        assert_eq!(h.disk_tmp_free_kb, Some(5_900_000));
        assert_eq!(h.load_1m, Some(5.25));
        assert_eq!(h.mem_avail_kb, Some(1_234_567));
        assert_eq!(h.uptime_secs, Some(12_441_600));
        let mac = "dfhome=/dev/disk3s5 488245288 440000000 45000000 91% /System/Volumes/Data\n\
                   dftmp=\n\
                   load={ 1.62 1.80 1.91 }\n\
                   memkb=\n\
                   uptime=86400\n\
                   cpus=10\n\
                   memtotal=\n\
                   bootat=0\n";
        let h = parse_host_health(mac);
        assert_eq!(h.cpu_count, Some(10));
        assert_eq!(h.mem_total_kb, None);
        assert_eq!(h.boot_at, None, "a zero epoch is no answer");
        assert_eq!(h.disk_home_free_kb, Some(45_000_000));
        assert_eq!(h.disk_tmp_free_kb, None);
        assert_eq!(h.load_1m, Some(1.62));
        assert_eq!(h.mem_avail_kb, None);
        assert_eq!(parse_host_health(""), HostHealthSample::default());
    }

    #[cfg(unix)]
    #[test]
    fn the_macos_uptime_fallback_reads_the_seconds_not_the_microseconds() {
        // Run the real script with the Linux path forced off (`cut` fails,
        // as it does when /proc/uptime is absent) and a `sysctl` printing
        // macOS's actual `kern.boottime` shape. Its `usec = ` also contains
        // `sec = `, so a greedy match took the microseconds as the boot
        // epoch and reported ~56 years of uptime. `date` is pinned so the
        // uptime is exact.
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        for (tool, body) in [
            (
                "sysctl",
                "#!/bin/sh\n[ \"$2\" = kern.boottime ] || exit 1\n\
                 echo '{ sec = 1790420987, usec = 963287 } Sat Sep 26 13:09:47 2026'\n",
            ),
            ("cut", "#!/bin/sh\nexit 1\n"),
            ("date", "#!/bin/sh\necho 1790421987\n"),
        ] {
            let p = bin.path().join(tool);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = std::process::Command::new("sh")
            .args(["-c", HOST_HEALTH_SCRIPT])
            .env("PATH", path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(
            parse_host_health(&stdout).uptime_secs,
            Some(1000),
            "{stdout}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_health_script_reads_cpus_memory_and_boot_time_on_linux() {
        let out = std::process::Command::new("sh")
            .args(["-c", HOST_HEALTH_SCRIPT])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let h = parse_host_health(&stdout);
        assert!(h.cpu_count.is_some_and(|n| n >= 1), "{stdout}");
        assert!(h.mem_total_kb.is_some_and(|n| n > 0), "{stdout}");
        // A boot in the past, and after 2001.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            h.boot_at.is_some_and(|b| b > 1_000_000_000 && b <= now),
            "{stdout}"
        );
    }

    #[test]
    fn parse_host_health_reads_the_agents_on_path_in_checklist_order() {
        let h = parse_host_health("agents=codex claude vim gemini \n");
        assert_eq!(
            h.agents_on_path,
            Some(vec![
                "claude".to_string(),
                "codex".to_string(),
                "gemini".to_string()
            ]),
            "known names only, in AGENT_BINARIES order"
        );
        assert_eq!(parse_host_health("agents=\n").agents_on_path, Some(vec![]));
        // An older agent's sample has no line at all: unknown, not none.
        assert_eq!(parse_host_health("uptime=5\n").agents_on_path, None);
        // The script asks about exactly the checklist's agents.
        let asked = format!(
            "for a in {}; do",
            crate::service::host_check::AGENT_BINARIES.join(" ")
        );
        assert!(HOST_HEALTH_SCRIPT.contains(&asked), "{HOST_HEALTH_SCRIPT}");
    }

    /// The real script, with a fake `codex` and `agy` first on PATH and
    /// nothing else of the four reachable: it reports exactly those two.
    #[cfg(unix)]
    #[test]
    fn the_health_script_finds_the_agents_on_path() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        for tool in ["codex", "agy"] {
            let p = bin.path().join(tool);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Only the fake dir and the system tool dirs: a developer's own
        // `claude` in ~/.local/bin must not leak into the answer.
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", HOST_HEALTH_SCRIPT])
            .env("PATH", path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let found = parse_host_health(&stdout)
            .agents_on_path
            .unwrap_or_default();
        assert!(
            found.contains(&"codex".to_string()) && found.contains(&"agy".to_string()),
            "{stdout}"
        );
        assert!(!found.iter().any(|a| a == "gemini"), "{stdout}");
    }

    #[test]
    fn parse_host_health_reads_auth_overrides_in_precedence_order() {
        let h = parse_host_health(
            "authenv=ANTHROPIC_API_KEY CLAUDE_CODE_USE_BEDROCK SOMETHING_ELSE \n",
        );
        assert_eq!(
            h.auth_overrides,
            Some(vec![
                "CLAUDE_CODE_USE_BEDROCK".to_string(),
                "ANTHROPIC_API_KEY".to_string()
            ])
        );
        assert_eq!(parse_host_health("authenv=\n").auth_overrides, Some(vec![]));
        // An older agent's sample has no line at all: unknown, not clean.
        assert_eq!(parse_host_health("uptime=5\n").auth_overrides, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_health_script_names_auth_overrides_without_printing_their_values() {
        // `tmux` is replaced so the machine's own server cannot add names;
        // its global environment carries one more override.
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let tmux = bin.path().join("tmux");
        std::fs::write(
            &tmux,
            "#!/bin/sh\nprintf 'CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat-from-tmux\\n-ANTHROPIC_PROFILE\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", HOST_HEALTH_SCRIPT]).env("PATH", path);
        for v in AUTH_OVERRIDE_VARS {
            cmd.env_remove(v);
        }
        let out = cmd
            .env("ANTHROPIC_API_KEY", "sk-ant-api-secret-value")
            .env("ANTHROPIC_AUTH_TOKEN", "")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(!stdout.contains("secret-value"), "{stdout}");
        assert!(!stdout.contains("from-tmux"), "{stdout}");
        assert_eq!(
            parse_host_health(&stdout).auth_overrides,
            Some(vec![
                "ANTHROPIC_API_KEY".to_string(),
                "CLAUDE_CODE_OAUTH_TOKEN".to_string()
            ]),
            "{stdout}"
        );
    }

    #[test]
    fn a_missing_boot_id_is_just_unknown_boot() {
        let id = parse_host_identity("boot=\ntmuxrc=0\ntmuxout=7\n").unwrap();
        assert_eq!(id.boot_id, None);
        assert_eq!(id.tmux_server_pid, Some(7));
    }

    #[test]
    fn unreadable_output_is_unknown_so_it_can_never_mark_a_host_lost() {
        // No tmuxrc=/tmuxout= lines at all: a login banner, a wrapper, a
        // truncated run.
        assert_eq!(parse_host_identity("Welcome to Ubuntu\n"), None);
        assert_eq!(parse_host_identity(""), None);
        // Only one of the two required lines present.
        assert_eq!(parse_host_identity("boot=a\ntmuxrc=0\n"), None);
        assert_eq!(parse_host_identity("boot=a\ntmuxout=4242\n"), None);
        // An rc that is not a number is garbage, not a real exit code.
        assert_eq!(
            parse_host_identity("boot=a\ntmuxrc=not-a-number\ntmuxout=4242\n"),
            None
        );
    }

    #[test]
    fn a_missing_tmux_binary_is_unknown_not_no_server() {
        // exit 127: `tmux` isn't on PATH at all (a login profile edit, a
        // brew relink mid-upgrade). The server may well be alive and
        // holding every session — this must never read as "no server".
        assert_eq!(
            parse_host_identity("boot=a\ntmuxrc=127\ntmuxout=bash: tmux: command not found\n"),
            None
        );
    }

    #[test]
    fn a_protocol_mismatch_is_unknown_not_no_server() {
        // The realistic trigger: `brew`/`apt upgrade tmux` leaves a client
        // that can't talk to the still-running old server.
        assert_eq!(
            parse_host_identity(
                "boot=a\ntmuxrc=1\ntmuxout=protocol version mismatch (client 8, server 7)\n"
            ),
            None
        );
    }

    #[test]
    fn no_server_running_is_the_only_path_to_tmux_server_pid_none() {
        let id = parse_host_identity(
            "boot=a\ntmuxrc=1\ntmuxout=no server running on /tmp/tmux-501/default\n",
        )
        .unwrap();
        assert_eq!(id.tmux_server_pid, None);
    }

    #[test]
    fn a_successful_but_empty_tmux_output_is_unknown_not_no_server() {
        // rc=0 (tmux ran fine) but no pid printed — e.g. `list-sessions`
        // formatted zero lines. We can't tell the pid, so `None`, not a
        // guess at "no server" (the server may hold zero sessions and still
        // be very much alive).
        assert_eq!(parse_host_identity("boot=a\ntmuxrc=0\ntmuxout=\n"), None);
        assert_eq!(
            parse_host_identity("boot=a\ntmuxrc=0\ntmuxout=not-a-pid-either\n"),
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_identity_script_runs_under_local_bash() {
        // Real bash, real `tmux` if installed — CI's `ubuntu-24.04` runner
        // has tmux but no server, `macos-latest` has neither, and a
        // `command -v tmux` pre-check is not a safe proxy for "the script's
        // own tmux invocation will succeed": a stale wrapper/shim can
        // resolve as a command yet still fail with exit 127 when run (this
        // is not hypothetical — it's exactly how a botched `brew upgrade`
        // can leave PATH). So the only trustworthy signal is the `tmuxrc=`
        // the script itself observed, read straight from its own output.
        let out = tokio::process::Command::new("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(stdout.lines().any(|l| l.starts_with("boot=")), "{stdout}");
        assert!(stdout.lines().any(|l| l.starts_with("tmuxrc=")), "{stdout}");
        assert!(
            stdout.lines().any(|l| l.starts_with("tmuxout=")),
            "{stdout}"
        );
        match stdout.lines().find_map(|l| l.strip_prefix("tmuxrc=")) {
            Some("127") => {
                // tmux did not resolve (missing binary, or a wrapper that
                // fails exactly like one) — the parser must never invent a
                // verdict from that.
                assert_eq!(parse_host_identity(&stdout), None, "{stdout}");
            }
            Some(_) => {
                // tmux resolved and actually ran a real client against a
                // real socket — whether that found a live server (a
                // numeric pid) or a confirmed-dead one ("no server
                // running", the only other trustworthy shape reachable
                // here), the parser must produce a verdict, not `None`.
                // (The other untrustworthy shapes — a protocol mismatch, a
                // permission error — need a crafted string to trigger and
                // are covered by their own dedicated parser tests instead.)
                assert!(
                    parse_host_identity(&stdout).is_some(),
                    "tmux ran (rc != 127), so its output must classify: {stdout}"
                );
            }
            None => panic!("script printed no tmuxrc= line: {stdout}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_script_is_unknown_when_tmux_is_missing_from_path() {
        // The 127 case this whole fix exists for: a login profile edit or a
        // brew relink mid-upgrade drops tmux's directory off PATH entirely,
        // while the server it can no longer reach is still holding every
        // session. A system dir can't stand in for that PATH: GitHub's
        // `ubuntu-24.04` image ships `/usr/bin/tmux`. So build a PATH of
        // symlinks to just the script's other tools, which keeps the boot-id
        // half resolvable and hides only tmux — the real regression shape.
        let bin = tempfile::tempdir().unwrap();
        for tool in ["cat", "head", "sysctl"] {
            let found = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
                .iter()
                .map(|d| std::path::Path::new(d).join(tool))
                .find(|p| p.exists());
            if let Some(target) = found {
                std::os::unix::fs::symlink(target, bin.path().join(tool)).unwrap();
            }
        }
        let out = tokio::process::Command::new("/bin/bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .env("PATH", bin.path())
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(parse_host_identity(&stdout), None, "{stdout}");
    }

    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn the_boot_id_does_not_depend_on_the_timezone() {
        // This is a claim about the `boot=` line alone, so read it straight
        // off the script's stdout instead of routing through
        // `parse_host_identity`: the parser's outer `None` legitimately
        // depends on whether tmux is installed (CI's `ubuntu-24.04` runner
        // has it, `macos-latest` does not; neither runs a server), which has
        // nothing to do with the boot id and must not make this test flaky.
        fn boot_id(stdout: &str) -> Option<String> {
            stdout.lines().find_map(|l| {
                let v = l.strip_prefix("boot=")?.trim();
                (!v.is_empty()).then(|| v.to_string())
            })
        }
        let run = |tz: &'static str| async move {
            let out = tokio::process::Command::new("bash")
                .args(["-c", HOST_IDENTITY_SCRIPT])
                .env("TZ", tz)
                .output()
                .await
                .unwrap();
            boot_id(&String::from_utf8_lossy(&out.stdout))
        };
        let utc = run("UTC").await;
        assert!(utc.is_some(), "boot id must be readable on this OS");
        assert_eq!(utc, run("America/Los_Angeles").await);
    }

    #[tokio::test]
    async fn remote_read_oauth_account_parses_json_and_is_none_when_it_cannot_tell() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let tmux = |fake: &FakeSsh| RemoteTmux {
            client: fake.clone(),
            host: "h".to_string(),
        };

        // Logged in: the compact JSON `jq -c .oauthAccount` prints.
        let fake = FakeSsh::new();
        fake.on(
            Match::script_contains(".claude.json"),
            Reply::ok(r#"{"accountUuid":"acc-2","emailAddress":"new@x.com","seatTier":null}"#),
        );
        let acc = tmux(&fake)
            .read_oauth_account()
            .await
            .expect("a logged-in host yields its account");
        assert_eq!(acc.uuid.as_deref(), Some("acc-2"));
        assert_eq!(acc.email.as_deref(), Some("new@x.com"));

        // Logged out / no file: the script prints nothing (or `null`) and
        // still exits 0 — no account, never an error.
        for stdout in ["", "\n", "null\n", "{}\n"] {
            let fake = FakeSsh::new();
            fake.on(Match::Any, Reply::ok(stdout));
            assert!(
                tmux(&fake).read_oauth_account().await.is_none(),
                "stdout {stdout:?} must not yield an account"
            );
        }

        // Unreachable host (ssh exit 255) → None.
        let fake = FakeSsh::new();
        fake.unreachable("h");
        assert!(tmux(&fake).read_oauth_account().await.is_none());

        // Spawn error → None.
        let fake = FakeSsh::new();
        fake.on(
            Match::Any,
            Reply::SpawnError {
                message: "no ssh".into(),
            },
        );
        assert!(tmux(&fake).read_oauth_account().await.is_none());
    }

    #[tokio::test]
    async fn remote_host_identity_is_none_on_ssh_failure_and_some_through_a_login_banner() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let tmux = |fake: &FakeSsh| RemoteTmux {
            client: fake.clone(),
            host: "h".to_string(),
        };

        // A login banner (MOTD, a shell wrapper) ahead of the script's own
        // lines must not stop the tagged lines from being found.
        let fake = FakeSsh::new();
        fake.on(
            Match::script_contains("tmux list-sessions"),
            Reply::ok("Welcome to Ubuntu 24.04 LTS\nboot=abc-123\ntmuxrc=0\ntmuxout=4242\n"),
        );
        let id = tmux(&fake)
            .host_identity()
            .await
            .expect("a login banner ahead of the tagged lines must still parse");
        assert_eq!(id.boot_id.as_deref(), Some("abc-123"));
        assert_eq!(id.tmux_server_pid, Some(4242));

        // Unreachable host (ssh exit 255) → None: the transport failed, not
        // "no tmux server".
        let fake = FakeSsh::new();
        fake.unreachable("h");
        assert!(tmux(&fake).host_identity().await.is_none());

        // A non-zero exit from the `bash -lc` invocation itself (distinct
        // from tmux's own exit code, which the script always captures into
        // `tmuxrc=` rather than propagating) → None.
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::fail(1, "bash: some unrelated failure"));
        assert!(tmux(&fake).host_identity().await.is_none());
    }

    #[tokio::test]
    async fn remote_transcript_mtimes_is_none_on_failure_and_some_on_success() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7".to_string();
        let ids = std::slice::from_ref(&id);
        let tmux = |fake: &FakeSsh| RemoteTmux {
            client: fake.clone(),
            host: "h".to_string(),
        };

        // Non-zero exit → None (not "no transcript").
        let fake = FakeSsh::new();
        fake.on(Match::script_contains("date -r"), Reply::fail(1, "boom"));
        assert_eq!(tmux(&fake).transcript_mtimes(ids).await, None);

        // Unreachable host (ssh exit 255) → None.
        let fake = FakeSsh::new();
        fake.unreachable("h");
        assert_eq!(tmux(&fake).transcript_mtimes(ids).await, None);

        // Spawn error → None.
        let fake = FakeSsh::new();
        fake.on(
            Match::Any,
            Reply::SpawnError {
                message: "no ssh".into(),
            },
        );
        assert_eq!(tmux(&fake).transcript_mtimes(ids).await, None);

        // Success: a map, possibly empty.
        let fake = FakeSsh::new();
        fake.on(
            Match::script_contains("date -r"),
            Reply::ok(&format!("{id}\t1779999999\n")),
        );
        let got = tmux(&fake).transcript_mtimes(ids).await;
        assert_eq!(got.and_then(|m| m.get(&id).copied()), Some(1_779_999_999));
        let fake = FakeSsh::new();
        fake.on(Match::script_contains("date -r"), Reply::ok(""));
        assert_eq!(
            tmux(&fake).transcript_mtimes(ids).await,
            Some(std::collections::HashMap::new())
        );

        // No valid id: nothing to ask, not a failure.
        let fake = FakeSsh::new();
        assert_eq!(
            tmux(&fake).transcript_mtimes(&["bad".into()]).await,
            Some(std::collections::HashMap::new())
        );
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn named_keys_parse_exactly_and_build_a_send_keys_command() {
        assert_eq!(NamedKey::parse("Enter"), Some(NamedKey::Enter));
        assert_eq!(NamedKey::parse("Escape"), Some(NamedKey::Escape));
        assert_eq!(NamedKey::parse("C-c"), Some(NamedKey::CtrlC));
        assert_eq!(
            NamedKey::parse("enter"),
            None,
            "case matters: the value is a tmux key name"
        );
        assert_eq!(NamedKey::parse("rm -rf"), None);
        assert_eq!(
            send_named_key("my session", NamedKey::Escape),
            format!(
                "tmux send-keys -t {} Escape",
                crate::shell::quote(&exact_pane("my session"))
            )
        );
    }

    #[test]
    fn the_keys_vocabulary_names_every_accepted_key() {
        // The refusal message both `send_prompt` paths print comes from this
        // one constant, so a key the parser accepts can never go unnamed.
        let v = NamedKey::VOCABULARY;
        for accepted in [
            "Enter", "Escape", "Tab", "C-c", "1-9", "BTab", "Up", "Down", "Left", "Right",
        ] {
            assert!(v.contains(accepted), "{v:?} must mention {accepted}");
        }
        for name in CTRL_NAMES {
            let letter = name.strip_prefix("C-").expect("a Ctrl name");
            assert!(v.contains(letter), "{v:?} must mention {name}");
        }
    }

    #[test]
    fn every_listed_key_parses_and_names_itself() {
        // `all_names` is what the schema enumerates; each must round-trip,
        // or a client would be told a key the hub then refuses.
        for name in NamedKey::all_names() {
            let key = NamedKey::parse(name).unwrap_or_else(|| panic!("{name} must parse"));
            assert_eq!(key.tmux_name(), name);
            assert_eq!(
                send_named_key("s", key),
                format!(
                    "tmux send-keys -t {} {name}",
                    crate::shell::quote(&exact_pane("s"))
                )
            );
        }
    }

    #[test]
    fn ctrl_keys_outside_the_list_are_refused() {
        // Tab and Enter under other names, flow control, suspend; and
        // anything that is not exactly a listed name.
        for refused in [
            "C-i", "C-j", "C-m", "C-s", "C-q", "C-z", "C-A", "C-", "C-ab", "C-1", "M-x", "C-\\",
            "S-Up", "up", "Home", "C-a ", " C-a",
        ] {
            assert_eq!(
                NamedKey::parse(refused),
                None,
                "{refused:?} must be refused"
            );
        }
    }

    #[test]
    fn digit_keys_answer_a_numbered_dialog() {
        // Answering a permission/question dialog is one literal digit
        // keystroke — the text path pastes (bracketed paste) and would then
        // press Enter into the REPL, which a select dialog does not survive.
        for n in 1..=9u8 {
            let s = n.to_string();
            let key = NamedKey::parse(&s).unwrap_or_else(|| panic!("digit {n} must parse"));
            assert_eq!(key.tmux_name(), s, "digit {n} keeps its own tmux key name");
        }
        assert_eq!(
            send_named_key("my session", NamedKey::parse("3").expect("3")),
            format!(
                "tmux send-keys -t {} 3",
                crate::shell::quote(&exact_pane("my session"))
            )
        );
    }

    #[test]
    fn digit_keys_outside_one_to_nine_are_rejected() {
        // A dialog may carry up to PENDING_OPTIONS_MAX (16) options, but the
        // REPL has no keystroke for a two-digit ordinal — better no key than
        // a "1" that silently answers option 1 for a click on option 10.
        for bad in ["0", "10", "16", "-1", " 1", "1 ", "1.", "a"] {
            assert_eq!(NamedKey::parse(bad), None, "{bad:?} is not a digit key");
        }
        assert_eq!(DigitKey::new(0), None);
        assert_eq!(DigitKey::new(10), None);
        assert!(DigitKey::new(1).is_some());
        assert!(DigitKey::new(9).is_some());
    }

    #[test]
    fn parse_mtimes_reads_tab_lines_and_ignores_noise() {
        let m = crate::tmux::parse_mtimes(
            "motd\n44366faf-ae97-426a-91cd-beaf3c74f1d7\t1779999999\nx\tnotanumber\n",
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m["44366faf-ae97-426a-91cd-beaf3c74f1d7"], 1_779_999_999);
    }

    #[tokio::test]
    async fn remote_tmux_runs_under_sh_with_the_login_path_once_the_toolchain_is_known() {
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.set_toolchain(
            "h",
            crate::ssh::HostToolchain {
                home: "/h".into(),
                path: "/opt/homebrew/bin:/usr/bin".into(),
                tmux: Some("/opt/homebrew/bin/tmux".into()),
                claude: None,
            },
        );
        fake.on_host(
            "h",
            crate::ssh_fake::Match::script_contains("tmux list-sessions"),
            crate::ssh_fake::Reply::ok(""),
        );
        let t = RemoteTmux {
            client: fake.clone(),
            host: "h".into(),
        };
        let _ = t.list_sessions().await;
        let call = fake.calls_for("h").pop().unwrap();
        assert_eq!(
            &call.args[..2],
            &["sh".to_string(), "-c".to_string()],
            "{:?}",
            call.args
        );
        let body = crate::ssh_fake::unquote(&call.args[2]).unwrap();
        assert!(
            body.starts_with("export PATH='/opt/homebrew/bin:/usr/bin'; "),
            "{body}"
        );
        assert!(
            call.script().unwrap().starts_with("tmux list-sessions"),
            "Call::script strips the export prefix"
        );
    }

    #[tokio::test]
    async fn remote_tmux_falls_back_to_a_login_shell_without_a_toolchain() {
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.on_host(
            "h",
            crate::ssh_fake::Match::script_contains("tmux list-sessions"),
            crate::ssh_fake::Reply::ok(""),
        );
        let t = RemoteTmux {
            client: fake.clone(),
            host: "h".into(),
        };
        let _ = t.list_sessions().await;
        let call = fake.calls_for("h").pop().unwrap();
        assert_eq!(&call.args[..2], &["bash".to_string(), "-lc".to_string()]);
    }

    #[tokio::test]
    async fn remote_new_session_forwards_the_login_path_into_the_pane() {
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.set_toolchain(
            "h",
            crate::ssh::HostToolchain {
                home: "/h".into(),
                path: "/a:/b".into(),
                tmux: None,
                claude: None,
            },
        );
        let t = RemoteTmux {
            client: fake.clone(),
            host: "h".into(),
        };
        t.new_session("s", std::path::Path::new("/w"), "cl")
            .await
            .unwrap();
        let script = fake.calls_for("h").pop().unwrap().script().unwrap();
        assert!(script.contains(" -e PATH='/a:/b'"), "{script}");
    }
}
