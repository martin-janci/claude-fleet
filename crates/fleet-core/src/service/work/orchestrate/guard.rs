//! The worker guard (orchestration §7.2): a mission's workers run without
//! permission prompts, so a step the design keeps for a person — merging a
//! PR, marking it ready, pushing to the default branch, writing to a
//! tracker (§7.3) — is stopped where the worker would run it, in a
//! `PreToolUse(Bash)` hook, not by a flag in the database.
//!
//! The installed hook is a shell prefilter
//! (`hooks_install::guard_command`): only a command that mentions one of
//! these tools reaches the hub, which answers `deny` here for a mission's
//! worker and nothing for any other session. A hub that does not answer
//! lets the command run (the hook is `|| true`): branch protection on the
//! remote stays the backstop, as the design says.

/// Branches a worker never pushes to.
pub const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];

/// Tracker command-line tools a worker never runs (writes to Jira / Asana
/// / Linear are a person's, C3).
pub const TRACKER_CLIS: [&str; 4] = ["jira", "acli", "asana", "linear"];

/// Words that may stand before the real command in a segment.
const PREFIXES: [&str; 6] = ["sudo", "command", "env", "exec", "nohup", "time"];

/// PURE: why `command` is a person's step, or `None` when a worker may run
/// it. `branch` is the session's current branch, for a bare `git push`.
pub fn deny_reason(command: &str, branch: Option<&str>) -> Option<&'static str> {
    segments(command).find_map(|words| segment_reason(&words, branch))
}

/// The command's simple commands, each as its words with quotes dropped.
fn segments(command: &str) -> impl Iterator<Item = Vec<String>> + '_ {
    command
        .split(['\n', ';', '|', '&', '(', ')', '`'])
        .map(|seg| {
            seg.split_whitespace()
                .map(|w| w.trim_matches(['\'', '"']).to_string())
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|w| !w.is_empty())
}

fn segment_reason(words: &[String], branch: Option<&str>) -> Option<&'static str> {
    // `FOO=1 sudo git push` → `git push`.
    let start = words.iter().position(|w| {
        !(w.contains('=') && !w.starts_with('-')) && !PREFIXES.contains(&w.as_str())
    })?;
    let words = &words[start..];
    let tool = words[0].rsplit('/').next().unwrap_or(&words[0]);
    let rest = &words[1..];
    match tool {
        "gh" => gh_reason(rest),
        "git" => git_reason(rest, branch),
        t if TRACKER_CLIS.contains(&t) => {
            Some("a tracker write is a person's step (orchestration §7.3)")
        }
        _ => None,
    }
}

fn gh_reason(args: &[String]) -> Option<&'static str> {
    let mut pos = args.iter().filter(|a| !a.starts_with('-'));
    match (
        pos.next().map(String::as_str),
        pos.next().map(String::as_str),
    ) {
        (Some("pr"), Some("merge")) => Some("merging a PR is a person's step (orchestration §7.3)"),
        (Some("pr"), Some("ready")) => {
            Some("marking a PR ready is a person's step (orchestration §7.3)")
        }
        _ => None,
    }
}

fn git_reason(args: &[String], branch: Option<&str>) -> Option<&'static str> {
    // Global options before the subcommand: `-C dir`, `-c k=v` take a value.
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        i += if matches!(args[i].as_str(), "-C" | "-c" | "--git-dir" | "--work-tree") {
            2
        } else {
            1
        };
    }
    if args.get(i).map(String::as_str) != Some("push") {
        return None;
    }
    const WHY: &str = "pushing to the default branch is a person's step (orchestration §7.3)";
    let push = &args[i + 1..];
    if push.iter().any(|a| a == "--all" || a == "--mirror") {
        return Some(WHY);
    }
    // Options that take a value as the next word.
    let mut positional = Vec::new();
    let mut j = 0;
    while j < push.len() {
        let a = &push[j];
        if matches!(
            a.as_str(),
            "-o" | "--push-option" | "--repo" | "--receive-pack" | "--exec"
        ) {
            j += 2;
            continue;
        }
        if !a.starts_with('-') {
            positional.push(a.as_str());
        }
        j += 1;
    }
    // `git push [remote [refspec…]]`: no refspec pushes the current branch.
    if positional.len() < 2 {
        return branch.filter(|b| is_default(b)).map(|_| WHY);
    }
    positional[1..]
        .iter()
        .any(|spec| {
            let dst = spec.rsplit(':').next().unwrap_or(spec);
            let dst = dst.trim_start_matches('+');
            let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
            is_default(dst) || (dst == "HEAD" && branch.is_some_and(is_default))
        })
        .then_some(WHY)
}

fn is_default(b: &str) -> bool {
    DEFAULT_BRANCHES.contains(&b)
}

#[cfg(test)]
mod tests;
