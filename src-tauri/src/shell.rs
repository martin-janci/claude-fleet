//! Shared shell-quoting helper.
//!
//! Every value interpolated into a remote `bash -lc` script or a local
//! `bash -c` command MUST go through [`quote`]. Previously this logic was
//! copy-pasted into four separate functions across `tmux.rs`, `pty.rs`, and
//! `commands/sessions.rs`; a single audited implementation removes the risk
//! of one copy drifting or a call site forgetting to quote.

/// Conservative POSIX single-quote escape: wraps the string in `'...'` and
/// replaces each embedded `'` with the canonical `'\''` sequence. The result
/// is a single shell word with every metacharacter (`;`, `$`, backticks,
/// spaces, newlines, …) rendered inert.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn wraps_basic_strings() {
        assert_eq!(quote("foo"), "'foo'");
        assert_eq!(quote("dev-foo"), "'dev-foo'");
        assert_eq!(quote("/tmp/with space"), "'/tmp/with space'");
    }

    #[test]
    fn escapes_embedded_single_quotes() {
        assert_eq!(quote("don't"), "'don'\\''t'");
    }

    #[test]
    fn neutralises_shell_metacharacters() {
        // The whole point: a hostile value stays a single inert word.
        assert_eq!(quote("a; rm -rf /"), "'a; rm -rf /'");
        assert_eq!(quote("$(evil)"), "'$(evil)'");
        assert_eq!(quote("`evil`"), "'`evil`'");
    }

    #[test]
    fn quote_round_trips_through_bash() {
        for raw in [
            "plain",
            "with space",
            "single'quote",
            "double\"quote",
            "new\nline",
            "$(cmd)",
            "`backtick`",
            "semi;colon",
            "a && b",
            "glob*",
            "tab\tchar",
            "emoji 🦀",
        ] {
            let cmd = format!("printf %s {}", quote(raw));
            let out = std::process::Command::new("bash")
                .args(["-c", &cmd])
                .output()
                .unwrap();
            assert!(out.status.success(), "bash failed for {raw:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                raw,
                "mismatch for {raw:?}"
            );
        }
    }
}
