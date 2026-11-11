//! The worker guard (orchestration §7.2): a mission's workers run without
//! permission prompts, so a step the design keeps for a person — merging a
//! PR, marking it ready, pushing to the default branch, writing to a
//! tracker (§7.3) — is stopped where the worker would run it, in a
//! `PreToolUse` hook, not by a flag in the database: `Bash` for the
//! command-line tools ([`deny_reason`]), `mcp__.*` for an MCP server's tools
//! ([`mcp_deny_reason`]: a tracker or GitHub server's writes, and a PR's
//! merge or ready on any server).
//!
//! The installed hooks are shell prefilters
//! (`hooks_install::guard_command`, `hooks_install::mcp_guard_command`):
//! only a command that mentions one of these tools, or an MCP tool that is
//! not plainly a read, reaches the hub, which answers `deny` here for a
//! mission's worker and nothing for any other session. A hub that does not answer
//! lets the command run (the hook is `|| true`): branch protection on the
//! remote stays the backstop, as the design says.

/// Branches a worker never pushes to.
pub const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];

/// Tracker command-line tools a worker writes with only through a person
/// (writes to Jira / Asana / Linear are a person's, C3). Reading is the
/// worker's own: a ticket, its comments, a search ([`READ_VERBS`]).
pub const TRACKER_CLIS: [&str; 4] = ["jira", "acli", "asana", "linear"];

/// Subcommands that only read a tracker (`jira issue view ABC-1`, `acli jira
/// workitem search --jql …`, `linear issue list`). A tracker command passes
/// when its verb, the first of its leading words that is not one of
/// [`NOUNS`], is one of these; anything else, a subcommand this list does
/// not know included, stays a person's step.
pub const READ_VERBS: [&str; 9] = [
    "list",
    "ls",
    "view",
    "show",
    "get",
    "search",
    "me",
    "serverinfo",
    "version",
];

/// What a tracker command acts on, named before its verb (`jira issue
/// view`, `acli jira workitem search`).
const NOUNS: [&str; 18] = [
    "jira",
    "issue",
    "issues",
    "workitem",
    "workitems",
    "project",
    "projects",
    "sprint",
    "sprints",
    "board",
    "boards",
    "epic",
    "epics",
    "task",
    "tasks",
    "ticket",
    "tickets",
    "team",
];

/// How many leading words name a tracker command's subcommand (`acli jira
/// workitem view`: three).
const VERB_WORDS: usize = 3;

/// Words that may stand before the real command in a segment.
const PREFIXES: [&str; 6] = ["sudo", "command", "env", "exec", "nohup", "time"];

/// Shells whose `-c` string is itself a command (`bash -c "gh pr merge 1"`).
const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "ksh"];

/// `gh`'s options that take a value as the next word, before or after the
/// subcommand (`gh -R acme/app pr merge 41`).
const GH_VALUE_FLAGS: [&str; 3] = ["-R", "--repo", "--hostname"];

/// GitHub GraphQL mutations that merge or ready a PR (or merge a branch),
/// sent with `gh api graphql`.
const GH_MUTATIONS: [&str; 4] = [
    "mergePullRequest",
    "markPullRequestReadyForReview",
    "enablePullRequestAutoMerge",
    "mergeBranch",
];

const WHY_MERGE: &str = "merging a PR is a person's step (orchestration §7.3)";
const WHY_READY: &str = "marking a PR ready is a person's step (orchestration §7.3)";
const WHY_PUSH: &str = "pushing to the default branch is a person's step (orchestration §7.3)";
const WHY_TRACKER: &str = "a tracker write is a person's step (orchestration §7.3)";
/// A bare `git push` (or `git push <remote> HEAD`) in a command that also
/// changes folder or branch: the branch it pushes is not the one fleet
/// knows, so it may be the default branch.
const WHY_BARE_PUSH: &str = "a bare `git push` after changing folder or branch may push the \
     default branch, which is a person's step (orchestration §7.3): name the branch, as in \
     `git push origin <branch>`";

