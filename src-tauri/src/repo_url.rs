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
    if !is_component(owner) || !is_component(repo) {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// A safe single path component: non-empty, no `/`, not `.`/`..`, and only
/// characters GitHub allows in an owner or repo name.
fn is_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
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
        ] {
            assert_eq!(parse_repo_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn clone_url_is_always_github_ssh() {
        assert_eq!(
            clone_url_for("martin-janci", "claude-fleet"),
            "git@github.com:martin-janci/claude-fleet.git"
        );
    }
}
