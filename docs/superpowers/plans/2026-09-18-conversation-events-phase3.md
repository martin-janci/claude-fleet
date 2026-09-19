# Conversation Event Tracking — Phase 3 (Detail UX) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Conversations tab shows tool calls as compact lines (verb, target, duration, error), expands a call to its input and result on demand, shows subagents as their own blocks, shows a live "doing now" line, and adds find, copy and a turn index.

**Architecture:**
- **Backend.** The tool item carries structured fields: tool-use id, name, target, start and end timestamps, and a done flag. Task/Agent calls become their own `Subagent` item.
- **Lazy detail fetch.** Inputs and results stay out of the 5 s poll payload. A new `session_tool_detail` command greps the transcript for one tool-use id on demand.
- **Frontend.** Two small components, `ToolLine.svelte` and `SubagentBlock.svelte`, plus pure helpers in `conversation.ts`. Find, copy and turn index live in `ConversationPanel.svelte`, with helpers in a new `conversation_nav.ts`.

**Tech Stack:** Rust (fleet-core, serde), Tauri command, Svelte 5 runes, TypeScript, Vitest.

**Spec:** `docs/superpowers/specs/2026-09-18-conversation-events-design.md` Phase 3 (§3.1 tool calls, §3.2 doing now, §3.3 navigation). Builds on phase 2, branch `feature/conversation-ui-phase2` (PR #133).

## Global Constraints

- **Branch and git:** work on branch `feature/conversation-ui-phase3`, created from `feature/conversation-ui-phase2`. Run all git as `git -C <worktree>`. Subagents never pull, push, rebase, checkout or stash.
- **Wire format:** fields are snake_case. Rust `Option<T>` maps to TS `T | null`. `ConvItem` keeps `kind` in snake_case.
- **Payload budget:** the poll payload must not carry tool inputs or results. Detail comes only through `session_tool_detail`. Caps: 8 000 chars each for input text and result text.
- **Shell quoting:** every value in a shell script goes through `crate::shell::quote`. The tool-use id is validated: `^[A-Za-z0-9_-]{1,100}$`.
- **Logging:** transcript text is never logged.
- **Store lock:** never hold the `Mutex<Store>` guard across `.await`.
- **API docs:** a new Tauri command needs `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, then a normal run.
- **Test files:** the filesystem is case-insensitive. Never create a test file whose name differs from an existing one only by case.
- **Existing behaviour:**
  - Keep existing `data-testid`s working. `conv-tool` stays on each tool line, and `conv-tools` stays on a folded group.
  - Keep the MCP `session_transcript` text format for tools: `[tool_use] Name(input…)`.
- **Frontend rules:**
  - No new npm dependencies.
  - Colours use the panel's CSS vars. Amber `#e6a23c` and red `#e64a4a` only where the panel already hardcodes them.
  - Copy uses `copyText` from `src/lib/clipboard.ts`.
- **Commands:**
  - Frontend: `npx vitest run`, `npx svelte-check`, with 0 errors and no new warnings.
  - Rust: the full `cargo test -p fleet-core` runs unpiped before each commit, plus `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`, and `cargo check -p claude-fleet` when `src-tauri` changes.

## File Map

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/transcript.rs` | Structured `Tool` item, `Subagent` item; `locate_script` split out of `read_script`; `tool_lines_script`; `ToolDetail` + `parse_tool_detail` + `fetch_tool_detail` |
| `crates/fleet-core/src/validate.rs` | `tool_use_id` validator |
| `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs` | `session_tool_detail` command |
| `src/lib/conversation.ts` | TS mirrors; `toolDetail()`; tool/subagent/doing-now helpers |
| `src/lib/conversation_nav.ts` (new) | Find + turn-index helpers (pure) |
| `src/lib/ToolLine.svelte` (new) | One tool call: compact line, lazy expand |
| `src/lib/SubagentBlock.svelte` (new) | One Task/Agent call |
| `src/lib/ConversationPanel.svelte` | Use the components; doing-now line; find bar; copy buttons; turn index |
| Tests | `transcript.rs` tests, `conversation.test.ts`, `conversation_nav.test.ts` (new), `ToolLine.test.ts` (new), `SubagentBlock.test.ts` (new), `ConversationPanel.test.ts` |

Real shapes (Claude Code transcripts):

```json
{"type":"assistant","timestamp":"2026-09-18T09:00:20.101Z","message":{"content":[{"type":"tool_use","id":"toolu_01AbC","name":"Bash","input":{"command":"cargo test -p fleet-core","description":"Run tests"}}]}}
{"type":"user","timestamp":"2026-09-18T09:00:41.990Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_01AbC","content":"test result: ok…","is_error":false}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_02X","name":"Edit","input":{"file_path":"/w/src/a.rs","old_string":"let a = 1;","new_string":"let a = 2;"}}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_03Y","name":"Task","input":{"description":"Map the store","prompt":"…","subagent_type":"Explore"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_03Y","content":[{"type":"text","text":"Final report: …"}]}]}}
```

`tool_result.content` is either a string or an array of blocks (`text` and `image`).

---

### Task 1: Backend — structured tool and subagent items

**Files:** Modify `crates/fleet-core/src/service/transcript.rs`; `src/lib/conversation.ts` (mirrors + `groupItems`/`toolGroupLabel` adaptation); fixtures in `conversation.test.ts`, `ConversationPanel.test.ts`.

**Interfaces — produces:**

```rust
ConvItem::Tool {
    summary: String,          // unchanged one-liner (MCP text + title)
    #[serde(default)] error: bool,
    id: Option<String>,       // tool_use id
    name: String,             // "Bash", "Edit", "mcp__x__y", …
    target: Option<String>,   // what it touched (see tool_target)
    at: Option<String>,       // ISO timestamp of the tool_use entry
    ended_at: Option<String>, // ISO timestamp of its tool_result entry
    done: bool,               // a tool_result was seen
}
ConvItem::Subagent {
    id: Option<String>,
    name: String,                 // "Task" or "Agent"
    agent_type: Option<String>,   // input.subagent_type
    description: Option<String>,  // input.description
    result: Option<String>,       // final text of its tool_result, ≤ 1 500 chars
    error: bool,
    at: Option<String>,
    ended_at: Option<String>,
    done: bool,
}
pub fn tool_target(name: &str, input: Option<&serde_json::Value>) -> Option<String>
```

```ts
// conversation.ts
| { kind: 'tool'; summary: string; error?: boolean; id: string | null; name: string; target: string | null; at: string | null; ended_at: string | null; done: boolean }
| { kind: 'subagent'; id: string | null; name: string; agent_type: string | null; description: string | null; result: string | null; error: boolean; at: string | null; ended_at: string | null; done: boolean }
export interface ToolLine { summary: string; error: boolean; id: string | null; name: string; target: string | null; at: string | null; ended_at: string | null; done: boolean }
// ConvGroup gains | { kind: 'subagent'; … same fields … }
```

- [ ] **Step 1: Failing Rust tests** (add to the tests in `transcript.rs`, reusing the `jl`/`user`/`asst` helpers from phase 2):

```rust
    fn tool_use(ts: &str, id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"assistant","timestamp":ts,
            "message":{"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
    }
    fn tool_result(ts: &str, id: &str, content: serde_json::Value, err: bool) -> serde_json::Value {
        serde_json::json!({"type":"user","timestamp":ts,
            "message":{"content":[{"type":"tool_result","tool_use_id":id,"content":content,"is_error":err}]}})
    }

    #[test]
    fn a_tool_item_carries_id_name_target_times_and_done() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use("2026-09-18T09:00:20.100Z", "toolu_1", "Bash", serde_json::json!({"command":"cargo test -p fleet-core\nsecond line"})),
            tool_result("2026-09-18T09:00:41.900Z", "toolu_1", serde_json::json!("ok"), false),
            tool_use("2026-09-18T09:00:42Z", "toolu_2", "Edit", serde_json::json!({"file_path":"/w/src/a.rs","old_string":"a","new_string":"b"})),
        ]));
        match &t[0].items[0] {
            ConvItem::Tool { id, name, target, at, ended_at, done, error, .. } => {
                assert_eq!(id.as_deref(), Some("toolu_1"));
                assert_eq!(name, "Bash");
                assert_eq!(target.as_deref(), Some("cargo test -p fleet-core"));
                assert_eq!(at.as_deref(), Some("2026-09-18T09:00:20.100Z"));
                assert_eq!(ended_at.as_deref(), Some("2026-09-18T09:00:41.900Z"));
                assert!(*done && !*error);
            }
            other => panic!("{other:?}"),
        }
        match &t[0].items[1] {
            ConvItem::Tool { target, done, ended_at, .. } => {
                assert_eq!(target.as_deref(), Some("/w/src/a.rs"));
                assert!(!*done);
                assert_eq!(*ended_at, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn task_and_agent_calls_become_subagent_items_with_their_final_text() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use("2026-09-18T09:00:00Z", "toolu_3", "Task", serde_json::json!({"description":"Map the store","prompt":"p","subagent_type":"Explore"})),
            tool_result("2026-09-18T09:02:00Z", "toolu_3", serde_json::json!([{"type":"text","text":"Final report: all good"}]), false),
            tool_use("2026-09-18T09:03:00Z", "toolu_4", "Agent", serde_json::json!({"description":"Second"})),
        ]));
        assert_eq!(
            t[0].items[0],
            ConvItem::Subagent {
                id: Some("toolu_3".into()),
                name: "Task".into(),
                agent_type: Some("Explore".into()),
                description: Some("Map the store".into()),
                result: Some("Final report: all good".into()),
                error: false,
                at: Some("2026-09-18T09:00:00Z".into()),
                ended_at: Some("2026-09-18T09:02:00Z".into()),
                done: true,
            }
        );
        assert!(matches!(&t[0].items[1], ConvItem::Subagent { done: false, agent_type: None, .. }));
    }

    #[test]
    fn tool_target_prefers_what_the_tool_touched() {
        let j = |v: serde_json::Value| Some(v);
        assert_eq!(tool_target("Read", j(serde_json::json!({"file_path":"/a/b.rs"})).as_ref()).as_deref(), Some("/a/b.rs"));
        assert_eq!(tool_target("Grep", j(serde_json::json!({"pattern":"fn x","path":"src"})).as_ref()).as_deref(), Some("fn x"));
        assert_eq!(tool_target("WebFetch", j(serde_json::json!({"url":"https://x.y"})).as_ref()).as_deref(), Some("https://x.y"));
        assert_eq!(tool_target("Bash", j(serde_json::json!({"command":"a\nb"})).as_ref()).as_deref(), Some("a"));
        assert_eq!(tool_target("TodoWrite", j(serde_json::json!({"todos":[]})).as_ref()), None);
        let long = "x".repeat(300);
        assert_eq!(tool_target("Bash", j(serde_json::json!({"command": long})).as_ref()).unwrap().chars().count(), 121);
    }

    #[test]
    fn the_mcp_text_rendering_of_tools_is_unchanged() {
        let turns = parse_turns(&jl(&[
            user(serde_json::json!("go")),
            tool_use("2026-09-18T09:00:00Z", "toolu_1", "Bash", serde_json::json!({"command":"ls"})),
        ]));
        assert!(turns.join("\n").contains("[tool_use] Bash(command=ls)"));
    }
