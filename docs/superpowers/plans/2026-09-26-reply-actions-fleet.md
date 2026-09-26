# Reply Actions (claude-fleet) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put Copy, Quote, Retry, Fork here and Rewind here under every reply in the desktop Conversation view, on one backend engine that truncates a transcript into a new conversation id.

**Architecture:** One anchor field on the wire (`ConvTurn.prompt_uuid`). One host-side script that copies the head of a transcript into a new `<uuid>.jsonl`, rewriting `sessionId`. One service function with two modes: `rewind` rebinds this session to the new id and restarts its pane; `fork` spawns a new session on it. Retry is `rewind` + `send_prompt`, so it has no code path of its own.

**Tech Stack:** Rust (fleet-core, src-tauri, rmcp), Svelte 5 runes, Vitest, `awk`/`bash` over SSH.

**Spec:** `docs/superpowers/specs/2026-09-26-reply-actions-design.md`

## Global Constraints

- **Shell quoting has one implementation.** Every value interpolated into an SSH/bash command string goes through `crate::shell::quote` (alias `shq`). Never reintroduce `shell_quote` / `shell_escape`.
- **Never hold the `Store` mutex across an `.await`.** Take a snapshot under a short lock, release, then do I/O.
- **New `ConvTurn` field is `Option<String>` + `#[serde(default)]`.** An older hub sends nothing and must still deserialise.
- **Rewind confirmation copy, verbatim:** "The conversation is rewound to before this turn. **Your files are left as they are.**"
- **Adding a command means:** a row in `src-tauri/src/backend/verdicts.rs`, `route` **by command name** (never a second tool literal), then `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`, then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- **The MCP definition budget is guarded.** `BUDGET_BYTES` in `the_served_definition_budget_stays_bounded` (`crates/fleet-core/src/mcp/tools/tests.rs:3291`) is currently `71_658`. Trim first; raise only with the measured before/after numbers written into its doc comment, as every previous raise there does.
- **Verify commands:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, `pnpm test`, `pnpm check`. `scripts/ci-local.sh` runs all of it in CI order.
- **Caveat:** `cargo` needs the Tauri system libs (dbus, gtk/atk, pkg-config). On a box without them the failure is in a build script — an environment gap, not your code.

---

### Task 1: The anchor on the wire

Adds `prompt_uuid` to `ConvTurn` in Rust and TypeScript, set by the parser at the one site that opens a turn with a prompt.

**Files:**
- Modify: `crates/fleet-core/src/service/transcript.rs:330-351` (the `ConvTurn` struct), `:1120-1127` (the one prompted construction), and the seven other `ConvTurn { … }` sites at `:870`, `:1003`, `:1026`, `:1063`, `:1084`, `:1101`, `:1132`
- Modify: `src/lib/conversation.ts:51-60` (the TS `ConvTurn`)
- Test: `crates/fleet-core/src/service/transcript.rs` (the existing `mod tests` at the bottom of the same file — this repo keeps transcript tests inline)

**Interfaces:**
- Consumes: nothing.
- Produces: `ConvTurn.prompt_uuid: Option<String>` (Rust) / `prompt_uuid?: string | null` (TS). Task 3 takes this value as its `anchor_uuid`; Task 6 reads it to decide which buttons to draw.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/fleet-core/src/service/transcript.rs`. The helpers `jl` and `user` already exist there (see `the_mcp_text_rendering_of_tools_is_unchanged` for their use); these tests build raw JSONL directly because they need the `uuid` field, which `user()` does not set.

```rust
#[test]
fn a_prompted_turn_carries_the_uuid_of_its_prompt_entry() {
    let jsonl = concat!(
        r#"{"type":"user","uuid":"aaaaaaaa-0000-0000-0000-000000000001","sessionId":"s","timestamp":"2026-09-26T09:00:00Z","message":{"role":"user","content":"first"}}"#,
        "\n",
        r#"{"type":"assistant","uuid":"bbbbbbbb-0000-0000-0000-000000000001","sessionId":"s","timestamp":"2026-09-26T09:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#,
        "\n",
        r#"{"type":"user","uuid":"aaaaaaaa-0000-0000-0000-000000000002","sessionId":"s","timestamp":"2026-09-26T09:01:00Z","message":{"role":"user","content":"second"}}"#,
        "\n",
    );
    let turns = parse_conversation(jsonl);
    assert_eq!(turns.len(), 2, "two prompts open two turns");
    assert_eq!(
        turns[0].prompt_uuid.as_deref(),
        Some("aaaaaaaa-0000-0000-0000-000000000001"),
        "the anchor is the uuid of the entry that OPENED the turn, not of its reply"
    );
    assert_eq!(
        turns[1].prompt_uuid.as_deref(),
        Some("aaaaaaaa-0000-0000-0000-000000000002")
    );
}

#[test]
fn a_turn_with_no_prompt_has_no_anchor() {
    // A compact boundary opens a turn with `prompt: None`. There is no prompt
    // to rewind to, so there must be no anchor either — the clients key Rewind
    // and Retry off exactly this being `None`.
    let jsonl = concat!(
        r#"{"type":"system","subtype":"compact_boundary","uuid":"cccccccc-0000-0000-0000-000000000001","sessionId":"s","timestamp":"2026-09-26T09:00:00Z","compactMetadata":{"trigger":"auto","preTokens":1000}}"#,
        "\n",
    );
    let turns = parse_conversation(jsonl);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].prompt_uuid, None);
}

#[test]
fn a_prompt_entry_with_no_uuid_field_leaves_the_anchor_none() {
    // Defensive: the field is `Option` precisely so a transcript shape without
    // it degrades to "no anchor" instead of panicking or inventing one.
    let jsonl = concat!(
        r#"{"type":"user","sessionId":"s","timestamp":"2026-09-26T09:00:00Z","message":{"role":"user","content":"hi"}}"#,
        "\n",
    );
    let turns = parse_conversation(jsonl);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].prompt_uuid, None);
}

