//! The one reader of ssh's OWN error wording.
//!
//! ssh's stderr also carries the remote command's stderr, so connect-failure
//! kinds match only the LAST non-empty line, as a prefix. A broken
//! ControlMaster is recognised anywhere in the text, as `is_mux_failure`
//! always did.

use serde::{Deserialize, Serialize};

/// What went wrong, in ssh's terms. Serialized `snake_case`; this enum IS
/// the vocabulary (MCP descriptions and the UI derive from it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshFailureKind {
    /// The server's key is not in known_hosts under the name ssh looked up.
    HostKeyUnknown,
    /// known_hosts has a DIFFERENT key for that name (reinstall, or MITM).
    HostKeyChanged,
    /// Authentication was refused (no usable key, wrong user).
    AuthDenied,
    /// The hostname did not resolve.
    DnsFail,
    /// TCP connect was actively refused (nothing listening on the port).
    Refused,
    /// TCP connect timed out, or there is no route / network.
    Timeout,
    /// sshd dropped the connection before auth (MaxStartups, fail2ban,
    /// a dying sshd).
    Handshake,
    /// The ControlMaster under a multiplexed call died.
    MuxBroken,
    /// Exit 255 with wording none of the above recognise. Also the landing
    /// spot for any future variant an older reader does not know yet, so a
    /// newer writer's row never fails to deserialize on an older binary.
    #[serde(other)]
    Unknown,
}

impl SshFailureKind {
    /// True when nothing ran on the host: the failure happened before the
    /// session came up, so a retry elsewhere cannot duplicate work.
    pub fn never_connected(self) -> bool {
        use SshFailureKind::*;
        matches!(
            self,
            HostKeyUnknown | HostKeyChanged | AuthDenied | DnsFail | Refused | Timeout | Handshake
        )
    }

    /// One short English phrase for logs and error messages.
    pub fn summary(self) -> &'static str {
        use SshFailureKind::*;
        match self {
            HostKeyUnknown => "host key not in known_hosts",
            HostKeyChanged => "host key CHANGED since it was recorded",
            AuthDenied => "authentication refused",
            DnsFail => "hostname does not resolve",
            Refused => "connection refused",
            Timeout => "host unreachable (timeout / no route)",
            Handshake => "sshd closed the connection before login",
            MuxBroken => "shared ssh connection (ControlMaster) died",
            Unknown => "unrecognised ssh failure",
        }
    }
}

/// A classified ssh client failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshFailure {
    pub kind: SshFailureKind,
    /// The ssh destination ssh was invoked with (an ssh config alias or
    /// hostname), not the fleet host alias.
    pub ssh_alias: String,
    /// The end of ssh's stderr, bounded (see [`tail`]).
    pub raw_tail: String,
}

/// Substrings of a dead ControlMaster, matched ANYWHERE in stderr. Not the
/// bare "Broken pipe", which a remote `foo | head` can print: a reset+retry
/// of a possibly non-idempotent command must not hang on that.
const MUX_NEEDLES: &[&str] = &[
    "mux_client_request_session",
    "Control socket",
    "read from master failed",
    "send disconnect: Broken pipe",
    "Connection closed by remote host",
];

const TAIL_LINES: usize = 40;
const TAIL_BYTES: usize = 4096;

/// Classify an ssh run. `None` unless ssh itself exited 255.
pub fn classify(ssh_alias: &str, exit_code: Option<i32>, stderr: &str) -> Option<SshFailure> {
    if exit_code != Some(255) {
        return None;
    }
    let kind = connect_failure_kind(stderr)
        .or_else(|| mentions_mux_failure(stderr).then_some(SshFailureKind::MuxBroken))
        .unwrap_or(SshFailureKind::Unknown);
    Some(SshFailure {
        kind,
        ssh_alias: ssh_alias.to_string(),
        raw_tail: tail(stderr),
    })
}

/// The connect-failure kind named by the LAST non-empty stderr line, if any.
pub(crate) fn connect_failure_kind(stderr: &str) -> Option<SshFailureKind> {
    use SshFailureKind::*;
    let last = stderr
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '\r' || c.is_whitespace()))
        .rfind(|l| !l.is_empty())?;
    if last.starts_with("Host key verification failed.") {
        let changed = stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
            || stderr.contains("has changed and you have requested strict checking");
        return Some(if changed {
            HostKeyChanged
        } else {
            HostKeyUnknown
        });
    }
    if last.starts_with("No ") && last.contains(" host key is known for ") {
        return Some(HostKeyUnknown);
    }
    // OpenSSH 9.x: `user@host: Permission denied (publickey).`
    let auth = last
        .split_once(": ")
        .filter(|(who, _)| who.contains('@') && !who.contains(' '))
        .map_or(last, |(_, rest)| rest);
    if auth.starts_with("Permission denied (") {
        return Some(AuthDenied);
    }
    if last.starts_with("ssh: Could not resolve hostname") {
        return Some(DnsFail);
    }
    if let Some(rest) = last.strip_prefix("ssh: connect to host ") {
        return Some(if rest.ends_with("Connection refused") {
            Refused
        } else {
            Timeout
        });
    }
    if last.starts_with("kex_exchange_identification:")
        || (last.starts_with("Connection closed by ") && last.contains(" port "))
    {
        return Some(Handshake);
    }
    None
}

/// Whether stderr carries a dead-ControlMaster message anywhere.
pub(crate) fn mentions_mux_failure(stderr: &str) -> bool {
    MUX_NEEDLES.iter().any(|n| stderr.contains(n))
}