```

- [ ] **Step 2: Run to fail**: `cargo test -p fleet-core service::transcript` fails with compile errors.

- [ ] **Step 3: Implement**

```rust
/// Chars kept of a tool target (plus "…").
const TOOL_TARGET_MAX_CHARS: usize = 120;
/// Chars kept of a subagent's final text.
const SUBAGENT_RESULT_MAX_CHARS: usize = 1_500;
const SUBAGENT_TOOLS: [&str; 2] = ["Task", "Agent"];

/// What a tool call touched, for its compact line: a path, a pattern, a
/// URL, a query or the first line of a command. `None` when nothing
/// identifying is in the input.
pub fn tool_target(_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    let map = input?.as_object()?;
    for key in ["file_path", "notebook_path", "pattern", "url", "query", "command", "path", "skill", "description"] {
        if let Some(s) = map.get(key).and_then(|v| v.as_str()) {
            let first = s.lines().next().unwrap_or("").trim();
            if first.is_empty() {
                continue;
            }
            return Some(cap_chars(first, TOOL_TARGET_MAX_CHARS));
        }
    }
    None
}

/// The text of a tool_result's content (a string, or its text blocks
/// joined); images are skipped.
fn tool_result_text(block: &serde_json::Value) -> Option<String> {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}
```

(`cap_chars` from phase 2 appends `…`, which gives 121 chars for a capped target. The test pins that.)

Parser changes:
- The `tool_use` arm pushes `ConvItem::Subagent { … }` when the name is in `SUBAGENT_TOOLS`. Otherwise it pushes the structured `Tool`. It sets `at` to the entry's timestamp and records `tool_items[id] = index`.
- In the user-entry `tool_result` loop, run the existing `is_error` filter over every tool_result, not only the errored ones. For each result that matches a tool item:
  - set `done = true`, `ended_at = entry timestamp`, and `error |= is_error`;
  - for a `Subagent`, also set `result = tool_result_text(b).map(|t| cap_chars(t.trim(), SUBAGENT_RESULT_MAX_CHARS))`.
- Keep the existing error-flag semantics.

Other changes:
- `item_chars` counts `summary`/`target`, or `description`/`result` for subagents.
- `parse_turns` renders a `Subagent` as `format!("{TOOL_USE_PREFIX}{name}(description={})", one_line(description.unwrap_or_default()))`. That matches what `tool_summary` produced before, so the MCP text is unchanged. Add a test that pins it.

TS side:
- Mirror both variants.
- `groupItems` folds `tool` items into `tools` as today, with `ToolLine` carrying the new fields. A `subagent` becomes its own group and breaks a tool run.
- `toolName(summary)` stays and uses `line.name` when present.
- Update fixtures to include the new fields. Add a `tool()` factory in the tests to keep them short.

- [ ] **Step 4: Run** the full `cargo test -p fleet-core`, clippy, fmt, `cargo check -p claude-fleet`, `npx svelte-check`, `npx vitest run`, and confirm they all pass. `ConversationPanel.svelte` must render the `subagent` group as nothing yet: add an empty `{:else if g.kind === 'subagent'}` (Task 3 renders it).

- [ ] **Step 5: Commit** `feat(transcript): structured tool items with id/target/timing and subagent items`

---

### Task 2: Backend — `session_tool_detail`

**Files:** `crates/fleet-core/src/validate.rs`; `crates/fleet-core/src/service/transcript.rs`; `src-tauri/src/commands/sessions.rs`; `src-tauri/src/lib.rs`; `src/lib/conversation.ts`; `docs/control-api-reference.md` (regenerated).

**Interfaces — produces:**

```rust
pub fn tool_use_id(value: &str) -> Result<(), IpcError>            // validate.rs, E_INVALID
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ToolDetail {
    pub id: String,
    pub name: String,
    /// Pretty JSON of the input, ≤ 8 000 chars ("…" when cut).
    pub input: String,
    /// Edit / MultiEdit / Write: the file path and the before/after text,
    /// each ≤ 8 000 chars; None for other tools.
    pub edit: Option<EditDetail>,
    /// Bash: the full command (≤ 8 000 chars); None otherwise.
    pub command: Option<String>,
    /// Result text (string or joined text blocks), ≤ 8 000 chars; None until it arrives.
    pub result: Option<String>,
    pub is_error: bool,
}
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EditDetail { pub file_path: String, pub old: String, pub new: String }
pub fn parse_tool_detail(lines: &str, id: &str) -> Option<ToolDetail>
pub async fn fetch_tool_detail(store: &Mutex<Store>, ssh: &Arc<SshClient>, row: &SessionRow, claude_session_id: Option<&str>, tool_use_id: &str) -> Result<ToolDetail, IpcError>
```

TS: `export interface ToolDetail {…}` and `export function toolDetail(sessionId: number, toolUseId: string, claudeSessionId?: string): Promise<Result<ToolDetail>>`, which calls `invokeCmd('session_tool_detail', { args: { session_id, tool_use_id, claude_session_id } })`.

- [ ] **Step 1: Failing tests**:

```rust
    #[test]
    fn tool_use_id_validation() {
        assert!(crate::validate::tool_use_id("toolu_01AbC-9").is_ok());
        for bad in ["", "a b", "x;rm", &"a".repeat(101), "toolu_$x"] {
            assert!(crate::validate::tool_use_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn detail_of_an_edit_with_its_result() {
        let lines = jl(&[
            tool_use("t", "toolu_2", "Edit", serde_json::json!({"file_path":"/w/a.rs","old_string":"let a = 1;","new_string":"let a = 2;"})),
            tool_result("t", "toolu_2", serde_json::json!("The file /w/a.rs has been updated."), false),
        ]);
        let d = parse_tool_detail(&lines, "toolu_2").unwrap();
        assert_eq!(d.name, "Edit");
        assert_eq!(d.edit, Some(EditDetail { file_path: "/w/a.rs".into(), old: "let a = 1;".into(), new: "let a = 2;".into() }));
        assert_eq!(d.result.as_deref(), Some("The file /w/a.rs has been updated."));
        assert!(!d.is_error);
        assert_eq!(d.command, None);
    }

    #[test]
    fn detail_of_a_failed_bash_and_of_a_pending_call() {
        let lines = jl(&[
            tool_use("t", "toolu_1", "Bash", serde_json::json!({"command":"cargo test"})),
            tool_result("t", "toolu_1", serde_json::json!([{"type":"text","text":"error: 2 failed"}]), true),
            tool_use("t", "toolu_9", "Read", serde_json::json!({"file_path":"/x"})),
        ]);
        let d = parse_tool_detail(&lines, "toolu_1").unwrap();
        assert_eq!(d.command.as_deref(), Some("cargo test"));
        assert_eq!(d.result.as_deref(), Some("error: 2 failed"));
        assert!(d.is_error);
        let p = parse_tool_detail(&lines, "toolu_9").unwrap();
        assert_eq!(p.result, None);
        assert!(p.input.contains("\"file_path\""));
        assert_eq!(parse_tool_detail(&lines, "toolu_missing"), None);
    }

    #[test]
    fn detail_text_is_capped() {
        let big = "y".repeat(20_000);
        let lines = jl(&[
            tool_use("t", "toolu_5", "Bash", serde_json::json!({"command": big.clone()})),
            tool_result("t", "toolu_5", serde_json::json!(big), false),
        ]);
        let d = parse_tool_detail(&lines, "toolu_5").unwrap();
        assert_eq!(d.command.unwrap().chars().count(), 8_001);
        assert_eq!(d.result.unwrap().chars().count(), 8_001);
        assert!(d.input.chars().count() <= 8_001);
    }

    #[test]
    fn the_tool_lines_script_quotes_the_id_and_greps_fixed_strings() {
        let s = tool_lines_script(None, Some("/h/.claude/projects/x/a.jsonl"), None, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "toolu_1", 262_144);
        assert!(s.contains("grep -F -- 'toolu_1'"));
        assert!(s.contains("head -c 262144"));
    }
```

(If `jl`, `tool_use` and `tool_result` are not visible from the test module these tests live in, put them in the same test module as Task 1's helpers.)

- [ ] **Step 2: Implement**
  - Split `read_script` into `fn locate_script(tmux_name, stored_path, fallback_dir, claude_session_id) -> String`, which ends with `$f` set or `exit 4` and prints `NO_TRANSCRIPT`. Then:
    - `read_script` becomes `locate_script(..) + "tail -c {max_bytes} \"$f\"\n"`;
    - `pub fn tool_lines_script(.., tool_use_id, max_bytes)` becomes `locate_script(..) + format!("grep -F -- {} \"$f\" | head -c {max_bytes}\n", quote(tool_use_id))`.
    - `grep` exits 1 on no match. Append `|| true` inside the pipeline so a missing id is not a shell failure: `{ grep -F -- ID "$f" || true; } | head -c N`.
    - All existing `read_script` tests must still pass unchanged.
  - `parse_tool_detail(lines, id)`:
    - Scan the JSONL lines. For the assistant `tool_use` block with `id`, take `name` and `input`.
    - For the user `tool_result` with `tool_use_id == id`, take the text (via `tool_result_text`) and `is_error`.
    - Return `None` when no `tool_use` is found.
    - `input` is `serde_json::to_string_pretty(input)` capped with `cap_chars(…, 8_000)`.
    - `edit` is set for `Edit`/`MultiEdit`/`Write`. For `MultiEdit`, `old`/`new` join the edits' strings with `\n…\n`. For `Write`, `old` is `""` and `new` is `content`.
    - `command` is set for `Bash`.
  - `fetch_tool_detail`:
    - validate the id;
    - resolve args (`resolve_args_for` when `claude_session_id` is given, else `resolve_args`);
    - build `tool_lines_script` with `max_bytes = 2 * 1_048_576`;
    - run it with the existing `run_shell` and map errors exactly like `read_tail` (reuse `read_tail` with the script);
    - `parse_tool_detail`, returning `E_NOTFOUND` ("tool call not in transcript") on `None`.
  - Tauri command `session_tool_detail(args: { session_id: i64, tool_use_id: String, claude_session_id: Option<String> })`: look up the row like `session_conversation` does, call `fetch_tool_detail`, and register it next to `session_conversation`. No MCP tool (YAGNI).
  - Regenerate the reference.
  - TS: `toolDetail()` plus a wire test in `conversation.test.ts` that mocks `invoke`, the same way the `listConversations` test does.

- [ ] **Step 3: Run** the Rust suite, clippy, fmt, `cargo check -p claude-fleet`, `reference_is_current`, svelte-check and vitest. **Commit** `feat(transcript): session_tool_detail — lazy input/result for one tool call`

---

### Task 3: Frontend — tool lines, subagent blocks, "doing now"

**Files:** create `src/lib/ToolLine.svelte`, `src/lib/ToolLine.test.ts`, `src/lib/SubagentBlock.svelte`, `src/lib/SubagentBlock.test.ts`; modify `src/lib/conversation.ts`, `src/lib/conversation.test.ts`, `src/lib/ConversationPanel.svelte`, `src/lib/ConversationPanel.test.ts`.

**Interfaces — produces (conversation.ts):**

```ts
export function toolVerb(name: string): string;          // Read→"Read", Edit/MultiEdit→"Edit", Write→"Write", Bash→"Run", Grep→"Search", Glob→"Find", WebFetch→"Fetch", WebSearch→"Search web", TodoWrite→"Update todos", mcp__srv__tool→"srv · tool", other→name
export function shortTarget(target: string | null, cwdHint?: string | null): string | null; // paths: last 2 segments with "…/" prefix when longer; others unchanged
export function formatDuration(ms: number): string;      // <1000 → "0.4s"; <60 000 → "12s"; else "3m 05s"
export function toolDurationMs(at: string | null, endedAt: string | null, nowMs: number | null): number | null; // running: nowMs - at when nowMs given
export interface DoingNow { label: string; sinceMs: number | null }
export function doingNow(conv: Conversation | null, working: boolean, nowMs: number): DoingNow | null; // last turn's last not-done tool or subagent while working
export interface DiffLine { kind: 'del' | 'add' | 'ctx'; text: string }
export function editDiffLines(old: string, next: string): DiffLine[]; // common prefix/suffix lines as ctx (max 2 each side), middle as del then add
```

`ToolLine.svelte` props: `{ line: ToolLine; sessionId: number; claudeSessionId: string | null; nowMs: number }`.
- The collapsed row is a `button` (`data-testid="conv-tool"`, `data-error`, `aria-expanded`) that shows the verb, the short target (`title` = full summary), and the duration (`formatDuration`, or "running Ns" when not done). A failed call gets a red ✕.
- The first expand calls `toolDetail(sessionId, line.id, claudeSessionId ?? undefined)` once and caches it in component state.
- The expanded body (`data-testid="conv-tool-detail"`) shows one of:
  - `edit`: the file path, then the diff lines (`<pre class="diff">` with `.del` / `.add` / `.ctx` spans, first 200 lines with "N more lines");
  - `command`: `<pre class="cmd">$ {command}</pre>`;
  - otherwise the input JSON in `<pre>`.
  - Then the result `<pre class="result">` (`data-testid="conv-tool-result"`), clamped to 20 lines with "Show all". A failed call gets `data-error`.
- If `line.id` is null the row is not expandable (no button, plain div).
- A fetch error shows `data-testid="conv-tool-detail-error"` with the message and a Retry button.

`SubagentBlock.svelte` props: `{ item: Extract<ConvGroup, { kind: 'subagent' }>; nowMs: number }`.
- A bordered block (`data-testid="conv-subagent"`) with a header: agent type (or "subagent") · description · duration or "running…". It also has a red marker on error.
- The body holds the `result` rendered with `Markdown`, collapsed to 6 lines by default with "Show more".

Panel:
- Replace the single-tool and multi-tool line markup with `<ToolLine>`. Keep the `<details class="tools" data-testid="conv-tools">` folding for groups of 2 or more, with the summary from `toolGroupLabel`.
- Render `subagent` groups with `<SubagentBlock>`.
- **Doing now.** While the session is `working` and the viewed conversation is current, the existing activity indicator's label becomes `doingNow(conv, true, nowMs)?.label` + ` · ` + elapsed when available. Otherwise it keeps the current spinner label. The panel's 30 s `nowMs` tick is too coarse for a running timer: while a `doingNow` exists, tick `nowMs` every 1 s (clear the interval when it disappears).

- [ ] **Step 1: Failing unit tests** in `conversation.test.ts`:

```ts
describe('tool helpers', () => {
  it('verbs', () => {
    expect(toolVerb('Bash')).toBe('Run');
    expect(toolVerb('MultiEdit')).toBe('Edit');
    expect(toolVerb('mcp__claude-fleet__list_sessions')).toBe('claude-fleet · list_sessions');
    expect(toolVerb('Whatever')).toBe('Whatever');
  });
  it('short targets', () => {
    expect(shortTarget('/Users/m/p/claude-fleet/crates/fleet-core/src/store/reconcile.rs')).toBe('…/store/reconcile.rs');
    expect(shortTarget('src/a.rs')).toBe('src/a.rs');
    expect(shortTarget('cargo test -p fleet-core')).toBe('cargo test -p fleet-core');
    expect(shortTarget(null)).toBeNull();
  });
  it('durations', () => {
    expect(formatDuration(400)).toBe('0.4s');
    expect(formatDuration(12_300)).toBe('12s');
    expect(formatDuration(185_000)).toBe('3m 05s');
    expect(toolDurationMs('2026-09-18T09:00:00Z', '2026-09-18T09:00:12Z', null)).toBe(12_000);
    expect(toolDurationMs('2026-09-18T09:00:00Z', null, Date.parse('2026-09-18T09:00:05Z'))).toBe(5_000);
    expect(toolDurationMs(null, null, 1)).toBeNull();
  });
  it('doing now is the last unfinished tool of the last turn while working', () => {
    const t = (o: object) => ({ kind: 'tool', summary: 'Bash(x)', error: false, id: 'i', name: 'Bash', target: 'cargo test', at: '2026-09-18T09:00:00Z', ended_at: null, done: false, ...o });
    const c = { turns: [{ prompt: 'p', at: null, ended_at: null, items: [t({ done: true, name: 'Read', target: '/a' }), t({})] }], truncated: false, context: null, events: [] } as Conversation;
    expect(doingNow(c, true, Date.parse('2026-09-18T09:00:07Z'))).toEqual({ label: 'Run cargo test', sinceMs: 7_000 });
    expect(doingNow(c, false, 0)).toBeNull();
  });
  it('edit diff keeps shared context and marks changes', () => {
    expect(editDiffLines('a\nb\nc', 'a\nB\nc')).toEqual([
      { kind: 'ctx', text: 'a' }, { kind: 'del', text: 'b' }, { kind: 'add', text: 'B' }, { kind: 'ctx', text: 'c' },
    ]);
    expect(editDiffLines('', 'new')).toEqual([{ kind: 'add', text: 'new' }]);
  });
});
```

- [ ] **Step 2: Failing component tests.** Write these tests in full, following the `ConversationHeader.test.ts` style and mocking `toolDetail` via `vi.mock('./conversation', …importActual…)`.
  - `ToolLine.test.ts`:
    - "a finished call shows verb, short target and duration" (`Run`, `cargo test`, `12s`);
    - "a failed call is marked" (`data-error="true"`);
    - "expanding fetches the detail once and renders an edit diff" (click twice, then `toolDetail` is called once; `.del` contains the old line and `.add` the new line);
    - "a bash detail shows the command and the result" (`$ cargo test`, `conv-tool-result` text);
    - "a fetch error shows retry, and retry refetches";
    - "a call without an id is not expandable" (no button role).
  - `SubagentBlock.test.ts`:
    - "shows type, description and duration" (`Explore`, `Map the store`, `2m 00s`);
    - "running shows running…";
    - "error is marked";
    - "the result renders as markdown, clamped with Show more".
  - `ConversationPanel.test.ts`:
    - "tool groups render ToolLine rows" (existing `conv-tool` / `conv-tools` assertions still pass; update any expectation that read the raw summary text to the verb/target form);
    - "a subagent renders as a block";
    - "the indicator shows what is running" (session `claude_status: 'working'`, last tool not done → indicator text contains `Run cargo test`).

- [ ] **Step 3: Implement**, **Step 4: run** `npx vitest run`, `npx svelte-check`. **Step 5: Commit** `feat(conversation-ui): compact tool lines with lazy detail, subagent blocks, doing-now indicator`

---

### Task 4: Frontend — find, copy, turn index

**Files:** create `src/lib/conversation_nav.ts`, `src/lib/conversation_nav.test.ts`; modify `src/lib/ConversationPanel.svelte`, `src/lib/ConversationPanel.test.ts`.

**Interfaces — produces:**

```ts
// conversation_nav.ts
export interface Match { rowKey: string }
/** Row keys (the panel's `{#each thread}` keys: `t<index>` / `e<id>`) whose
 *  searchable text contains `query` (case-insensitive, trimmed; empty → []).
 *  Searchable text: prompt, text items, tool summaries/targets, command
 *  name/args/output, subagent description/result, compaction summary, event
 *  label/detail. In document order. */
export function findMatches(rows: ThreadRow[], query: string): Match[];
export interface TurnIndexEntry { rowKey: string; label: string; at: string | null }
/** One entry per turn that has a prompt or a command: first line of the
 *  prompt (≤ 80 chars, "…"), or the command text ("/model opus"). */
export function turnIndex(rows: ThreadRow[]): TurnIndexEntry[];
```

Panel behaviour:
- **Find.** Cmd/Ctrl+F while focus is inside `.conversation-panel` (listen on the panel root, `preventDefault`) opens a find bar at the top of the thread (`data-testid="conv-find"`: input `conv-find-input`, counter `conv-find-count` "2 / 7", prev/next buttons `conv-find-prev`/`conv-find-next`, close `conv-find-close`).
  - Typing updates the matches (`findMatches(thread, q)`). Enter goes to next, Shift+Enter to previous, Escape closes.
  - The current match's row gets `data-current-match` and is scrolled into view (`scrollIntoView({ block: 'center' })`). Every matching row gets `data-match`.
  - Highlight the query inside text with the CSS Custom Highlight API when `CSS.highlights` exists. Otherwise the row outline (`[data-match]`, `[data-current-match]` styles) is the only highlight. Feature-detect, and never throw when it is missing (jsdom has none).
  - Closing clears the marks. Wrap each thread row in an element carrying `data-row-key`.
- **Copy.** A copy button appears on hover or focus (`data-testid="conv-copy"`, `aria-label="Copy"`):
  - on each prompt block (copies the prompt);
  - on each text group (copies the markdown source);
  - inside an expanded tool detail's result (copies the result; add a `ToolLine` prop `onCopy?` or import `copyText` directly).
  - It uses `copyText` from `clipboard.ts`. After success it shows "Copied" for 1.5 s.
- **Turn index.** A small button next to the find icon in the thread toolbar (`data-testid="conv-turns-button"`, text "N turns") opens a popover list (`data-testid="conv-turn-index"`, items `conv-turn-index-item`) from `turnIndex(thread)`. Picking an item scrolls that row into view and closes the list. Escape and an outside pointerdown also close it.
- The thread toolbar is a slim row at the top of the scroller holding the find and turns buttons. When the find bar is open, the find bar replaces the toolbar.

- [ ] **Step 1: Failing tests.**
  - `conversation_nav.test.ts` (write in full):
    - `findMatches` finds case-insensitively across a prompt, a text item, a tool target and an event label, and returns keys in order;
    - empty or whitespace query → `[]`;
    - `turnIndex` gives prompt first lines truncated at 80 with `…`, includes command rows as `/model opus`, and skips prompt-less assistant-only turns.
  - `ConversationPanel.test.ts`:
    - "Ctrl+F opens find; typing marks matches; Enter moves to the next; Escape closes" (stub `Element.prototype.scrollIntoView = vi.fn()` in the test);
    - "copy on a prompt calls copyText with the prompt" (mock `./clipboard`'s `copyText` via `vi.mock` + importActual);
    - "the turn index lists prompts and jumps" (`scrollIntoView` called on the picked row).

- [ ] **Step 2: Implement**, **Step 3: run** vitest and svelte-check. **Step 4: Commit** `feat(conversation-ui): find in conversation, copy buttons and turn index`

---

### Task 5: Docs and verification

- [ ] Update the spec: under Phase 3 add "Status: implemented", note that tool detail is fetched lazily through `session_tool_detail` (not in the poll payload), and that find uses the CSS Custom Highlight API when available.
- [ ] Add a line to `docs/control-api.md` only if it lists Tauri-only commands. Check first, and leave it alone otherwise.
- [ ] Run `scripts/ci-local.sh` unpiped. All stages must be green.
- [ ] The manual UI check is done by the user. **Do not launch a dev build**: it SIGTERMs the installed app (see memory "Dev build kills installed app").
- [ ] Commit `docs: phase 3 status`.

---

## Self-Review Notes

- **Spec coverage:**
  - §3.1 compact line (verb, target, duration, error), grouping kept, expand to input/diff/command/result, subagent block: Tasks 1–3.
  - §3.2 doing now: Task 3.
  - §3.3 jump-to-latest and "N new" pill already exist; find, copy and turn index: Task 4.
- **Payload:** only small fields ride the poll (id, name, target ≤ 121 chars, two timestamps, done). Detail is lazy.
- **Old hub:** a hub without `session_tool_detail` returns an unknown-command error. `ToolLine` shows it as a detail error with Retry, and the line itself still renders.
- **Subagent nested tool calls stay out:** sidechain entries remain skipped, as the spec says.
