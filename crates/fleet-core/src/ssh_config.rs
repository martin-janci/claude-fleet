//! Pure parser of OpenSSH client config (~/.ssh/config or any file path).
//!
//! Returns the list of named Host blocks with optional Hostname/User/Port.
//! Wildcards (Host *), the literal `github.com` host, and `*` patterns are
//! intentionally skipped — we only surface real, user-defined machine aliases
//! in the AddHostPicker UI.
//!
//! Resilient to malformed lines; never panics on unknown keywords.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// Parse a slice of `~/.ssh/config` lines into a list of named hosts. We
/// drop wildcards and a small denylist of well-known non-machine aliases.
///
/// As `ssh` reads it: every alias on a `Host` line (`Host a b`) is a host of
/// its own sharing the block's values, a negated one (`!c`) none; the first
/// value given for a host wins, across repeated blocks too; and a `Match`
/// block's values belong to no host here. `Include` is resolved by
/// [`load_user_config`] before this runs.
pub fn parse(input: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    // Indices into `hosts` of the block being read.
    let mut current: Vec<usize> = Vec::new();
    // A byte-order mark is not part of the first keyword: Windows editors
    // (older Notepad) save `.ssh\config` with one, and the first `Host` line
    // would otherwise be lost.
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = match split_kv(line) {
            Some(kv) => kv,
            None => continue,
        };
        let key_l = key.to_ascii_lowercase();
        if key_l == "host" {
            current.clear();
            for alias in value.split_ascii_whitespace() {
                if !is_real_alias(alias) {
                    continue;
                }
                let idx = match hosts.iter().position(|h| h.alias == alias) {
                    Some(i) => i,
                    None => {
                        hosts.push(SshHost {
                            alias: alias.to_string(),
                            hostname: None,
                            user: None,
                            port: None,
                        });
                        hosts.len() - 1
                    }
                };
                if !current.contains(&idx) {
                    current.push(idx);
                }
            }
            continue;
        }
        if key_l == "match" {
            current.clear();
            continue;
        }
        let value = value.trim();
        for &i in &current {
            let host = &mut hosts[i];
            match key_l.as_str() {
                "hostname" if host.hostname.is_none() => host.hostname = Some(value.to_string()),
                "user" if host.user.is_none() => host.user = Some(value.to_string()),
                "port" if host.port.is_none() => host.port = value.parse::<u16>().ok(),
                _ => {}
            }
        }
    }
    hosts
}

/// How deep `Include` may nest, as in `ssh` (a loop stops here too).
const MAX_INCLUDE_DEPTH: usize = 16;

/// A config file's text with every `Include` replaced by the files it names,
/// as `ssh_config(5)` reads them: a relative path is under `ssh_dir` (the
/// user config's `~/.ssh`), `~/` is the home, and a `*` / `?` in the last
/// component matches files there in lexical order. A file that is missing
/// or unreadable adds nothing, like in `ssh`.
fn inline_includes(
    text: &str,
    ssh_dir: &std::path::Path,
    home: Option<&std::path::Path>,
    depth: usize,
) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if let Some((key, value)) = split_kv(line) {
            if key.eq_ignore_ascii_case("include") {
                if depth < MAX_INCLUDE_DEPTH {
                    for pattern in value.split_ascii_whitespace() {
                        for file in include_files(pattern, ssh_dir, home) {
                            if let Ok(t) = std::fs::read_to_string(&file) {
                                out.push_str(&inline_includes(&t, ssh_dir, home, depth + 1));
                                out.push('\n');
                            }
                        }
                    }
                }
                continue;
            }
        }
        out.push_str(raw);
        out.push('\n');
    }
    out
}

/// The files one `Include` pattern names (see [`inline_includes`]).
fn include_files(
    pattern: &str,
    ssh_dir: &std::path::Path,
    home: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let pattern = pattern.trim_matches('"');
    let tilde = pattern
        .strip_prefix("~/")
        .or_else(|| pattern.strip_prefix("~\\").filter(|_| cfg!(windows)));
    let path = match tilde {
        Some(rest) => match home {
            Some(h) => h.join(rest),
            None => return Vec::new(),
        },
        None if std::path::Path::new(pattern).is_absolute() => std::path::PathBuf::from(pattern),
        None => ssh_dir.join(pattern),
    };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| glob_match(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

/// `*` (any run) and `?` (any one character) against a whole file name.
fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Convenience wrapper: load and parse the user's `~/.ssh/config` — every
/// file [`config_paths`] names, the first one's hosts first. A file that does
/// not exist or cannot be read contributes nothing.
pub fn load_user_config() -> Vec<SshHost> {
    let home = dirs_home();
    let parsed = config_paths()
        .into_iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            let ssh_dir = p.parent()?.to_path_buf();
            Some(parse(&inline_includes(&text, &ssh_dir, home.as_deref(), 0)))
        })
        .collect();
    merge_hosts(parsed)
}

