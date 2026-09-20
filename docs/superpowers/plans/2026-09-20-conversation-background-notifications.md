# Background work in the Conversations tab — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `<task-notification>` entries read as finished background work instead of raw XML, and give the Conversations tab one switcher over every background thing that belongs to the session.

**Architecture:** The transcript parser gains a `ConvItem::Notification` variant and a post-pass that joins each notification to the `tool_use` that launched it, so a background agent's block shows its real report instead of the launch acknowledgement. The frontend renders a notification as a one-line clickable event, derives a background list from the parsed turns plus the already-loaded `$sessions` / `$tasks` stores, and replaces the thread with a detail view when one is picked. `new_bg_session` gains an optional requester so fleet-spawned children have a parent.

**Tech Stack:** Rust (`fleet-core`, serde, rusqlite), Svelte 5 runes, Vitest, TypeScript.

**Spec:** `docs/superpowers/specs/2026-09-20-conversation-background-notifications-design.md`

## Global Constraints

- Shell-quoting has one canonical implementation: `crate::shell::quote` (alias `shq`). No task here builds a shell command, so no task introduces one.
- `Store` access goes through a `std::sync::Mutex`; never hold the guard across an `.await`.
- Backend errors are `IpcError` with `E_*` codes; the frontend unwraps via `src/lib/result.ts`.
- `cargo` is a zsh **function** on this host that forces `/Volumes/CargoSD/target/<root basename>`. Use plain `cargo` as written; if you need to bypass it, `command cargo`.
- `pnpm test` / `pnpm check` do not work on this host (binary not on PATH). Use `npx vitest run …` and `npx svelte-check …` exactly as written in the steps.
- Run `pnpm install --frozen-lockfile` once before the first frontend task. A `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"` is a stale-`node_modules` symptom, not a code error.
- Final verification is the whole `scripts/ci-local.sh`. Never judge a run through `| tail` — read the unpiped output.
- `NOTIFICATION_RESULT_MAX_CHARS = 20_000`. `SUBAGENT_RESULT_MAX_CHARS` stays `1_500`.
- Notification `status` values, verbatim: `completed`, `failed`, `stopped`, `killed`.
- Recognised notification tags, verbatim: `task-id`, `tool-use-id`, `status`, `summary`, `result`, `output-file`, `event`. `note` and `usage` are read by nobody.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/fleet-core/src/service/transcript.rs` | `ConvItem::Notification`, its parsing, the join pass, the launch-ack rule | 1, 2 |
| `src-tauri/src/backend/tests_contract.rs` | Golden wire-shape sample for the new variant | 1 |
| `src/lib/conversation.ts` | Wire types, notification presentation helpers, background list derivation | 3, 4 |
| `src/lib/ConversationPanel.svelte` | Notification row in the thread; switcher, detail mount, composer guard | 3, 6 |
| `src/lib/BackgroundDetail.svelte` | The replaced-thread view for one background entry | 5 |
| `crates/fleet-core/src/service/bg_sessions.rs` | `requester_session_id` arg, parent stamping | 7 |
| `crates/fleet-core/src/mcp/tools/session_ops.rs` | Tool schema regen trigger (no body change) | 7 |
| `src-tauri/src/backend/tests_routing.rs` | Non-default requester in the routed-args row | 7 |
| `src/lib/sessions.ts` | `newBgSession` optional requester | 7 |

**Two files have more than one writer:** `transcript.rs` (Tasks 1, 2) and `conversation.ts` (Tasks 3, 4), `ConversationPanel.svelte` (Tasks 3, 6). Those pairs must run **sequentially**, never as parallel subagents.

---

### Task 1: `ConvItem::Notification` — the variant and its parsing

A `<task-notification>` is a user entry whose text starts with that tag. Today it falls through to `prompt_text` and becomes a turn whose prompt is a wall of XML. This task makes it its own item in its own prompt-less turn. Joining it to the call that launched it is Task 2.

**Files:**
- Modify: `crates/fleet-core/src/service/transcript.rs` (enum at `:309`, constants near `:406`, parse loop at `:628`, `parse_turns` at `:784`, `item_chars` at `:815`)
- Modify: `src-tauri/src/backend/tests_contract.rs:403` (add the golden sample)
- Test: `crates/fleet-core/src/service/transcript.rs` (the `#[cfg(test)]` module; helpers `jl`, `user`, `asst`, `tool_use` live at `:1781`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `ConvItem::Notification { task_id: Option<String>, tool_use_id: Option<String>, status: Option<String>, summary: Option<String>, result: Option<String>, output_file: Option<String>, event: Option<String> }`, serialised with `#[serde(tag = "kind", rename_all = "snake_case")]` so its wire `kind` is `"notification"`. `const NOTIFICATION_RESULT_MAX_CHARS: usize = 20_000`.

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/fleet-core/src/service/transcript.rs`, next to the other `parse_conversation` tests:

```rust
    /// The exact shape a background `Agent` reports in with.
    fn task_notification(ts: &str, body: &str) -> serde_json::Value {
        serde_json::json!({"type":"user","timestamp":ts,
            "message":{"content":body}})
    }

    #[test]
    fn a_task_notification_becomes_its_own_item_not_a_prompt() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            asst("on it"),
            task_notification(
                "2026-09-18T10:01:00Z",
                "<task-notification>\n\
                 <task-id>a623962a33b4c9765</task-id>\n\
                 <tool-use-id>toolu_1</tool-use-id>\n\
                 <output-file>/private/tmp/x/tasks/a6.output</output-file>\n\
                 <status>completed</status>\n\
                 <summary>Agent \"Posúdiť stratégiu testov\" finished</summary>\n\
                 <note>A task-notification fires each time this agent stops.</note>\n\
                 <result>Mám naštudované všetky zdroje.</result>\n\
                 </task-notification>",
            ),
        ]));
        assert_eq!(t.len(), 2, "the notification opens a turn of its own");
        assert_eq!(t[1].prompt, None, "it is never a prompt");
        assert_eq!(
            t[1].items,
            vec![ConvItem::Notification {
                task_id: Some("a623962a33b4c9765".into()),
                tool_use_id: Some("toolu_1".into()),
                status: Some("completed".into()),
                summary: Some("Agent \"Posúdiť stratégiu testov\" finished".into()),
                result: Some("Mám naštudované všetky zdroje.".into()),
                output_file: Some("/private/tmp/x/tasks/a6.output".into()),
                event: None,
            }]
        );
    }

    #[test]
    fn a_monitor_event_without_a_tool_use_id_still_parses() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            task_notification(
                "2026-09-18T10:01:00Z",
                "<task-notification>\n\
                 <task-id>buzo0s189</task-id>\n\
                 <summary>Monitor event: \"PR #165 CI checks\"</summary>\n\
                 <event>frontend (ubuntu-24.04): pass</event>\n\
                 </task-notification>",
            ),
        ]));
        let ConvItem::Notification {
            tool_use_id,
            status,
            event,
            ..
        } = &t[1].items[0]
        else {
            panic!("expected a notification, got {:?}", t[1].items[0]);
        };
        assert_eq!(*tool_use_id, None);
        assert_eq!(*status, None, "a mid-stream event reports no completion");
        assert_eq!(event.as_deref(), Some("frontend (ubuntu-24.04): pass"));
    }

    #[test]
    fn a_prompt_that_merely_quotes_the_tag_stays_a_prompt() {
        let text = "why did this fire?\n<task-notification>\n<task-id>x</task-id>\n</task-notification>";
        let t = parse_conversation(&jl(&[user(serde_json::json!(text))]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].prompt.as_deref(), Some(text));
        assert!(t[0].items.is_empty());
    }

    #[test]
    fn consecutive_notifications_share_one_prompt_less_turn() {
        let note = |id: &str| {
            task_notification(
                "2026-09-18T10:01:00Z",
                &format!(
                    "<task-notification>\n<task-id>{id}</task-id>\n\
                     <status>completed</status>\n<summary>Agent {id} finished</summary>\n\
                     </task-notification>"
                ),
            )
        };
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            asst("dispatched three"),
            note("a1"),
            note("a2"),
            note("a3"),
        ]));
        assert_eq!(t.len(), 2, "three arrivals with no reply between them = one turn");
        assert_eq!(t[1].items.len(), 3);
    }

    #[test]
    fn a_notification_after_a_reply_opens_a_fresh_turn() {
        let note = task_notification(
            "2026-09-18T10:01:00Z",
            "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n\
             <summary>Agent a1 finished</summary>\n</task-notification>",
        );
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            note.clone(),
            asst("thanks, agent"),
            note,
        ]));
        assert_eq!(t.len(), 3, "a reply between two notifications separates them");
    }

    #[test]
    fn a_notification_result_is_capped_on_a_char_boundary() {
        let long = "é".repeat(NOTIFICATION_RESULT_MAX_CHARS + 500);
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            task_notification(
                "2026-09-18T10:01:00Z",
                &format!(
                    "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n\
                     <summary>done</summary>\n<result>{long}</result>\n</task-notification>"
                ),
            ),
        ]));
        let ConvItem::Notification { result, .. } = &t[1].items[0] else {
            panic!("expected a notification");
        };
        let got = result.as_deref().unwrap();
        assert_eq!(got.chars().count(), NOTIFICATION_RESULT_MAX_CHARS + 1, "capped plus the ellipsis");
        assert!(got.ends_with('…'));
    }

    #[test]
    fn notification_result_cap_is_the_compaction_budget_not_the_subagent_one() {
        assert_eq!(NOTIFICATION_RESULT_MAX_CHARS, 20_000);
        assert_eq!(SUBAGENT_RESULT_MAX_CHARS, 1_500);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core --lib service::transcript`

Expected: FAIL to **compile**, with `cannot find variant Notification in enum ConvItem` and `cannot find value NOTIFICATION_RESULT_MAX_CHARS`. A compile failure is the correct red here — the variant does not exist yet.

- [ ] **Step 3: Add the variant and the constant**

In `crates/fleet-core/src/service/transcript.rs`, add to the `ConvItem` enum immediately after the `Command { … }` variant (before `Interrupt`):

```rust
    /// A `<task-notification>` user entry: a background agent, command,
    /// monitor or workflow reporting in. `tool_use_id` names the `tool_use`
    /// that launched it, which `join_notifications` uses to fill that call's
    /// block in; it is absent on a mid-stream Monitor event, which reports
    /// progress rather than completion.
    Notification {
        #[serde(default)]
        task_id: Option<String>,
        #[serde(default)]
        tool_use_id: Option<String>,
        /// `completed` | `failed` | `stopped` | `killed`; `None` on a
        /// mid-stream event.
        #[serde(default)]
        status: Option<String>,
        /// The harness's own one-line sentence. Always present in practice.
        #[serde(default)]
        summary: Option<String>,
        /// The agent's report, capped at [`NOTIFICATION_RESULT_MAX_CHARS`].
        #[serde(default)]
        result: Option<String>,
        /// Path to the task's full output, on the session's host.
        #[serde(default)]
        output_file: Option<String>,
        /// Monitor's streamed line.
        #[serde(default)]
        event: Option<String>,
    },
```

Add the constant beside `COMMAND_OUTPUT_MAX_CHARS` (around `:406`):

```rust
/// Cap on a task notification's carried report (chars). A background
/// agent's report is routinely several KB, so this is the compaction
/// summary's budget rather than [`SUBAGENT_RESULT_MAX_CHARS`], which was
/// sized for a tool one-liner's neighbour.
const NOTIFICATION_RESULT_MAX_CHARS: usize = 20_000;
```

- [ ] **Step 4: Parse the entry**

In `parse_conversation`, inside the `if let Some(text) = user_text(content) {` block, immediately after `let head = text.trim_start();` and **before** the `let is_command = …` line, insert:

```rust
                    // Only an entry that *starts* with the tag is a
                    // notification — the same rule slash commands follow, so
                    // a pasted transcript stays a prompt.
                    if head.starts_with("<task-notification>") {
                        let item = ConvItem::Notification {
                            task_id: tag_text(&text, "task-id"),
                            tool_use_id: tag_text(&text, "tool-use-id"),
                            status: tag_text(&text, "status"),
                            summary: tag_text(&text, "summary"),
                            result: tag_text(&text, "result")
                                .map(|r| cap_chars(&r, NOTIFICATION_RESULT_MAX_CHARS)),
                            output_file: tag_text(&text, "output-file"),
                            event: tag_text(&text, "event"),
                        };
                        // Notifications that arrive back to back, with no
                        // assistant output between them, share one turn: three
                        // agents finishing together should not make three
                        // near-empty turns.
                        let coalesce = current.as_ref().is_some_and(|t| {
                            t.prompt.is_none()
                                && !t.items.is_empty()
                                && t.items
                                    .iter()
                                    .all(|i| matches!(i, ConvItem::Notification { .. }))
                        });
                        if !coalesce {
                            push(&mut turns, current.take());
                            tool_items.clear();
                            current = Some(ConvTurn {
                                prompt: None,
                                at: at(),
                                ended_at: None,
                                items: Vec::new(),
                            });
                        }
                        if let Some(t) = current.as_mut() {
                            t.items.push(item);
                        }
                        continue;
                    }
```

- [ ] **Step 5: Extend the two exhaustive matches**

`parse_turns` (around `:790`) — add before the closing brace of the `match i` block:

```rust
                    ConvItem::Notification { summary, .. } => format!(
                        "[notification] {}",
                        one_line(summary.as_deref().unwrap_or(""))
                    ),
```

`item_chars` (around `:820`) — add before the closing brace of its `match item`:

```rust
        ConvItem::Notification {
            summary,
            result,
            event,
            ..
        } => {
            summary.as_deref().map_or(0, |s| s.chars().count())
                + result.as_deref().map_or(0, |s| s.chars().count())
                + event.as_deref().map_or(0, |s| s.chars().count())
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p fleet-core --lib service::transcript`

Expected: PASS, including all seven new tests.

- [ ] **Step 7: Add the golden wire sample**

In `src-tauri/src/backend/tests_contract.rs`, after the `put("ConvItem::Command", …)` block and before `put("ConvItem::Interrupt", …)`:

```rust
    put(
        "ConvItem::Notification",
        wire_keys(&ConvItem::Notification {
            task_id: Some("a623962a33b4c9765".into()),
            tool_use_id: Some("toolu_1".into()),
            status: Some("completed".into()),
            summary: Some("Agent finished".into()),
            result: Some("r".into()),
            output_file: Some("/private/tmp/x/tasks/a6.output".into()),
            event: Some("e".into()),
        }),
    );
```

- [ ] **Step 8: Regenerate the hub contract golden**

Run: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`

This rewrites `src-tauri/src/backend/hub_contract.golden.json` and then **deliberately panics** — regenerating is never the end of the job, so a green run is not on offer. That failure is expected and is not a signal. Read the diff, then verify without the env var:

```bash
git diff -- src-tauri/src/backend/hub_contract.golden.json
cargo test -p claude-fleet --lib contract
```

Expected: the diff adds `ConvItem::Notification` and changes nothing else; the second run PASSES.

If the regen instead refuses with *"a field was renamed or removed without bumping the wire-contract revision"*, stop: this task only **adds** a type, so a lost field means something else in the change broke an existing wire shape. Find it rather than bumping `CONTRACT_REVISION`.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/fleet-core/src/service/transcript.rs src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/
git commit -m "feat(transcript): parse task notifications instead of printing their XML"
```

Note: `src-tauri/src/backend/hub_contract.golden.json` is regenerated, not hand-edited. The `git add` line above covers it.

---

### Task 2: Join a notification to the call that launched it

A backgrounded `Agent` call's `tool_result` is only `"Async agent launched successfully. … agentId: …"` — internal metadata that `SubagentBlock` currently renders as the agent's report. The real report arrives later in a notification carrying the same `tool-use-id`. This task drops the acknowledgement, leaves the block open, and fills it from the notification.

**Files:**
- Modify: `crates/fleet-core/src/service/transcript.rs` (the `Subagent` arm of the `tool_result` handling at `:594-611`; new `join_notifications` after `parse_conversation`)
- Test: same file's test module

**Interfaces:**
- Consumes: `ConvItem::Notification` from Task 1.
- Produces: `fn join_notifications(turns: &mut [ConvTurn])`, called as the last step of `parse_conversation` before it returns. `const AGENT_LAUNCH_ACK: &str = "Async agent launched successfully"`.

**Deliberate deviation from the spec:** the spec says a joined `ConvItem::Tool` "takes the notification's `summary` as its text". Do **not** do that — a tool line's summary is `Bash(command=gh run watch …)`, which is the useful part, and the notification's sentence is already shown on its own row. A joined `Tool` takes only `done`, `error` and `ended_at`. The spec has been corrected to match.

- [ ] **Step 1: Write the failing tests**

Add to the test module:

```rust
    #[test]
    fn a_backgrounded_agents_launch_ack_is_not_its_report() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T10:00:01Z",
                "toolu_1",
                "Agent",
                serde_json::json!({"description":"Posúdiť stratégiu testov","subagent_type":"general-purpose","prompt":"p"}),
            ),
            tool_result(
                "2026-09-18T10:00:02Z",
                "toolu_1",
                serde_json::json!("Async agent launched successfully. (This tool result is internal metadata.)\nagentId: a623962a33b4c9765"),
                false,
            ),
        ]));
        let ConvItem::Subagent { result, done, .. } = &t[0].items[0] else {
            panic!("expected a subagent, got {:?}", t[0].items[0]);
        };
        assert_eq!(*result, None, "the ack is metadata, not a report");
        assert!(!*done, "the agent is still running until it notifies");
    }

    #[test]
    fn a_notification_fills_in_the_agent_block_it_names() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T10:00:01Z",
                "toolu_1",
                "Agent",
                serde_json::json!({"description":"Posúdiť stratégiu testov","subagent_type":"general-purpose","prompt":"p"}),
            ),
            tool_result(
                "2026-09-18T10:00:02Z",
                "toolu_1",
                serde_json::json!("Async agent launched successfully.\nagentId: a6"),
                false,
            ),
            task_notification(
                "2026-09-18T10:12:00Z",
                "<task-notification>\n<task-id>a6</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n\
                 <status>completed</status>\n<summary>Agent finished</summary>\n\
                 <result># Posudok\n\nVšetko overené.</result>\n</task-notification>",
            ),
        ]));
        let ConvItem::Subagent {
            result,
            done,
            error,
            ended_at,
            ..
        } = &t[0].items[0]
        else {
            panic!("expected a subagent");
        };
        assert_eq!(result.as_deref(), Some("# Posudok\n\nVšetko overené."));
        assert!(*done);
        assert!(!*error);
        assert_eq!(ended_at.as_deref(), Some("2026-09-18T10:12:00Z"));
    }

    #[test]
    fn a_failed_notification_flags_the_block_it_names() {
        for status in ["failed", "killed", "stopped"] {
            let t = parse_conversation(&jl(&[
                user(serde_json::json!("go")),
                tool_use(
                    "2026-09-18T10:00:01Z",
                    "toolu_1",
                    "Agent",
                    serde_json::json!({"description":"d","subagent_type":"general-purpose","prompt":"p"}),
                ),
                task_notification(
                    "2026-09-18T10:12:00Z",
                    &format!(
                        "<task-notification>\n<tool-use-id>toolu_1</tool-use-id>\n\
                         <status>{status}</status>\n<summary>s</summary>\n</task-notification>"
                    ),
                ),
            ]));
            let ConvItem::Subagent { error, done, .. } = &t[0].items[0] else {
                panic!("expected a subagent");
            };
            assert!(*error, "{status} is not a success");
            assert!(*done, "{status} closes the call");
        }
    }

    #[test]
    fn a_notification_closes_a_background_bash_without_rewriting_its_summary() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T10:00:01Z",
                "toolu_9",
                "Bash",
                serde_json::json!({"command":"gh run watch","description":"Watch Docker build CI"}),
            ),
            task_notification(
                "2026-09-18T10:05:00Z",
                "<task-notification>\n<task-id>bk9</task-id>\n<tool-use-id>toolu_9</tool-use-id>\n\
                 <status>completed</status>\n\
                 <summary>Background command \"Watch Docker build CI\" completed (exit code 0)</summary>\n\
                 </task-notification>",
            ),
        ]));
        let ConvItem::Tool {
            summary,
            done,
            error,
            ended_at,
            ..
        } = &t[0].items[0]
        else {
            panic!("expected a tool line");
        };
        assert!(summary.contains("gh run watch"), "the command stays the line's text");
        assert!(*done);
        assert!(!*error);
        assert_eq!(ended_at.as_deref(), Some("2026-09-18T10:05:00Z"));
    }

    #[test]
    fn a_mid_stream_event_closes_nothing() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T10:00:01Z",
                "toolu_5",
                "Monitor",
                serde_json::json!({"description":"PR #165 CI checks"}),
            ),
            task_notification(
                "2026-09-18T10:02:00Z",
                "<task-notification>\n<task-id>bu1</task-id>\n<tool-use-id>toolu_5</tool-use-id>\n\
                 <summary>Monitor event</summary>\n<event>frontend: pass</event>\n</task-notification>",
            ),
        ]));
        let ConvItem::Tool { done, .. } = &t[0].items[0] else {
            panic!("expected a tool line");
        };
        assert!(!*done, "an event with no status is progress, not completion");
    }

    #[test]
    fn an_unmatched_notification_is_left_standing_alone() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            task_notification(
                "2026-09-18T10:12:00Z",
                "<task-notification>\n<tool-use-id>toolu_gone</tool-use-id>\n\
                 <status>completed</status>\n<summary>s</summary>\n</task-notification>",
            ),
        ]));
        assert!(matches!(t[1].items[0], ConvItem::Notification { .. }));
    }

    #[test]
    fn the_last_notification_wins_when_an_agent_is_resumed() {
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
            asst("keep going"),
            task_notification(
                "2026-09-18T10:20:00Z",
                "<task-notification>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n\
                 <summary>s</summary>\n<result>second pass</result>\n</task-notification>",
            ),
        ]));
        let ConvItem::Subagent { result, ended_at, .. } = &t[0].items[0] else {
            panic!("expected a subagent");
        };
        assert_eq!(result.as_deref(), Some("second pass"));
        assert_eq!(ended_at.as_deref(), Some("2026-09-18T10:20:00Z"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core --lib service::transcript`

Expected: FAIL. `a_backgrounded_agents_launch_ack_is_not_its_report` fails on `assertion left == right` (the ack text is in `result`); the join tests fail with `result` still `None` and `done` false.

- [ ] **Step 3: Stop treating the launch ack as a report**

In `parse_conversation`, replace the `Some(ConvItem::Subagent { … })` arm of the `tool_result` match (currently `:594-611`) with:

```rust
                                Some(ConvItem::Subagent {
                                    error,
                                    done,
                                    ended_at,
                                    result,
                                    ..
                                }) => {
                                    *error |= is_err;
                                    let ack = tool_result_text(b)
                                        .map(|t| t.trim().starts_with(AGENT_LAUNCH_ACK))
                                        .unwrap_or(false);
                                    if ack {
                                        // Backgrounded: this result only says
                                        // the agent started. Leave the block
                                        // open for its notification to fill.
                                        *done = false;
                                        *ended_at = None;
                                    } else {
                                        *done = true;
                                        *ended_at = ended;
                                        if let Some(t) = tool_result_text(b) {
                                            *result = Some(cap_chars(
                                                t.trim(),
                                                SUBAGENT_RESULT_MAX_CHARS,
                                            ));
                                        }
                                    }
                                }
```

Add the constant beside `SUBAGENT_RESULT_MAX_CHARS` (around `:46`):

```rust
/// The `tool_result` of a backgrounded `Agent` call is an acknowledgement
/// that the agent was launched, not its report — the report arrives later in
/// a `<task-notification>`. Recognising it keeps the block reading as running
/// until the real one lands.
const AGENT_LAUNCH_ACK: &str = "Async agent launched successfully";
```

- [ ] **Step 4: Add the join pass**

At the end of `parse_conversation`, change the final lines from:

```rust
    push(&mut turns, current.take());
    turns
}
```

to:

```rust
    push(&mut turns, current.take());
    join_notifications(&mut turns);
    turns
}

