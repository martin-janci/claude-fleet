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
pub fn stage_script(root: &str) -> String {
    let dir = quote(&format!("{root}/{ATTACH_DIR}"));
    let excl = quote(&format!("{root}/.git/info/exclude"));
    let line = quote(&format!("/{ATTACH_DIR}/"));
    format!(
        "mkdir -p {dir} && \
         {{ [ -d {excl} ] || mkdir -p \"$(dirname {excl})\"; }} && \
         {{ grep -qxF {line} {excl} 2>/dev/null || printf '%s\\n' {line} >> {excl}; }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!s.contains("rm -rf /;\n"));
        assert!(s.contains(r"'\''"));
    }

    #[test]
    fn staging_creates_the_dir_and_excludes_it_untracked() {
        let s = stage_script("/w/proj");
        assert!(s.contains("mkdir -p '/w/proj/.claude-fleet-attachments'"));
        // .git/info/exclude, never the tracked .gitignore: an attachment must
        // not show up as a change the user has to explain.
        assert!(s.contains(".git/info/exclude"));
        assert!(!s.contains(".gitignore"));
        // Idempotent: appending twice must not double the line.
        assert!(s.contains("grep -qxF"));
    }
}