/// The config files host discovery reads: `<home>/.ssh/config`, plus, on
/// Windows, the one under `$HOME` when fleet runs an `ssh` other than the
/// system OpenSSH ([`crate::ssh::default_ssh_binary`]): a Cygwin or MSYS2
/// `ssh.exe` reads its own `HOME`, which Cygwin sets to its own home tree,
/// not the Windows profile.
pub fn config_paths() -> Vec<std::path::PathBuf> {
    let Some(home) = dirs_home() else {
        return Vec::new();
    };
    let mut out = vec![home.join(".ssh").join("config")];
    if cfg!(windows) {
        let system_ssh = crate::ssh::windows_openssh() == Some(crate::ssh::default_ssh_binary());
        if let Some(extra) = extra_home_config(
            &home,
            std::env::var_os("HOME").map(std::path::PathBuf::from),
            system_ssh,
        ) {
            out.push(extra);
        }
    }
    out
}

/// `$HOME/.ssh/config` as a second config file, when it applies: a non-system
/// `ssh` is in use, `HOME` is set to an absolute path of this platform
/// (Cygwin converts it for a native child, `C:\cygwin64\home\me`; a
/// POSIX-form value is skipped), and it is not the profile itself.
fn extra_home_config(
    profile: &std::path::Path,
    home_env: Option<std::path::PathBuf>,
    system_ssh: bool,
) -> Option<std::path::PathBuf> {
    if system_ssh {
        return None;
    }
    let home = home_env?;
    if !home.is_absolute() || home == profile {
        return None;
    }
    Some(home.join(".ssh").join("config"))
}

/// Concatenate host lists, keeping the first definition of each alias.
fn merge_hosts(lists: Vec<Vec<SshHost>>) -> Vec<SshHost> {
    let mut out: Vec<SshHost> = Vec::new();
    for h in lists.into_iter().flatten() {
        if !out.iter().any(|o| o.alias == h.alias) {
            out.push(h);
        }
    }
    out
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    // `key value` separated by whitespace OR `key=value`. Either form is
    // legal per ssh_config(5).
    if let Some(eq) = line.find('=') {
        // Make sure '=' actually appears before any whitespace.
        if line[..eq].chars().all(|c| !c.is_whitespace()) {
            return Some((line[..eq].trim(), line[eq + 1..].trim()));
        }
    }
    let mut it = line.splitn(2, char::is_whitespace);
    let key = it.next()?.trim();
    let val = it.next()?.trim();
    if key.is_empty() || val.is_empty() {
        return None;
    }
    Some((key, val))
}

fn is_real_alias(alias: &str) -> bool {
    // Reject anything that isn't a safe machine alias: empty, wildcards
    // (`*`/`?`), an option-like leading `-` (an alias beginning with `-`
    // could be parsed by `ssh` as an option — arbitrary local command
    // execution), whitespace, and non-alias characters. All of these are
    // caught by the shared validator.
    if crate::validate::host_alias(alias).is_err() {
        return false;
    }
    // github.com etc. are valid aliases but used to pin IdentityFile, not
    // machine aliases the user can ssh to for tmux.
    const DENYLIST: &[&str] = &["github.com", "gitlab.com", "bitbucket.org"];
    !DENYLIST.contains(&alias)
}

