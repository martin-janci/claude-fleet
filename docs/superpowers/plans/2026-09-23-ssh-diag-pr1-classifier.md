# SSH self-diagnosis — PR 1: shared failure classifier

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One classifier in `fleet-core` turns an ssh client failure (exit
255 + stderr) into a typed `SshFailure`. The two existing ad-hoc stderr
readers use it, and the strict host probe carries it in `E_PROBE` details.

**Architecture:** New module `crates/fleet-core/src/ssh_diag/classify.rs`,
pure functions only, no I/O. `account_usage::connection_never_established`
and `ssh::is_mux_failure` keep their exact behaviour, but read the shared
needle and prefix tables. `hosts::probe_with_token` attaches
`{"ssh_failure": SshFailure}` to `IpcError.details`. No UI change and no wire
contract change (`details` is already an optional JSON value).

**Tech Stack:** Rust (fleet-core), serde, tokio tests with `FakeSsh`.

**Spec:** `docs/superpowers/specs/2026-09-23-ssh-self-diagnosis-design.md`
(this plan is delivery item 1 of 5).

## Global Constraints

- The kind vocabulary is exactly `HostKeyUnknown | HostKeyChanged | AuthDenied | DnsFail | Refused | Timeout | Handshake | MuxBroken | Unknown`, serde `snake_case`.
- Input contract: only exit status 255 is an ssh failure; anything else → `None`.
- Connect-failure kinds match the LAST non-empty stderr line as a prefix (remote stderr is mixed in; see the doc comment at `account_usage.rs:962`). `MuxBroken` matches anywhere (existing `is_mux_failure` semantics).
- `raw_tail`: at most the last 40 lines and at most 4096 bytes, cut on a char boundary.
- Behaviour of `connection_never_established` and `is_mux_failure` must not change, with ONE deliberate exception: `user@host: Permission denied (…)` (OpenSSH 9.x wording) now also counts as never-connected. Otherwise: their existing tests (`account_usage.rs` `connect_failure_detection_is_strict`, `ssh.rs` `mux_failures_are_exit_255_with_a_master_message`) must pass unmodified.
- Build/test: `cargo` needs the Tauri system libs; fleet-core alone is enough here: `cargo test -p fleet-core <filter>`. In a worktree, export `CARGO_TARGET_DIR` to a worktree-specific path before running scripts (the shared target can compile another worktree's crates).

---

### Task 1: `ssh_diag::classify` module

**Files:**
- Create: `crates/fleet-core/src/ssh_diag/mod.rs`
- Create: `crates/fleet-core/src/ssh_diag/classify.rs` (code + `#[cfg(test)] mod tests`)
- Modify: `crates/fleet-core/src/lib.rs` (add `pub mod ssh_diag;` after `pub mod ssh;`, keeping alphabetical order with `ssh_config`)

**Interfaces:**
- Produces:
  - `pub enum SshFailureKind { HostKeyUnknown, HostKeyChanged, AuthDenied, DnsFail, Refused, Timeout, Handshake, MuxBroken, Unknown }` (`Copy`, serde `snake_case`)
  - `impl SshFailureKind { pub fn never_connected(self) -> bool; pub fn summary(self) -> &'static str }`
  - `pub struct SshFailure { pub kind: SshFailureKind, pub ssh_alias: String, pub raw_tail: String }` (serde)
  - `pub fn classify(ssh_alias: &str, exit_code: Option<i32>, stderr: &str) -> Option<SshFailure>`
  - `pub(crate) fn connect_failure_kind(stderr: &str) -> Option<SshFailureKind>`
  - `pub(crate) fn mentions_mux_failure(stderr: &str) -> bool`
  - re-exports from `ssh_diag`: `pub use classify::{classify, SshFailure, SshFailureKind};`

- [ ] **Step 1: Write the module skeleton and the failing tests**

`crates/fleet-core/src/ssh_diag/mod.rs`:

```rust
//! SSH self-diagnosis: classify an ssh client failure, and (later PRs)
//! diagnose it, fix it, and remember the user's standing grants.
//! Design: docs/superpowers/specs/2026-09-23-ssh-self-diagnosis-design.md

pub mod classify;

pub use classify::{classify, SshFailure, SshFailureKind};
```

`crates/fleet-core/src/ssh_diag/classify.rs`, with an empty body first so the
tests fail to compile and then fail:

```rust
//! The one reader of ssh's OWN error wording.
//!
//! ssh's stderr also carries the remote command's stderr, so connect-failure
//! kinds match only the LAST non-empty line, as a prefix. A broken
//! ControlMaster is recognised anywhere in the text, as `is_mux_failure`
//! always did.

use serde::{Deserialize, Serialize};

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
        for k in [HostKeyUnknown, HostKeyChanged, AuthDenied, DnsFail, Refused, Timeout, Handshake] {
            assert!(k.never_connected(), "{k:?}");
        }
        for k in [MuxBroken, Unknown] {
            assert!(!k.never_connected(), "{k:?}");
        }
    }

    #[test]
    fn serializes_snake_case() {
        let f = classify("mac", Some(255), "Host key verification failed.\n").unwrap();
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["kind"], "host_key_unknown");
        assert_eq!(v["ssh_alias"], "mac");
    }
}
```

Note the two `Permission denied` cases. OpenSSH 9.x prefixes the line with
`user@host: `, and the classifier accepts exactly that shape (one `@`-bearing
token with no spaces before `: `). Anything else before `Permission denied`
is some remote tool's stderr and stays `Unknown`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core ssh_diag::classify`
Expected: compile errors (`cannot find function classify`, `SshFailureKind`).

- [ ] **Step 3: Implement**

Insert above the `#[cfg(test)]` block in `classify.rs`:

```rust
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
    /// Exit 255 with wording none of the above recognise.
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
        return Some(if changed { HostKeyChanged } else { HostKeyUnknown });
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
        return Some(if rest.ends_with("Connection refused") { Refused } else { Timeout });
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
```

Then add `pub mod ssh_diag;` to `crates/fleet-core/src/lib.rs` directly after
`pub mod ssh_config;` (line 29).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p fleet-core ssh_diag::classify`
Expected: 7 passed. Check the output says `7 passed`, not only the exit code.

- [ ] **Step 5: Clippy + commit**

Run: `cargo clippy -p fleet-core --all-targets -- -D warnings`
Expected: no warnings.

```bash
git add crates/fleet-core/src/ssh_diag crates/fleet-core/src/lib.rs
git commit -m "feat(ssh_diag): one classifier for ssh client failures"
```

---

### Task 2: `is_mux_failure` and `connection_never_established` read the shared tables

**Files:**
- Modify: `crates/fleet-core/src/ssh.rs:1533-1553` (`is_mux_failure`)
- Modify: `crates/fleet-core/src/service/account_usage.rs:962-990` (`connection_never_established`)

**Interfaces:**
- Consumes: `crate::ssh_diag::classify::{mentions_mux_failure, connect_failure_kind}` from Task 1.
- Produces: nothing new. Both functions keep their signatures and behaviour.

- [ ] **Step 1: Confirm the existing guard tests pass before the change**

Run: `cargo test -p fleet-core mux_failures_are_exit_255_with_a_master_message`
Run: `cargo test -p fleet-core connect_failure_detection_is_strict`
Expected: both pass. These are the regression tests for this task; do not
edit their existing cases.

- [ ] **Step 2: Rewire `is_mux_failure`**

Replace the body of `is_mux_failure` in `ssh.rs` (keep the doc comment and
signature):

```rust
pub(crate) fn is_mux_failure(out: &Output) -> bool {
    out.status.code() == Some(255)
        && crate::ssh_diag::classify::mentions_mux_failure(&String::from_utf8_lossy(&out.stderr))
}
```

The needle list and its "not bare Broken pipe" comment now live in
`ssh_diag/classify.rs` (`MUX_NEEDLES`). Delete the inline array.

- [ ] **Step 3: Rewire `connection_never_established`**

Replace its body in `account_usage.rs` (keep the doc comment, and add one
line to it: "The ssh wording lives in `ssh_diag::classify`."):

```rust
fn connection_never_established(stdout: &str, stderr: &str) -> bool {
    if stdout
        .lines()
        .any(|l| l.trim_end_matches('\r') == MARK_USAGE_START)
    {
        return false;
    }
    crate::ssh_diag::classify::connect_failure_kind(stderr).is_some()
}
```

This is behaviour-identical except for the OpenSSH 9.x
`user@host: Permission denied (…)` line, which now also counts as
never-connected (correct: auth failed, nothing ran). Every former `PREFIXES` entry maps to a kind,
the `Connection closed by … port` rule is the `Handshake` arm, and the
last-line rule is the same. `connect_failure_kind` only returns kinds for
which `never_connected()` is true; Task 1's test pins that.

- [ ] **Step 4: Run the regression tests plus the full fleet-core suite**

Run: `cargo test -p fleet-core mux_failures_are_exit_255_with_a_master_message`
Run: `cargo test -p fleet-core connect_failure_detection_is_strict`
Run: `cargo test -p fleet-core`
Expected: all pass. Read the summary line of the full run; do not pipe it
through `tail`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/ssh.rs crates/fleet-core/src/service/account_usage.rs
git commit -m "refactor(ssh): mux and usage-fallback checks read the shared classifier"
```

---

### Task 3: strict probe carries the classified failure

**Files:**
- Modify: `crates/fleet-core/src/service/hosts.rs:407-416` (`probe_with_token`, non-success branch)
- Test: `crates/fleet-core/src/service/hosts.rs` test module (next to `add_host_unreachable_is_e_probe_and_persists_nothing`, ~line 1347)

**Interfaces:**
- Consumes: `crate::ssh_diag::classify(host, code, stderr) -> Option<SshFailure>`.
- Produces: the `E_PROBE` `IpcError.details == {"ssh_failure": {kind, ssh_alias, raw_tail}}` when ssh exits 255. PR 2/3 read this shape; the message text keeps its current form plus a summary.

- [ ] **Step 1: Write the failing test**

Add to the `FakeSsh` test section of `hosts.rs`:

```rust
    #[tokio::test]
    async fn add_host_host_key_failure_carries_the_classified_kind() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        fake.on_host(
            "hk.example",
            Match::Any,
            Reply::fail(255, "Host key verification failed.\r\n"),
        );
        let err = add_host(
            AddHostArgs {
                alias: "hk".into(),
                ssh_alias: "hk.example".into(),
                transport: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_PROBE");
        assert!(
            err.message.contains("host key not in known_hosts"),
            "summary in the message: {}",
            err.message
        );
        assert!(
            err.message.contains("Host key verification failed."),
            "ssh's own line survives: {}",
            err.message
        );
        let failure = &err.details.as_ref().expect("details")["ssh_failure"];
        assert_eq!(failure["kind"], "host_key_unknown");
        assert_eq!(failure["ssh_alias"], "hk.example");
    }
```

`ssh_alias` here is the ssh alias the probe dialled (`probe_with_token`'s
`host`), which is the name the ssh config and known_hosts know it by.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleet-core add_host_host_key_failure_carries_the_classified_kind`
Expected: FAIL on the `summary in the message` assertion. If instead the
fake's host rule does not win over `fake_fleet()`'s script rule, the test
passes the probe. In that case copy the ordering that
`add_host_unreachable_is_e_probe_and_persists_nothing` uses (it calls
`fake.unreachable(...)` after `fake_fleet()`), and re-run until the failure
is the assertion.

- [ ] **Step 3: Implement**

Replace the non-success branch in `probe_with_token`:

```rust
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let failure = crate::ssh_diag::classify(host, out.status.code(), &stderr);
        let summary = failure
            .as_ref()
            .map(|f| format!("{}; ", f.kind.summary()))
            .unwrap_or_default();
        let mut err = IpcError::new(
            codes::E_PROBE,
            format!(
                "ssh {host}: {summary}exited {:?}: {}",
                out.status.code(),
                stderr.trim()
            ),
        );
        if let Some(f) = failure {
            err.details = Some(serde_json::json!({ "ssh_failure": f }));
        }
        return Err(err);
    }
