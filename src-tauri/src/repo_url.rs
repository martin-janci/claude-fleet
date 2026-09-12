//! Parsing the repo identifiers the Add-project dialog accepts. GitHub only:
//! the clone URL is always normalised to GitHub SSH, matching
//! `ensure_remote_project`.

/// `(owner, repo)` from `owner/repo`, an https URL, or an SSH URL. `None`
/// for anything else — including a host other than github.com and any
/// component that is not a safe path component, so a parsed pair is always
/// safe to interpolate into a path.
#[allow(dead_code)] // Task 2 (add_project) wires this in.
pub fn parse_repo_url(input: &str) -> Option<(String, String)> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let rest = if let Some(r) = s.strip_prefix("git@github.com:") {
        r
    } else if let Some(r) = s.strip_prefix("ssh://git@github.com/") {
        r
    } else if let Some(r) = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
    {
        r
    } else if s.contains("://") || s.contains('@') {
        // Some other host, or an SSH form we do not accept.
        return None;
    } else {
        s
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    // GitHub's own limits: an owner (user/org) name is capped at 39
    // characters, a repo name at 100.
    if !is_component(owner, 39) || !is_component(repo, 100) {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// A safe single path component. Beyond being non-empty and free of `/`,
/// this rejects a component made entirely of dots (`.`, `..`, `...`, ...,
/// which would otherwise let a segment resolve to the current or parent
/// directory), the literal component `.git` case-insensitively (this
/// codebase uses `<path>/.git` as its "is this a project" marker — see the
/// clone script in `service/sessions/lifecycle.rs` — so a project root
/// named `.git` would make the directory holding it resolve as a git repo
/// itself), and anything longer than `max_len`. The character-class check
/// below is a conservative *subset* of the characters GitHub allows in an
/// owner or repo name — it exists to guarantee the result is safe to use as
/// a path component, not to guarantee the name is valid or exists on
/// GitHub.
fn is_component(s: &str, max_len: usize) -> bool {
    !s.is_empty()
        && s.len() <= max_len
        && !s.chars().all(|c| c == '.')
        && !s.eq_ignore_ascii_case(".git")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The URL fleet clones with, for a pair from [`parse_repo_url`].
#[allow(dead_code)] // Task 2 (add_project) wires this in.
pub fn clone_url_for(owner: &str, repo: &str) -> String {
    format!("git@github.com:{owner}/{repo}.git")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_accepted_github_form() {
        for input in [
            "martin-janci/claude-fleet",
            "https://github.com/martin-janci/claude-fleet",
            "https://github.com/martin-janci/claude-fleet.git",
            "http://github.com/martin-janci/claude-fleet",
            "https://github.com/martin-janci/claude-fleet/",
            "git@github.com:martin-janci/claude-fleet.git",
            "git@github.com:martin-janci/claude-fleet",
            "ssh://git@github.com/martin-janci/claude-fleet.git",
            "  martin-janci/claude-fleet  ",
        ] {
            assert_eq!(
                parse_repo_url(input),
                Some(("martin-janci".to_string(), "claude-fleet".to_string())),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_what_is_not_a_github_repo() {
        for bad in [
            "",
            "   ",
            "claude-fleet",
            "martin-janci/",
            "/claude-fleet",
            "martin-janci/claude-fleet/extra",
            "https://gitlab.com/o/r",
            "https://github.com/",
            "https://github.com/only-owner",
            "../etc/passwd",
            "martin-janci/../escape",
            "o/r; rm -rf /",
            // A `.git` component would make the parent directory resolve as
            // a git repo (`<path>/.git` is this codebase's project marker).
            "owner/.git.git",
            ".git/repo",
            "git@github.com:o/.git.git",
            // All-dots components, beyond plain `.`/`..`.
            ".../repo",
            "owner/...",
            // GitHub's length caps: 39 for owner, 100 for repo.
            &format!("{}/repo", "a".repeat(40)),
            &format!("owner/{}", "a".repeat(101)),
            // Non-ASCII, control characters, and percent-encoded traversal.
            "о/repo", // Cyrillic о (U+043E), not ASCII 'o'
            "owner/re\0po",
            "owner/re\npo",
            "%2e%2e/repo",
            // URL edge cases that must not be mistaken for github.com.
            "https://github.com/o/r?x=1",
            "https://user:pw@github.com/o/r",
            "https://github.com.evil.com/o/r",
            "git@github.com:/o/r",
            "https://github.com:443/o/r",
        ] {
            assert_eq!(parse_repo_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn accepts_legitimate_dotted_components() {
        // A leading dot alone is fine — only all-dots and the exact `.git`
        // component are rejected.
        assert_eq!(
            parse_repo_url("owner/.github"),
            Some(("owner".to_string(), ".github".to_string()))
        );
        assert_eq!(
            parse_repo_url(".hidden/repo"),
            Some((".hidden".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_repo_url("owner/repo.name"),
            Some(("owner".to_string(), "repo.name".to_string()))
        );
    }

    #[test]
    fn clone_url_is_always_github_ssh() {
        assert_eq!(
            clone_url_for("martin-janci", "claude-fleet"),
            "git@github.com:martin-janci/claude-fleet.git"
        );
    }

    #[test]
    fn clone_url_round_trips_through_parse() {
        for (owner, repo) in [
            ("martin-janci", "claude-fleet"),
            ("owner", ".github"),
            ("owner", "repo.name"),
            ("a", "b"),
        ] {
            assert_eq!(
                parse_repo_url(&clone_url_for(owner, repo)),
                Some((owner.to_string(), repo.to_string())),
                "{owner}/{repo}"
            );
        }
    }
}
