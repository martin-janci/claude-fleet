//! Truncating a transcript into a new conversation id: the shared engine
//! behind Fork here, Rewind here and Retry.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, lock, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionRow, StartSource, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

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

/// Run a bash script on `host_alias` (local or remote) and return the
/// captured output. Mirrors `service::safe_kill::run_shell` /
/// `service::transcript`'s own local wrapper: `crate::ssh::run_shell` takes
/// a `&dyn SshExec`, not the `Arc<SshClient>` service functions carry, so
/// each service module keeps a thin wrapper rather than repeating the
/// `ssh.as_ref()` + timeout at every call site.
async fn run_shell(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell(
        ssh.as_ref(),
        host_alias,
        script,
        std::time::Duration::from_secs(30),
    )
    .await
}

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
    /// name, `None` reuses the source session's. NOT implemented yet (Task
    /// 7): the fork arm below reuses the source's worktree unconditionally
    /// and leaves this field unread.
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
///
/// `reg` is threaded through to `sessions::new_session` (the Fork arm) so a
/// forked spawn's remote git/clone step is cancellable through the same
/// `CancellationRegistry` every other spawn uses — `rewind_script`'s brief
/// omitted it, but `new_session` hard-requires one and minting a throwaway
/// registry per call would make that cancellation silently unreachable.
pub async fn rewind_conversation(
    args: RewindArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    if let Some(a) = args.anchor_uuid.as_deref() {
        crate::validate::claude_session_id(a)
            .map_err(|_| IpcError::new(codes::E_INVALID, "anchor_uuid must be a lowercase UUID"))?;
    }
    // Snapshot under one short lock; every I/O below happens with it released.
    let (sess, claude_id, stored_transcript_path) = {
        let s = lock(store)?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
        let claude_id = sess.claude_session_id.clone().ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "this session has no Claude conversation to rewind",
            )
        })?;
        // `SessionRow` carries no `transcript_path` field of its own (only
        // conversation rows do); the session table's copy is a separate
        // getter, same as `transcript::resolve_args` uses.
        let stored_transcript_path = s.session_transcript_path(sess.id)?;
        (sess, claude_id, stored_transcript_path)
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
        Some(sess.tmux_name.as_str()),
        stored_transcript_path.as_deref(),
        None,
        &claude_id,
        &new_id,
        args.anchor_uuid.as_deref(),
        None,
        None,
    );
    // `run_shell` above is this module's copy of the same thin wrapper every
    // transcript script already goes through (`transcript::read_tail`), so
    // the SSH quoting round-trip is handled inside `rewind_script` and needs
    // nothing extra here.
    let out = run_shell(ssh, &sess.host_alias, &script).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Map the script's sentinels to codes rather than letting prose leak
        // out, exactly as `read_tail` maps NO_TRANSCRIPT.
        if stderr.contains(NO_ANCHOR) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                "that reply is no longer in the transcript",
            ));
        }
        if stderr.contains(crate::service::transcript::NO_TRANSCRIPT) {
            return Err(IpcError::new(
                codes::E_NO_TRANSCRIPT,
                format!("no transcript for this session on {}", sess.host_alias),
            ));
        }
        return Err(IpcError::new(
            codes::E_SHELL,
            format!("rewind failed: {}", stderr.trim()),
        ));
    }
    let new_path = String::from_utf8_lossy(&out.stdout).trim().to_string();

    match args.mode {
        RewindMode::Rewind => {
            {
                let s = lock(store)?;
                s.rebind_conversation(
                    sess.id,
                    &new_id,
                    StartSource::Fork,
                    Some(&new_path),
                    sess.context.model.as_deref(),
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
            let project_id = sess.project_id.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID_STATE,
                    "this session has no project to fork",
                )
            })?;
            crate::service::sessions::new_session(
                crate::service::sessions::NewSessionArgs {
                    host_alias: sess.host_alias.clone(),
                    project_id,
                    worktree_id: sess.worktree_id,
                    name: String::new(),
                    call_id: None,
                    new_worktree: None,
                    base_branch: None,
                    kind: None,
                    start_command: None,
                    friendly_name: None,
                    resume_claude_session_id: Some(new_id),
                },
                store,
                ssh,
                reg,
            )
            .await
        }
    }
}

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
                r#"{{"type":"mode","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","cwd":"/src/app","message":{{"role":"user","content":"one"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"bbbb","sessionId":"{old}","cwd":"/src/app","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a2}","sessionId":"{old}","cwd":"/src/app","message":{{"role":"user","content":"two"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"cccc","sessionId":"{old}","cwd":"/src/app","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1,
            a2 = A2
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

    /// A fixture built to trap a `gsub`-based (rather than `rep()`'s
    /// `index`-based literal) rewrite: `/src/app.old` read as a REGEX also
    /// matches `/src/appXold`, because `.` means "any character". A buggy
    /// `gsub` implementation would clobber both lines; `rep()` must touch
    /// only the one that is an exact literal match.
    fn fixture_with_gsub_trap(dir: &std::path::Path) -> std::path::PathBuf {
        let body = format!(
            concat!(
                r#"{{"type":"mode","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","cwd":"/src/app.old","message":{{"role":"user","content":"one"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"bbbb","sessionId":"{old}","cwd":"/src/appXold","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a2}","sessionId":"{old}","cwd":"/src/app.old","message":{{"role":"user","content":"two"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"cccc","sessionId":"{old}","cwd":"/src/app.old","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1,
            a2 = A2
        );
        let p = dir.join(format!("{OLD}.jsonl"));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn keeps_everything_strictly_before_the_anchor() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert_eq!(
            written.lines().count(),
            3,
            "header + turn 1's two entries, and NOT the anchor line"
        );
        assert!(written.contains(A1), "the turn before the anchor is kept");
        assert!(
            !written.contains(A2),
            "the anchor line itself is dropped — 'strictly before'"
        );
        assert!(
            !written.contains("cccc"),
            "nothing after the anchor survives"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rewrites_the_session_id_on_every_copied_line() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert!(out.status.success());
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(
            !written.contains(OLD),
            "no copied line may still name the old conversation"
        );
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
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            None,
            None,
            None,
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
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some("dddddddd-0000-0000-0000-000000000009"),
            None,
            None,
        ));
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_ANCHOR));
        assert!(
            !d.join(format!("{NEW}.jsonl")).exists(),
            "a failed truncation leaves no half-written transcript"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_cross_worktree_fork_rewrites_cwd_literally() {
        // `/src/app.old` read as a REGEX also matches `/src/appXold` (`.`
        // means "any character"), so a `gsub`-based rewrite would clobber
        // both lines. `rep()` (index-based, literal) must touch only the
        // exact match — this fixture is what actually discriminates the
        // two implementations; a plain `/src/app` fixture would pass under
        // either.
        let d = tmp();
        let src = fixture_with_gsub_trap(&d);
        let dest = d.join("elsewhere");
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            Some(dest.to_str().unwrap()),
            Some(("/src/app.old", "/src/app.new-fork")),
        ));
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(dest.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(
            written.contains(r#""cwd":"/src/app.new-fork""#),
            "the exact literal match is rewritten: {written}"
        );
        assert!(
            !written.contains(r#""cwd":"/src/app.old""#),
            "no copied line may still carry the un-rewritten old cwd: {written}"
        );
        assert!(
            written.contains(r#""cwd":"/src/appXold""#),
            "a `gsub` would treat '.' as 'any char' and clobber this line too; \
             rep() must leave it untouched: {written}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn every_interpolated_value_is_shell_quoted() {
        let s = rewind_script(
            None,
            Some("/tmp/a b/t.jsonl"),
            None,
            OLD,
            NEW,
            Some(A1),
            None,
            None,
        );
        assert!(
            s.contains("'/tmp/a b/t.jsonl'"),
            "paths with spaces must arrive quoted: {s}"
        );
    }

    #[test]
    fn hostile_values_in_every_argument_this_function_owns_are_shell_quoted() {
        // `every_interpolated_value_is_shell_quoted` above only exercises
        // `stored_path`, which `locate_script` already owned and quoted
        // before this task existed. This test covers the six values
        // `rewind_script` itself interpolates: `new_id`, `anchor_uuid`,
        // `dest_dir`, both halves of `cwd_rewrite`, and `claude_session_id`
        // (embedded a second time here, into `-v oldid=...`, separately
        // from locate_script's own use of it). None of these is validated
        // at this layer — no caller exists yet (that's Task 3) — so a
        // hostile value (a space, an embedded single quote) is a legitimate
        // input to test here.
        let claude_session_id = "sess d'e";
        let new_id = "a b'c";
        let anchor = "anchor f'g";
        let dest_dir = "/tmp/dest h'i";
        let oldcwd = "/old j'k";
        let newcwd = "/new l'm";
        let s = rewind_script(
            None,
            Some("/tmp/t.jsonl"),
            None,
            claude_session_id,
            new_id,
            Some(anchor),
            Some(dest_dir),
            Some((oldcwd, newcwd)),
        );
        for raw in [claude_session_id, new_id, anchor, dest_dir, oldcwd, newcwd] {
            let quoted = crate::shell::quote(raw);
            assert!(
                s.contains(&quoted),
                "expected the shell-quoted form of {raw:?} (i.e. {quoted}) \
                 somewhere in the generated script, found none:\n{s}"
            );
        }
    }

    // ── `rewind_conversation` ──────────────────────────────────────────
    //
    // Helper/plumbing note: the plan this task followed guessed
    // `SshClient::for_tests()` and `Store::set_claude_status`; neither
    // exists. `SshClient::new()` is the real no-agent-routing constructor
    // (see the many `Arc::new(SshClient::new())` call sites in
    // `transcript.rs`'s own async tests), and the real per-id status setter
    // is the `#[cfg(test)]` helper `Store::set_session_claude_status_for_test`
    // in `store/orgs.rs`. `Store` and `StartSource` are already in scope via
    // `use super::*` (this module's own top-level `use`), so no extra import
    // is needed here.

    fn store_with_session(status: &str, claude_status: Option<&str>) -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let id = s
            .upsert_session("sess", "h1", None, None, 0, 0, status, None)
            .unwrap();
        s.set_claude_session_id(id, OLD).unwrap();
        if let Some(cs) = claude_status {
            s.set_session_claude_status_for_test(id, cs);
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
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("a mid-turn restart throws the turn away");
        assert_eq!(err.code, codes::E_INVALID);
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
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("unreachable host");
        assert_ne!(
            err.code,
            codes::E_INVALID,
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
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("no such session");
        assert_eq!(err.code, codes::E_NOTFOUND);
    }

    #[test]
    fn the_new_conversation_id_is_a_fresh_uuid_each_time() {
        assert_ne!(mint_conversation_id(), mint_conversation_id());
        assert!(crate::validate::claude_session_id(&mint_conversation_id()).is_ok());
    }
}