```

This changes the message from `ssh {host} exited …` to
`ssh {host}: <summary>; exited …`. The existing test at ~line 1363 asserts
`contains("exited Some(255)")`, which still holds. Grep the frontend for a
parser of that message before committing:

Run: `grep -rn "exited Some" src/ crates/ src-tauri/ --include=*.ts --include=*.svelte --include=*.rs`
Expected: only Rust test assertions using `contains`. If any code parses the
prefix `ssh <host> exited`, keep the old prefix and append the summary at the
end instead.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p fleet-core add_host_host_key_failure_carries_the_classified_kind`
Run: `cargo test -p fleet-core service::hosts`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/hosts.rs
git commit -m "feat(hosts): E_PROBE carries the classified ssh failure"
```

---

### Task 4: full local CI and PR

**Files:** none new.

- [ ] **Step 1: Full CI in CI order**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/ssh-diag-pr1
scripts/ci-local.sh --rust-only
```

Expected: fmt, clippy (`-D warnings`), `cargo test --workspace`, and
`cargo deny` all green. Read the whole output, not `| tail`. If
`control-api-reference` or hub verdict generation complains, something
outside this plan was touched: stop and report.

- [ ] **Step 2: Push the branch and open the PR**

```bash
git push -u origin HEAD
gh pr create --base main --title "feat(ssh_diag): shared ssh failure classifier (SSH self-diagnosis 1/5)" --body-file <(cat <<'EOF'
First of five PRs for SSH self-diagnosis (spec: docs/superpowers/specs/2026-09-23-ssh-self-diagnosis-design.md).

- New `fleet_core::ssh_diag::classify`: exit 255 + stderr → typed `SshFailure` (host_key_unknown, host_key_changed, auth_denied, dns_fail, refused, timeout, handshake, mux_broken, unknown).
- `is_mux_failure` and usage-fallback's `connection_never_established` read the shared tables, with no behaviour change (their existing tests are untouched).
- Strict probe `E_PROBE` now says what failed and carries `details.ssh_failure`.

No UI change. Triggered by the 2026-09-23 `Host key verification failed.` on host `mac`.
EOF
)
```

- [ ] **Step 3: Wait for GitHub CI on the final head**

CI clippy is newer than local, so a locally clean run can still fail there.
Do not merge. Merging needs the user's explicit go.
