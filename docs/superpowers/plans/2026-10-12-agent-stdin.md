# Agent stdin (CF-3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A host reached through `fleet-agent` can run a command with input on
stdin (`SshExec::run_with_stdin`), so a tracker's `via_host:` / `via_cli:`
request and the haiku benchmark prompt reach it the way they reach an SSH
host, instead of `E_UNSUPPORTED: <host> is reached through fleet-agent, which
cannot pipe stdin`.

**Architecture:** The wire already carries it: `HubFrame::Exec { stdin:
Option<String> }` has been in the protocol since proto 1 and
`fleet-agent`'s `exec::execute` writes it to the child and closes the pipe.
Only the hub side never sends it. `AgentTransport` gains the
`run_with_stdin` override (text only, as the frame is; refused before the
wire when the bytes are not UTF-8 or past the payload limit),
`SshClient::run_with_stdin` routes an agent host to it, and the two guards
that existed only because of the gap (a tracker may not use an agent host;
an agent host may not be a tracker host) go. No proto bump: every agent the
hub accepts (`MIN_SUPPORTED_PROTO == 1`) already understands the field.

**Tech Stack:** Rust (fleet-core `agent/transport.rs`, `ssh.rs`,
`service/trackers/admin.rs`, `service/hosts.rs`; fleet-proto; the
`fleet-agent-e2e` crate), docs.