#[test]
fn an_older_hub_sending_no_prompt_uuid_still_deserialises() {
    let wire = r#"{"prompt":"hi","at":null,"ended_at":null,"items":[],"reminders":[]}"#;
    let turn: ConvTurn = serde_json::from_str(wire).expect("must decode without prompt_uuid");
    assert_eq!(turn.prompt_uuid, None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib prompt_uuid -- --nocapture
cargo test -p fleet-core --lib a_turn_with_no_prompt_has_no_anchor
```

Expected: FAIL to **compile** — `no field prompt_uuid on type ConvTurn`.

- [ ] **Step 3: Add the field to the Rust struct**

In `crates/fleet-core/src/service/transcript.rs`, inside `pub struct ConvTurn`, after the `reminders` field:

```rust
    /// The JSONL `uuid` of the entry that OPENED this turn — the anchor every
    /// truncation is expressed against ("keep strictly before this prompt").
    ///
    /// `None` for a turn no prompt opened: a compact boundary, a
    /// notification-only turn, or assistant output whose prompt lies before
    /// the read tail. Those turns offer no Rewind and no Retry, which is
    /// right on its own terms — there is no prompt to put back in the
    /// composer.
    ///
    /// `serde(default)` because a client may read a hub that predates it.
    #[serde(default)]
    pub prompt_uuid: Option<String>,
```

- [ ] **Step 4: Set it at the one site that opens a prompted turn**

At `crates/fleet-core/src/service/transcript.rs:1120`, the only `ConvTurn` construction with a prompt. Add the last field:

```rust
                    current = Some(ConvTurn {
                        // An image-only prompt still opens a turn, unquoted.
                        prompt: (!prompt.is_empty()).then_some(prompt),
                        at: at(),
                        ended_at: None,
                        items: Vec::new(),
                        reminders: std::mem::take(&mut pending_reminders),
                        prompt_uuid: v.get("uuid").and_then(|u| u.as_str()).map(String::from),
                    });
```

Then add `prompt_uuid: None,` to the other seven `ConvTurn { … }` constructions in this file (`:870`, `:1003`, `:1026`, `:1063`, `:1084`, `:1101`, `:1132`). The compiler names every one you miss.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib transcript
```

Expected: PASS, including every pre-existing transcript test.

- [ ] **Step 6: Mirror the field in TypeScript**

In `src/lib/conversation.ts`, inside `export interface ConvTurn`, after `reminders`:

```ts
  /** The JSONL uuid of the entry that opened this turn — the truncation
   *  anchor for Rewind/Retry (this turn's own) and Fork (the next later
   *  turn's). `null`/absent for a turn no prompt opened, and for a hub that
   *  predates the field. */
  prompt_uuid?: string | null;
```

- [ ] **Step 7: Verify the frontend still type-checks**

```bash
pnpm check && pnpm test
```

Expected: PASS. The field is optional, so no existing construction breaks.

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-core/src/service/transcript.rs src/lib/conversation.ts
git commit -m "feat(transcript): carry each turn's prompt uuid as a truncation anchor

Every truncation Fork/Rewind/Retry need is \"keep strictly before some
prompt\", so one optional field per turn is enough. Set at the single parser
site that opens a turn with a prompt; None everywhere else, which is what
makes Rewind absent on a compact boundary.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 2: The truncation script

A pure function that builds the host-side script, plus the literal-replace `awk` that does the copying. No network, no store.

**Files:**
- Create: `crates/fleet-core/src/service/rewind.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (add `pub mod rewind;`)
- Modify: `crates/fleet-core/src/service/transcript.rs:131` (`fn locate_script` → `pub(crate) fn locate_script`)
- Test: inline `mod tests` in `crates/fleet-core/src/service/rewind.rs`

**Interfaces:**
- Consumes: `transcript::locate_script(tmux_name, stored_path, fallback_dir, claude_session_id) -> String` (made `pub(crate)` here).
- Produces:
  ```rust
  pub const NO_ANCHOR: &str = "__CF_NO_ANCHOR__";
  pub fn rewind_script(
      tmux_name: Option<&str>,
      stored_path: Option<&str>,
      fallback_dir: Option<&str>,
      claude_session_id: &str,
      new_id: &str,
      anchor_uuid: Option<&str>,
      dest_dir: Option<&str>,
      cwd_rewrite: Option<(&str, &str)>,
  ) -> String;
  ```
  Exit codes: `0` wrote the file (its path is the only thing on stdout); `4` no transcript (the sentinel `locate_script` already prints); `3` the anchor was not in the file, printing `NO_ANCHOR` on stderr.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleet-core/src/service/rewind.rs` with **only** this test module and the `use` line, so the test names exist before the implementation:

```rust
//! Truncating a transcript into a new conversation id: the shared engine
//! behind Fork here, Rewind here and Retry.

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a generated script against a real file in a temp dir. `bash` in a
    /// test is established here (`crate::shell::tests`, `crate::tmux::tests`).
    fn run(script: &str) -> std::process::Output {
        std::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .output()
            .expect("bash must be available")
    }

    const OLD: &str = "11111111-1111-1111-1111-111111111111";
    const NEW: &str = "22222222-2222-2222-2222-222222222222";
    const A1: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const A2: &str = "aaaaaaaa-0000-0000-0000-000000000002";

    fn fixture(dir: &std::path::Path) -> std::path::PathBuf {
        // Leading metadata lines carry no uuid and MUST survive: they are the
        // transcript's header (`custom-title`, `mode`, …).
        let body = format!(
            concat!(
                r#"{{"type":"mode","sessionId":"{old}"}}"#, "\n",
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","cwd":"/src/app","message":{{"role":"user","content":"one"}}}}"#, "\n",
                r#"{{"type":"assistant","uuid":"bbbb","sessionId":"{old}","cwd":"/src/app","message":{{"role":"assistant","content":[]}}}}"#, "\n",
                r#"{{"type":"user","uuid":"{a2}","sessionId":"{old}","cwd":"/src/app","message":{{"role":"user","content":"two"}}}}"#, "\n",
                r#"{{"type":"assistant","uuid":"cccc","sessionId":"{old}","cwd":"/src/app","message":{{"role":"assistant","content":[]}}}}"#, "\n",
            ),
            old = OLD, a1 = A1, a2 = A2
        );
        let p = dir.join(format!("{OLD}.jsonl"));
        std::fs::write(&p, body).unwrap();
        p
    }

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cf-rewind-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keeps_everything_strictly_before_the_anchor() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None, Some(src.to_str().unwrap()), None, OLD, NEW, Some(A2), None, None,
        ));
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert_eq!(written.lines().count(), 3, "header + turn 1's two entries, and NOT the anchor line");
        assert!(written.contains(A1), "the turn before the anchor is kept");
        assert!(!written.contains(A2), "the anchor line itself is dropped — 'strictly before'");
        assert!(!written.contains("cccc"), "nothing after the anchor survives");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rewrites_the_session_id_on_every_copied_line() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None, Some(src.to_str().unwrap()), None, OLD, NEW, Some(A2), None, None,
        ));
        assert!(out.status.success());
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(!written.contains(OLD), "no copied line may still name the old conversation");
        assert_eq!(written.lines().filter(|l| l.contains(NEW)).count(), 3);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn no_anchor_copies_the_whole_file() {
        // Fork on the LAST turn: there is no later prompt, so "keep
        // everything" is the correct meaning, not an error.
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None, Some(src.to_str().unwrap()), None, OLD, NEW, None, None, None,
        ));
        assert!(out.status.success());
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert_eq!(written.lines().count(), 5);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_anchor_that_is_not_in_the_file_fails_with_the_sentinel() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None, Some(src.to_str().unwrap()), None, OLD, NEW,
            Some("dddddddd-0000-0000-0000-000000000009"), None, None,
        ));
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_ANCHOR));
        assert!(!d.join(format!("{NEW}.jsonl")).exists(), "a failed truncation leaves no half-written transcript");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_cross_worktree_fork_rewrites_cwd_literally() {
        // `/src/app` contains no regex metacharacters, but a real path does
        // (`.`, `+`), so the replacement must be literal. This asserts the
        // behaviour; `rep()` in the script is what provides it.
        let d = tmp();
        let src = fixture(&d);
        let dest = d.join("elsewhere");
        let out = run(&rewind_script(
            None, Some(src.to_str().unwrap()), None, OLD, NEW, Some(A2),
            Some(dest.to_str().unwrap()), Some(("/src/app", "/src/app-fork")),
        ));
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let written = std::fs::read_to_string(dest.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(written.contains(r#""cwd":"/src/app-fork""#));
        assert!(!written.contains(r#""cwd":"/src/app""#));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn every_interpolated_value_is_shell_quoted() {
        let s = rewind_script(
            None, Some("/tmp/a b/t.jsonl"), None, OLD, NEW, Some(A1), None, None,
        );
        assert!(s.contains("'/tmp/a b/t.jsonl'"), "paths with spaces must arrive quoted: {s}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib service::rewind
```

Expected: FAIL to compile — `cannot find function rewind_script`.

- [ ] **Step 3: Make `locate_script` reusable**

In `crates/fleet-core/src/service/transcript.rs:131`, change the signature only:

```rust
pub(crate) fn locate_script(
```

- [ ] **Step 4: Write the implementation**

Prepend to `crates/fleet-core/src/service/rewind.rs`, above the test module:

```rust
use crate::shell::quote;

/// Sentinel the script prints (on stderr) when the anchor uuid is not in the
/// transcript — so the caller maps it to a code instead of parsing prose.
pub const NO_ANCHOR: &str = "__CF_NO_ANCHOR__";

/// The bash script that copies the head of a transcript into a new
/// conversation's `.jsonl`.
///
/// Locating the source file is [`crate::service::transcript::locate_script`]'s
/// job, unchanged — stored path, then the pane's cwd, then a glob over
/// `~/.claude/projects/*/<id>.jsonl` — and a missing transcript still exits 4
/// with its own sentinel.
///
/// What this adds is one `awk` pass:
///
/// - lines are copied from the start and stop **before** the line whose
///   `uuid` is `anchor_uuid`; `None` copies the whole file, which is what
///   forking the newest turn means;
/// - `sessionId` is rewritten to `new_id` on every copied line;
/// - `cwd` is rewritten when `cwd_rewrite` is set (a fork into a different
///   worktree, whose pane must start where the transcript says it did);
/// - the result lands in `dest_dir` (else beside the source), named
///   `<new_id>.jsonl`, and its path is the only thing on stdout.
///
/// Both replacements go through the script's own `rep()`, which is
/// `index`-based and therefore LITERAL. `gsub` would treat the search text as
/// a regex, and a cwd is full of `.` and `+`.
///
/// Copying a prefix rather than filtering by turn is deliberate: it keeps the
/// header entries that carry no uuid (`custom-title`, `mode`, `agent-name`,
/// `bridge-session`) and leaves the `parentUuid` chain intact, because a
/// prefix of a chain is still a chain.
///
/// The output is written to a temp file and moved into place only on success,
/// so a missing anchor leaves no half-written transcript for `--resume` to
/// find.
#[allow(clippy::too_many_arguments)]
pub fn rewind_script(
    tmux_name: Option<&str>,
    stored_path: Option<&str>,
    fallback_dir: Option<&str>,
    claude_session_id: &str,
    new_id: &str,
    anchor_uuid: Option<&str>,
    dest_dir: Option<&str>,
    cwd_rewrite: Option<(&str, &str)>,
) -> String {
    let mut s = crate::service::transcript::locate_script(
        tmux_name,
        stored_path,
        fallback_dir,
        claude_session_id,
    );
    let new_q = quote(new_id);
    let anchor_q = quote(anchor_uuid.unwrap_or(""));
    let destdir_q = quote(dest_dir.unwrap_or(""));
    let (oldcwd, newcwd) = cwd_rewrite.unwrap_or(("", ""));
    let oldcwd_q = quote(oldcwd);
    let newcwd_q = quote(newcwd);
    s.push_str(&format!(
        r#"newid={new_q}
anchor={anchor_q}
destdir={destdir_q}
oldcwd={oldcwd_q}
newcwd={newcwd_q}
if [ -z "$destdir" ]; then destdir=$(dirname -- "$f"); fi
mkdir -p -- "$destdir" || exit 1
dest="$destdir/$newid.jsonl"
tmp="$dest.part.$$"
awk -v anchor="$anchor" -v oldid={id_q} -v newid="$newid" \
    -v oldcwd="$oldcwd" -v newcwd="$newcwd" '
function rep(s, from, to,    out, p) {{
  if (from == "") return s
  out = ""
  while ((p = index(s, from)) > 0) {{
    out = out substr(s, 1, p - 1) to
    s = substr(s, p + length(from))
  }}
  return out s
}}
anchor != "" && index($0, "\"uuid\":\"" anchor "\"") > 0 {{ found = 1; exit }}
{{
  line = rep($0, "\"sessionId\":\"" oldid "\"", "\"sessionId\":\"" newid "\"")
  if (newcwd != "") line = rep(line, "\"cwd\":\"" oldcwd "\"", "\"cwd\":\"" newcwd "\"")
  print line
}}
END {{ if (anchor != "" && found != 1) exit 3 }}
' "$f" > "$tmp"
rc=$?
if [ "$rc" -ne 0 ]; then
  rm -f -- "$tmp"
  if [ "$rc" -eq 3 ]; then printf '{NO_ANCHOR} %s\n' "$anchor" >&2; fi
  exit "$rc"
fi
mv -- "$tmp" "$dest" || exit 1
printf '%s\n' "$dest"
"#,
        id_q = quote(claude_session_id),
    ));
    s
}
```

Register the module in `crates/fleet-core/src/service/mod.rs`, in the alphabetical position among its neighbours:

```rust
pub mod rewind;
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::rewind -- --nocapture
```

Expected: PASS, all six.

- [ ] **Step 6: Lint and format**

```bash
cargo fmt --all && cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/rewind.rs crates/fleet-core/src/service/mod.rs crates/fleet-core/src/service/transcript.rs
git commit -m "feat(rewind): host-side script that truncates a transcript into a new id

Reuses transcript::locate_script for resolution and adds one awk pass:
copy the head, stop before the anchor, rewrite sessionId (and cwd for a
cross-worktree fork). Replacements are index-based and literal, because a
cwd is full of regex metacharacters. Writes to a temp file and moves on
success, so a missing anchor leaves nothing for --resume to find.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 3: The service function

Runs the script on the session's host, then either rebinds-and-restarts this session or spawns a new one.

**Files:**
- Modify: `crates/fleet-core/src/service/rewind.rs` (add `RewindArgs`, `RewindMode`, `rewind_conversation`)
- Test: inline `mod tests` in the same file

**Interfaces:**
- Consumes: `rewind_script` (Task 2); `Store::get_session_by_id`, `Store::rebind_conversation(session_id, claude_session_id, StartSource::Fork, transcript_path, model)`, `sessions::restart_session(RestartSessionArgs { host_alias, name, force })`, `sessions::new_session(NewSessionArgs { .., resume_claude_session_id })`.
- Produces:
  ```rust
  // Both derive Serialize + Deserialize; RewindMode is
  // `#[serde(rename_all = "snake_case")]`, so it serialises to exactly the
  // wire's "rewind" / "fork" and is usable as a #[tauri::command] parameter.
  pub enum RewindMode { Rewind, Fork }
  pub struct RewindArgs {
      pub session_id: i64,
      pub anchor_uuid: Option<String>,
      pub mode: RewindMode,
      /// Fork only: `Some(name)` puts the new session in a new worktree of
      /// that name; `None` reuses the source session's worktree.
      pub new_worktree: Option<String>,
  }
  pub async fn rewind_conversation(
      args: RewindArgs,
      store: &Mutex<Store>,
      ssh: &Arc<SshClient>,
  ) -> Result<SessionRow, IpcError>;
  ```
  Task 4 wraps this as an MCP tool; Task 5 as a Tauri command.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `crates/fleet-core/src/service/rewind.rs`:

```rust
    use crate::store::Store;

    fn store_with_session(status: &str, claude_status: Option<&str>) -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let id = s
            .upsert_session("sess", "h1", None, None, 0, 0, status, None)
            .unwrap();
        s.set_claude_session_id(id, OLD).unwrap();
        if let Some(cs) = claude_status {
            s.set_claude_status(id, Some(cs)).unwrap();
        }
        (s, id)
    }

    #[tokio::test]
    async fn rewinding_a_working_session_is_refused_before_anything_runs() {
        let (s, id) = store_with_session("running", Some("working"));
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(crate::ssh::SshClient::for_tests()),
        )
        .await
        .expect_err("a mid-turn restart throws the turn away");
        assert_eq!(err.code, crate::ipc_error::codes::E_INVALID);
        assert!(
            err.message.contains("interrupt"),
            "the refusal must say what to do about it: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn forking_a_working_session_is_allowed() {
        // Fork touches nothing live, so the working-session guard must NOT
        // apply to it. Asserted on the refusal, not on success: the spawn
        // itself needs a host.
        let (s, id) = store_with_session("running", Some("working"));
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Fork,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(crate::ssh::SshClient::for_tests()),
        )
        .await
        .expect_err("unreachable host");
        assert_ne!(
            err.code,
            crate::ipc_error::codes::E_INVALID,
            "fork must not be refused for being busy; it failed for another reason: {err:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_session_is_not_found() {
        let s = Store::open_in_memory().unwrap();
        let err = rewind_conversation(
            RewindArgs {
                session_id: 9999,
                anchor_uuid: None,
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(crate::ssh::SshClient::for_tests()),
        )
        .await
        .expect_err("no such session");
        assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
    }

    #[test]
    fn the_new_conversation_id_is_a_fresh_uuid_each_time() {
        assert_ne!(mint_conversation_id(), mint_conversation_id());
        assert!(crate::validate::claude_session_id(&mint_conversation_id()).is_ok());
    }
```

> **Note for the implementer:** `SshClient::for_tests()` and
> `Store::set_claude_status` are the names this task assumes. Check them
> against the current source before writing the tests — the surrounding test
> modules in `crates/fleet-core/src/service/sessions/lifecycle_tests.rs` show
> how a session-addressed service test is set up in this repo, and that is the
> pattern to copy. If a helper differs, follow the repo, not this plan, and
> keep the assertions identical.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib service::rewind
```

Expected: FAIL to compile — `cannot find function rewind_conversation`.

- [ ] **Step 3: Write the implementation**

Add to `crates/fleet-core/src/service/rewind.rs`:

```rust
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use crate::store::conversations::StartSource;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};

/// What happens after the truncated transcript exists.
///
/// `snake_case` so the serialised form IS the wire vocabulary — `"rewind"` /
/// `"fork"` — in the MCP params, in the Tauri command's arguments and in the
/// hub route alike. One vocabulary beats a `String` in one layer and an enum
/// in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindMode {
    /// Rebind THIS session to the new conversation and restart its pane.
    Rewind,
    /// Leave this session alone; spawn a new one on the new conversation.
    Fork,
}

/// Serde-derived because this is also the `#[tauri::command]` parameter type
/// (Task 5): the desktop deserialises it from the frontend and the hub route
/// serialises it back out.
#[derive(Debug, Serialize, Deserialize)]
pub struct RewindArgs {
    pub session_id: i64,
    /// Keep strictly before this entry. `None` keeps the whole transcript,
    /// which is what forking the newest turn means.
    pub anchor_uuid: Option<String>,
    pub mode: RewindMode,
    /// Fork only: `Some(name)` puts the new session in a new worktree of that
    /// name, `None` reuses the source session's.
    pub new_worktree: Option<String>,
}

/// A fresh conversation id, minted the way `claude_id_and_pane_cmd` does.
fn mint_conversation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Truncate a session's transcript into a new conversation and act on it.
///
/// One engine for three buttons: Fork here spawns on the copy, Rewind here
/// rebinds this session to it and restarts the pane, and Retry is Rewind
/// followed by a `send_prompt` the CALLER makes — so Retry inherits every
/// refusal below instead of duplicating it.
///
/// The source transcript is never mutated. That is what makes a rewind
/// undoable and leaves the pre-rewind conversation listed; `StartSource::Fork`
/// is what labels the new one, a value both clients already render.
pub async fn rewind_conversation(
    args: RewindArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    if let Some(a) = args.anchor_uuid.as_deref() {
        crate::validate::claude_session_id(a).map_err(|_| {
            IpcError::new(codes::E_INVALID, "anchor_uuid must be a lowercase UUID")
        })?;
    }
    // Snapshot under one short lock; every I/O below happens with it released.
    let (sess, claude_id) = {
        let s = crate::service::lock(store)?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
        let claude_id = sess.claude_session_id.clone().ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "this session has no Claude conversation to rewind",
            )
        })?;
        (sess, claude_id)
    };

    // Rewind respawns the pane, so doing it mid-turn throws the turn away.
    // Fork is exempt: it touches nothing live.
    if args.mode == RewindMode::Rewind && sess.claude_status.as_deref() == Some("working") {
        return Err(IpcError::new(
            codes::E_INVALID,
            "this session is mid-turn; interrupt it first, then rewind",
        ));
    }

    let new_id = mint_conversation_id();
    let script = rewind_script(
        Some(&sess.tmux_name),
        sess.transcript_path.as_deref(),
        None,
        &claude_id,
        &new_id,
        args.anchor_uuid.as_deref(),
        None,
        None,
    );
    let out = ssh.run_script(&sess.host_alias, &script).await?;
    let new_path = out.trim().to_string();

    match args.mode {
        RewindMode::Rewind => {
            {
                let s = crate::service::lock(store)?;
                s.rebind_conversation(
                    sess.id,
                    &new_id,
                    StartSource::Fork,
                    Some(&new_path),
                    sess.model.as_deref(),
                )?;
            }
            crate::service::sessions::restart_session(
                crate::service::sessions::RestartSessionArgs {
                    host_alias: sess.host_alias.clone(),
                    name: sess.tmux_name.clone(),
                    force: false,
                },
                store,
                ssh,
            )
            .await
        }
        RewindMode::Fork => {
            // Left for Task 7 to extend with `new_worktree`; reusing the
            // source's worktree is the shape that needs no git.
            let _ = &args.new_worktree;
            crate::service::sessions::new_session(
                crate::service::sessions::NewSessionArgs {
                    project_id: sess.project_id,
                    worktree_id: sess.worktree_id,
                    resume_claude_session_id: Some(new_id),
                    ..Default::default()
                },
                store,
                ssh,
            )
            .await
        }
    }
}
```

> **Note for the implementer:** `ssh.run_script`, `NewSessionArgs::default()`
> and the exact field set of `NewSessionArgs` / `SessionRow` are what to check
> against the current source first — `crates/fleet-core/src/service/sessions/lifecycle.rs:30`
> for the args and `crates/fleet-core/src/ssh.rs` for the run helper other
> transcript readers use (`fetch_conversation_for_row` at
> `transcript.rs:1625` is the closest caller to copy). Keep the behaviour and
> the refusals exactly as written; adapt the plumbing to what exists.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::rewind
```

Expected: PASS.

- [ ] **Step 5: Full backend suite, lint, format**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/rewind.rs
git commit -m "feat(rewind): one engine for fork, rewind and retry

Rewind rebinds this session to the truncated copy and restarts its pane;
fork spawns a new session on it. Retry is the caller doing rewind then
send_prompt, so it inherits the refusals rather than repeating them —
including the mid-turn one, which fork is deliberately exempt from.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 4: The MCP tool

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (add `RewindConversationParams`)
- Modify: `crates/fleet-core/src/mcp/tools/lifecycle.rs` (add the `#[tool]` method beside `restart_session` at `:169`)
- Modify: the `generate_handler!`-equivalent tool list the router uses (follow `restart_session`'s registration)
- Modify: `crates/fleet-core/src/mcp/guard.rs` — a `TOOL_POLICIES` row (beside `restart_session`'s at `:376`) **and** an `OPERATOR_CONFIRMS` entry (`:858`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs:3291` (`BUDGET_BYTES` + its doc comment)
- Test: `crates/fleet-core/src/mcp/tools/tests.rs`

**Registration is mandatory, and it comes first.** A tool with no
`TOOL_POLICIES` row fails the router exhaustiveness test (`guard.rs:878` spells
this out), and `confirm_gate`'s `debug_assert` (`support.rs:1165`) panics unless
the tool is in `CONFIRM_TOOLS` or `OPERATOR_CONFIRMS`. Take exact parity with
`restart_session`:

```rust
    ToolPolicy {
        name: "rewind_conversation",
        access: Access::Client,
        readonly: false,
        confirm: false,
        deadline: Deadline::Lifecycle,
    },
```

and add `"rewind_conversation"` to `OPERATOR_CONFIRMS`.

`Access::Client` is what lets a paired phone with a `full` token call it — the
mobile plan depends on it. `confirm: false` **plus** `OPERATOR_CONFIRMS` is
exactly `restart_session`'s treatment and it is not a contradiction: a person
driving the desktop is ungated (the desktop shows its own confirm dialog),
while the operator agent is always forced to get a human's approval
(`operator_must_confirm`, decision D12).

**Interfaces:**
- Consumes: `service::rewind::{rewind_conversation, RewindArgs, RewindMode}` (Task 3).
- Produces: MCP tool `rewind_conversation { session_id, anchor_uuid?, mode, new_worktree?, confirm_nonce? }`. Task 5's Tauri command routes to this name.

- [ ] **Step 1: Write the failing tests**

Add to `crates/fleet-core/src/mcp/tools/tests.rs`:

```rust
#[tokio::test]
async fn rewind_conversation_rejects_an_unknown_mode() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h1").unwrap();
    let id = s
        .upsert_session("sess", "h1", None, None, 0, 0, "running", None)
        .unwrap();
    let t = test_tools(s);
    let err = t
        .rewind_conversation(
            Extension(Caller::master()),
            Parameters(RewindConversationParams {
                session_id: id,
                anchor_uuid: None,
                mode: "sideways".into(),
                new_worktree: None,
                confirm_nonce: None,
            }),
        )
        .await
        .expect_err("mode is a closed set");
    assert!(format!("{err:?}").contains("mode"));
}

#[tokio::test]
async fn a_master_caller_is_not_confirm_gated_for_a_rewind() {
    // `confirm: false` + OPERATOR_CONFIRMS means a person at the desktop is
    // ungated — the desktop shows its own dialog. So this must NOT fail for
    // the confirmation; it fails later, for an unreachable host.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h1").unwrap();
    let id = s
        .upsert_session("sess", "h1", None, None, 0, 0, "running", None)
        .unwrap();
    s.set_claude_session_id(id, "11111111-1111-1111-1111-111111111111")
        .unwrap();
    let t = test_tools(s);
    let err = t
        .rewind_conversation(
            Extension(Caller::master()),
            Parameters(RewindConversationParams {
                session_id: id,
                anchor_uuid: None,
                mode: "rewind".into(),
                new_worktree: None,
                confirm_nonce: None,
            }),
        )
        .await
        .expect_err("no reachable host in a unit test");
    let text = format!("{err:?}").to_lowercase();
    assert!(
        !text.contains("confirm"),
        "a master caller must not be confirm-gated: {text}"
    );
}
```

> **The operator gate is guard.rs's, not this tool's.** `confirm_gate` is a
> no-op unless the caller is the operator (`support.rs:1172-1175`), so a test
> driving `Caller::master()` can never observe it — asserting otherwise would
> pass for the wrong reason or fail a correct implementation. Membership in
> `OPERATOR_CONFIRMS` is what this task adds, and `guard.rs`'s own tests (which
> walk `CONFIRM_TOOLS` / `OPERATOR_CONFIRMS`, e.g. `guard.rs:1523`) are what
> cover it. If an existing test covers `restart_session`'s operator gate
> specifically, copy it for `rewind_conversation`; otherwise do not invent a
> new harness for it.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib rewind_conversation
```

Expected: FAIL to compile — no `RewindConversationParams`, no `rewind_conversation` method.

- [ ] **Step 3: Add the params struct**

In `crates/fleet-core/src/mcp/tools/params.rs`, following the shape of the struct at `:115`:

```rust
#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RewindConversationParams")]
pub struct RewindConversationParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Keep the transcript strictly before this turn's prompt_uuid (from
    /// session_conversation). Omit to keep all of it.
    #[serde(default)]
    pub anchor_uuid: Option<String>,
    /// "rewind" restarts this session on the truncated copy; "fork" leaves it
    /// alone and starts a new session on the copy.
    pub mode: String,
    /// fork only: name a new worktree for the new session; omit to reuse this
    /// session's.
    #[serde(default)]
    pub new_worktree: Option<String>,
    /// Confirmation nonce; required for "rewind".
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}
```

- [ ] **Step 4: Add the tool method**

In `crates/fleet-core/src/mcp/tools/lifecycle.rs`, after `restart_session` (which ends at `:210`). Keep the description short — every byte is charged to the budget in Step 6.

```rust
    #[tool(description = "Truncate a session's Claude transcript into a new \
        conversation and act on it: mode \"fork\" starts a new session from \
        that point, mode \"rewind\" restarts this one there. The original \
        transcript is never changed. Returns the affected session row as JSON.")]
    pub(super) async fn rewind_conversation(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RewindConversationParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "rewind_conversation",
            &format!(
                "session_id={} mode={} anchor={:?}",
                p.session_id, p.mode, p.anchor_uuid
            ),
        );
        let mode = match p.mode.as_str() {
            "rewind" => rewind::RewindMode::Rewind,
            "fork" => rewind::RewindMode::Fork,
            other => {
                return Err(to_mcp_err(IpcError::new(
                    codes::E_INVALID,
                    format!("mode must be \"rewind\" or \"fork\", not {other:?}"),
                )))
            }
        };
        // Rewind rebuilds a live pane, so it needs a person (D12), the same
        // rule restart_session follows. Fork starts something new and does not.
        if mode == rewind::RewindMode::Rewind {
            self.confirm_gate(
                "rewind_conversation",
                p.confirm_nonce.as_deref(),
                &format!("session_id={} anchor={:?}", p.session_id, p.anchor_uuid),
                &caller,
            )?;
        }
        let row = rewind::rewind_conversation(
            rewind::RewindArgs {
                session_id: p.session_id,
                anchor_uuid: p.anchor_uuid,
                mode,
                new_worktree: p.new_worktree,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }
```

Add `use crate::service::rewind;` to the file's imports, and register the tool wherever `restart_session` is registered in the router list.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib rewind_conversation
```

Expected: PASS.

- [ ] **Step 6: Measure and raise the definition budget**

```bash
cargo test -p fleet-core --lib the_served_definition_budget_stays_bounded -- --nocapture
```

Expected: FAIL, printing the actual and the budget. Take those two numbers and:

1. Try trimming the tool description first. Every field doc is mandatory (`every_tool_parameter_is_documented`), so the description is the only slack.
2. Then raise `BUDGET_BYTES` (`crates/fleet-core/src/mcp/tools/tests.rs:3291`, currently `71_658`) to the measured value plus ~100 bytes of headroom, and append a paragraph to its doc comment in the established style, e.g.:

```rust
    /// Raised from 71,658 to <new> for `rewind_conversation`: one tool with a
    /// `mode` rather than separate fork/rewind tools, five fields whose
    /// per-field docs `every_tool_parameter_is_documented` makes mandatory.
    /// The surface before it measured <before>; trimming the description to
    /// one sentence was tried first and freed <n> bytes, not enough.
```

- [ ] **Step 7: Regenerate the reference and run the suite**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: PASS, with `docs/control-api-reference.md` updated.

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-core/src docs/control-api-reference.md
git commit -m "feat(mcp): rewind_conversation tool

One tool with a mode rather than two, to keep the definition-budget raise to
a single addition. Rewind is confirm-gated like restart_session, since it
rebuilds a live pane; fork is not, since it starts something new.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 5: The Tauri command, the verdict row, and the frontend wrapper

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs` (the `#[tauri::command]` at `:156`'s neighbourhood, and the `routed::` module at `:571`'s neighbourhood)
- Modify: `src-tauri/src/lib.rs` (`generate_handler!` list)
- Modify: `src-tauri/src/backend/verdicts.rs` (a row beside `restart_session`'s at `:411`)
- Modify: `src-tauri/src/backend/tests_routing.rs` (the handler list and the routed-call assertion)
- Modify: `src/lib/sessions.ts` (a wrapper beside `restartSession` at `:405`)
- Test: `src/lib/sessions.test.ts`

**Interfaces:**
- Consumes: MCP tool name `rewind_conversation` (Task 4).
- Produces: `rewindConversation(sessionId: number, mode: 'rewind' | 'fork', anchorUuid: string | null, newWorktree?: string | null): Promise<Result<SessionRow>>` — Task 6 calls this.

- [ ] **Step 1: Write the failing frontend test**

Add to `src/lib/sessions.test.ts`, following the `restart_session` case at `:88`:

```ts
it('rewindConversation invokes the command with snake_case args and accepts the row', async () => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample[0]);
  const r = await rewindConversation(7, 'fork', 'aaaaaaaa-0000-0000-0000-000000000002');
  expect(r.ok).toBe(true);
  expect(mockedInvoke).toHaveBeenCalledWith('rewind_conversation', {
    args: {
      session_id: 7,
      mode: 'fork',
      anchor_uuid: 'aaaaaaaa-0000-0000-0000-000000000002',
      new_worktree: null,
    },
  });
});

it('rewindConversation passes a null anchor through as null', async () => {
  // Forking the newest turn has no later prompt, and null means "keep it all".
  (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample[0]);
  await rewindConversation(7, 'fork', null);
  const args = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.at(-1)![1].args;
  expect(args.anchor_uuid).toBeNull();
});
```

Import `rewindConversation` from `./sessions` at the top of the test file.

- [ ] **Step 2: Run the test to verify it fails**

```bash
pnpm test src/lib/sessions.test.ts
```

Expected: FAIL — `rewindConversation is not a function`.

- [ ] **Step 3: Add the verdict row**

In `src-tauri/src/backend/verdicts.rs`, in `generate_handler!` order (beside `restart_session`'s row at `:411`):

```rust
    (
        "rewind_conversation",
        Verdict::Routed {
            tool: "rewind_conversation",
        },
    ),
```

Routed, not `LocalOnly`: the hub has the session and the transcript both, so there is nothing about this that only the local machine can do.

- [ ] **Step 4: Add the Tauri command**

In `src-tauri/src/commands/sessions.rs`, beside `restart_session`:

```rust
#[tauri::command]
pub async fn rewind_conversation(
    args: RewindArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::rewind_conversation(&backend, args, &store, &ssh).await
}
```

And in the `routed` module in the same file, follow `routed::restart_session` at `:571` exactly, routing **by command name** `"rewind_conversation"` — never a second tool literal.

Register the command in `src-tauri/src/lib.rs`'s `generate_handler!` list, in the same position the verdict row occupies.

- [ ] **Step 5: Add the frontend wrapper**

In `src/lib/sessions.ts`, after `restartSession` at `:411`:

```ts
/** Truncate this session's transcript into a new conversation.
 *
 *  `mode: 'rewind'` restarts THIS session on the copy; `'fork'` leaves it
 *  running and starts a new session on the copy. `anchorUuid` is the turn's
 *  `prompt_uuid` for a rewind, and the NEXT later turn's for a fork — `null`
 *  keeps the whole transcript, which is what forking the newest turn means.
 */
export async function rewindConversation(
  sessionId: number,
  mode: 'rewind' | 'fork',
  anchorUuid: string | null,
  newWorktree: string | null = null,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rewind_conversation', {
    args: {
      session_id: sessionId,
      mode,
      anchor_uuid: anchorUuid,
      new_worktree: newWorktree,
    },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}
```

- [ ] **Step 6: Regenerate the hub verdicts**

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
```

Expected: PASS, updating `src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md`. (No `REASONS` entry is needed — this is `Routed`, not `LocalOnly`.)

- [ ] **Step 7: Run everything**

```bash
pnpm test && pnpm check
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: PASS. `every_command_has_a_verdict` and `every_commands_body_does_what_its_row_says` in `src-tauri/src/backend/tests_routing.rs` will name anything you missed.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src src/lib/sessions.ts src/lib/sessions.test.ts src/lib/hub_verdicts.generated.json docs/hub.md
git commit -m "feat: route rewind_conversation from the desktop

Routed, not local-only: the hub holds the session and the transcript both.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 6: The reply action row

**Files:**
- Create: `src/lib/ReplyActions.svelte`
- Create: `src/lib/reply_actions.ts` (the pure "which buttons, which anchor" logic)
- Modify: `src/lib/ConversationPanel.svelte:1827-1834` (the `.text` block)
- Test: Create `src/lib/reply_actions.test.ts`; modify `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `rewindConversation` (Task 5); `ConvTurn.prompt_uuid` (Task 1); `insertIntoComposer(sessionId, text)` (`src/lib/conversation.ts:902`); `CopyButton.svelte`; `ConfirmDialog.svelte`.
- Produces:
  ```ts
  export interface ReplyActionsView {
    canFork: boolean;
    canRewind: boolean;   // also gates Retry
    forkAnchor: string | null;    // null = keep the whole transcript
    rewindAnchor: string | null;  // null when !canRewind
  }
  export function replyActionsFor(
    turns: ConvTurn[], index: number, truncated: boolean, supported: boolean,
  ): ReplyActionsView;
  export function quoteText(text: string): string;
  ```

- [ ] **Step 1: Write the failing tests**

Create `src/lib/reply_actions.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { replyActionsFor, quoteText } from './reply_actions';
import type { ConvTurn } from './conversation';

const turn = (prompt_uuid: string | null): ConvTurn => ({
  prompt: prompt_uuid ? 'hi' : null,
  at: null,
  ended_at: null,
  items: [],
  prompt_uuid,
});

describe('replyActionsFor', () => {
  it('forks on the NEXT later turn’s anchor, so this turn is kept', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, false, true);
    expect(v.forkAnchor).toBe('a2');
    expect(v.canFork).toBe(true);
  });

  it('forking the newest turn keeps the whole transcript', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, true);
    expect(v.canFork).toBe(true);
    expect(v.forkAnchor).toBeNull();
  });

  it('skips prompt-less turns when scanning forward for the fork anchor', () => {
    // A compact boundary between them has no anchor; fork must keep going and
    // keep MORE history rather than give up or keep less.
    const v = replyActionsFor([turn('a1'), turn(null), turn('a3')], 0, false, true);
    expect(v.forkAnchor).toBe('a3');
  });

  it('rewinds on this turn’s own anchor', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, true);
    expect(v.canRewind).toBe(true);
    expect(v.rewindAnchor).toBe('a2');
  });

  it('offers no rewind on the first turn of an untruncated conversation', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, false, true);
    expect(v.canRewind).toBe(false);
  });

  it('DOES offer rewind on index 0 when the window is truncated', () => {
    // Index 0 is only the conversation's first turn when nothing older was
    // dropped. Hiding it here would hide rewind on most of a long session.
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, true, true);
    expect(v.canRewind).toBe(true);
    expect(v.rewindAnchor).toBe('a1');
  });

  it('offers no rewind on a prompt-less turn', () => {
    const v = replyActionsFor([turn('a1'), turn(null)], 1, false, true);
    expect(v.canRewind).toBe(false);
  });

  it('offers nothing but copy and quote when the backend does not support it', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, false);
    expect(v.canFork).toBe(false);
    expect(v.canRewind).toBe(false);
  });
});

describe('quoteText', () => {
  it('prefixes every line, including blank ones, and ends with a blank line', () => {
    expect(quoteText('a\n\nb')).toBe('> a\n>\n> b\n\n');
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
pnpm test src/lib/reply_actions.test.ts
```

Expected: FAIL — cannot resolve `./reply_actions`.

- [ ] **Step 3: Write the pure logic**

Create `src/lib/reply_actions.ts`:

```ts
import type { ConvTurn } from './conversation';

/** Which of the five reply actions this turn offers, and with what anchor. */
export interface ReplyActionsView {
  canFork: boolean;
  /** Gates Rewind here AND Retry — Retry is a rewind plus a re-send. */
  canRewind: boolean;
  /** Keep strictly before this; `null` keeps the whole transcript. */
  forkAnchor: string | null;
  rewindAnchor: string | null;
}

/**
 * `truncated` is the conversation's own flag: index 0 is the conversation's
 * FIRST turn only when nothing older was dropped, and rewinding the first
 * turn would leave an empty conversation (that is `/clear`, under a
 * misleading name). `supported` is the backend gate — the hub version on a
 * phone, always true on a local desktop.
 */
export function replyActionsFor(
  turns: ConvTurn[],
  index: number,
  truncated: boolean,
  supported: boolean,
): ReplyActionsView {
  const none = { canFork: false, canRewind: false, forkAnchor: null, rewindAnchor: null };
  if (!supported) return none;

  // Fork keeps everything through THIS turn, so it anchors on the next later
  // prompt. Prompt-less turns (a compact boundary, a notification-only turn)
  // carry no anchor, so scan past them: keeping more history is safe, keeping
  // less would silently discard work.
  let forkAnchor: string | null = null;
  for (let j = index + 1; j < turns.length; j++) {
    const a = turns[j].prompt_uuid;
    if (a) {
      forkAnchor = a;
      break;
    }
  }

  const own = turns[index]?.prompt_uuid ?? null;
  const isConversationStart = index === 0 && !truncated;
  const canRewind = own !== null && !isConversationStart;

  return {
    canFork: true,
    canRewind,
    forkAnchor,
    rewindAnchor: canRewind ? own : null,
  };
}

/** A reply as a Markdown block quote, ready to precede the user's own words. */
export function quoteText(text: string): string {
  const body = text
    .split('\n')
    .map((l) => (l.length === 0 ? '>' : `> ${l}`))
    .join('\n');
  return `${body}\n\n`;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
pnpm test src/lib/reply_actions.test.ts
```

Expected: PASS, all nine.

- [ ] **Step 5: Write the component**

Create `src/lib/ReplyActions.svelte`:

```svelte
<script lang="ts">
  // The action row under one reply. Always visible, never hover-revealed —
  // CopyButton's own comment says why: a control you must hover to find is
  // not a control a keyboard or touch user has.
  //
  // Retry is deliberately not a third backend mode. It is a rewind followed
  // by a send, so it inherits every refusal the rewind has (including the
  // mid-turn one) instead of keeping a second copy of them in step.
  import CopyButton from './CopyButton.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { rewindConversation } from './sessions';
  import { insertIntoComposer } from './conversation';
  import { sendPrompt } from './sessions';
  import { replyActionsFor, quoteText } from './reply_actions';
  import type { ConvTurn } from './conversation';

  let {
    turns,
    index,
    truncated,
    text,
    sessionId,
    hostAlias,
    tmuxName,
    supported = true,
    onFork,
  }: {
    turns: ConvTurn[];
    index: number;
    truncated: boolean;
    text: string;
    sessionId: number;
    /** `sendPrompt` addresses a session by host + tmux name, not by id. */
    hostAlias: string;
    tmuxName: string;
    supported?: boolean;
    /** Opens the worktree sheet (Task 7); it calls the backend itself. */
    onFork: (anchor: string | null) => void;
  } = $props();

  const view = $derived(replyActionsFor(turns, index, truncated, supported));
  const prompt = $derived(turns[index]?.prompt ?? null);

  let confirming = $state<'rewind' | 'retry' | null>(null);
  let busy = $state(false);

  // The one sentence that must not be softened: this is where fleet diverges
  // from Claude Code's own /rewind, which restores files from its checkpoints.
  const REWIND_COPY =
    'The conversation is rewound to before this turn. Your files are left as they are.';

  async function doRewind(retry: boolean) {
    busy = true;
    const r = await rewindConversation(sessionId, 'rewind', view.rewindAnchor);
    busy = false;
    confirming = null;
    if (!r.ok) return;
    if (retry && prompt) {
      // sendPrompt(hostAlias, tmuxName, prompt) — see `src/lib/sessions.ts:598`.
      // `send_prompt` has no session-id form.
      await sendPrompt(hostAlias, tmuxName, prompt);
    } else if (prompt) {
      insertIntoComposer(sessionId, prompt);
    }
  }
</script>

<div class="reply-actions" role="group" aria-label="Actions for this reply">
  <CopyButton {text} label="Copy reply" />
  <button
    type="button"
    class="btn btn--icon btn--quiet"
    data-testid="reply-quote"
    aria-label="Quote this reply in the composer"
    title="Quote"
    onclick={() => insertIntoComposer(sessionId, quoteText(text))}>❝</button
  >
  {#if view.canRewind}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-retry"
      aria-label="Retry this turn"
      title="Retry — rewind and send the same prompt again"
      disabled={busy}
      onclick={() => (confirming = 'retry')}>↻</button
    >
  {/if}
  {#if view.canFork}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-fork"
      aria-label="Fork a new session from this reply"
      title="Fork here"
      onclick={() => onFork(view.forkAnchor)}>⑂</button
    >
  {/if}
  {#if view.canRewind}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-rewind"
      aria-label="Rewind this session to before this turn"
      title="Rewind here"
      disabled={busy}
      onclick={() => (confirming = 'rewind')}>⏪</button
    >
  {/if}
</div>

{#if confirming}
  <ConfirmDialog
    title={confirming === 'retry' ? 'Retry this turn?' : 'Rewind to before this turn?'}
    body={confirming === 'retry'
      ? `${REWIND_COPY} The same prompt is then sent again.`
      : `${REWIND_COPY} The prompt returns to the composer.`}
    confirmLabel={confirming === 'retry' ? 'Retry' : 'Rewind'}
    onconfirm={() => void doRewind(confirming === 'retry')}
    oncancel={() => (confirming = null)}
  />
{/if}

<style>
  .reply-actions {
    display: flex;
    gap: 2px;
    align-items: center;
  }
</style>
```

> **Note for the implementer:** `ConfirmDialog.svelte`'s and `sendPrompt`'s
> actual prop/parameter names are what to check first — copy the call shape
> from an existing caller (`src/lib/Sidebar.svelte`'s restart confirmation is
> the closest) rather than trusting the names above. The copy string and the
> `data-testid`s must stay exactly as written.

- [ ] **Step 6: Wire it into the panel**

In `src/lib/ConversationPanel.svelte`, replace the `.text` block at `:1827-1834`:

```svelte
                  {#if g.kind === 'text'}
                    <div class="text" data-testid="conv-text">
                      <Markdown source={g.text} />
                      <span class="copy-slot text-copy">
                        <ReplyActions
                          turns={conv.turns}
                          index={i}
                          truncated={conv.truncated}
                          text={g.text}
                          sessionId={session.id}
                          hostAlias={session.host_alias}
                          tmuxName={session.tmux_name}
                          onFork={(anchor) => openForkSheet(anchor)}
                        />
                      </span>
                    </div>
```

`i` is the `{@const i = row.index}` already in scope at `:1777`. Import `ReplyActions from './ReplyActions.svelte'` beside the `CopyButton` import at `:32`, and add `openForkSheet` as a stub `(anchor: string | null) => {}` for now — Task 7 implements it.

- [ ] **Step 7: Add the component tests**

Create `src/lib/ReplyActions.test.ts`. The second test is the one that matters
most: Retry is two backend calls, and a refused rewind that still sent the
prompt would append it to the conversation it failed to rewind — the worst of
both outcomes.

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import ReplyActions from './ReplyActions.svelte';

const rewindConversation = vi.fn();
const sendPrompt = vi.fn();
vi.mock('./sessions', () => ({ rewindConversation, sendPrompt, acceptCommandRow: vi.fn() }));

const turns = [
  { prompt: 'one', at: null, ended_at: null, items: [], prompt_uuid: 'a1' },
  { prompt: 'two', at: null, ended_at: null, items: [], prompt_uuid: 'a2' },
];

// `sendPrompt` addresses a session by host + tmux name, not by id
// (src/lib/sessions.ts:598), so the component needs both.
const base = {
  turns,
  truncated: false,
  text: 'reply',
  sessionId: 7,
  hostAlias: 'h1',
  tmuxName: 'sess',
  onFork: () => {},
};

beforeEach(() => {
  rewindConversation.mockReset();
  sendPrompt.mockReset();
});

describe('ReplyActions', () => {
  it('retry rewinds and then sends the same prompt', async () => {
    rewindConversation.mockResolvedValue({ ok: true, value: { id: 7 } });
    render(ReplyActions, {
      props: { ...base, index: 1 },
    });
    await screen.getByTestId('reply-retry').click();
    await screen.getByTestId('confirm-ok').click();
    expect(rewindConversation).toHaveBeenCalledWith(7, 'rewind', 'a2');
    expect(sendPrompt).toHaveBeenCalledWith('h1', 'sess', 'two');
  });

  it('a refused rewind does NOT then send the prompt', async () => {
    rewindConversation.mockResolvedValue({
      ok: false,
      error: { code: 'E_INVALID', message: 'this session is mid-turn' },
    });
    render(ReplyActions, {
      props: { ...base, index: 1 },
    });
    await screen.getByTestId('reply-retry').click();
    await screen.getByTestId('confirm-ok').click();
    expect(sendPrompt).not.toHaveBeenCalled();
  });

  it('offers no rewind or retry on the first turn of an untruncated conversation', () => {
    render(ReplyActions, {
      props: { ...base, index: 0 },
    });
    expect(screen.queryByTestId('reply-rewind')).toBeNull();
    expect(screen.queryByTestId('reply-retry')).toBeNull();
    expect(screen.getByTestId('reply-quote')).toBeTruthy();
  });
});
```

`confirm-ok` is whatever `ConfirmDialog.svelte`'s confirm button is actually
labelled or tagged — read it and use the real selector. The click-then-confirm
sequence and the two `expect`s must stay as written.

Then add one panel-level test to `src/lib/ConversationPanel.test.ts`, following
that file's existing render helper rather than inventing one:

```ts
it('renders an action row per reply text group', async () => {
  // …render the panel with two prompted turns and truncated: false…
  expect(screen.getAllByTestId('reply-quote').length).toBe(2);
  expect(screen.queryAllByTestId('reply-rewind').length).toBe(1); // the second turn only
});
```

- [ ] **Step 8: Run the frontend suite**

```bash
pnpm test && pnpm check
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/lib/ReplyActions.svelte src/lib/ReplyActions.test.ts src/lib/reply_actions.ts src/lib/reply_actions.test.ts src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(ui): reply action row — copy, quote, retry, fork, rewind

The which-buttons logic is a pure function so its edges are testable: fork
anchors on the NEXT later prompt (so this turn is kept), rewind on its own,
and index 0 hides rewind only when the window is not truncated.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 7: The fork worktree sheet

**Files:**
- Create: `src/lib/ForkSheet.svelte`
- Modify: `src/lib/ConversationPanel.svelte` (replace the `openForkSheet` stub from Task 6)
- Modify: `crates/fleet-core/src/service/rewind.rs` (honour `new_worktree` in the `Fork` arm)
- Test: Create `src/lib/ForkSheet.test.ts`; add to `crates/fleet-core/src/service/rewind.rs`'s tests

**Interfaces:**
- Consumes: `rewindConversation(sessionId, 'fork', anchor, newWorktree)` (Task 5); `RewindArgs.new_worktree` (Task 3).
- Produces: nothing downstream — this is the last task.

- [ ] **Step 1: Write the failing backend test**

Append to `mod tests` in `crates/fleet-core/src/service/rewind.rs`:

```rust
    #[test]
    fn a_new_worktree_fork_rewrites_cwd_and_targets_the_new_project_dir() {
        // The pane must start where the transcript says it did, or
        // `cl --resume` will not find the conversation (see the constraint on
        // NewSessionArgs::resume_claude_session_id). So a cross-worktree fork
        // writes into the NEW cwd's encoded project dir with `cwd` rewritten.
        let enc = crate::service::transcript::encode_project_dir("/src/app-fork");
        assert_eq!(enc, "-src-app-fork");
        let s = rewind_script(
            None, Some("/t/x.jsonl"), None, OLD, NEW, Some(A2),
            Some(&format!("/home/u/.claude/projects/{enc}")),
            Some(("/src/app", "/src/app-fork")),
        );
        assert!(s.contains(&format!("projects/{enc}")), "dest dir must be the new cwd's: {s}");
        assert!(s.contains("'/src/app-fork'"));
    }
```

- [ ] **Step 2: Write the failing frontend test**

Create `src/lib/ForkSheet.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import ForkSheet from './ForkSheet.svelte';

describe('ForkSheet', () => {
  it('defaults to a new worktree, because two sessions in one checkout lose work', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'fork-of-canopus', onclose: () => {} } });
    const newWt = screen.getByTestId('fork-new-worktree') as HTMLInputElement;
    expect(newWt.checked).toBe(true);
  });

  it('warns in the same-worktree option rather than only in a tooltip', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose: () => {} } });
    expect(screen.getByTestId('fork-same-warning').textContent).toMatch(/same files/i);
  });

  it('passes the chosen worktree name through to the backend', async () => {
    // …click Fork with the default selected, assert rewindConversation was
    // called with ('fork', anchor, 'fork-of-canopus')…
  });
});
```

Fill the third test's body using this file's mocking style for `./sessions` (`vi.mock`), as `src/lib/sessions.test.ts` does.

- [ ] **Step 3: Run both to verify they fail**

```bash
cargo test -p fleet-core --lib a_new_worktree_fork
pnpm test src/lib/ForkSheet.test.ts
```

Expected: FAIL — no `ForkSheet.svelte`; the script assertion fails on the dest dir.

- [ ] **Step 4: Honour `new_worktree` in the service**

In `crates/fleet-core/src/service/rewind.rs`, replace the `RewindMode::Fork` arm's `let _ = &args.new_worktree;` with: resolve or create the target worktree for `sess.project_id` on `sess.host_alias`, compute its absolute path, and pass `dest_dir = Some(format!("$HOME/.claude/projects/{}", encode_project_dir(&new_cwd)))` plus `cwd_rewrite = Some((&old_cwd, &new_cwd))` into `rewind_script`. Reuse the worktree machinery `new_session` already calls (`crates/fleet-core/src/service/worktrees.rs`); do not add a second way to make a worktree.

When `new_worktree` is `None`, pass `None` for both, which is the same-worktree behaviour Task 3 shipped.

- [ ] **Step 5: Write the sheet**

Create `src/lib/ForkSheet.svelte`: a `Modal`-based sheet with two radios (`data-testid="fork-new-worktree"` checked by default, and a same-worktree option whose label carries `data-testid="fork-same-warning"` reading "⚠ both sessions edit the same files"), a text input for the new worktree name prefilled with `suggestedName`, and Cancel / Fork buttons. Fork calls `rewindConversation(sessionId, 'fork', anchor, newWorktreeOrNull)` and closes. Follow `src/lib/NewSessionDialog.svelte` for the modal and form idiom.

The sheet **is** Fork's confirmation. Do not add a second dialog.

- [ ] **Step 6: Replace the stub in the panel**

In `src/lib/ConversationPanel.svelte`, replace Task 6's `openForkSheet` stub with state that records the anchor and renders `<ForkSheet>` when it is set.

- [ ] **Step 7: Run everything**

```bash
pnpm test && pnpm check
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
scripts/ci-local.sh
```

Expected: PASS.

- [ ] **Step 8: Commit and open the PR**

```bash
git add -A
git commit -m "feat(ui): fork sheet — choose the new session's worktree

Defaults to a new worktree: two live Claude sessions editing one checkout is
the standard way to lose work. The sheet is fork's confirmation, so there is
no second dialog.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
gh pr create --fill
```

---

## Self-Review

**Spec coverage.** §1 five buttons → Tasks 6, 7. §2 one engine → Task 3. §3 one anchor → Task 1. §4.1 script → Task 2. §4.2 service → Task 3. §4.3 refusals → Task 3 (mid-turn, not-found, anchor-missing via Task 2's exit 3) and Task 6 (first-turn, client side). §5.1 action row + the gating table → Task 6. §5.2 fork sheet → Task 7. §5.3 confirmation copy → Task 6, verbatim in `REWIND_COPY`. §6 MCP + verdicts + regens + budget → Tasks 4 and 5. §7 testing → the test steps throughout. §8 out of scope → nothing to build. §9 order → this plan is step 1; the mobile plan is step 2.

**One gap found and accepted:** §4.3's "session is the fleet controller" refusal is inherited from `restart_session`'s own `guard_not_controller` rather than re-checked in `rewind_conversation`, so it is covered by existing tests, not new ones. That is the right place for it — one guard, not two.

**Type consistency.** `prompt_uuid` is spelled the same in Rust, TS and every test. `RewindMode::{Rewind, Fork}` maps to the wire strings `"rewind"` / `"fork"` in Task 4 and to the TS union `'rewind' | 'fork'` in Task 5. `rewindConversation`'s four parameters match its two call sites (Task 6's `doRewind`, Task 7's sheet). `replyActionsFor`'s four parameters match its call in `ReplyActions.svelte`. `forkAnchor`/`rewindAnchor` are the names used in both the interface and the component.

**Placeholders.** Tasks 1–6 carry complete code. Task 7 Steps 4 and 5 describe structure rather than showing every line, because both depend on worktree helpers and a modal idiom whose current signatures must be read first; each names the exact file to copy from and the exact test IDs and defaults that must result. Three steps carry an explicit "check this against the source first" note for the same reason — those are the places where this plan is least able to be certain, and saying so beats a confident wrong signature.
