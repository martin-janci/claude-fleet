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
pub fn parse(input: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Option<SshHost> = None;
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
            // Close out previous block (if it was a real host).
            if let Some(prev) = current.take() {
                hosts.push(prev);
            }
            // A single `Host` line may declare multiple aliases (`Host a b c`).
            // For our purposes we take the FIRST alias and ignore the rest —
            // exotic shared blocks aren't common in user configs.
            let first = value.split_ascii_whitespace().next().unwrap_or("");
            if is_real_alias(first) {
                current = Some(SshHost {
                    alias: first.to_string(),
                    hostname: None,
                    user: None,
                    port: None,
                });
            } else {
                current = None;
            }
            continue;
        }
        let Some(host) = current.as_mut() else { continue };
        match key_l.as_str() {
            "hostname" => host.hostname = Some(value.trim().to_string()),
            "user" => host.user = Some(value.trim().to_string()),
            "port" => host.port = value.trim().parse::<u16>().ok(),
            _ => {}
        }
    }
    if let Some(last) = current.take() {
        hosts.push(last);
    }
    hosts
}

/// Convenience wrapper: load and parse the user's `~/.ssh/config`. Returns
/// an empty list if the file does not exist or cannot be read.
pub fn load_user_config() -> Vec<SshHost> {
    let Some(home) = dirs_home() else { return Vec::new() };
    let path = home.join(".ssh").join("config");
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse(&contents),
        Err(_) => Vec::new(),
    }
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
    std::env::var_os("HOME").map(std::path::PathBuf::from)
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
    fn handles_first_alias_in_multi_alias_line() {
        // OpenSSH allows `Host a b c` to share a block. We just take `a`.
        let cfg = "
Host primary backup tertiary
    Hostname pool.lan
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "primary");
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
