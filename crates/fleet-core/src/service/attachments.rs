//! Staging prompt attachments inside the session's worktree.
//!
//! The terminal's `upload_to_session` stages under ~/.claude-fleet/uploads/,
//! which is outside the working directory: Claude Code asks permission before
//! reading an absolute path from there. In the terminal a human approves it;
//! a prompt sent from the composer has nobody to answer. So attachments land
//! under the worktree root instead, and the directory is excluded untracked
//! so it never appears as a change the user has to explain.

use crate::shell::quote;

/// Where attachments live, relative to the worktree root.
pub const ATTACH_DIR: &str = ".claude-fleet-attachments";

// ---- the byte budget ------------------------------------------------------
//
// The composer enforces these too (`src/lib/attachments.ts`), but that is UX:
// it stops a user queueing a file that will not go. THIS is the bound — the
// one that still holds when the frontend is wrong, when a command is called
// directly, or when a tray built before a limit changed is sent afterwards.
// Without it `upload_attachments` would stream a 4 GB file under a 60-second
// per-file timeout over a link the user may not control.
//
// The numbers live here, on the side that enforces them; a test in
// `src-tauri/src/commands/upload.rs` reads `src/lib/attachments.ts` and fails
// if the two ever drift.

/// Per-file ceiling.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Ceiling for one send's attachments together.
pub const MAX_TOTAL: u64 = 25 * 1024 * 1024;

const MAX_BYTES_MB: u64 = MAX_BYTES / (1024 * 1024);
const MAX_TOTAL_MB: u64 = MAX_TOTAL / (1024 * 1024);

/// Mirrors `fmtBytes` in `src/lib/attachments.ts`, so a refusal from here
/// reads exactly like the one the composer already showed.
pub fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", (n as f64 / 1024.0).round() as u64)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Check `(name, size)` pairs against both ceilings, in order, and return the
/// first refusal as the sentence to show. Pure: the caller does the `stat`,
/// so every file is measured BEFORE anything is transferred — a batch is
/// refused whole rather than three files in.
pub fn check_budget(files: &[(String, u64)]) -> Result<(), String> {
    let mut total: u64 = 0;
    for (name, size) in files {
        if *size > MAX_BYTES {
            return Err(format!(
                "{name} is {} — the limit is {MAX_BYTES_MB} MB.",
                fmt_bytes(*size)
            ));
        }
        total = total.saturating_add(*size);
        if total > MAX_TOTAL {
            return Err(format!(
                "{name} would make {} in total — the limit is {MAX_TOTAL_MB} MB in total.",
                fmt_bytes(total)
            ));
        }
    }
    Ok(())
}

/// Print the session's worktree root on stdout, or fail.
pub fn root_script(tmux_name: &str) -> String {
    let target = quote(&crate::tmux::exact_pane(tmux_name));
    format!(
        "p=$(tmux display-message -t {target} -p '#{{pane_current_path}}') && \
         cd \"$p\" && git rev-parse --show-toplevel"
    )
}

