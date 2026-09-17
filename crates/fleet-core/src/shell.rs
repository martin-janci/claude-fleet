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

/// Property tests for [`quote`]. Three properties, each over ~256 generated
/// strings biased towards shell metacharacters, control characters (NUL
/// excluded — it cannot cross an argv boundary), quotes, backslashes and
/// arbitrary Unicode:
///
/// 1. round-trip: `bash -c "printf %s <quote(s)>"` prints exactly `s`;
/// 2. structural: the output is well-formed under the single-quote scheme —
///    every `'` is either a segment delimiter or part of a `\'` escape, and
///    decoding it yields `s`;
/// 3. nesting: `quote(quote(s))` decoded twice yields `s`.
#[cfg(test)]
mod prop_tests {
    use super::quote;
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};

    const CASES: u32 = 256;

    /// Strings weighted towards the characters a quoting bug would trip on.
    fn shellish_string() -> impl Strategy<Value = String> {
        let special = prop::sample::select(vec![
            '\'', '\\', '$', '`', '"', '\n', '\r', '\t', ' ', ';', '&', '|', '*', '?', '!', '#',
            '~', '{', '}', '(', ')', '<', '>', '=', '%', '-',
        ]);
        // Every control character except NUL (0x01..=0x1F plus DEL).
        let control = prop_oneof![
            (1u32..0x20).prop_map(|c| char::from_u32(c).unwrap()),
            Just('\u{7f}'),
        ];
        let unicode = any::<char>().prop_filter("NUL cannot cross argv", |c| *c != '\0');
        prop::collection::vec(prop_oneof![4 => special, 1 => control, 2 => unicode], 0..48)
            .prop_map(|chars| chars.into_iter().collect())
    }

    fn config() -> Config {
        Config {
            cases: CASES,
            ..Config::default()
        }
    }

    /// Decode a string under the scheme [`quote`] emits: a sequence of
    /// `'…'` segments (no `'` inside) and `\'` escapes between them, nothing
    /// else. Returns `None` for anything malformed — a bare character outside
    /// a segment, an unterminated segment, or a stray backslash.
    fn unquote(q: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = q.chars();
        while let Some(c) = chars.next() {
            match c {
                '\'' => loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(inner) => out.push(inner),
                        None => return None,
                    }
                },
                '\\' => match chars.next() {
                    Some('\'') => out.push('\''),
                    _ => return None,
                },
                _ => return None,
            }
        }
        Some(out)
    }

    fn bash_available() -> bool {
        std::process::Command::new("bash")
            .args(["-c", "true"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn quote_round_trips_through_bash_for_arbitrary_strings() {
        if !bash_available() {
            eprintln!(
                "SKIP quote_round_trips_through_bash_for_arbitrary_strings: bash not on PATH"
            );
            return;
        }
        let mut runner = TestRunner::new(config());
        runner
            .run(&shellish_string(), |s| {
                let cmd = format!("printf %s {}", quote(&s));
                let out = std::process::Command::new("bash")
                    .args(["-c", &cmd])
                    .output()
                    .expect("spawn bash");
                prop_assert!(
                    out.status.success(),
                    "bash failed for {:?}: {}",
                    s,
                    String::from_utf8_lossy(&out.stderr)
                );
                prop_assert_eq!(out.stdout.as_slice(), s.as_bytes(), "mismatch for {:?}", s);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn quote_output_is_well_formed_under_the_single_quote_scheme() {
        let mut runner = TestRunner::new(config());
        runner
            .run(&shellish_string(), |s| {
                let q = quote(&s);
                prop_assert!(q.starts_with('\''), "must open with a quote: {:?}", q);
                prop_assert!(q.ends_with('\''), "must close with a quote: {:?}", q);
                // A quote that is neither a delimiter nor `\'`-escaped makes
                // `unquote` reject the string, so `Some(s)` is the structural
                // guarantee.
                prop_assert_eq!(unquote(&q), Some(s.clone()), "malformed: {:?}", q);
                // Every embedded quote becomes the 4-char `'\''` sequence
                // (3 chars of overhead) plus the 2 wrapper quotes.
                let quotes = s.chars().filter(|c| *c == '\'').count();
                prop_assert_eq!(q.chars().count(), s.chars().count() + 2 + 3 * quotes);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn quote_nests_and_unquotes_twice_to_the_original() {
        let mut runner = TestRunner::new(config());
        runner
            .run(&shellish_string(), |s| {
                let once = quote(&s);
                let twice = quote(&once);
                let inner = unquote(&twice);
                prop_assert_eq!(inner.as_deref(), Some(once.as_str()));
                let back = inner.as_deref().and_then(unquote);
                prop_assert_eq!(back, Some(s));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn unquote_rejects_bare_and_unterminated_input() {
        // Sanity-check the decoder the properties rely on.
        assert_eq!(unquote("''"), Some(String::new()));
        assert_eq!(unquote("'a'\\''b'"), Some("a'b".to_string()));
        assert_eq!(unquote("abc"), None);
        assert_eq!(unquote("'abc"), None);
        assert_eq!(unquote("'a'b'"), None);
        assert_eq!(unquote("'a'\\x"), None);
    }
}