/// Fill each launching call's block from the notification that reports on
/// it. A notification always lands in a later turn than its call (it is a
/// user entry, which closes the turn in progress), so the map is built over
/// the finished turns and applied in a second walk — the notification and
/// its target are in different turns and cannot both be borrowed mutably.
///
/// A notification with no `tool_use_id`, or one naming a call outside the
/// loaded window, is left standing on its own. So is a mid-stream event:
/// no `status` means progress, not completion.
fn join_notifications(turns: &mut [ConvTurn]) {
    let mut launched: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for (ti, turn) in turns.iter().enumerate() {
        for (ii, item) in turn.items.iter().enumerate() {
            let id = match item {
                ConvItem::Tool { id, .. } | ConvItem::Subagent { id, .. } => id.as_deref(),
                _ => None,
            };
            if let Some(id) = id {
                launched.insert(id.to_string(), (ti, ii));
            }
        }
    }
    // (target, report, failed, arrived_at), in transcript order, so a
    // resumed agent's later notification overwrites its earlier one.
    let mut updates: Vec<((usize, usize), Option<String>, bool, Option<String>)> = Vec::new();
    for turn in turns.iter() {
        for item in turn.items.iter() {
            let ConvItem::Notification {
                tool_use_id,
                status,
                result,
                ..
            } = item
            else {
                continue;
            };
            let Some(status) = status.as_deref() else {
                continue;
            };
            let Some(target) = tool_use_id
                .as_deref()
                .and_then(|i| launched.get(i))
                .copied()
            else {
                continue;
            };
            updates.push((target, result.clone(), status != "completed", turn.at.clone()));
        }
    }
    for ((ti, ii), report, failed, ended) in updates {
        match turns[ti].items.get_mut(ii) {
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
                *error |= failed;
                *done = true;
                *ended_at = ended;
            }
            // A tool line keeps its own summary — `Bash(command=…)` is the
            // useful text, and the notification's sentence has its own row.
            Some(ConvItem::Tool {
                error,
                ended_at,
                done,
                ..
            }) => {
                *error |= failed;
                *done = true;
                *ended_at = ended;
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p fleet-core --lib service::transcript`

Expected: PASS, all tests in the module including the seven from Task 1.

- [ ] **Step 6: Correct the spec line this task deviates from**

In `docs/superpowers/specs/2026-09-20-conversation-background-notifications-design.md`, replace the second bullet under "Joining to the launching item":

```markdown
- `ConvItem::Tool { id }` (`Bash`, `Monitor`, `Workflow`) — the line becomes
  `done` and takes `ended_at`, with `error` set the same way. Its own summary
  is kept: `Bash(command=…)` is the useful text, and the notification's
  sentence already has its own row.
```

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/fleet-core/src/service/transcript.rs docs/superpowers/specs/2026-09-20-conversation-background-notifications-design.md
git commit -m "feat(transcript): a background agent's block shows its report, not its launch ack"
```

---

### Task 3: Render a notification as a one-line event

**Files:**
- Modify: `src/lib/conversation.ts` (the `ConvItem` union at `:9`, the `ConvGroup` union at `:198`)
- Modify: `src/lib/ConversationPanel.svelte` (import list at `:33`, group rendering at `:1158`, styles)
- Test: `src/lib/conversation.test.ts`, `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: the `notification` wire item from Tasks 1–2.
- Produces:
  - `type NotificationItem = Extract<ConvItem, { kind: 'notification' }>`
  - `type NotificationTone = 'info' | 'warn' | 'error'`
  - `notificationTone(status: string | null): NotificationTone`
  - `notificationMark(status: string | null): string`
  - `notificationLabel(n: { summary: string | null; event: string | null }): string`

- [ ] **Step 1: Write the failing tests**

Add to `src/lib/conversation.test.ts`:

```ts
import { notificationTone, notificationMark, notificationLabel } from './conversation';

describe('notification presentation', () => {
  it('reads completion as info, failure as error, a stop as a warning', () => {
    expect(notificationTone('completed')).toBe('info');
    expect(notificationTone('failed')).toBe('error');
    expect(notificationTone('killed')).toBe('error');
    expect(notificationTone('stopped')).toBe('warn');
  });

  it('treats a mid-stream event, which has no status, as plain progress', () => {
    expect(notificationTone(null)).toBe('info');
    expect(notificationMark(null)).toBe('•');
  });

  it('marks each terminal status distinctly', () => {
    expect(notificationMark('completed')).toBe('✓');
    expect(notificationMark('failed')).toBe('✕');
    expect(notificationMark('killed')).toBe('✕');
    expect(notificationMark('stopped')).toBe('⏸');
  });

  it('uses the harness sentence as the label', () => {
    expect(notificationLabel({ summary: 'Agent "Posúdiť" finished', event: null })).toBe(
      'Agent "Posúdiť" finished',
    );
  });

  it('appends a streamed event as one line', () => {
    expect(
      notificationLabel({ summary: 'Monitor event', event: 'frontend: pass\nALL DONE' }),
    ).toBe('Monitor event: frontend: pass');
  });

  it('never renders an empty row', () => {
    expect(notificationLabel({ summary: null, event: null })).toBe('Background task reported');
    expect(notificationLabel({ summary: '   ', event: null })).toBe('Background task reported');
  });
});
```

Add to `src/lib/ConversationPanel.test.ts` (follow the file's existing render helper and `invoke` mock — copy the setup of the nearest `conv-command` or `conv-interrupt` test):

```ts
  it('renders a task notification as an event row, never as XML', async () => {
    // A conversation whose only turn carries one notification item.
    const conv = {
      turns: [
        {
          prompt: null,
          at: '2026-09-18T10:01:00Z',
          ended_at: null,
          items: [
            {
              kind: 'notification',
              task_id: 'a6',
              tool_use_id: 'toolu_1',
              status: 'failed',
              summary: 'Agent "Posúdiť stratégiu testov" failed',
              result: null,
              output_file: '/private/tmp/x/tasks/a6.output',
              event: null,
            },
          ],
        },
      ],
      truncated: false,
      context: null,
      events: [],
    };
    const { getByTestId, container } = await renderWithConversation(conv);
    const row = getByTestId('conv-notification');
    expect(row.textContent).toContain('Agent "Posúdiť stratégiu testov" failed');
    expect(row.getAttribute('data-tone')).toBe('error');
    expect(container.textContent).not.toContain('<task-notification>');
  });
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npx vitest run src/lib/conversation.test.ts src/lib/ConversationPanel.test.ts`

Expected: FAIL — `notificationTone is not a function` in the first file, and `Unable to find an element by: [data-testid="conv-notification"]` in the second.

- [ ] **Step 3: Extend the wire types**

In `src/lib/conversation.ts`, add to the `ConvItem` union (after the `command` member, before `interrupt`):

```ts
  | {
      kind: 'notification';
      task_id: string | null;
      tool_use_id: string | null;
      status: string | null;
      summary: string | null;
      result: string | null;
      output_file: string | null;
      event: string | null;
    }
```

Add the identical member to the `ConvGroup` union (`groupItems` already passes unknown items straight through, so no change there):

```ts
  | {
      kind: 'notification';
      task_id: string | null;
      tool_use_id: string | null;
      status: string | null;
      summary: string | null;
      result: string | null;
      output_file: string | null;
      event: string | null;
    }
```

- [ ] **Step 4: Add the presentation helpers**

Append to `src/lib/conversation.ts`, after `toolGroupLabel`:

```ts
// ─── Background work: notifications (spec §"Frontend — the thread") ─────────

/** One `<task-notification>` as the backend parsed it. */
export type NotificationItem = Extract<ConvItem, { kind: 'notification' }>;

export type NotificationTone = 'info' | 'warn' | 'error';

/** A notification's tone, from its status. A mid-stream Monitor event has no
 *  status at all: it is progress, so it reads as plain info. */
export function notificationTone(status: string | null): NotificationTone {
  if (status === 'failed' || status === 'killed') return 'error';
  if (status === 'stopped') return 'warn';
  return 'info';
}

/** The glyph in front of the row. Keyed off the status rather than the tone,
 *  so a status-less event is a bullet rather than a tick. */
export function notificationMark(status: string | null): string {
  if (status === null) return '•';
  if (status === 'failed' || status === 'killed') return '✕';
  if (status === 'stopped') return '⏸';
  return '✓';
}

/** The row's single line: the harness's own sentence, plus the streamed
 *  event's first line when there is one. */
export function notificationLabel(n: { summary: string | null; event: string | null }): string {
  const head = n.summary?.trim() || 'Background task reported';
  const tail = n.event?.trim().split('\n')[0].trim();
  return tail ? `${head}: ${tail}` : head;
}
```

- [ ] **Step 5: Render the row**

In `src/lib/ConversationPanel.svelte`, add to the `from './conversation'` import list:

```ts
    notificationTone,
    notificationMark,
    notificationLabel,
```

In the groups `{#each}` (at `:1158`), add a branch after the `interrupt` one and before `subagent`:

```svelte
                  {:else if g.kind === 'notification'}
                    <div class="notification" data-testid="conv-notification" data-tone={notificationTone(g.status)}>
                      <span class="note-mark" aria-hidden="true">{notificationMark(g.status)}</span>
                      <span class="note-label">{notificationLabel(g)}</span>
                    </div>
```

Add to the component's `<style>` block, beside the `.interrupt` rule:

```css
  .notification {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    margin: 0.3rem 0;
    padding: 0.2rem 0.5rem;
    border-left: 3px solid var(--border);
    border-radius: 4px;
    font-size: 0.82rem;
    color: var(--fg-muted);
    background: var(--bg-pane);
  }
  .notification[data-tone='warn'] {
    border-left-color: var(--usage-warn);
  }
  .notification[data-tone='error'] {
    border-left-color: var(--usage-crit);
  }
  .note-mark {
    flex: 0 0 auto;
  }
  .note-label {
    min-width: 0;
    overflow-wrap: anywhere;
  }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `npx vitest run src/lib/conversation.test.ts src/lib/ConversationPanel.test.ts`

Expected: PASS.

- [ ] **Step 7: Type-check and commit**

```bash
npx svelte-check --threshold error
git add src/lib/conversation.ts src/lib/ConversationPanel.svelte src/lib/conversation.test.ts src/lib/ConversationPanel.test.ts
git commit -m "feat(ui): a task notification reads as an event row, not as XML"
```

---

### Task 4: Derive the background list

Two pure functions, no component. One walks the parsed turns; the other reads the two stores the app already loads.

**Files:**
- Modify: `src/lib/conversation.ts`
- Test: `src/lib/conversation.test.ts`

**Interfaces:**
- Consumes: `NotificationItem`, `ConvTurn` (Task 3); `SessionRow` from `./sessions`; `TaskRow` from `./tasks`.
- Produces:
  - `interface BackgroundEntry { key: string; source: 'transcript' | 'fleet_task' | 'fleet_session'; kind: string; label: string; status: BackgroundStatus; at: string | null; result: string | null; error: string | null; outputFile: string | null; sessionId: number | null; taskId: number | null; history: BackgroundReport[] }`
  - `type BackgroundStatus = 'running' | 'done' | 'failed' | 'stopped'`
  - `interface BackgroundReport { at: string | null; status: string | null; summary: string | null; result: string | null }`
  - `transcriptBackground(turns: ConvTurn[]): BackgroundEntry[]`
  - `fleetBackground(sessions: SessionRow[], tasks: TaskRow[], sessionId: number): BackgroundEntry[]`

- [ ] **Step 1: Write the failing tests**

Add to `src/lib/conversation.test.ts`:

```ts
import { transcriptBackground, fleetBackground, type ConvTurn } from './conversation';

/** A turn carrying exactly the items given. */
const turn = (items: ConvTurn['items'], at = '2026-09-18T10:00:00Z'): ConvTurn => ({
  prompt: null,
  at,
  ended_at: null,
  items,
});

const agentItem = (id: string, done = false) => ({
  kind: 'subagent' as const,
  id,
  name: 'Agent',
  agent_type: 'general-purpose',
  description: 'Posúdiť stratégiu testov',
  result: null,
  error: false,
  at: '2026-09-18T10:00:01Z',
  ended_at: null,
  done,
});

const noteItem = (toolUseId: string | null, status: string | null, extra = {}) => ({
  kind: 'notification' as const,
  task_id: 'a6',
  tool_use_id: toolUseId,
  status,
  summary: 'Agent finished',
  result: 'the report',
  output_file: '/private/tmp/x/tasks/a6.output',
  event: null,
  ...extra,
});

describe('transcriptBackground', () => {
  it('lists an agent that has not reported back as running', () => {
    const got = transcriptBackground([turn([agentItem('toolu_1')])]);
    expect(got).toHaveLength(1);
    expect(got[0].status).toBe('running');
    expect(got[0].kind).toBe('Agent');
    expect(got[0].label).toBe('Posúdiť stratégiu testov');
    expect(got[0].key).toBe('tool:toolu_1');
  });

  it('keys an entry by its task id once one has been seen', () => {
    const got = transcriptBackground([
      turn([agentItem('toolu_1')]),
      turn([noteItem('toolu_1', 'completed')], '2026-09-18T10:12:00Z'),
    ]);
    expect(got[0].key).toBe('task:a6');
    expect(got[0].status).toBe('done');
    expect(got[0].result).toBe('the report');
    expect(got[0].outputFile).toBe('/private/tmp/x/tasks/a6.output');
  });

  it('maps each terminal status onto the entry', () => {
    const of = (status: string) =>
      transcriptBackground([
        turn([agentItem('toolu_1')]),
        turn([noteItem('toolu_1', status)], '2026-09-18T10:12:00Z'),
      ])[0].status;
    expect(of('completed')).toBe('done');
    expect(of('failed')).toBe('failed');
    expect(of('killed')).toBe('failed');
    expect(of('stopped')).toBe('stopped');
  });

  it('keeps every report of a resumed agent, newest state last', () => {
    const got = transcriptBackground([
      turn([agentItem('toolu_1')]),
      turn([noteItem('toolu_1', 'completed', { result: 'first pass' })], '2026-09-18T10:05:00Z'),
      turn([noteItem('toolu_1', 'completed', { result: 'second pass' })], '2026-09-18T10:20:00Z'),
    ]);
    expect(got[0].history).toHaveLength(2);
    expect(got[0].history[1].at).toBe('2026-09-18T10:20:00Z');
    expect(got[0].result).toBe('second pass');
  });

  it('lists a Bash call only once a notification proves it was backgrounded', () => {
    const bash = (id: string) => ({
      kind: 'tool' as const,
      summary: 'Bash(command=gh run watch)',
      error: false,
      id,
      name: 'Bash',
      target: null,
      at: '2026-09-18T10:00:01Z',
      ended_at: null,
      done: true,
    });
    expect(transcriptBackground([turn([bash('toolu_fg')])])).toHaveLength(0);
    const got = transcriptBackground([
      turn([bash('toolu_bg')]),
      turn([noteItem('toolu_bg', 'completed')], '2026-09-18T10:05:00Z'),
    ]);
    expect(got).toHaveLength(1);
    expect(got[0].kind).toBe('Bash');
  });

  it('drops a finished foreground agent', () => {
    expect(transcriptBackground([turn([agentItem('toolu_1', true)])])).toHaveLength(0);
  });

  it('puts running entries first, then the newest', () => {
    const got = transcriptBackground([
      turn([{ ...agentItem('toolu_old'), at: '2026-09-18T09:00:00Z' }]),
      turn([noteItem('toolu_old', 'completed')], '2026-09-18T09:30:00Z'),
      turn([{ ...agentItem('toolu_new'), at: '2026-09-18T11:00:00Z' }]),
    ]);
    expect(got.map((e) => e.status)).toEqual(['running', 'done']);
  });

  it('ignores a notification that names nothing in the window', () => {
    expect(transcriptBackground([turn([noteItem('toolu_gone', 'completed')])])).toHaveLength(0);
  });
});

describe('fleetBackground', () => {
  const row = (over: Partial<SessionRow>): SessionRow =>
    ({ ...baseSessionRow, ...over }) as SessionRow;

  it('lists sessions whose parent is this one', () => {
    const got = fleetBackground(
      [
        row({ id: 2, parent_session_id: 7, kind: 'bg', friendly_name: 'Load layers', claude_status: 'working' }),
        row({ id: 3, parent_session_id: 9, kind: 'bg', friendly_name: 'Someone else' }),
      ],
      [],
      7,
    );
    expect(got).toHaveLength(1);
    expect(got[0].key).toBe('session:2');
    expect(got[0].label).toBe('Load layers');
    expect(got[0].status).toBe('running');
    expect(got[0].sessionId).toBe(2);
  });

  it('lists tasks this session dispatched, with the worker to switch to', () => {
    const got = fleetBackground(
      [],
      [
        {
          id: 11,
          requester_session_id: 7,
          worker_session_id: 4,
          prompt: 'Implement task 2\nmore detail',
          state: 'running',
          result: null,
          error: null,
          created_at: 1,
          started_at: 2,
          finished_at: null,
        },
      ],
      7,
    );
    expect(got[0].key).toBe('fleettask:11');
    expect(got[0].label).toBe('Implement task 2');
    expect(got[0].status).toBe('running');
    expect(got[0].sessionId).toBe(4);
    expect(got[0].taskId).toBe(11);
  });

  it('maps every task state', () => {
    const of = (state: TaskRow['state']) =>
      fleetBackground([], [{ ...baseTaskRow, requester_session_id: 7, state }], 7)[0].status;
    expect(of('queued')).toBe('running');
    expect(of('running')).toBe('running');
    expect(of('done')).toBe('done');
    expect(of('failed')).toBe('failed');
    expect(of('cancelled')).toBe('stopped');
  });
});
```

Define `baseSessionRow` and `baseTaskRow` at the top of the new `describe` blocks by copying the full mock row already used elsewhere in the repo — `src/App.test.ts:63` has a complete `SessionRow` literal; `src/lib/tasks.test.ts` has a `TaskRow` one. Do not invent field names; copy them.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npx vitest run src/lib/conversation.test.ts`

Expected: FAIL — `transcriptBackground is not a function`.

- [ ] **Step 3: Implement the two functions**

Append to `src/lib/conversation.ts`, after the notification helpers from Task 3:

```ts
import type { TaskRow } from './tasks';

/** How a background entry currently stands. `running` covers "launched and
 *  has not reported back" as well as a queued fleet task. */
export type BackgroundStatus = 'running' | 'done' | 'failed' | 'stopped';

/** One report a background task filed. A resumed agent files several. */
export interface BackgroundReport {
  at: string | null;
  status: string | null;
  summary: string | null;
  result: string | null;
}

/** One background thing that belongs to a session: something this
 *  conversation launched, or a fleet row/task spawned from it. */
export interface BackgroundEntry {
  /** Stable across renders: `task:<task-id>` or `tool:<tool_use id>` for a
   *  transcript entry, `session:<id>` / `fleettask:<id>` for a fleet one. */
  key: string;
  source: 'transcript' | 'fleet_task' | 'fleet_session';
  /** `Agent` | `Bash` | `Monitor` | … for a transcript entry; the session
   *  row's `kind`, or `task`, for a fleet one. */
  kind: string;
  label: string;
  status: BackgroundStatus;
  /** When it was launched; ISO for a transcript entry, null for fleet rows
   *  whose own list already shows their age. */
  at: string | null;
  result: string | null;
  error: string | null;
  outputFile: string | null;
  /** The fleet session to switch to, when there is one. */
  sessionId: number | null;
  taskId: number | null;
  history: BackgroundReport[];
}

/** Tools whose calls can be backgrounded and then report in. A foreground
 *  call of the same tool never gets a notification, which is exactly how the
 *  two are told apart — the launch input carries no flag to read. */
const BACKGROUND_TOOLS = new Set(['Bash', 'Monitor', 'Workflow', 'SendMessage']);

function statusFromReports(reports: BackgroundReport[]): BackgroundStatus {
  const last = [...reports].reverse().find((r) => r.status !== null);
  if (!last) return 'running';
  if (last.status === 'completed') return 'done';
  if (last.status === 'stopped') return 'stopped';
  return 'failed';
}

/** Running first, then newest launch first. */
function byRunningThenNewest(a: BackgroundEntry, b: BackgroundEntry): number {
  const run = (e: BackgroundEntry) => (e.status === 'running' ? 0 : 1);
  if (run(a) !== run(b)) return run(a) - run(b);
  return (b.at ?? '').localeCompare(a.at ?? '');
}

/** The background work this conversation launched, from its own turns.
 *
 *  A subagent block is listed when a notification named it, or while it is
 *  still open — a finished call with no notification was a foreground one.
 *  A tool line is listed only when a notification named it. */
export function transcriptBackground(turns: ConvTurn[]): BackgroundEntry[] {
  const reports = new Map<string, BackgroundReport[]>();
  const taskIds = new Map<string, string>();
  for (const t of turns) {
    for (const item of t.items) {
      if (item.kind !== 'notification' || item.tool_use_id === null) continue;
      const list = reports.get(item.tool_use_id) ?? [];
      list.push({ at: t.at, status: item.status, summary: item.summary, result: item.result });
      reports.set(item.tool_use_id, list);
      if (item.task_id !== null) taskIds.set(item.tool_use_id, item.task_id);
    }
  }
  const lastWith = <K extends keyof BackgroundReport>(rs: BackgroundReport[], k: K) =>
    [...rs].reverse().find((r) => r[k] !== null)?.[k] ?? null;

  const out: BackgroundEntry[] = [];
  for (const t of turns) {
    for (const item of t.items) {
      if (item.kind === 'subagent') {
        const rs = (item.id !== null && reports.get(item.id)) || [];
        if (rs.length === 0 && item.done) continue;
        out.push({
          key: item.id !== null && taskIds.has(item.id) ? `task:${taskIds.get(item.id)}` : `tool:${item.id ?? ''}`,
          source: 'transcript',
          kind: item.name || 'Agent',
          label: item.description ?? item.agent_type ?? 'subagent',
          status: statusFromReports(rs),
          at: item.at,
          result: lastWith(rs, 'result') ?? item.result,
          error: null,
          outputFile: null,
          sessionId: null,
          taskId: null,
          history: rs,
        });
      } else if (item.kind === 'tool' && item.id !== null && BACKGROUND_TOOLS.has(item.name)) {
        const rs = reports.get(item.id) ?? [];
        if (rs.length === 0) continue;
        out.push({
          key: taskIds.has(item.id) ? `task:${taskIds.get(item.id)}` : `tool:${item.id}`,
          source: 'transcript',
          kind: item.name,
          label: item.target ?? item.summary,
          status: statusFromReports(rs),
          at: item.at,
          result: lastWith(rs, 'result'),
          error: null,
          outputFile: null,
          sessionId: null,
          taskId: null,
          history: rs,
        });
      }
    }
  }
  // The output file is per task and only the notification carries it.
  for (const e of out) {
    const id = e.key.startsWith('task:') ? e.key.slice(5) : null;
    if (id === null) continue;
    for (const [toolUseId, taskId] of taskIds) {
      if (taskId !== id) continue;
      const item = turns
        .flatMap((t) => t.items)
        .find((i) => i.kind === 'notification' && i.tool_use_id === toolUseId && i.output_file !== null);
      if (item && item.kind === 'notification') e.outputFile = item.output_file;
    }
  }
  return out.sort(byRunningThenNewest);
}

/** The fleet rows and tasks this session spawned. A worker session appears
 *  both as a session and as its task: they are different things — one is a
 *  place to go, the other a unit of work with a result. */
export function fleetBackground(
  sessions: SessionRow[],
  tasks: TaskRow[],
  sessionId: number,
): BackgroundEntry[] {
  const out: BackgroundEntry[] = [];
  for (const s of sessions) {
    if (s.parent_session_id !== sessionId) continue;
    out.push({
      key: `session:${s.id}`,
      source: 'fleet_session',
      kind: s.kind,
      label: s.friendly_name || s.tmux_name,
      status:
        s.claude_status === 'working' ? 'running'
        : s.claude_status === 'failed' ? 'failed'
        : s.claude_status === 'stopped' ? 'stopped'
        : 'done',
      at: null,
      result: null,
      error: null,
      outputFile: null,
      sessionId: s.id,
      taskId: null,
      history: [],
    });
  }
  for (const t of tasks) {
    if (t.requester_session_id !== sessionId) continue;
    out.push({
      key: `fleettask:${t.id}`,
      source: 'fleet_task',
      kind: 'task',
      label: (t.prompt ?? '').split('\n').find((l) => l.trim().length > 0)?.trim() ?? `task #${t.id}`,
      status:
        t.state === 'queued' || t.state === 'running' ? 'running'
        : t.state === 'done' ? 'done'
        : t.state === 'cancelled' ? 'stopped'
        : 'failed',
      at: null,
      result: t.result,
      error: t.error,
      outputFile: null,
      sessionId: t.worker_session_id,
      taskId: t.id,
      history: [],
    });
  }
  return out;
}
```

Move the `import type { TaskRow } from './tasks';` line up to the other imports at the top of the file — it is written inline above only to show where it comes from.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `npx vitest run src/lib/conversation.test.ts`

Expected: PASS.

- [ ] **Step 5: Type-check and commit**

```bash
npx svelte-check --threshold error
git add src/lib/conversation.ts src/lib/conversation.test.ts
git commit -m "feat(ui): derive a session's background work from its turns and fleet rows"
```

---

### Task 5: `BackgroundDetail.svelte`

The view that replaces the thread when an entry is picked. It renders one `BackgroundEntry`; it does no fetching and owns no state beyond its expand toggle.

**Files:**
- Create: `src/lib/BackgroundDetail.svelte`
- Test: `src/lib/BackgroundDetail.test.ts`

**Interfaces:**
- Consumes: `BackgroundEntry`, `BackgroundStatus` (Task 4); `MarkdownView.svelte` as `Markdown`; `CopyButton.svelte`.
- Produces: a component with props `{ entry: BackgroundEntry; onBack: () => void }`.

- [ ] **Step 1: Write the failing test**

Create `src/lib/BackgroundDetail.test.ts`:

```ts
import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import BackgroundDetail from './BackgroundDetail.svelte';
import type { BackgroundEntry } from './conversation';