**Spec:** the ticket CF-3 (fleet handover #3323): "mefistos is reached
through fleet-agent, which cannot pipe stdin; run the command there and
paste the code in that terminal". The error text is `ssh.rs:565`.

## Global Constraints

- Validate with the ladder in `CLAUDE.md`: `cargo fleet-fast-check` after
  every edit, `cargo fleet-check` at a checkpoint, `cargo fleet-test --
  <filter>` for what you touched, `cargo fmt --all --check` and
  `cargo fleet-lint` before committing.
- The agent protocol's stdin is **text** (`Option<String>`); widening to
  bytes needs a `stdin_b64` field and a proto bump, which this plan does not
  do. Non-UTF-8 input is refused with `E_UNSUPPORTED` before anything is
  sent.
- Nothing oversize reaches the wire: refuse stdin past
  `fleet_proto::MAX_PAYLOAD_BYTES` from its length alone.
- Every value interpolated into a shell command is quoted with
  `crate::shell::quote`. (This plan adds no new command strings.)
- Commits are Conventional Commits; the branch is `cf-3-fix-art-stdin`.

---

### Task 1: `AgentTransport::run_with_stdin`

**Files:**
- Modify: `crates/fleet-core/src/agent/transport.rs:1-18` (module doc: "the same seven methods" → eight), `:56-110` (`exec` takes `stdin`), `:235-325` (the trait impl gains `run_with_stdin`), tests at `:430-500`.
- Modify: `crates/fleet-proto/src/lib.rs:328-333` (the `stdin` field doc no longer says nothing sends it).

**Interfaces:**
- Consumes: `HubFrame::Exec { id, argv, stdin: Option<String>, timeout_ms, cap_bytes }` (fleet-proto), `fleet_proto::MAX_PAYLOAD_BYTES`, `crate::agent::fake::FakeAgent`.
- Produces: `impl SshExec for AgentTransport` now overrides `async fn run_with_stdin(&self, host: &str, args: &[&str], stdin: Vec<u8>, _connect_timeout: Duration, wall_clock: Duration, max_output: usize) -> Result<Output, IpcError>`; a private `fn stdin_text(host: &str, stdin: Vec<u8>) -> Result<String, IpcError>`.

- [ ] **Step 1: Write the failing tests** (in `mod tests` of `transport.rs`, next to `run_hands_the_agent_a_shell_command_not_the_callers_argv`)

```rust
    // ── stdin ───────────────────────────────────────────────────────────────

    /// `run_with_stdin` is the one method that sets `Exec.stdin`: the bytes
    /// go as the frame's text, the args take the same shell shape as `run`,
    /// and the output cap rides as `cap_bytes` the way `run_bounded_capped`
    /// sends it.
    #[tokio::test]
    async fn run_with_stdin_hands_the_agent_the_text_and_the_cap() {
        let (t, agent) = setup(answer_with(0, b"echoed", b""));
        let quoted = quote("cat");
        let out = t
            .run_with_stdin(
                "laptop",
                &["bash", "-lc", &quoted],
                b"a prompt\n".to_vec(),
                Duration::from_secs(5),
                Duration::from_secs(30),
                4096,
            )
            .await
            .unwrap();
        assert_eq!(out.stdout, b"echoed");
        let (_, argv, stdin, timeout_ms, cap) = as_exec(agent.only_frame());
        assert_eq!(
            argv,
            vec!["bash".to_string(), "-c".to_string(), format!("bash -lc {quoted}")]
        );
        assert_eq!(stdin.as_deref(), Some("a prompt\n"), "the bytes ride as text");
        assert_eq!(timeout_ms, 30_000, "the caller's wall clock, no connect step");
        assert_eq!(cap, Some(4096));
    }

    /// The frame's stdin is text. Bytes that are not UTF-8 cannot be sent
    /// faithfully (a lossy conversion would change what the command reads),
    /// so they are refused before any frame is built — and the refusal
    /// names the transport, as the SSH path's `E_UNSUPPORTED` used to.
    #[tokio::test]
    async fn run_with_stdin_refuses_bytes_that_are_not_text_without_sending() {
        let (t, agent) = setup(answer_exit(0));
        let err = t
            .run_with_stdin(
                "laptop",
                &["cat"],
                vec![0xff, 0xfe, b'x'],
                Duration::from_secs(5),
                Duration::from_secs(5),
                1024,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_UNSUPPORTED, "{err:?}");
        assert!(err.message.contains("UTF-8"), "{}", err.message);
        assert!(agent.sent().is_empty(), "nothing reaches the wire");
    }

    /// Stdin travels in one frame, so it is bound by the payload limit —
    /// judged from its length, before the UTF-8 check walks it.
    #[tokio::test]
    async fn run_with_stdin_refuses_an_oversize_input_without_sending() {
        let (t, agent) = setup(answer_exit(0));
        // Zeroed, so the allocation is lazy pages, not a 200 MiB memset.
        let big = vec![0u8; fleet_proto::MAX_PAYLOAD_BYTES + 1];
        let err = t
            .run_with_stdin(
                "laptop",
                &["cat"],
                big,
                Duration::from_secs(5),
                Duration::from_secs(5),
                1024,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_UNSUPPORTED, "{err:?}");
        assert!(err.message.contains("200 MiB"), "{}", err.message);
        assert!(agent.sent().is_empty(), "nothing oversized reaches the wire");
    }
```

Also change the existing assertion in `run_hands_the_agent_a_shell_command_not_the_callers_argv`:

```rust
        assert_eq!(stdin, None, "`run` sends no stdin; only `run_with_stdin` does");
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo fleet-test -- agent::transport::tests::run_with_stdin`
Expected: FAIL — the trait's default answers `E_UNSUPPORTED` with
"this transport cannot pipe stdin", so the first test's `unwrap()` panics
and the second's `contains("UTF-8")` fails.

- [ ] **Step 3: Implement**

In `exec`, add a `stdin: Option<String>` parameter (after `args`) and send
it; every existing caller passes `None`:

```rust
    async fn exec(
        &self,
        host: &str,
        args: &[&str],
        stdin: Option<String>,
        wall_clock: Duration,
        cap: Option<usize>,
        token: Option<CancellationToken>,
    ) -> Result<Output, IpcError> {
        let id = uuid::Uuid::new_v4().to_string();
        // Every frame this transport builds — here and in `upload_file` below
        // — is one an agent at `proto` 1 already understands (`stdin`
        // included: the field and the agent's write of it are as old as
        // proto 1), so nothing is gated on
        // `self.registry.negotiated_proto(host)`. …(rest of the comment unchanged)
        let frame = HubFrame::Exec {
            id: id.clone(),
            argv: shell_argv(args),
            stdin,
            timeout_ms: wall_clock.as_millis() as u64,
            cap_bytes: cap.map(|c| c as u64),
        };
```

Add the helper next to `upload_mode`:

```rust
/// What `run_with_stdin` may send: the frame's `stdin` is text, so the bytes
/// must be UTF-8 (a lossy conversion would change what the command reads,
/// and the one secret-bearing caller, `via_host`'s curl header block, must
/// arrive byte for byte), and it travels in one frame, so it is bound by the
/// payload limit — judged from the length first, before the UTF-8 check
/// walks the bytes. Both refusals happen before any frame is built, so
/// nothing refused reaches the wire. Widening to arbitrary bytes needs a
/// `stdin_b64` field and a proto bump (see `HubFrame::Exec`).
fn stdin_text(host: &str, stdin: Vec<u8>) -> Result<String, IpcError> {
    if stdin.len() > fleet_proto::MAX_PAYLOAD_BYTES {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            format!(
                "stdin for {host} is {} MiB: the agent transport carries at most {} MiB in one frame",
                stdin.len() / (1024 * 1024),
                fleet_proto::MAX_PAYLOAD_BYTES / (1024 * 1024),
            ),
        ));
    }
    String::from_utf8(stdin).map_err(|_| {
        IpcError::new(
            codes::E_UNSUPPORTED,
            format!("stdin for {host} is not UTF-8: the agent transport carries stdin as text"),
        )
    })
}
```

In `impl SshExec for AgentTransport`, after `run_bounded_cancellable`:

```rust
    /// `run_bounded_capped` with the bytes on the child's stdin: the one
    /// method that sets `Exec.stdin`, which the agent writes to the child
    /// and closes. Text only — see [`stdin_text`] for what is refused.
    async fn run_with_stdin(
        &self,
        host: &str,
        args: &[&str],
        stdin: Vec<u8>,
        _connect_timeout: Duration,
        wall_clock: Duration,
        max_output: usize,
    ) -> Result<Output, IpcError> {
        let text = stdin_text(host, stdin)?;
        self.exec(host, args, Some(text), wall_clock, Some(max_output), None)
            .await
    }
```

Module doc, line 4: "the same seven methods" → "the same eight methods".

In `crates/fleet-proto/src/lib.rs:328-333`, replace the field doc:

```rust
        /// Written to the child's stdin and closed. Absent means no stdin.
        /// Text only: the hub's `AgentTransport::run_with_stdin` refuses
        /// non-UTF-8 input before sending (a tracker request body or a
        /// `claude -p` prompt is always text). If a caller ever needs
        /// non-UTF-8 stdin, this has to become a `stdin_b64` — it cannot be
        /// widened in place.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fleet-fast-check && cargo fleet-test -- agent::transport`
Expected: every `agent::transport` test passes, the three new ones included.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/agent/transport.rs crates/fleet-proto/src/lib.rs
git commit -m "feat(agent): AgentTransport pipes stdin through the exec frame"
```

---

### Task 2: `SshClient::run_with_stdin` routes an agent host

**Files:**
- Modify: `crates/fleet-core/src/ssh.rs:540-581` (the method and its doc).
- Modify: `crates/fleet-core/src/agent/router.rs:19` (module doc), `:357-404` (the coverage test), plus one new test.

**Interfaces:**
- Consumes: Task 1's `AgentTransport::run_with_stdin`.
- Produces: `SshClient::run_with_stdin` on an agent host returns the agent's output instead of `E_UNSUPPORTED`.

- [ ] **Step 1: Write the failing tests** (in `mod tests` of `router.rs`, after `every_transport_method_reaches_the_agent`)

Extend `every_transport_method_reaches_the_agent`: after the `upload_file`
call add

```rust
        ssh.run_with_stdin("laptop", &a, b"in\n".to_vec(), CONNECT, WALL, 1024)
            .await
            .unwrap();
```

and change the final count to `8` with the message `"all eight methods route: {:?}"`.
Rename the section comment at `:357` to `// ── the eight methods ──…`.

Add:

```rust
    /// The stdin path routes too, with the bytes on the frame — the one
    /// `SshExec` method the agent transport did not serve until CF-3, when
    /// a tracker on an agent host failed every request with
    /// `E_UNSUPPORTED`.
    #[tokio::test]
    async fn run_with_stdin_reaches_the_agent_with_the_bytes() {
        let (ssh, reg, _store) = hub_client(&[("laptop", None, "agent")]);
        let agent = FakeAgent::connect(&reg, "laptop", fake::answer_with(0, b"ok", b""));
        let a = script();
        let out = ssh
            .run_with_stdin("laptop", &args(&a), b"the body".to_vec(), CONNECT, WALL, 1024)
            .await
            .unwrap();
        assert_eq!(out.stdout, b"ok");
        match agent.only_frame() {
            fleet_proto::HubFrame::Exec { stdin, .. } => {
                assert_eq!(stdin.as_deref(), Some("the body"));
            }
            other => panic!("expected an exec frame, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo fleet-test -- agent::router::tests`
Expected: `run_with_stdin_reaches_the_agent_with_the_bytes` and
`every_transport_method_reaches_the_agent` FAIL with `E_UNSUPPORTED …
cannot pipe stdin`.

- [ ] **Step 3: Implement** (`ssh.rs:540-581`)

```rust
    /// `run_bounded_capped` with `stdin` written to the remote command and
    /// then closed (see [`SshExec::run_with_stdin`]). Not retried on a mux
    /// failure: the bytes may already have been consumed. A fleet-agent host
    /// takes it through its exec frame (text only; see
    /// `AgentTransport::run_with_stdin`). The `local` host is a local spawn
    /// under [`ensure_local_allowed`], as in [`run_shell`] — never
    /// `ssh local`, which resolves nothing (or, with an `~/.ssh/config`
    /// entry of that name, the wrong machine).
    ///
    /// [`ensure_local_allowed`]: crate::service::hub::ensure_local_allowed
    pub async fn run_with_stdin(
        &self,
        host: &str,
        args: &[&str],
        stdin: Vec<u8>,
        connect_timeout: Duration,
        wall_clock: Duration,
        max_output: usize,
    ) -> Result<Output, IpcError> {
        if host == crate::service::projects::LOCAL_HOST {
            crate::service::hub::ensure_local_allowed(host)?;
            return run_local_with_stdin(args, stdin, wall_clock, max_output).await;
        }
        if let Some(alias) = self.agent_route(host) {
            return self
                .agent()
                .run_with_stdin(&alias, args, stdin, connect_timeout, wall_clock, max_output)
                .await;
        }
        let mux_opts = self.prepare(host, connect_timeout).await;
        // …unchanged from here
```

`router.rs:19`: "the seven `SshExec` methods" → "the eight `SshExec` methods".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fleet-fast-check && cargo fleet-test -- agent::`
Expected: all `agent::` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/ssh.rs crates/fleet-core/src/agent/router.rs
git commit -m "fix(ssh): route run_with_stdin to a fleet-agent host instead of refusing"
```

---

### Task 3: End to end against the real agent

**Files:**
- Modify: `crates/fleet-agent-e2e/src/agent_e2e.rs:243-253` (after the `bash -c` script check in `hub_and_agent_talking_over_a_real_socket`).

**Interfaces:**
- Consumes: `SshExec::run_with_stdin` on `hub.ssh` (Tasks 1–2).

- [ ] **Step 1: Write the failing test** — insert after the "children start in the agent's home" assertion:

```rust
    // Stdin reaches the child and is closed after it: `cat` reads the bytes
    // and gets EOF, so the script finishes on its own.
    let script = fleet_core::shell::quote("cat; printf ' <- on stdin'");
    let out = SshExec::run_with_stdin(
        &hub.ssh,
        "laptop",
        &["bash", "-c", &script],
        b"piped".to_vec(),
        PATIENCE,
        PATIENCE,
        1024,
    )
    .await
    .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "piped <- on stdin");
```

- [ ] **Step 2: Run it to verify it fails on the pre-fix code** — not possible without reverting Tasks 1–2; instead confirm the test is actually exercised:

Run: `cargo fleet-test -- hub_and_agent_talking_over_a_real_socket`
Expected: PASS, and `1 passed` in the `fleet_agent_e2e` section of the output.

- [ ] **Step 3: Commit**

```bash
git add crates/fleet-agent-e2e/src/agent_e2e.rs
git commit -m "test(agent-e2e): stdin reaches the real agent's child and is closed"
```

---

### Task 4: Lift the two guards that existed only for the gap

**Files:**
- Modify: `crates/fleet-core/src/service/trackers/admin.rs:343-380` (remove `check_host_transport` and `refuse_agent_transport_on_tracker_host`), `:477`, `:586` (their calls), and the tests `a_host_side_transport_on_a_fleet_agent_host_is_refused` (`:807-860`) plus whichever test covers `refuse_agent_transport_on_tracker_host` (find it with `grep -n 'agent' crates/fleet-core/src/service/trackers/admin.rs crates/fleet-core/src/service/hosts.rs`).
- Modify: `crates/fleet-core/src/service/hosts.rs:180-190` (the `add_host` call and its comment).

**Interfaces:**
- Consumes: nothing new — the guards' only reason (`E_UNSUPPORTED` on every request) is gone after Task 2.
- Produces: `work_admin { add | update, transport: "via_host:<agent host>" }` and `add_host { transport: "agent" }` on a tracker's host succeed.

- [ ] **Step 1: Rewrite the refusal test into its inverse** (replace `a_host_side_transport_on_a_fleet_agent_host_is_refused`)

```rust
    /// A host reached through fleet-agent runs curl or gh with the request
    /// on stdin like any SSH host (CF-3: the agent transport pipes stdin),
    /// so `via_host` / `via_cli` on it is accepted at add and update time.
    #[test]
    fn a_host_side_transport_on_a_fleet_agent_host_is_accepted() {
        let (st, id) = added();
        {
            let s = st.lock().unwrap();
            s.upsert_host("agentbox").unwrap();
            s.set_host_transport("agentbox", "agent").unwrap();
        }
        admin_sync(
            &WorkAdminArgs {
                site_url: Some("https://beta.atlassian.net".into()),
                transport: Some("via_host:agentbox".into()),
                ..args("add")
            },
            &st,
        )
        .unwrap();
        admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                transport: Some("via_host:agentbox".into()),
                ..args("update")
            },
            &st,
        )
        .unwrap();
        assert_eq!(
            st.lock().unwrap().require_tracker(id).unwrap().transport,
            "via_host:agentbox"
        );
    }
```

If a test asserts `add_host { transport: "agent" }` is refused for a tracker
host, invert it the same way (the call succeeds and the host row reads
`transport == "agent"`); keep its name's shape, ending in `_is_accepted`.

- [ ] **Step 2: Run to verify the inverted tests fail**

Run: `cargo fleet-test -- fleet_agent_host_is_accepted`
Expected: FAIL with `E_INVALID … is reached through fleet-agent`.

- [ ] **Step 3: Remove the guards**

Delete `check_host_transport` and `refuse_agent_transport_on_tracker_host`
with their doc comments, the two `check_host_transport(…)?;` lines
(`:477`, `:586`), and the `refuse_agent_transport_on_tracker_host(&s,
&args.alias)?;` line with its comment in `hosts.rs` (`:182-187`). Keep the
`cli` arm above `check_host_transport` (`no trusted CLI is known…`): that
one is about the provider, not the host.

- [ ] **Step 4: Verify**

Run: `cargo fleet-check && cargo fleet-test -- trackers::admin && cargo fleet-test -- service::hosts`
Expected: all pass; `cargo fleet-check` shows no dead-code warning for the removed functions.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/trackers/admin.rs crates/fleet-core/src/service/hosts.rs
git commit -m "feat(trackers): allow via_host / via_cli on a fleet-agent host"
```

---

### Task 5: Docs and the stale comments

**Files:**
- Modify: `docs/hub.md:708-712` ("A child's stdin is `/dev/null`.").
- Modify: `docs/decisions.md:816-817` (the haiku benchmark's host rule).
- Modify: `crates/fleet-core/src/service/decide/haiku.rs:86-87` (`reason::SSH_ERROR` doc).
- Modify: `crates/fleet-core/src/ssh.rs:1455-1460` (the trait method doc still says "only transports that can pipe stdin opt in" — fine; add "`SshClient`, `LocalExec`, `FakeSsh` and `AgentTransport` do").

- [ ] **Step 1: Edit the four spots**

`docs/hub.md`: replace "A child's stdin is `/dev/null`." with

```
A child's stdin is `/dev/null`, unless the hub pipes input (a tracker
request body for `via_host:` / `via_cli:`, the haiku benchmark's prompt),
which the agent writes to the child and closes.
```

`docs/decisions.md:816-817`: replace "The host must be reachable over SSH
from where the benchmark runs (an agent-only host is not: its transport
cannot pipe stdin)." with "The host must be reachable from where the
benchmark runs, over SSH or through its fleet-agent."

`haiku.rs:86-87`: replace "SSH failed (unreachable host, spawn failure, or a
transport that cannot pipe stdin: a host reached only through fleet-agent)."
with "SSH or the agent transport failed (unreachable host, spawn failure,
a prompt the agent frame cannot carry)."

- [ ] **Step 2: Check nothing generated changed**

Run: `grep -rn 'cannot pipe stdin\|cannot run curl or gh' crates docs src --include='*.rs' --include='*.md' --include='*.ts' --include='*.svelte'`
Expected: only the trait default at `ssh.rs` ("this transport cannot pipe
stdin to a command") and historical plan documents under
`docs/superpowers/plans/` remain.

- [ ] **Step 3: Full validation and commit**

Run: `cargo fmt --all --check && cargo fleet-lint && cargo fleet-test -- agent:: && cargo fleet-test -- trackers && cargo fleet-test -- decide::haiku`
Expected: clean.

```bash
git add docs/hub.md docs/decisions.md crates/fleet-core/src/service/decide/haiku.rs crates/fleet-core/src/ssh.rs
git commit -m "docs: a fleet-agent host takes stdin"
```

---

## Self-review

- **Coverage:** the ticket's one requirement (stdin on an agent host) is
  Tasks 1–2; proof against the real agent is Task 3; the refusals that
  turned the gap into a user-visible wall (the tracker guards) are Task 4;
  every sentence that documented the gap is Task 5.
- **Out of scope, on purpose:** a `stdin_b64` byte path and a proto bump
  (no caller needs bytes); the desktop's terminal attach to an agent host
  (`TerminalView.svelte`, a different limitation: no SSH route for the PTY).
- **Types:** `run_with_stdin(host, args, stdin: Vec<u8>, connect_timeout, wall_clock, max_output)` is the trait's signature everywhere; `exec`'s new parameter is `stdin: Option<String>` in position 3; `stdin_text(host: &str, stdin: Vec<u8>) -> Result<String, IpcError>`.