/// The last [`TAIL_LINES`] lines, then at most [`TAIL_BYTES`] from the end,
/// cut forward to a char boundary.
fn tail(stderr: &str) -> String {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(stderr.match_indices('\n').map(|(i, _)| i + 1))
        .filter(|&i| i < stderr.len())
        .collect();
    let from = starts
        .len()
        .checked_sub(TAIL_LINES)
        .map_or(0, |skip| starts[skip]);
    let mut s = &stderr[from..];
    if s.len() > TAIL_BYTES {
        let mut cut = s.len() - TAIL_BYTES;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        s = &s[cut..];
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(stderr: &str) -> Option<SshFailureKind> {
        classify("h", Some(255), stderr).map(|f| f.kind)
    }

    #[test]
    fn classifies_real_openssh_stderr() {
        use SshFailureKind::*;
        let cases: &[(&str, SshFailureKind)] = &[
            ("Host key verification failed.\n", HostKeyUnknown),
            (
                "No ED25519 host key is known for mac.rlt.sk and you have requested strict checking.\r\nHost key verification failed.\r\n",
                HostKeyUnknown,
            ),
            (
                "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
                 @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!\n\
                 Host key for mac.rlt.sk has changed and you have requested strict checking.\n\
                 Host key verification failed.\n",
                HostKeyChanged,
            ),
            ("martin@h: Permission denied (publickey,password).\n", AuthDenied),
            ("a remote tool: Permission denied (x)\n", Unknown),
            ("Permission denied (publickey).\n", AuthDenied),
            (
                "ssh: Could not resolve hostname nope.rlt.sk: nodename nor servname provided, or not known\n",
                DnsFail,
            ),
            ("ssh: connect to host h port 22: Connection refused\n", Refused),
            ("ssh: connect to host h port 22: Operation timed out\n", Timeout),
            ("ssh: connect to host h port 22: Connection timed out\n", Timeout),
            ("ssh: connect to host h port 22: No route to host\n", Timeout),
            ("ssh: connect to host h port 22: Network is unreachable\n", Timeout),
            ("kex_exchange_identification: read: Connection reset by peer\n", Handshake),
            ("Connection closed by 10.0.0.1 port 22\n", Handshake),
            ("mux_client_request_session: read from master failed: Broken pipe\n", MuxBroken),
            ("Control socket connect(/x/cm-h.sock): Connection refused\nsomething else\n", MuxBroken),
            ("bash: line 1: tmux: command not found\n", Unknown),
            ("", Unknown),
        ];
        for (stderr, want) in cases {
            assert_eq!(kind(stderr), Some(*want), "{stderr:?}");
        }
    }

    #[test]
    fn connect_failures_must_be_the_last_line() {
        // A remote command printing ssh-like text earlier is not a connect failure.
        assert_eq!(
            kind("ssh: connect to host h port 22: Connection refused\nlater line\n"),
            Some(SshFailureKind::Unknown)
        );
        // Trailing blank lines and CRs do not hide the last real line.
        assert_eq!(
            kind("noise\r\nHost key verification failed.\r\n\r\n  \n"),
            Some(SshFailureKind::HostKeyUnknown)
        );
    }

    #[test]
    fn a_connect_failure_wins_over_an_earlier_mux_message() {
        // The master was stale, ssh fell back to a fresh connect, which then
        // failed the host key check: the host key is what the user must fix.
        assert_eq!(
            kind("Control socket connect(/x/cm-h.sock): No such file or directory\nHost key verification failed.\n"),
            Some(SshFailureKind::HostKeyUnknown)
        );
    }

    #[test]
    fn only_exit_255_is_an_ssh_failure() {
        assert!(classify("h", Some(0), "Host key verification failed.").is_none());
        assert!(classify("h", Some(1), "Host key verification failed.").is_none());
        assert!(classify("h", None, "Host key verification failed.").is_none());
        let f = classify("mac", Some(255), "Host key verification failed.\n").unwrap();
        assert_eq!(f.ssh_alias, "mac");
        assert_eq!(f.raw_tail, "Host key verification failed.\n");
    }

    #[test]
    fn raw_tail_is_bounded() {
        let many: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let f = classify("h", Some(255), &many).unwrap();
        assert_eq!(f.raw_tail.lines().count(), 40);
        assert!(f.raw_tail.starts_with("line 60\n"));

        let wide = format!("{}\n", "é".repeat(5000)); // 10_000 bytes, 2-byte chars
        let f = classify("h", Some(255), &wide).unwrap();
        assert!(f.raw_tail.len() <= 4096);
        assert!(f.raw_tail.ends_with('\n'));
    }

    #[test]
    fn never_connected_is_every_kind_that_fails_before_auth_completes() {
        use SshFailureKind::*;
        for k in [
            HostKeyUnknown,
            HostKeyChanged,
            AuthDenied,
            DnsFail,
            Refused,
            Timeout,
            Handshake,
        ] {
            assert!(k.never_connected(), "{k:?}");
        }
        for k in [MuxBroken, Unknown] {
            assert!(!k.never_connected(), "{k:?}");
        }
    }

    #[test]
    fn unknown_kind_is_forward_compatible_and_round_trips() {
        assert_eq!(
            serde_json::from_str::<SshFailureKind>("\"future_kind\"").unwrap(),
            SshFailureKind::Unknown
        );
        let round_tripped: SshFailureKind =
            serde_json::from_str(&serde_json::to_string(&SshFailureKind::HostKeyChanged).unwrap())
                .unwrap();
        assert_eq!(round_tripped, SshFailureKind::HostKeyChanged);
    }

    #[test]
    fn serializes_snake_case() {
        let f = classify("mac", Some(255), "Host key verification failed.\n").unwrap();
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["kind"], "host_key_unknown");
        assert_eq!(v["ssh_alias"], "mac");
    }
}
