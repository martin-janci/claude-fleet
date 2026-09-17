//! Derive a human-readable session label from a branch / worktree name.
//!
//! The dialog convention is `dev-<owner>-<repo>--<slug>` (or `-<slug>-term`
//! for shell sessions). The raw form looks like
//! `dev-martin-janci-claude-fleet--friendly-name` in the sidebar, which is
//! both long and noisy. This module strips the prefix, normalises the
//! separator, sentence-cases the result, and caps it to the same 80-char
//! limit the `friendly_name` validator enforces — so the output is always a
//! legal value to persist to `sessions.friendly_name`.
//!
//! Used in two places:
//!  1. `service::sessions::new_session` — when the user doesn't supply an
//!     explicit friendly name, derive one from the branch they picked /
//!     created so the sidebar is never ugly.
//!  2. `Store::backfill_friendly_names` — one-shot startup pass that gives
//!     every pre-existing NULL row a deterministic name.

/// Convert a branch / worktree name into a sidebar-friendly label.
///
/// Strips a `dev-<owner>-<repo>--` (or `-`) prefix and a trailing `-term`
/// shell-suffix, turns `-`/`_` into spaces, sentence-cases the result, and
/// truncates to 80 Unicode chars. Falls back to a humanised `repo` when the
/// branch is the bare project base (e.g. `dev-martin-janci-claude-fleet`).
pub fn humanize_branch(branch: &str, owner: &str, repo: &str) -> String {
    let trimmed = branch.trim();
    // Strip the `-term` shell-session suffix BEFORE prefix removal: otherwise
    // `dev-<owner>-<repo>-term` (a shell at the project root) gets matched by
    // the single-dash prefix and reduces to just `term` — losing the project
    // base entirely. Stripping first means the bare-base path takes over and
    // we fall back to the repo name.
    let de_termed = trimmed.strip_suffix("-term").unwrap_or(trimmed);
    let stripped = strip_project_prefix(de_termed, owner, repo);
    let core = stripped.trim_matches('-').trim();
    let base = if core.is_empty() { repo } else { core };
    let spaced: String = base
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c })
        .collect();
    let cleaned = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let sentence_cased = sentence_case(&cleaned);
    truncate_chars(&sentence_cased, 80)
}

/// Strip the `dev-<owner>-<repo>--` or `dev-<owner>-<repo>-` prefix. Returns
/// the remainder, or the original input if no prefix matched. An exact match
/// of `dev-<owner>-<repo>` (the bare base used by the main worktree) returns
/// an empty slice so the caller can fall back to the repo name.
fn strip_project_prefix<'a>(branch: &'a str, owner: &str, repo: &str) -> &'a str {
    let bare = format!("dev-{owner}-{repo}");
    if branch == bare {
        return "";
    }
    let double = format!("{bare}--");
    if let Some(rest) = branch.strip_prefix(&double) {
        return rest;
    }
    let single = format!("{bare}-");
    if let Some(rest) = branch.strip_prefix(&single) {
        return rest;
    }
    branch
}

/// First Unicode char upper, remainder lower. Empty input returns empty.
fn sentence_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let head: String = first.to_uppercase().collect();
            let tail: String = chars.as_str().to_lowercase();
            head + &tail
        }
    }
}

/// Truncate a string to at most `max` Unicode chars (not bytes). Matches the
/// `validate::friendly_name` length rule so a humanised label is always a
/// legal value to write.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "martin-janci";
    const REPO: &str = "claude-fleet";

    #[test]
    fn strips_double_dash_prefix_and_sentence_cases() {
        assert_eq!(
            humanize_branch("dev-martin-janci-claude-fleet--friendly-name", OWNER, REPO),
            "Friendly name"
        );
    }

    #[test]
    fn strips_single_dash_prefix() {
        // `-term` shell session at the project root, no feature suffix.
        assert_eq!(
            humanize_branch("dev-martin-janci-claude-fleet-term", OWNER, REPO),
            "Claude fleet"
        );
    }

    #[test]
    fn strips_trailing_term_suffix() {
        assert_eq!(
            humanize_branch("dev-martin-janci-claude-fleet--polishing-term", OWNER, REPO),
            "Polishing"
        );
    }

    #[test]
    fn bare_base_falls_back_to_repo_name() {
        assert_eq!(
            humanize_branch("dev-martin-janci-claude-fleet", OWNER, REPO),
            "Claude fleet"
        );
    }

    #[test]
    fn unknown_prefix_passes_through_with_normalisation() {
        // No `dev-owner-repo` prefix — should still humanise dashes/case.
        assert_eq!(
            humanize_branch("feature/SOMETHING_loud", OWNER, REPO),
            "Feature/something loud"
        );
    }

    #[test]
    fn collapses_repeated_separators_and_whitespace() {
        assert_eq!(
            humanize_branch(
                "dev-martin-janci-claude-fleet--fix__double___underscore",
                OWNER,
                REPO
            ),
            "Fix double underscore"
        );
    }

    #[test]
    fn empty_branch_falls_back_to_repo() {
        assert_eq!(humanize_branch("", OWNER, REPO), "Claude fleet");
    }

    #[test]
    fn truncates_to_eighty_chars() {
        let long = format!("dev-{OWNER}-{REPO}--{}", "x".repeat(200));
        let out = humanize_branch(&long, OWNER, REPO);
        assert_eq!(out.chars().count(), 80);
    }

    #[test]
    fn handles_unicode_safely() {
        // Multi-byte chars must count as ONE for truncation, and lowercasing
        // must not panic on combining sequences.
        let branch = format!("dev-{OWNER}-{REPO}--Příliš-žluťoučký-kůň");
        let out = humanize_branch(&branch, OWNER, REPO);
        assert_eq!(out, "Příliš žluťoučký kůň");
    }

    #[test]
    fn similar_owner_repo_with_other_branch_still_strips() {
        // Branch belongs to a different repo's prefix scheme — should leave
        // it alone (no false-positive strip).
        assert_eq!(
            humanize_branch("dev-other-user-other-repo--foo", OWNER, REPO),
            "Dev other user other repo foo"
        );
    }
}