/// PURE: why `command` is a person's step, or `None` when a worker may run
/// it. `branch` is the session's current branch, for a bare `git push`.
pub fn deny_reason(command: &str, branch: Option<&str>) -> Option<&'static str> {
    // A GraphQL mutation's text may span lines and parentheses, which split
    // segments: look for it in the whole command.
    if GH_MUTATIONS.iter().any(|m| command.contains(m))
        && segments(command).any(|w| is_gh_api_graphql(&w))
    {
        return Some(WHY_MERGE);
    }
    // A `cd`, `git -C`, `git checkout` or `git switch` anywhere in the
    // command leaves the branch a bare push takes unknown.
    let moved = segments(command).any(|w| moves(&w));
    let branch = if moved {
        Branch::Unknown
    } else {
        Branch::Known(branch)
    };
    segments(command).find_map(|words| segment_reason(&words, branch))
}

/// The branch a bare `git push` pushes.
#[derive(Clone, Copy)]
enum Branch<'a> {
    /// The session's branch (`None`: not known, and not guessed).
    Known(Option<&'a str>),
    /// The command changes folder or branch first.
    Unknown,
}

/// The words of a segment past its prefixes (`FOO=1 sudo git push` → `git
/// push`) and any shell, `eval`, `xargs` or `timeout` wrapping
/// (`bash -c "gh pr merge 1"` → `gh pr merge 1`), with the tool's base name.
fn command_words(words: &[String]) -> Option<(&str, &[String])> {
    let start = words.iter().position(|w| {
        !(w.contains('=') && !w.starts_with('-')) && !PREFIXES.contains(&w.as_str())
    })?;
    let words = &words[start..];
    let tool = words[0].rsplit('/').next().unwrap_or(&words[0]);
    let rest = &words[1..];
    let inner = match tool {
        // `bash -c CMD`, `bash -lc CMD`: the command after the `-…c` flag.
        t if SHELLS.contains(&t) => {
            let c = rest
                .iter()
                .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))?;
            &rest[c + 1..]
        }
        "eval" => rest,
        // `xargs -I{} -n 1 gh pr merge {}`: past its options.
        "xargs" => {
            let mut i = 0;
            while i < rest.len() && rest[i].starts_with('-') {
                let takes = matches!(
                    rest[i].as_str(),
                    "-I" | "-n" | "-P" | "-L" | "-d" | "-E" | "-s" | "-a"
                );
                i += if takes { 2 } else { 1 };
            }
            rest.get(i..)?
        }
        // `timeout -s KILL 30 gh pr merge 1`: past its options and duration.
        "timeout" => {
            let mut i = 0;
            while i < rest.len() && rest[i].starts_with('-') {
                i += if matches!(rest[i].as_str(), "-s" | "-k" | "--signal" | "--kill-after") {
                    2
                } else {
                    1
                };
            }
            rest.get(i + 1..)?
        }
        _ => return Some((tool, rest)),
    };
    command_words(inner)
}

/// Whether a segment changes folder or branch: `cd`, `pushd`, `git -C`,
/// `git checkout`, `git switch`.
fn moves(words: &[String]) -> bool {
    let Some((tool, rest)) = command_words(words) else {
        return false;
    };
    match tool {
        "cd" | "pushd" => true,
        "git" => {
            let mut i = 0;
            while i < rest.len() && rest[i].starts_with('-') {
                if matches!(rest[i].as_str(), "-C" | "--git-dir" | "--work-tree")
                    || rest[i].starts_with("--git-dir=")
                    || rest[i].starts_with("--work-tree=")
                {
                    return true;
                }
                i += if rest[i] == "-c" { 2 } else { 1 };
            }
            matches!(rest.get(i).map(String::as_str), Some("checkout" | "switch"))
        }
        _ => false,
    }
}