const entry = (over: Partial<BackgroundEntry> = {}): BackgroundEntry => ({
  key: 'task:a6',
  source: 'transcript',
  kind: 'Agent',
  label: 'Posúdiť stratégiu testov',
  status: 'done',
  at: '2026-09-18T10:00:01Z',
  result: '# Posudok\n\nVšetko overené.',
  error: null,
  outputFile: '/private/tmp/x/tasks/a6.output',
  sessionId: null,
  taskId: null,
  history: [],
  ...over,
});

describe('BackgroundDetail', () => {
  it('shows what the agent was asked to do and what it reported', () => {
    const { getByTestId } = render(BackgroundDetail, { entry: entry(), onBack: () => {} });
    expect(getByTestId('bg-detail-label').textContent).toContain('Posúdiť stratégiu testov');
    expect(getByTestId('bg-detail-kind').textContent).toContain('Agent');
    expect(getByTestId('bg-detail-result').textContent).toContain('Všetko overené.');
  });

  it('offers the output file path to copy', () => {
    const { getByTestId } = render(BackgroundDetail, { entry: entry(), onBack: () => {} });
    expect(getByTestId('bg-detail-output').textContent).toContain('/private/tmp/x/tasks/a6.output');
  });

  it('says so plainly when nothing has been reported yet', () => {
    const { getByTestId, queryByTestId } = render(BackgroundDetail, {
      entry: entry({ status: 'running', result: null, outputFile: null }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-empty').textContent).toContain('has not reported back yet');
    expect(queryByTestId('bg-detail-output')).toBeNull();
  });

  it('shows a failure reason instead of a report', () => {
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({ source: 'fleet_task', status: 'failed', result: null, error: 'worker died' }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-error').textContent).toContain('worker died');
  });

  it('lists every report when an agent was resumed', () => {
    const { getAllByTestId } = render(BackgroundDetail, {
      entry: entry({
        history: [
          { at: '2026-09-18T10:05:00Z', status: 'completed', summary: 's', result: 'first pass' },
          { at: '2026-09-18T10:20:00Z', status: 'completed', summary: 's', result: 'second pass' },
        ],
      }),
      onBack: () => {},
    });
    expect(getAllByTestId('bg-detail-report')).toHaveLength(2);
  });

  it('does not list a single report twice', () => {
    const { queryAllByTestId } = render(BackgroundDetail, {
      entry: entry({
        history: [{ at: '2026-09-18T10:05:00Z', status: 'completed', summary: 's', result: 'only' }],
      }),
      onBack: () => {},
    });
    expect(queryAllByTestId('bg-detail-report')).toHaveLength(0);
  });

  it('goes back', async () => {
    const onBack = vi.fn();
    const { getByTestId } = render(BackgroundDetail, { entry: entry(), onBack });
    await fireEvent.click(getByTestId('bg-detail-back'));
    expect(onBack).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/lib/BackgroundDetail.test.ts`

Expected: FAIL — `Failed to resolve import "./BackgroundDetail.svelte"`.

- [ ] **Step 3: Write the component**

Create `src/lib/BackgroundDetail.svelte`:

```svelte
<script lang="ts">
  // The Conversations tab's replaced-thread view for one background entry:
  // an agent or command this conversation launched, or a fleet task it
  // dispatched. Read-only — it renders what the transcript and the stores
  // already carry, and fetches nothing.
  import type { BackgroundEntry } from './conversation';
  import Markdown from './MarkdownView.svelte';
  import CopyButton from './CopyButton.svelte';

  let { entry, onBack }: { entry: BackgroundEntry; onBack: () => void } = $props();

  const STATUS_WORD: Record<BackgroundEntry['status'], string> = {
    running: 'running',
    done: 'done',
    failed: 'failed',
    stopped: 'stopped',
  };
  // One report is the entry's own result, already shown above; only a
  // resumed task's several are worth listing separately.
  const reports = $derived(entry.history.length > 1 ? entry.history : []);
  const nothingYet = $derived(entry.result === null && entry.error === null);
</script>

<div class="bg-detail" data-testid="bg-detail">
  <div class="bg-head">
    <button type="button" class="linkish" data-testid="bg-detail-back" onclick={onBack}>← Back to conversation</button>
  </div>
  <h3 class="bg-title">
    <span class="bg-kind" data-testid="bg-detail-kind">{entry.kind}</span>
    <span class="bg-label" data-testid="bg-detail-label">{entry.label}</span>
    <span class="bg-status" data-status={entry.status}>{STATUS_WORD[entry.status]}</span>
  </h3>

  {#if entry.error}
    <p class="bg-error" data-testid="bg-detail-error">{entry.error}</p>
  {:else if entry.result}
    <div class="bg-result" data-testid="bg-detail-result"><Markdown source={entry.result} /></div>
  {:else}
    <p class="muted" data-testid="bg-detail-empty">This background task has not reported back yet.</p>
  {/if}

  {#if reports.length > 0}
    <h4 class="bg-sub">Reports ({reports.length})</h4>
    {#each reports as r, i (i)}
      <div class="bg-report" data-testid="bg-detail-report">
        <div class="bg-report-head">
          {#if r.at}<time datetime={r.at}>{new Date(r.at).toLocaleTimeString()}</time>{/if}
          {#if r.status}<span class="bg-report-status">{r.status}</span>{/if}
        </div>
        {#if r.result}<Markdown source={r.result} />{/if}
      </div>
    {/each}
  {/if}

  {#if entry.outputFile}
    <div class="bg-output" data-testid="bg-detail-output">
      <span class="bg-output-label">Full output on the host</span>
      <code>{entry.outputFile}</code>
      <CopyButton text={entry.outputFile} label="Copy path" />
    </div>
  {/if}
</div>

<style>
  .bg-detail {
    padding: 0.6rem 0.9rem 1.2rem;
    overflow-y: auto;
    min-height: 0;
  }
  .bg-head {
    margin-bottom: 0.5rem;
  }
  .bg-title {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin: 0 0 0.6rem;
    font-size: 0.95rem;
    min-width: 0;
  }
  .bg-kind {
    flex: 0 0 auto;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .bg-label {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .bg-status {
    flex: 0 0 auto;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .bg-status[data-status='failed'] {
    color: var(--usage-crit);
  }
  .bg-status[data-status='stopped'] {
    color: var(--usage-warn);
  }
  .bg-error {
    color: var(--usage-crit);
    overflow-wrap: anywhere;
  }
  .bg-sub {
    margin: 1rem 0 0.4rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
  .bg-report {
    border-left: 2px solid var(--border);
    padding-left: 0.6rem;
    margin-bottom: 0.8rem;
  }
  .bg-report-head {
    display: flex;
    gap: 0.4rem;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .bg-output {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    margin-top: 1rem;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .bg-output code {
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--fg-muted);
  }
  .linkish {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
</style>
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/lib/BackgroundDetail.test.ts`

Expected: PASS.

- [ ] **Step 5: Type-check and commit**

```bash
npx svelte-check --threshold error
git add src/lib/BackgroundDetail.svelte src/lib/BackgroundDetail.test.ts
git commit -m "feat(ui): a detail view for one piece of background work"
```

---

### Task 6: Wire the switcher into the panel

**Files:**
- Modify: `src/lib/ConversationPanel.svelte` (imports, panel state, toolbar at `:1010`, thread-area branch at `:968`, notification row from Task 3, composer guard at `:1210`)
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `transcriptBackground`, `fleetBackground`, `BackgroundEntry` (Task 4); `BackgroundDetail.svelte` (Task 5); `selectSession` from `./selection`; the `sessions` and `tasks` stores.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Write the failing tests**

Add to `src/lib/ConversationPanel.test.ts`, reusing the file's existing render helper:

```ts
  it('offers a background switcher listing what the conversation launched', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-background-button'));
    const rows = getAllByTestId('conv-background-item');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('Posúdiť stratégiu testov');
  });

  it('hides the switcher when nothing ran in the background', async () => {
    const { queryByTestId } = await renderWithConversation(convWithNoBackground);
    expect(queryByTestId('conv-background-button')).toBeNull();
  });

  it('replaces the thread with the picked entry, and comes back', async () => {
    const { getByTestId, getAllByTestId, queryByTestId } =
      await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-background-button'));
    await fireEvent.click(getAllByTestId('conv-background-item')[0]);
    expect(getByTestId('bg-detail')).toBeTruthy();
    expect(queryByTestId('conv-scroller')).toBeNull();
    expect(queryByTestId('conv-composer')).toBeNull();
    await fireEvent.click(getByTestId('bg-detail-back'));
    expect(getByTestId('conv-scroller')).toBeTruthy();
  });

  it('opens the entry a notification row names', async () => {
    const { getByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-notification'));
    expect(getByTestId('bg-detail-label').textContent).toContain('Posúdiť stratégiu testov');
  });

  it('switches the app to a fleet child session rather than showing a detail', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithNoBackground, {
      sessions: [{ ...baseSessionRow, id: 2, parent_session_id: 1, friendly_name: 'Load layers' }],
    });
    await fireEvent.click(getByTestId('conv-background-button'));
    await fireEvent.click(getAllByTestId('conv-background-item')[0]);
    expect(selectSessionSpy).toHaveBeenCalledWith(expect.objectContaining({ id: 2 }));
  });
```

Build `convWithBackgroundAgent` as a conversation with two turns: one carrying the `subagent` item (`id: 'toolu_1'`, `done: false`, `description: 'Posúdiť stratégiu testov'`) and one carrying the matching `notification` item (`tool_use_id: 'toolu_1'`, `task_id: 'a6'`, `status: 'completed'`). `convWithNoBackground` is a plain prompt-and-text conversation. Mock `./selection`'s `selectSession` with `vi.mock` and keep the spy in `selectSessionSpy`; seed the `sessions` / `tasks` stores through their existing `set` in the render helper.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npx vitest run src/lib/ConversationPanel.test.ts`

Expected: FAIL — `Unable to find an element by: [data-testid="conv-background-button"]`.

- [ ] **Step 3: Add the imports and the state**

In `src/lib/ConversationPanel.svelte`, add to the imports:

```ts
  import BackgroundDetail from './BackgroundDetail.svelte';
  import { selectSession } from './selection';
  import { sessions } from './sessions';
  import { tasks } from './tasks';
```

Add `transcriptBackground`, `fleetBackground` and `type BackgroundEntry` to the existing `from './conversation'` import list.

Add beside the other `$state` declarations (near `viewing`):

```ts
  // The background entry whose detail replaces the thread; null = the thread.
  // Keyed by BackgroundEntry.key, not by index: the list re-derives on every
  // poll and a running entry moves as it finishes.
  let background = $state<string | null>(null);
  let backgroundOpen = $state(false);
  let backgroundWrap: HTMLDivElement | undefined = $state();
```

And the derivations:

```ts
  const bgEntries = $derived(
    conv
      ? [...transcriptBackground(conv.turns), ...fleetBackground($sessions, $tasks, session.id)]
      : fleetBackground($sessions, $tasks, session.id),
  );
  const bgEntry = $derived(bgEntries.find((e) => e.key === background) ?? null);
```

Clear the selection whenever the panel changes what it is showing — add to the existing effect that reacts to `session.id` / `viewing`:

```ts
  $effect(() => {
    // Reading both is the point: stepping to another session or another
    // conversation drops the open background detail.
    void session.id;
    void viewing;
    untrack(() => {
      background = null;
      backgroundOpen = false;
    });
  });
```

- [ ] **Step 4: Add the toolbar dropdown**

In the toolbar (after the `turns-wrap` div, still inside `<div class="toolbar">`):

```svelte
          {#if bgEntries.length > 0}
            <div class="turns-wrap" bind:this={backgroundWrap}>
              <button
                type="button"
                class="tb-btn"
                data-testid="conv-background-button"
                aria-expanded={backgroundOpen}
                onclick={() => (backgroundOpen = !backgroundOpen)}
                >{bgEntries.length} background</button
              >
              {#if backgroundOpen}
                <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                <ul class="turn-index" aria-label="Background work" data-testid="conv-background-list">
                  {#each bgEntries as e (e.key)}
                    <li>
                      <button type="button" data-testid="conv-background-item" onclick={() => openBackground(e)}>
                        <span class="ti-label">{e.kind} · {e.label}</span>
                        <span class="bg-item-status" data-status={e.status}>{e.status}</span>
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/if}
```

Add the handler beside the other functions in `<script>`:

```ts
  /** Open a background entry. A fleet session is a place, not a report: it
   *  has its own transcript, terminal and composer, so it takes the whole
   *  app rather than this pane. */
  function openBackground(e: BackgroundEntry): void {
    backgroundOpen = false;
    if (e.source === 'fleet_session' && e.sessionId !== null) {
      const row = $sessions.find((s) => s.id === e.sessionId);
      if (row) selectSession(row);
      return;
    }
    background = e.key;
  }

  /** The entry a notification row belongs to, by task id first and by the
   *  call it named second — the same keying `transcriptBackground` uses. */
  function entryForNotification(n: { task_id: string | null; tool_use_id: string | null }): BackgroundEntry | null {
    const keys = [n.task_id ? `task:${n.task_id}` : null, n.tool_use_id ? `tool:${n.tool_use_id}` : null];
    for (const k of keys) {
      if (k === null) continue;
      const hit = bgEntries.find((e) => e.key === k);
      if (hit) return hit;
    }
    return null;
  }
```

Add to the `<style>` block:

```css
  .bg-item-status {
    margin-left: auto;
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .bg-item-status[data-status='failed'] {
    color: var(--usage-crit);
  }
  .bg-item-status[data-status='stopped'] {
    color: var(--usage-warn);
  }
```

- [ ] **Step 5: Make the notification row open its entry**

Replace the notification branch added in Task 3 with:

```svelte
                  {:else if g.kind === 'notification'}
                    {@const target = entryForNotification(g)}
                    <svelte:element
                      this={target ? 'button' : 'div'}
                      type={target ? 'button' : undefined}
                      class="notification"
                      class:clickable={target !== null}
                      data-testid="conv-notification"
                      data-tone={notificationTone(g.status)}
                      onclick={target ? () => openBackground(target) : undefined}
                    >
                      <span class="note-mark" aria-hidden="true">{notificationMark(g.status)}</span>
                      <span class="note-label">{notificationLabel(g)}</span>
                    </svelte:element>
```

Add to the styles:

```css
  .notification.clickable {
    width: 100%;
    text-align: left;
    font: inherit;
    cursor: pointer;
  }
  .notification.clickable:hover {
    border-left-color: var(--accent);
  }
```

- [ ] **Step 6: Mount the detail and guard the composer**

Change the opening of the thread area (at `:968`) from:

```svelte
  <div class="thread-area">
  {#if empty && !(pending && viewing === null)}
```

to:

```svelte
  <div class="thread-area">
  {#if bgEntry}
    <BackgroundDetail entry={bgEntry} onBack={() => (background = null)} />
  {:else if empty && !(pending && viewing === null)}
```

Change the composer guard at `:1210` from `{#if canPrompt}` to:

```svelte
  {#if canPrompt && bgEntry === null}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `npx vitest run src/lib/ConversationPanel.test.ts`

Expected: PASS.

- [ ] **Step 8: Run the whole frontend suite, type-check, commit**

```bash
npx vitest run
npx svelte-check --threshold error
git add src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(ui): switch between a session's background agents, tasks and sessions"
```

Expected: the full Vitest run is green. If a pre-existing failure appears, first confirm `node_modules` is current with `pnpm install --frozen-lockfile` — a stale tree fakes failures in `App.test.ts` and `clipboard_native.test.ts`.

---

### Task 7: `new_bg_session` records who asked for it

**Files:**
- Modify: `crates/fleet-core/src/service/bg_sessions.rs` (`NewBgSessionArgs` at `:22`, `new_bg_session_tracked` at `:145`, `stamp_bg_row` at `:250`)
- Modify: `src-tauri/src/backend/tests_routing.rs:877-895`
- Modify: `src/lib/sessions.ts:532`
- Test: `crates/fleet-core/src/service/bg_sessions.rs` test module (`:400` has the `stamp_bg_row` test to copy from)

**Interfaces:**
- Consumes: `Store::set_parent_session_id(id: i64, parent: Option<i64>)` (`store/sessions.rs:1195`).
- Produces: `NewBgSessionArgs.requester_session_id: Option<i64>`; `newBgSession(hostAlias, name, prompt, requesterSessionId?)` on the frontend.

- [ ] **Step 1: Write the failing test**

In the `bg_sessions.rs` test module, beside `stamp_bg_row_names_and_stamps_the_reconciled_row`:

```rust
    #[test]
    fn stamp_bg_row_records_the_session_that_asked_for_it() {
        let store = make_store();
        let parent = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u0", None, "u0", Some("working"), 5, "bg", 5)
                .unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5, "bg", 5)
                .unwrap();
            s.get_session_by_claude_id("u0").unwrap().unwrap().id
        };
        let row = stamp_bg_row(&store, "u1", "go", Some(parent)).expect("the row is reconciled");
        assert_eq!(row.parent_session_id, Some(parent));
    }

    #[test]
    fn stamp_bg_row_leaves_no_parent_when_nobody_asked() {
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5, "bg", 5)
                .unwrap();
        }
        let row = stamp_bg_row(&store, "u1", "go", None).expect("the row is reconciled");
        assert_eq!(row.parent_session_id, None);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p fleet-core --lib service::bg_sessions`

Expected: FAIL to compile — `this function takes 3 arguments but 4 arguments were supplied`.

- [ ] **Step 3: Add the argument and stamp the parent**

In `NewBgSessionArgs` (`:22`), add after `prompt`:

```rust
    /// The session asking for this one; becomes the new row's parent.
    #[serde(default)]
    pub requester_session_id: Option<i64>,
```

In `new_bg_session_tracked`, capture it beside the other clones and pass it on:

```rust
    let requester = args.requester_session_id;
```

and change the final line from `res.session = stamp_bg_row(store, claude_id, &prompt);` to:

```rust
    res.session = stamp_bg_row(store, claude_id, &prompt, requester);
```

In `stamp_bg_row`, add the parameter and the write:

```rust
fn stamp_bg_row(
    store: &Mutex<Store>,
    claude_id: &str,
    prompt: &str,
    requester: Option<i64>,
) -> Option<crate::store::SessionRow> {
```

and, immediately before the final `s.get_session_by_id(row.id).ok().flatten()`:

```rust
    // Parentage is how the requester's Conversations tab finds this row
    // again; `dispatch_task` stamps its worker the same way.
    if requester.is_some() {
        let _ = s.set_parent_session_id(row.id, requester);
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p fleet-core --lib service::bg_sessions`

Expected: PASS.

- [ ] **Step 5: Carry the field across the hub**

In `src-tauri/src/backend/tests_routing.rs`, change the `new_bg_session` row (`:877-895`) so the expected wire args and the constructed struct both carry a **non-default** value:

```rust
            json!({ "host_alias": "trn", "name": "worker", "prompt": "go", "requester_session_id": 41 }),
```

```rust
                    NewBgSessionArgs {
                        host_alias: "trn".into(),
                        name: "worker".into(),
                        prompt: "go".into(),
                        requester_session_id: Some(41),
                    },
```

- [ ] **Step 6: Regenerate the control API reference**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`

Then verify without the env var:

Run: `cargo test -p fleet-core reference_is_current`

Expected: PASS. `git status` will show `docs/control-api-reference.md` changed — never hand-edit it.

- [ ] **Step 7: Pass the requester from the frontend wrapper**

In `src/lib/sessions.ts`, change `newBgSession` to:

```ts
export async function newBgSession(
  hostAlias: string,
  name: string,
  prompt: string,
  /** The session asking for this one; the new row becomes its child. Null
   *  from the desktop dialog — nobody asked for it from inside a session. */
  requesterSessionId: number | null = null,
): Promise<Result<NewBgSessionResult>> {
  const r = await invokeCmd<NewBgSessionResult>('new_bg_session', {
    args: {
      host_alias: hostAlias,
      name,
      prompt,
      requester_session_id: requesterSessionId,
    },
  });
  if (r.ok && r.value?.session) acceptCommandRow(r.value.session);
  return r;
}
```

`NewBgSessionDialog.svelte` needs no change — the fourth argument defaults.

- [ ] **Step 8: Run the affected suites**

Run: `cargo test -p fleet-core --lib service::bg_sessions`
Run: `cargo test -p claude-fleet --lib tests_routing`
Run: `npx vitest run src/lib/sessions.test.ts`

Expected: PASS on all three. `sessions.test.ts:164` asserts the args `new_bg_session` is called with — update that assertion to include `requester_session_id: null`.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/fleet-core/src/service/bg_sessions.rs src-tauri/src/backend/tests_routing.rs src/lib/sessions.ts src/lib/sessions.test.ts docs/control-api-reference.md
git commit -m "feat(bg): a background session records the session that asked for it"
```

---

### Task 8: Full verification

**Files:** none changed unless a failure demands it.

- [ ] **Step 1: Refresh dependencies**

Run: `pnpm install --frozen-lockfile`

- [ ] **Step 2: Run the whole suite in CI order**

Run: `scripts/ci-local.sh`

Read the output unpiped — never through `| tail`. Expected: green. Note that CI's clippy is newer than the local one, so a locally clean run can still fail on GitHub; the branch's CI run on the final head is the real gate.

- [ ] **Step 3: Confirm the generated artefacts are committed**

```bash
git status --short
```

Expected: clean. If `docs/control-api-reference.md`, the hub contract golden, `src/lib/hub_verdicts.generated.json` or `docs/hub.md` show as modified, they were regenerated but not committed — commit them now rather than leaving CI to fail on them.

- [ ] **Step 4: Commit any fixes and push the branch**

```bash
git push -u origin feature/conversations-notifications-background-ebdffa
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| `ConvItem::Notification` variant, fields, `NOTIFICATION_RESULT_MAX_CHARS` | 1 |
| "Where it is recognised" (starts-with rule) | 1 |
| "Turn placement" (prompt-less turn, coalescing) | 1 |
| "Joining to the launching item" | 2 |
| Launch-ack suppression (spec problem 2) | 2 |
| Wire contract regen | 1 (step 8) |
| `new_bg_session` requester, docs/routing regen | 7 |
| Compact clickable row, tones | 3 (row), 6 (clickable) |
| `SubagentBlock` showing the real report | 2 (the parser fills it; the component needed no change) |
| Switcher, two groups, scoping | 4, 6 |
| Switching table (transcript / fleet task / fleet session) | 5, 6 |
| Breadcrumb back, composer hidden | 6 |
| Test list | 1, 2, 3, 4, 5, 6 |

**Known deviation:** a joined `ConvItem::Tool` keeps its own summary rather than taking the notification's, because `Bash(command=…)` is the useful text. Task 2 step 6 corrects the spec.

**Types:** `BackgroundEntry`, `BackgroundStatus`, `BackgroundReport` are defined once in Task 4 and used unchanged in Tasks 5 and 6. `notificationTone` / `notificationMark` / `notificationLabel` are defined in Task 3 and used in Tasks 3 and 6. `stamp_bg_row` gains its fourth parameter in Task 7 and has no other caller.