/// Create the attachment dir and make git ignore it without tracking that
/// decision. `grep -qxF` keeps a repeated run from doubling the line.
///
/// The exclude file is found with `--git-common-dir`, NOT `<root>/.git/info/`.
/// In a linked worktree `.git` is a FILE containing `gitdir: …`, so that path
/// does not exist — `mkdir -p` on it fails with "Not a directory" and takes the
/// whole `&&` chain down. Git also reads a linked worktree's excludes from the
/// COMMON dir, so writing beside the `.git` file would never take effect even
/// if it could be created. This repo develops in linked worktrees, so that is
/// the normal case here, not the exotic one.
pub fn stage_script(root: &str) -> String {
    let dir = quote(&format!("{root}/{ATTACH_DIR}"));
    let root_q = quote(root);
    let line = quote(&format!("/{ATTACH_DIR}/"));
    format!(
        "mkdir -p {dir} && \
         g=$(git -C {root_q} rev-parse --path-format=absolute --git-common-dir) && \
         mkdir -p \"$g/info\" && \
         {{ grep -qxF {line} \"$g/info/exclude\" 2>/dev/null || \
            printf '%s\\n' {line} >> \"$g/info/exclude\"; }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_bytes_reads_the_way_the_composer_already_showed_it() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_bytes(11 * 1024 * 1024), "11.0 MB");
    }

    #[test]
    fn a_file_over_the_per_file_limit_is_refused_in_the_wording_the_user_saw() {
        let err = check_budget(&[("big.png".to_string(), MAX_BYTES + 1)]).unwrap_err();
        assert_eq!(err, "big.png is 10.0 MB — the limit is 10 MB.");
    }

    #[test]
    fn a_batch_over_the_total_budget_is_refused_naming_the_file_that_crossed_it() {
        let nine = 9 * 1024 * 1024;
        let err = check_budget(&[
            ("a.bin".to_string(), nine),
            ("b.bin".to_string(), nine),
            ("c.bin".to_string(), nine),
        ])
        .unwrap_err();
        assert_eq!(
            err,
            "c.bin would make 27.0 MB in total — the limit is 25 MB in total."
        );
    }

    // Exactly on the line, both ways: without these a `>` flipped to `>=`
    // (or the reverse) passes every other test in this module.
    #[test]
    fn a_file_of_exactly_the_per_file_limit_is_allowed_and_one_byte_more_is_not() {
        assert!(check_budget(&[("edge.bin".to_string(), MAX_BYTES)]).is_ok());
        assert!(check_budget(&[("edge.bin".to_string(), MAX_BYTES + 1)]).is_err());
    }

    #[test]
    fn a_batch_of_exactly_the_total_budget_is_allowed_and_one_byte_more_is_not() {
        let fifth = MAX_TOTAL / 5;
        let batch = |last: u64| -> Vec<(String, u64)> {
            (0..5)
                .map(|i| (format!("t{i}.bin"), if i == 4 { last } else { fifth }))
                .collect()
        };
        assert!(check_budget(&batch(fifth)).is_ok());
        assert!(check_budget(&batch(fifth + 1)).is_err());
    }

    #[test]
    fn a_batch_inside_both_budgets_passes() {
        assert!(check_budget(&[
            ("a.bin".to_string(), 9 * 1024 * 1024),
            ("b.bin".to_string(), 9 * 1024 * 1024),
        ])
        .is_ok());
    }

    #[test]
    fn the_root_script_asks_tmux_then_git() {
        let s = root_script("demo");
        assert!(s.contains("display-message"));
        assert!(s.contains("rev-parse --show-toplevel"));
        // The pane target is quoted exactly once, by the canonical quoter.
        assert!(s.contains("'=demo:'"));
    }

    #[test]
    fn a_hostile_session_name_cannot_escape_the_script() {
        let s = root_script("a'; rm -rf /; echo '");
        // If `quote` were skipped, the target would be embedded raw and this
        // exact substring — the name's `'` sitting bare next to `; rm -rf` —
        // would appear verbatim. Quoting inserts `'\''` in between, so it
        // never does; this fails the moment quoting is dropped.
        assert!(!s.contains("=a'; rm -rf"));
        assert!(s.contains(r"'\''"));
    }

    #[test]
    fn staging_creates_the_dir_and_excludes_it_untracked() {
        let s = stage_script("/w/proj");
        assert!(s.contains("mkdir -p '/w/proj/.claude-fleet-attachments'"));
        // --git-common-dir, never a path built from `<root>/.git/info/`: in a
        // linked worktree `.git` is a FILE, so that path cannot be created,
        // and git reads a linked worktree's excludes from the common dir
        // regardless — never the tracked .gitignore either, so an attachment
        // never shows up as a change the user has to explain.
        assert!(s.contains("--git-common-dir"));
        assert!(!s.contains(".gitignore"));
        // Idempotent: appending twice must not double the line.
        assert!(s.contains("grep -qxF"));
    }

    /// The case FINDING 1 broke: `stage_script` run against a REAL linked
    /// worktree, where `.git` is a file (`gitdir: …`), not a directory — the
    /// normal case for sessions in this repo. Exercises the actual script
    /// through `bash`, not just its source text.
    #[test]
    fn stage_script_works_against_a_real_linked_worktree() {
        if !bash_and_git_available() {
            eprintln!(
                "SKIP stage_script_works_against_a_real_linked_worktree: bash or git not on PATH"
            );
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        run(&main, &["git", "init", "-q"]);
        run(&main, &["git", "config", "user.email", "t@example.com"]);
        run(&main, &["git", "config", "user.name", "t"]);
        std::fs::write(main.join("f.txt"), "x").unwrap();
        run(&main, &["git", "add", "f.txt"]);
        run(&main, &["git", "commit", "-q", "-m", "init"]);

        let wt = tmp.path().join("wt");
        run(
            &main,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );

        // Confirm the premise the finding relies on: a linked worktree's
        // `.git` is a file, not a directory.
        assert!(
            wt.join(".git").is_file(),
            "test setup: expected a linked worktree with a `.git` FILE"
        );

        let script = stage_script(wt.to_str().unwrap());
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("spawn bash");
        assert!(
            out.status.success(),
            "stage_script failed against a linked worktree: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        assert!(
            wt.join(ATTACH_DIR).is_dir(),
            "the attachment dir must be created inside the worktree"
        );
        // The exclude line must land in the COMMON dir's info/exclude (the
        // main checkout's `.git/info/exclude`) — never beside the worktree's
        // `.git` file, which git would not read anyway.
        let excl = std::fs::read_to_string(main.join(".git/info/exclude"))
            .expect("the common dir's info/exclude must exist");
        assert!(excl.contains(&format!("/{ATTACH_DIR}/")));
    }

    fn bash_and_git_available() -> bool {
        std::process::Command::new("bash")
            .args(["-c", "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            && std::process::Command::new("git")
                .args(["--version"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }

    fn run(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("spawn {args:?}: {e}"));
        assert!(
            out.status.success(),
            "command failed: {args:?}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
