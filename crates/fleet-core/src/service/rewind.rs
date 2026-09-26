//! Truncating a transcript into a new conversation id: the shared engine
//! behind Fork here, Rewind here and Retry.

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
}