/// Whether a segment is `gh api graphql …`.
fn is_gh_api_graphql(words: &[String]) -> bool {
    match command_words(words) {
        Some(("gh", rest)) => {
            let pos = gh_positionals(rest);
            pos.first().map(|s| s.as_str()) == Some("api")
                && pos.get(1).map(|s| s.as_str()) == Some("graphql")
        }
        _ => false,
    }
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

fn segment_reason(words: &[String], branch: Branch) -> Option<&'static str> {
    let (tool, rest) = command_words(words)?;
    match tool {
        "gh" => gh_reason(rest),
        "git" => git_reason(rest, branch),
        t if TRACKER_CLIS.contains(&t) => tracker_reason(rest),
        _ => None,
    }
}

fn tracker_reason(args: &[String]) -> Option<&'static str> {
    // Only the words before the first flag name the subcommand, and its verb
    // is the first that is no noun: a flag's value or a summary
    // (`jira issue create -s "Show login error"`) never reads as one.
    let verb = args
        .iter()
        .take_while(|a| !a.starts_with('-'))
        .take(VERB_WORDS)
        .map(|a| a.to_ascii_lowercase())
        .find(|a| !NOUNS.contains(&a.as_str()));
    let reads = verb.is_some_and(|v| READ_VERBS.contains(&v.as_str()));
    (!reads).then_some(WHY_TRACKER)
}

/// `gh`'s positional words: its options, and the values of
/// [`GH_VALUE_FLAGS`] and of `gh api`'s own options, left out.
fn gh_positionals(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let api = out.first().is_some_and(|p: &&String| p.as_str() == "api");
        let takes = GH_VALUE_FLAGS.contains(&a)
            || (api
                && matches!(
                    a,
                    "-X" | "--method"
                        | "-H"
                        | "--header"
                        | "-f"
                        | "-F"
                        | "--field"
                        | "--raw-field"
                        | "--input"
                        | "-q"
                        | "--jq"
                        | "-t"
                        | "--template"
                        | "-p"
                        | "--preview"
                        | "--cache"
                ));
        if takes {
            i += 2;
            continue;
        }
        if !a.starts_with('-') {
            out.push(&args[i]);
        }
        i += 1;
    }
    out
}

fn gh_reason(args: &[String]) -> Option<&'static str> {
    let pos = gh_positionals(args);
    match (
        pos.first().map(|s| s.as_str()),
        pos.get(1).map(|s| s.as_str()),
    ) {
        (Some("pr"), Some("merge")) => Some(WHY_MERGE),
        (Some("pr"), Some("ready")) => Some(WHY_READY),
        (Some("api"), Some(endpoint)) => gh_api_reason(args, endpoint),
        _ => None,
    }
}

/// `gh api` aimed at a PR's merge: `PUT repos/o/r/pulls/41/merge` (a `GET`
/// of it only reads whether the PR merged). `gh api` sends a `POST` when it
/// has fields and names no method.
fn gh_api_reason(args: &[String], endpoint: &str) -> Option<&'static str> {
    let path = endpoint.split('?').next().unwrap_or(endpoint);
    let path = path.trim_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    let merge =
        parts.len() >= 3 && parts[parts.len() - 1] == "merge" && parts[parts.len() - 3] == "pulls";
    if !merge {
        return None;
    }
    let mut method: Option<String> = None;
    let mut fields = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-X" || a == "--method" {
            method = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(m) = a.strip_prefix("--method=") {
            method = Some(m.to_string());
        } else if let Some(m) = a.strip_prefix("-X").filter(|m| !m.is_empty()) {
            method = Some(m.to_string());
        } else if matches!(a, "-f" | "-F" | "--field" | "--raw-field" | "--input")
            || a.starts_with("--field=")
            || a.starts_with("--raw-field=")
            || a.starts_with("--input=")
        {
            fields = true;
        }
        i += 1;
    }
    let method = method
        .map(|m| m.to_ascii_uppercase())
        .unwrap_or_else(|| if fields { "POST" } else { "GET" }.to_string());
    (method != "GET" && method != "HEAD").then_some(WHY_MERGE)
}

