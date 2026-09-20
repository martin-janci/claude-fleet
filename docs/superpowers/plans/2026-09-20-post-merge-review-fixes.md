# Post-merge Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the ten findings and three simplifications from the post-merge review of the Transfer sheet and Conversations background work, plus the repository hygiene items.

**Architecture:** Ten independent, single-file-owner tasks on one branch, one commit each. Rust first (the flaky test is fixed before anything else so the suite is trustworthy), then the shared TypeScript store, then the three Svelte components that read it. Only Task 6 depends on another task (Task 5 exports the helper it reuses); every other task can be reordered freely.

**Tech Stack:** Rust (fleet-core, workspace), Svelte 5 runes + TypeScript, Vitest, `@testing-library/svelte`.

**Spec:** `docs/specs/2026-09-20-post-merge-review.md` — every task below cites the finding it closes (F1–F10, S1–S3, H1–H3). Read the spec's finding before starting a task; it carries the reasoning the code comment should not repeat.

## Global Constraints

- **Branch:** all work lands on `fix/post-merge-review`, cut from `origin/main`. The repo lands work as merge commits, not squashes.
- **Commit messages:** Conventional Commits, lowercase subject, no trailing attribution lines of any kind.
- **Cargo target dir:** `cargo` is a zsh *function* in this environment that redirects the target directory; a non-interactive shell bypasses it and would silently use a different target. Export it yourself before any cargo command: `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet`.
- **Frontend runners:** `pnpm test` and `pnpm check` fail here (binaries not on PATH). Use `npx vitest run` and `npx svelte-check`.
- **Never judge a test run through `| tail` or `| head`.** Read the whole result line. A filtered per-task run is not evidence the suite is green.
- **Full gate before the PR** (all five, in this order):
  ```bash
  export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace --no-fail-fast
  npx svelte-check --output human
  npx vitest run
  ```
  `--no-fail-fast` is required: without it a single failing target stops the run and silently skips the rest (this is F1's second-order cost).
- **Doc regeneration** — after any `#[tool(...)]` description edit:
  ```bash
  REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
  ```
  Never hand-edit `docs/control-api-reference.md`.
- **MCP description budget:** `the_served_definition_budget_stays_bounded` caps the served tool surface at 56,000 bytes. Any added description text must be one slim sentence, and that test must be run.
- **Wire types:** a Rust `Option<T>` is `value | null` in TypeScript, snake_case on the wire, no serde renames.

---

### Task 1: The offline-agent test stops measuring scheduler latency (F1)

**Files:**
- Modify: `crates/fleet-core/src/agent/registry.rs:474-491`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. This task only makes the suite trustworthy for the tasks after it.

- [ ] **Step 1: Reproduce the flake**

The test passes in isolation, so reproduce it the way it actually fails — under the full suite's load:

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test --workspace --no-fail-fast 2>&1 | grep -E "^test result|waited .* before reporting"
```

Expected: either a pass (the flake is intermittent — it reproduced once in two runs here) or:

```
waited 305.552666ms before reporting an offline agent
```

Do not spend more than two runs on this. The bug is structural and visible in the source; a reproduction is a bonus, not a gate.

- [ ] **Step 2: Widen the bound and say what it is for**

Replace the doc comment and the assertion at `crates/fleet-core/src/agent/registry.rs:474-491`:

```rust
    // ── offline ─────────────────────────────────────────────────────────────

    /// The headline requirement: no live agent fails *now*, not after the
    /// timeout. The bound only has to separate "returned immediately" from
    /// "errored after the 60 s budget" — it is not a latency measurement, so
    /// it is loose enough that a loaded machine cannot fail it. A tight bound
    /// here used to redden the whole suite, which then skipped every target
    /// after this one.
    #[tokio::test]
    async fn a_request_with_no_connection_is_offline_immediately() {
        let reg = AgentRegistry::new();
        let started = Instant::now();
        let err = reg
            .request("ghost", exec("1"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?} before reporting an offline agent (the budget was 60s)",
            started.elapsed()
        );
    }
```

- [ ] **Step 3: Run the test**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib a_request_with_no_connection_is_offline_immediately
```

Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Confirm the whole suite now completes every target**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test --workspace --no-fail-fast 2>&1 | grep -E "^test result|Running"
```

Expected: a `test result: ok` line for every binary, including `claude_fleet_lib` (320 tests — this is the target the flake used to skip).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/agent/registry.rs
git commit -m "test(agent): the offline check bounds the budget, not the scheduler"
```

---

### Task 2: A notification's verdict replaces the block's error (F2)

**Files:**
- Modify: `crates/fleet-core/src/service/transcript.rs:937-965` (the `join_notifications` apply loop)
- Test: `crates/fleet-core/src/service/transcript.rs` — the `mod tests` block, beside `the_last_notification_wins_when_an_agent_is_resumed`

**Interfaces:**
- Consumes: nothing.
- Produces: `ConvItem::Subagent.error` and `ConvItem::Tool.error` now reflect the **newest** notification rather than the union of all of them. No signature changes; `src/lib/conversation.ts`'s `statusFromReports` already agrees with the new behaviour.

- [ ] **Step 1: Write the two failing tests**

Add both, after `the_last_notification_wins_when_an_agent_is_resumed`. The existing helpers `jl`, `user`, `asst`, `tool_use` and `task_notification` are already in scope.

```rust
    #[test]
    fn a_resumed_agent_that_recovers_is_no_longer_marked_failed() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T10:00:01Z",
                "toolu_1",
                "Agent",
                serde_json::json!({"description":"d","subagent_type":"general-purpose","prompt":"p"}),
            ),
            task_notification(
                "2026-09-18T10:05:00Z",
                "<task-notification>\n<tool-use-id>toolu_1</tool-use-id>\n<status>failed</status>\n\
                 <summary>s</summary>\n<result>boom</result>\n</task-notification>",
            ),
            asst("retrying"),
            task_notification(
                "2026-09-18T10:20:00Z",
                "<task-notification>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n\
                 <summary>s</summary>\n<result>fixed it</result>\n</task-notification>",
            ),
        ]));
        let ConvItem::Subagent { result, error, .. } = &t[0].items[0] else {
            panic!("expected a subagent, got {:?}", t[0].items[0]);
        };
        assert_eq!(result.as_deref(), Some("fixed it"));
        assert!(
            !*error,
            "the newest report said completed, so the block is not a failure"
        );
    }

    #[test]
    fn a_resumed_agent_that_then_fails_is_marked_failed() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T10:00:01Z",
                "toolu_1",
                "Agent",
                serde_json::json!({"description":"d","subagent_type":"general-purpose","prompt":"p"}),
            ),
            task_notification(
                "2026-09-18T10:05:00Z",
                "<task-notification>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n\
                 <summary>s</summary>\n<result>first pass</result>\n</task-notification>",
            ),
            asst("carry on"),
            task_notification(
                "2026-09-18T10:20:00Z",
                "<task-notification>\n<tool-use-id>toolu_1</tool-use-id>\n<status>failed</status>\n\
                 <summary>s</summary>\n<result>it broke</result>\n</task-notification>",
            ),
        ]));
        let ConvItem::Subagent { result, error, .. } = &t[0].items[0] else {
            panic!("expected a subagent, got {:?}", t[0].items[0]);
        };
        assert_eq!(result.as_deref(), Some("it broke"));
        assert!(*error, "the newest report said failed");
    }
```

- [ ] **Step 2: Run them to verify the first one fails**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib a_resumed_agent_that
```

Expected: `a_resumed_agent_that_recovers_is_no_longer_marked_failed` **FAILS** with
`the newest report said completed, so the block is not a failure`.
`a_resumed_agent_that_then_fails_is_marked_failed` passes already (it guards the direction that must not regress).

If the first test passes, stop — the premise is wrong and the plan needs revisiting.

- [ ] **Step 3: Make the newest notification authoritative**

In `join_notifications`, replace **both** `*error |= failed;` lines (currently at `:949` in the `Subagent` arm and `:961` in the `Tool` arm) with `*error = failed;`, and replace the comment above the apply loop so the rule is written down once.

The loop header comment at `:937` becomes:

```rust
    // Applied in transcript order, so the newest report wins every field —
    // `error` included. A notification arriving at all is proof the launch
    // succeeded, so there is no launch-time error worth carrying forward: an
    // agent that failed, was resumed and then finished is not a failure.
    for ((ti, ii), report, failed, ended) in updates {
```

The `Subagent` arm:

```rust
            Some(ConvItem::Subagent {
                result,
                error,
                ended_at,
                done,
                ..
            }) => {
                if report.is_some() {
                    *result = report;
                }
                *error = failed;
                *done = true;
                *ended_at = ended;
            }
```

The `Tool` arm:

```rust
            // A tool line keeps its own summary — `Bash(command=…)` is the
            // useful text, and the notification's sentence has its own row.
            Some(ConvItem::Tool {
                error,
                ended_at,
                done,
                ..
            }) => {
                *error = failed;
                *done = true;
                *ended_at = ended;
            }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib transcript
```

Expected: `test result: ok`, both new tests among them, and every pre-existing `transcript` test still passing — in particular `the_last_notification_wins_when_an_agent_is_resumed` and `coalesced_notifications_each_close_their_own_call_at_their_own_time`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/transcript.rs
git commit -m "fix(transcript): a background call's newest report decides whether it failed"
```

---

### Task 3: `new_bg_session` validates and documents its requester (F3, F5)

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/session_ops.rs:344-365`
- Modify: `docs/control-api-reference.md` — **regenerated, never hand-edited**
- Test: `crates/fleet-core/src/mcp/tools/tests.rs` — beside `per_host_callers_cannot_spawn_or_dispatch_on_another_host`

**Interfaces:**
- Consumes: `self.resolve_target_row(&caller, Some(id), None, None, what) -> Result<SessionRow, McpError>` from `crates/fleet-core/src/mcp/tools/support.rs:817`. It returns `E_NOTFOUND` for an unknown id and `E_FORBIDDEN` for a row on a host the caller is not scoped to.
- Produces: `new_bg_session` now refuses a `requester_session_id` that does not exist or lives on another host. `NewBgSessionArgs` is unchanged, so no wire, routing or contract change.

- [ ] **Step 1: Write the two failing tests**

Add after `per_host_callers_cannot_spawn_or_dispatch_on_another_host`. `two_host_store`, `test_tools`, `host_caller` and `forbidden` are already in scope.

```rust
/// F3: `dispatch_task` gates `requester_session_id` so an agent cannot file
/// work as somebody else. `new_bg_session` takes the same field and must gate
/// it the same way — the guard fires before any SSH is attempted.
#[tokio::test]
async fn a_background_session_cannot_name_a_requester_on_another_host() {
    let (s, _pid, on_b) = two_host_store();
    let t = test_tools(s);
    let a = host_caller("hosta", TokenMode::Full);
    forbidden(
        t.new_bg_session(
            Extension(a),
            Parameters(crate::service::bg_sessions::NewBgSessionArgs {
                host_alias: "hosta".into(),
                name: "x".into(),
                prompt: "p".into(),
                requester_session_id: Some(on_b),
            }),
        )
        .await
        .unwrap_err(),
    );
}

#[tokio::test]
async fn a_background_session_cannot_name_a_requester_that_does_not_exist() {
    let (s, _pid, _on_b) = two_host_store();
    let t = test_tools(s);
    let err = t
        .new_bg_session(
            Extension(Caller::master()),
            Parameters(crate::service::bg_sessions::NewBgSessionArgs {
                host_alias: "hosta".into(),
                name: "x".into(),
                prompt: "p".into(),
                requester_session_id: Some(9_999),
            }),
        )
        .await
        .unwrap_err();
    assert!(err.message.starts_with("E_NOTFOUND"), "{}", err.message);
}
```

If `Caller` is not already imported in `tests.rs`, add it to the existing `use` of `crate::mcp::auth::…`.

- [ ] **Step 2: Run them to verify they fail**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib a_background_session_cannot_name_a_requester
```

Expected: both FAIL. Without the guard the calls fall through to
`new_bg_session_tracked`, which attempts SSH against a host with no real connection — so the failure will be an SSH error or a panic, not `E_FORBIDDEN`/`E_NOTFOUND`. Either way the assertion does not hold.

- [ ] **Step 3: Add the guard and the description clause**

Replace the whole `#[tool]` block at `crates/fleet-core/src/mcp/tools/session_ops.rs:344-365`:

```rust
    #[tool(description = "Launch a supervised headless (background) Claude \
        session on a host with an initial prompt. Returns JSON with the new \
        claude_session_id AND the fleet row (`session`, registered by an \
        immediate reconcile; the key is absent if the agent was not matched \
        yet — it appears on the next tick) so the next call can be \
        session_transcript { session_id }. The prompt becomes the row's default \
        friendly name and last_prompt. Pass requester_session_id (your own \
        session, from whoami) to list the new session under that session's \
        background work.")]
    pub(super) async fn new_bg_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::bg_sessions::NewBgSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_bg_session",
            &format!("host={} name={}", args.host_alias, args.name),
        );
        require_host(&caller, &args.host_alias, "the new background session")?;
        // The requester (when given) must exist and, for a per-host caller,
        // live on that host — otherwise any agent could parent a background
        // session onto somebody else's conversation. Same gate as
        // `dispatch_task`; `parent_session_id` has no foreign key to catch it
        // later.
        if let Some(req) = args.requester_session_id {
            self.resolve_target_row(&caller, Some(req), None, None, "requester_session_id")?;
        }
        let res = crate::service::bg_sessions::new_bg_session_tracked(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&res)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib a_background_session_cannot_name_a_requester
cargo test -p fleet-core --lib per_host_callers_cannot_spawn_or_dispatch_on_another_host
```

Expected: `test result: ok` for both commands.

- [ ] **Step 5: Check the description budget**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib the_served_definition_budget_stays_bounded
```

Expected: `test result: ok`. If it fails, the failure prints the current byte count against the 56,000 budget — shorten the added sentence rather than raising the constant.

- [ ] **Step 6: Regenerate the reference**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
cargo test -p fleet-core reference_is_current
git diff --stat docs/control-api-reference.md
```

Expected: the second (plain) run passes, and the diff touches only the `new_bg_session` description paragraph.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/mcp/tools/session_ops.rs crates/fleet-core/src/mcp/tools/tests.rs docs/control-api-reference.md
git commit -m "fix(mcp): new_bg_session gates its requester, and says the field exists"
```

---

### Task 4: The move's git step names the work it carries, and the idle refusal is pinned (F6, F7)

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/progress.rs:106-114` and its test at `:245-255`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs:4982` (tighten one assertion)

**Interfaces:**
- Consumes: `count(n: usize, one: &str) -> String` from the same module.
- Produces: `git_detail(commits: u32, dirty: usize) -> String` — signature unchanged, output now names dirty files.

- [ ] **Step 1: Write the failing expectations**

Replace the `git_detail` block inside `details_are_counts` at `crates/fleet-core/src/service/move_session/progress.rs:247-250`:

```rust
        assert_eq!(git_detail(0, 0), "nothing to carry");
        assert_eq!(git_detail(0, 2), "2 files");
        assert_eq!(git_detail(1, 0), "1 commit");
        assert_eq!(git_detail(2, 5), "2 commits, 5 files");
        assert_eq!(git_detail(1, 1), "1 commit, 1 file");
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib details_are_counts
```

Expected: FAIL — `assertion `left == right` failed: left: "0 commits", right: "2 files"`.

- [ ] **Step 3: Make the detail name both halves**

Replace `crates/fleet-core/src/service/move_session/progress.rs:106-114`:

```rust
/// The `git` step's detail. ADR 0002 has the move carry uncommitted work as
/// well as unpushed commits, and `CarryReport.commits` counts only the
/// commits — so a move with dirty files and no unpushed commits used to
/// report "0 commits" and hide the thing it actually carried. Whichever half
/// is zero is left out.
pub(super) fn git_detail(commits: u32, dirty: usize) -> String {
    match (commits, dirty) {
        (0, 0) => "nothing to carry".to_string(),
        (0, d) => count(d, "file"),
        (c, 0) => count(c as usize, "commit"),
        (c, d) => format!("{}, {}", count(c as usize, "commit"), count(d, "file")),
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib details_are_counts
```

Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Check the move tests that read the git step's detail**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib move_session
```

Expected: `test result: ok`. If `a_step_event_carries_its_index_host_and_count` or
`a_dirty_unpushed_source_is_carried_and_reported` asserts on the old `"0 commits"` string, update the expectation to the new wording — the new string is the correct one.

- [ ] **Step 6: Tighten the idle-refusal pin**

`src/lib/moveErrors.ts:98` matches the substring `'is not idle'`, but the backend only asserts `"not idle"`. Replace the assertion at `crates/fleet-core/src/service/move_session/mod.rs:4982`:

```rust
            // `moveErrors.ts` matches this exact substring to offer "wait for
            // the turn to finish" instead of the raw sentence. Pin the whole
            // phrase, not just "not idle", or a reword breaks the frontend
            // silently.
            assert!(err.message.contains("is not idle"), "{}", err.message);
```

- [ ] **Step 7: Run it to verify it still passes**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib require_source_idle
cargo test -p fleet-core --lib move_session
```

Expected: `test result: ok` for both. The assertion is tighter but the current message already contains the phrase, so this is green immediately — it is a guard against a future reword, not a fix.

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-core/src/service/move_session/progress.rs crates/fleet-core/src/service/move_session/mod.rs
git commit -m "fix(move): the git step names the dirty files it carries, not just commits"
```

---

### Task 5: An unknown status is not a failure, and the last-non-null helper is shared (F8, S2)

**Files:**
- Modify: `src/lib/conversation.ts:383-389` (`statusFromReports`) and `:399` (`lastNonNull`)
- Test: `src/lib/conversation.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: **`export function lastNonNull<K extends keyof BackgroundReport>(rs: BackgroundReport[], k: K): BackgroundReport[K] | null`** — Task 6 imports this. `BackgroundReport` is already exported from the same module.

- [ ] **Step 1: Write the failing test**

Add to the `transcriptBackground` describe block in `src/lib/conversation.test.ts`, next to the existing status-mapping test. `bgTurn`, `bgAgentItem` and `bgNoteItem` are already in scope.

```ts
  it('treats a status it does not know as done, like the thread does', () => {
    // `notificationTone` sends an unrecognised status to `info` and
    // `notificationMark` to `✓`. The switcher must agree, or a status the
    // parser has not seen shows a green tick in the thread and a red
    // `failed` in the switcher.
    const of = (status: string) =>
      transcriptBackground([
        bgTurn([bgAgentItem('toolu_1')]),
        bgTurn([bgNoteItem('toolu_1', status, '2026-09-18T10:12:00Z')], '2026-09-18T10:12:00Z'),
      ])[0].status;
    expect(of('completed')).toBe('done');
    expect(of('failed')).toBe('failed');
    expect(of('killed')).toBe('failed');
    expect(of('stopped')).toBe('stopped');
    expect(of('superseded')).toBe('done');
  });
```

- [ ] **Step 2: Run it to verify it fails**

```bash
npx vitest run src/lib/conversation.test.ts -t 'status it does not know'
```

Expected: FAIL — `expected 'failed' to be 'done'` on the `'superseded'` line.

- [ ] **Step 3: Name the failing values and export the helper**

Replace `statusFromReports` at `src/lib/conversation.ts:383-389`:

```ts
/** The newest report that carried a status decides. The failing values are
 *  named explicitly and everything else is `done`, so this agrees with
 *  `notificationTone` and `notificationMark`: a status the parser has never
 *  seen is not evidence of a failure. */
function statusFromReports(reports: BackgroundReport[]): BackgroundStatus {
  const last = [...reports].reverse().find((r) => r.status !== null);
  if (!last) return 'running';
  if (last.status === 'failed' || last.status === 'killed') return 'failed';
  if (last.status === 'stopped') return 'stopped';
  return 'done';
}
```

Then export `lastNonNull` — change its declaration at `:399` from `function` to `export function`, leaving the body and doc comment as they are:

```ts
/** The last report to carry a non-null value for `k`, or null. */
export function lastNonNull<K extends keyof BackgroundReport>(rs: BackgroundReport[], k: K): BackgroundReport[K] | null {
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
npx vitest run src/lib/conversation.test.ts
npx svelte-check --output human
```

Expected: all `conversation.test.ts` tests pass, `0 errors and 0 warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/conversation.ts src/lib/conversation.test.ts
git commit -m "fix(conv): an unreadable status is not a failed background task"
```

---

### Task 6: The background detail always says something (F4, S1, F9 — detail half)

**Depends on Task 5** for the exported `lastNonNull`.

**Files:**
- Modify: `src/lib/BackgroundDetail.svelte` — script block `:19-56`, markup `:80-88`, style block
- Test: `src/lib/BackgroundDetail.test.ts`

**Interfaces:**
- Consumes: `lastNonNull(rs, k)` from `./conversation` (Task 5); `BackgroundEntry`, `BackgroundReport`, `formatDuration` already imported there.
- Produces: nothing other tasks read.

- [ ] **Step 1: Write the failing test**

Add to `src/lib/BackgroundDetail.test.ts`, after `says so plainly when nothing has been reported yet`:

```ts
  it('still says something when the one report carried no text', () => {
    // A report with a null summary AND a null result satisfied none of the
    // arms, so the body rendered empty — one click after a row that said the
    // task had reported.
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({
        status: 'done',
        result: null,
        outputFile: null,
        history: [{ at: '2026-09-18T10:05:00Z', status: 'completed', summary: null, result: null }],
      }),
      onBack: () => {},
    });
    const body = getByTestId('bg-detail-empty').textContent ?? '';
    expect(body.length).toBeGreaterThan(0);
    expect(body).not.toContain('has not reported back yet');
  });
```

- [ ] **Step 2: Run it to verify it fails**

```bash
npx vitest run src/lib/BackgroundDetail.test.ts -t 'carried no text'
```

Expected: FAIL — `Unable to find an element by: [data-testid="bg-detail-empty"]`.

- [ ] **Step 3: Close the cascade, drop the identity map, reuse the shared helper**

In `src/lib/BackgroundDetail.svelte`, change the import line to pull in the shared helper:

```ts
  import { formatDuration, lastNonNull, type BackgroundEntry, type BackgroundReport } from './conversation';
```

Replace the script block from `:21` (the `STATUS_WORD` declaration) through the `newest` function, so it reads:

```ts
  // One report is the entry's own result, already shown above; only a
  // resumed task's several are worth listing separately.
  const reports = $derived(entry.history.length > 1 ? entry.history : []);

  /** The last report to carry a non-null `k` — the same last-non-null-wins
   *  rule `transcriptBackground` uses for `result` and the output file. */
  function newest<K extends keyof BackgroundReport>(k: K): BackgroundReport[K] | null {
    return lastNonNull(entry.history, k);
  }
```

Replace the status span at `:67` — the map was five keys each returning their own name:

```svelte
    <span class="bg-status" data-status={entry.status} data-testid="bg-detail-status">{entry.status}</span>
```

Replace the body cascade at `:80-88`:

```svelte
  {#if entry.error}
    <p class="bg-error" data-testid="bg-detail-error">{entry.error}</p>
  {:else if entry.result}
    <div class="bg-result" data-testid="bg-detail-result"><Markdown source={entry.result} /></div>
  {:else if summary}
    <p class="bg-summary" data-testid="bg-detail-summary">{summary}</p>
  {:else if nothing}
    <p class="muted" data-testid="bg-detail-empty">This background task has not reported back yet.</p>
  {:else}
    <p class="muted" data-testid="bg-detail-empty">It reported, but the report carried no text.</p>
  {/if}
```

Add the `running` colour to the style block, beside the two rules already there:

```css
  .bg-status[data-status='running'] {
    color: var(--accent);
  }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
npx vitest run src/lib/BackgroundDetail.test.ts
npx svelte-check --output human
```

Expected: every test in the file passes — in particular the pre-existing
`says so plainly when nothing has been reported yet`, which still needs the "has not reported back yet" wording — and `0 errors and 0 warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/BackgroundDetail.svelte src/lib/BackgroundDetail.test.ts
git commit -m "fix(ui): a background detail with an empty report still says so"
```

---

### Task 7: A backgrounded agent reads as running in the thread too (F10)

**Files:**
- Modify: `src/lib/SubagentBlock.svelte:24-32`
- Test: `src/lib/SubagentBlock.test.ts`

**Interfaces:**
- Consumes: the existing `onOpen?: () => void` prop. `ConversationPanel` passes it exactly when the background switcher holds an entry for this call, so its presence is the proof the block is background work.
- Produces: nothing other tasks read.

- [ ] **Step 1: Write the failing test**

Add to `src/lib/SubagentBlock.test.ts`, after `a subagent that has not reported reads as running`:

```ts
  it('an unfinished background agent reads as running outside the live turn', () => {
    // The switcher lists this same call as `running`. `onOpen` is passed
    // only when it does, so its presence is what lets the block agree
    // instead of falling back to a wordless "no result".
    render(SubagentBlock, {
      item: item({ done: false, ended_at: null, result: null }),
      nowMs: 0,
      live: false,
      onOpen: () => {},
    });
    expect(screen.getByTestId('conv-subagent-status').textContent).toBe('running');
  });

  it('an unfinished foreground call outside the live turn still gets no word', () => {
    render(SubagentBlock, {
      item: item({ done: false, ended_at: null, result: null }),
      nowMs: 0,
      live: false,
    });
    expect(screen.queryByTestId('conv-subagent-status')).toBeNull();
  });
```

- [ ] **Step 2: Run them to verify the first fails**

```bash
npx vitest run src/lib/SubagentBlock.test.ts -t 'outside the live turn'
```

Expected: `an unfinished background agent reads as running outside the live turn` FAILS with `Unable to find an element by: [data-testid="conv-subagent-status"]`. The second test passes already.

- [ ] **Step 3: Let the block trust the switcher**

Replace the comment and `statusWord` at `src/lib/SubagentBlock.svelte:24-32`:

```ts
  // The same words the switcher and the detail use, read off what the block
  // already knows. `onOpen` is passed only when the switcher holds an entry
  // for this call — which is exactly the case where an unfinished block is
  // background work still running, rather than a call nothing is driving.
  // Without it, an unfinished block outside the live turn still gets no word
  // and the duration's "no result" stands alone.
  const statusWord = $derived(
    item.error ? 'failed' : item.done ? 'done' : live || onOpen ? 'running' : null,
  );
```

Leave `noResult` and `duration` alone: "no result" remains the right phrase for a foreground call nothing closed, and a background one now carries the `running` word beside it.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
npx vitest run src/lib/SubagentBlock.test.ts
npx svelte-check --output human
```

Expected: every test in the file passes, `0 errors and 0 warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/SubagentBlock.svelte src/lib/SubagentBlock.test.ts
git commit -m "fix(ui): a running background agent says so in the thread, not only the switcher"
```

---

### Task 8: The switcher shows what is live, and the notification row is written once (F9 — switcher half, S3)

**Files:**
- Modify: `src/lib/ConversationPanel.svelte` — import block `:81`, markup `:1261-1281`, style block `:1583-1593`
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `notificationMark`, `notificationLabel`, `notificationTone`, `relativeTime` — all already imported in this file.
- Produces: nothing other tasks read. The `conv-notification` and `conv-background-item` test ids are unchanged, which is what makes the existing tests the regression gate for the refactor.

- [ ] **Step 1: Add the regression guard for the CSS hook**

Be honest about what is testable here. The markup already emits the right
`data-status`; only the stylesheet was missing a rule, and a scoped-CSS colour
resolved through `var(--accent)` does not survive jsdom reliably. So this step does
**not** write a failing test — it pins the attribute the new selector binds to, so a
future refactor cannot drop the hook and silently un-style the row. The colour itself
is verified by eye in Step 6.

Add to `src/lib/ConversationPanel.test.ts`, in the `ConversationPanel background
switcher` describe block. `renderWithConversation`, `convWithBackgroundAgent` and
`session` are already in scope:

```ts
  it('marks a running fleet child so the row has something to colour', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithBackgroundAgent, {
      sessions: [
        session({
          id: 2,
          parent_session_id: 1,
          kind: 'bg',
          friendly_name: 'Load layers',
          claude_status: 'working',
        }),
      ],
    });
    await fireEvent.click(getByTestId('conv-background-button'));
    const statuses = getAllByTestId('conv-background-item').map((el) =>
      el.querySelector('.bg-item-status')?.getAttribute('data-status'),
    );
    expect(statuses).toContain('running');
    expect(statuses).toContain('done');
  });
```

- [ ] **Step 2: Run it**

```bash
npx vitest run src/lib/ConversationPanel.test.ts -t 'something to colour'
```

Expected: PASS. It is a guard, not a red-to-green cycle — say so when reporting, do
not describe it as a TDD step.

- [ ] **Step 3: Add the running colour**

In the style block at `src/lib/ConversationPanel.svelte:1583`, add one rule beside the two already there:

```css
  .bg-item-status[data-status='running'] {
    color: var(--accent);
  }
```

- [ ] **Step 4: Fold the duplicated notification row into a snippet**

Add `type ConvGroup` to the existing `from './conversation'` import (the type list ends with `type BackgroundEntry,` at `:81`):

```ts
    type BackgroundEntry,
    type ConvGroup,
  } from './conversation';
```

Replace the whole `{:else if g.kind === 'notification'}` arm at `:1261-1281` — the two branches differed only in their wrapper element:

```svelte
                  {:else if g.kind === 'notification'}
                    {@const target = entryForNotification(g)}
                    {#snippet noteBody(n: Extract<ConvGroup, { kind: 'notification' }>)}
                      <span class="note-mark" aria-hidden="true">{notificationMark(n.status)}</span>
                      <span class="note-label">{notificationLabel(n)}</span>
                      {#if n.at}<time class="note-time" datetime={n.at}>{relativeTime(n.at, nowMs)}</time>{/if}
                    {/snippet}
                    {#if target}
                      <button
                        type="button"
                        class="notification clickable"
                        data-testid="conv-notification"
                        data-tone={notificationTone(g.status)}
                        onclick={() => openBackground(target)}
                      >
                        {@render noteBody(g)}
                      </button>
                    {:else}
                      <div class="notification" data-testid="conv-notification" data-tone={notificationTone(g.status)}>
                        {@render noteBody(g)}
                      </div>
                    {/if}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
npx vitest run src/lib/ConversationPanel.test.ts
npx svelte-check --output human
```

Expected: every test in the file passes — the ~14 pre-existing `conv-notification` and `conv-background-item` assertions are the gate that the snippet renders identically — and `0 errors and 0 warnings`.

- [ ] **Step 6: Look at it**

The colour is the one thing no test here covers. Open the app, pick a session with a
running background entry, open the switcher, and confirm the running row reads in the
accent colour while the done rows stay muted — in both light and dark.

- [ ] **Step 7: Commit**

```bash
git add src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "fix(ui): the switcher colours what is running, and the notification row is written once"
```

---

### Task 9: The worktree tree stops being a `git add` hazard (H1)

**Files:**
- Modify: `.gitignore:19`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing.

- [ ] **Step 1: Confirm nothing tracked would be ignored**

```bash
git ls-files .claude .mcp.json .ignore
```

Expected: **empty**. If anything prints, stop — that file is tracked and must be excluded from the new rule rather than ignored under it.

- [ ] **Step 2: Widen the rule**

Replace `.gitignore:19` (currently `.claude/settings.local.json`) with:

```gitignore
# Session state, per-session worktrees (tens of GB) and local MCP wiring. The
# tree under .claude/worktrees is checkouts of this same repo — never staged.
.claude/
.mcp.json
.ignore
```

- [ ] **Step 3: Verify the working tree is clean**

```bash
git status --porcelain
git check-ignore -v .claude/worktrees .mcp.json .ignore
```

Expected: `git status --porcelain` prints **nothing** (the three untracked entries are gone), and `check-ignore` names `.gitignore` as the source for each of the three paths.

- [ ] **Step 4: Commit**

```bash
git add .gitignore
git commit -m "chore: ignore the session worktree tree and local MCP wiring"
```

---

### Task 10: Stale branches and the release (H2, H3) — **needs explicit approval before each half**

Not code. Both halves are outward-facing and irreversible-ish; do not run either without the user saying so in this session.

**Files:** none.

**Interfaces:**
- Consumes: a green gate from Tasks 1–9, merged to `main`.
- Produces: nothing other tasks read.

- [ ] **Step 1: Show the user what would be deleted**

```bash
git fetch --prune
git log --oneline origin/main..origin/feature/design-index-fixes-ffd2d0
diff <(git show 3747407 --format='') <(git show 3d918b5 --format='') && echo "IDENTICAL-PATCH"
for b in $(git branch -r --no-merged origin/main | grep -v HEAD); do
  echo "$b  ahead=$(git rev-list --count origin/main..$b)  last=$(git log -1 --format=%cs $b)"
done
git worktree list
```

Expected: `IDENTICAL-PATCH`, plus 13 unmerged remote branches and 16 registered worktrees.

- [ ] **Step 2: Ask, then delete only what the user names**

Present the list. On an explicit yes, and only for the branches they name:

```bash
git push origin --delete feature/design-index-fixes-ffd2d0
git worktree remove .claude/worktrees/design-index-fixes-ffd2d0
git worktree prune
```

The remaining twelve stale branches are a separate decision — several have 20+ commits (`feat/host-reboot-survival`, `feat/host-reboot-recovery`) and may hold unlanded work. Do not batch them with the dead one.

- [ ] **Step 3: Ask, then cut the release**

42 commits and two features have landed since `v0.2.26`. Never edit the version fields by hand; run the script from a clean `main`:

```bash
git checkout main && git pull --ff-only
git status --porcelain   # must be empty
scripts/release.sh 0.2.27
```

Then verify the script's edits actually landed — its CHANGELOG step has failed silently on macOS before:

```bash
git show --stat HEAD
git log -1 --format=%s
git tag --list 'v0.2.27'
head -20 CHANGELOG.md
```

Expected: six version files plus `Cargo.lock` in the commit, a populated `## [0.2.27]` section, and the tag present.

- [ ] **Step 4: Scan the release artifacts before publishing**

Check the diff and the changelog for anything that should not ship — hardcoded paths, tokens, host names, personal data — before pushing the tag.

---

## Verification before the PR

Run the whole gate from the **Global Constraints** section, unpiped, and read every result line:

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
npx svelte-check --output human
npx vitest run
```

Expected: `fmt` and `clippy` silent; a `test result: ok` line for **every** cargo target including `claude_fleet_lib`; `0 errors and 0 warnings`; 2075+ vitest tests passing.

CI's clippy is newer than the local toolchain and has failed on locally-clean code before (`unnecessary_sort_by`). Wait for GitHub CI on the final head before merging.

```bash
git push -u origin HEAD
gh pr create --base main --title "fix: close the post-merge review findings" --body-file <(echo "Closes F1-F10, S1-S3 and H1 from docs/specs/2026-09-20-post-merge-review.md")
```

`gh pr edit` fails on this account (missing `read:project`); to change the body afterwards use
`gh api -X PATCH repos/martin-janci/claude-fleet/pulls/<n> -F body=@file`.

Merging needs the user's explicit go.