fn dirs_home() -> Option<std::path::PathBuf> {
    crate::home::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "
Host alpha
    Hostname 10.0.0.5
    User martin
    Port 2222

Host beta
    Hostname beta.lan
";

    #[test]
    fn every_alias_on_a_host_line_is_a_host_and_the_first_value_wins() {
        let cfg = "Host a b !c *.lan\n  HostName shared\n  User one\n  User two\n\
                   Host b\n  HostName later\n  Port 2200\n";
        let hosts = parse(cfg);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["a", "b"]);
        assert_eq!(hosts[0].hostname.as_deref(), Some("shared"));
        assert_eq!(hosts[0].user.as_deref(), Some("one"), "first value wins");
        assert_eq!(
            hosts[1].hostname.as_deref(),
            Some("shared"),
            "first block wins"
        );
        assert_eq!(
            hosts[1].port,
            Some(2200),
            "a later block fills what was unset"
        );
    }

    #[test]
    fn a_match_block_gives_its_values_to_no_host() {
        let cfg =
            "Host a\n  HostName real\nMatch host a exec \"true\"\n  HostName other\n  Port 1\n";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname.as_deref(), Some("real"));
        assert_eq!(hosts[0].port, None);
    }

    #[test]
    fn includes_are_inlined_relative_to_the_ssh_dir_with_globs_and_a_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config.d").join("20-b.conf"),
            "Host b\n  HostName bee\n",
        )
        .unwrap();
        std::fs::write(
            ssh.join("config.d").join("10-a.conf"),
            "\u{feff}Host a\n  HostName ay\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config.d").join("skip.txt"), "Host nope\n").unwrap();
        std::fs::write(ssh.join("extra"), "Host c\n").unwrap();
        // A file that includes itself stops at the depth limit.
        std::fs::write(ssh.join("loop"), "Host d\nInclude loop\n").unwrap();
        let top = "Include config.d/*.conf ~/.ssh/extra missing\nInclude loop\nHost z\n";
        let text = inline_includes(top, &ssh, Some(dir.path()), 0);
        let aliases: Vec<String> = parse(&text).into_iter().map(|h| h.alias).collect();
        assert_eq!(aliases, ["a", "b", "c", "d", "z"]);
    }

    #[test]
    fn globs_match_whole_names() {
        assert!(glob_match("*.conf", "a.conf"));
        assert!(glob_match("a?c*", "abcdef"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("*.conf", "a.conf.bak"));
        assert!(!glob_match("a?", "a"));
    }

    #[test]
    fn a_second_config_adds_only_aliases_the_first_lacks() {
        let first = parse(SIMPLE);
        let second = parse("Host beta\n    Hostname elsewhere\nHost gamma\n    Hostname g\n");
        let merged = merge_hosts(vec![first, second]);
        let aliases: Vec<&str> = merged.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["alpha", "beta", "gamma"]);
        assert_eq!(
            merged[1].hostname.as_deref(),
            Some("beta.lan"),
            "the first wins"
        );
    }

    #[test]
    fn home_config_is_read_only_for_a_non_system_ssh_with_its_own_home() {
        let profile = std::env::temp_dir().join("profile");
        let other = std::env::temp_dir().join("cygwin-home");
        assert_eq!(
            extra_home_config(&profile, Some(other.clone()), false),
            Some(other.join(".ssh").join("config"))
        );
        assert_eq!(extra_home_config(&profile, Some(other), true), None);
        assert_eq!(
            extra_home_config(&profile, Some(profile.clone()), false),
            None
        );
        assert_eq!(extra_home_config(&profile, None, false), None);
        assert_eq!(
            extra_home_config(&profile, Some("relative/home".into()), false),
            None
        );
    }

    /// A Windows-edited config: a byte-order mark and CRLF line ends.
    #[test]
    fn a_bom_and_crlf_config_keeps_its_first_host() {
        let windows = format!("\u{feff}{}", SIMPLE.replace('\n', "\r\n"));
        let hosts = parse(&windows);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "alpha");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].hostname.as_deref(), Some("beta.lan"));
    }

    #[test]
    fn parses_two_simple_hosts() {
        let hosts = parse(SIMPLE);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "alpha");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[0].user.as_deref(), Some("martin"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].alias, "beta");
        assert_eq!(hosts[1].hostname.as_deref(), Some("beta.lan"));
        assert_eq!(hosts[1].user, None);
    }

    #[test]
    fn drops_wildcard_blocks() {
        let cfg = "
Host *
    StrictHostKeyChecking ask
Host real
    Hostname real.example.com
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "real");
    }

    #[test]
    fn drops_github_alias() {
        let cfg = "
Host github.com
    IdentityFile ~/.ssh/github_ed25519
Host work
    Hostname work.lan
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "work");
    }

    #[test]
    fn comments_are_stripped() {
        let cfg = "
# top-level comment
Host x  # trailing comment
    Hostname x.lan # this too
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "x");
        assert_eq!(hosts[0].hostname.as_deref(), Some("x.lan"));
    }

    #[test]
    fn supports_equals_form() {
        let cfg = "
Host eq
    Hostname=eq.lan
    Port=2244
";
        let hosts = parse(cfg);
        assert_eq!(hosts[0].hostname.as_deref(), Some("eq.lan"));
        assert_eq!(hosts[0].port, Some(2244));
    }

    #[test]
    fn handles_every_alias_in_multi_alias_line() {
        // OpenSSH allows `Host a b c` to share a block; each is a host the
        // user can `ssh` to, so each is offered, with the block's values.
        let cfg = "
Host primary backup tertiary
    Hostname pool.lan
";
        let hosts = parse(cfg);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["primary", "backup", "tertiary"]);
        assert!(hosts
            .iter()
            .all(|h| h.hostname.as_deref() == Some("pool.lan")));
    }

    #[test]
    fn empty_input_returns_empty_vec() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn unknown_keywords_are_ignored() {
        let cfg = "
Host h
    Hostname h.lan
    ServerAliveInterval 30
    PermitLocalCommand yes
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname.as_deref(), Some("h.lan"));
    }
}