fn git_reason(args: &[String], branch: Branch) -> Option<&'static str> {
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
    const WHY: &str = WHY_PUSH;
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
    // `git push [remote [refspec…]]`: no refspec pushes the current branch,
    // which a command that changed folder or branch first leaves unknown.
    let branch = match branch {
        Branch::Known(b) => b,
        Branch::Unknown => {
            let head = positional.len() < 2 || positional[1..].contains(&"HEAD");
            return head.then_some(WHY_BARE_PUSH).or_else(|| {
                positional[1..]
                    .iter()
                    .any(|spec| is_default(refspec_dst(spec)))
                    .then_some(WHY)
            });
        }
    };
    if positional.len() < 2 {
        return branch.filter(|b| is_default(b)).map(|_| WHY);
    }
    positional[1..]
        .iter()
        .any(|spec| match refspec_dst(spec) {
            "HEAD" => branch.is_some_and(is_default),
            dst => is_default(dst),
        })
        .then_some(WHY)
}

/// The branch a push refspec writes (`+feat:refs/heads/main` → `main`).
fn refspec_dst(spec: &str) -> &str {
    let dst = spec.rsplit(':').next().unwrap_or(spec);
    let dst = dst.trim_start_matches('+');
    dst.strip_prefix("refs/heads/").unwrap_or(dst)
}

/// Fleet's own MCP server: its tools are guarded where they act.
pub const FLEET_MCP_SERVER: &str = "claude-fleet";

/// MCP servers whose writes are a person's step for a mission's worker
/// (matched in the server's name, any case): the trackers', and GitHub's,
/// where a worker pushes and opens its draft PR with `git` and `gh`.
pub const GUARDED_MCP_SERVERS: [&str; 6] = [
    "atlassian",
    "jira",
    "confluence",
    "linear",
    "asana",
    "github",
];

/// The words an MCP tool's name begins with (or, for `…_read` / `…Info`,
/// ends with) when it only reads: `getJiraIssue`, `search_issues`,
/// `pull_request_read`, `atlassianUserInfo`. Anything else on a
/// [`GUARDED_MCP_SERVERS`] server, a name this list does not know included,
/// stays a person's step, as [`READ_VERBS`] does for a tracker's CLI.
pub const MCP_READ_VERBS: [&str; 10] = [
    "get", "list", "search", "view", "show", "read", "fetch", "find", "lookup", "download",
];

/// PURE: why the MCP tool `tool_name` (`mcp__<server>__<tool>`) is a
/// person's step for a mission's worker, or `None` when a worker may call
/// it: a read, fleet's own server, or a server that is neither a tracker's
/// nor GitHub's and the tool merges or readies no PR.
pub fn mcp_deny_reason(tool_name: &str) -> Option<&'static str> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server == FLEET_MCP_SERVER {
        return None;
    }
    let words = name_words(tool);
    let first = words.first().map(String::as_str);
    let last = words.last().map(String::as_str);
    let reads =
        first.is_some_and(|w| MCP_READ_VERBS.contains(&w)) || matches!(last, Some("read" | "info"));
    if reads {
        return None;
    }
    let has = |w: &str| words.iter().any(|x| x == w);
    // A PR's merge or ready, on whatever server offers it
    // (`merge_pull_request`, `enable_pr_auto_merge`, `mark_pr_ready`).
    let pr = has("pr") || (has("pull") && has("request"));
    if pr && has("merge") {
        return Some(WHY_MERGE);
    }
    if pr && has("ready") {
        return Some(WHY_READY);
    }
    let server = server.to_ascii_lowercase();
    if GUARDED_MCP_SERVERS.iter().any(|g| server.contains(g)) {
        return Some(if server.contains("github") {
            "a GitHub write through MCP is a person's step for a mission's worker \
             (orchestration §7.3): push and open a draft PR with git and gh"
        } else {
            WHY_TRACKER
        });
    }
    None
}

/// A tool name's words, lower-cased: split at `_`, `-`, `.` and before
/// each upper-case letter (`mergePullRequest` → merge, pull, request).
fn name_words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if matches!(c, '_' | '-' | '.') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_default(b: &str) -> bool {
    DEFAULT_BRANCHES.contains(&b)
}

#[cfg(test)]
mod tests;
