//! Whose fleet a host reports to, as far as its wiring says: the URL in its
//! `claude-fleet` MCP entry and in fleet's hook block. A fleet never takes
//! over by itself a host whose wiring names another fleet: a desktop that
//! migrated to a hub, or one beside it (docs/hub.md, "Migrating from the
//! desktop"), would otherwise rewrite the hub's entries to its own loopback
//! address on every pass, and the hub would take them back.
//!
//! The rule is a heuristic over the URL alone: a base that is this
//! process's own, or loopback (a desktop's tunnel, a hub's agent relay, an
//! older install of this machine's), is this fleet's to repair; any other
//! base is another fleet's, and only a person takes it over.

/// PURE: `scheme://authority` of an http(s) URL, lowercased; `None` for
/// anything else.
pub fn origin(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Userinfo never decides whose a URL is.
    let authority = authority.rsplit('@').next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

/// PURE: the host part of an origin's authority, without the port.
fn host_of(origin: &str) -> &str {
    let authority = origin.split_once("://").map_or(origin, |(_, a)| a);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

/// PURE: whether `url` points at this machine (127.0.0.0/8, `localhost`,
/// `::1`).
pub fn is_loopback(url: &str) -> bool {
    let Some(o) = origin(url) else { return false };
    let host = host_of(&o);
    host == "localhost"
        || host == "::1"
        || host.strip_prefix("127.").is_some_and(|r| {
            r.split('.').count() == 3 && r.split('.').all(|p| p.parse::<u8>().is_ok())
        })
}

/// PURE: whether `got` names a fleet other than this one: neither one of
/// this process's own bases (`ours`, any URL of it) nor loopback. A URL that
/// is not http(s) at all names no fleet.
pub fn foreign_base(got: &str, ours: &[&str]) -> bool {
    let Some(theirs) = origin(got) else {
        return false;
    };
    !is_loopback(got)
        && !ours
            .iter()
            .any(|u| origin(u).as_deref() == Some(theirs.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_another_non_loopback_base_is_another_fleet() {
        let ours = ["http://127.0.0.1:4180/mcp"];
        assert!(foreign_base("https://hub.example/mcp", &ours));
        assert!(foreign_base("https://hub.example:8443/hook", &ours));
        assert!(!foreign_base("http://127.0.0.1:4180/mcp", &ours));
        assert!(
            !foreign_base("http://127.0.0.1:39001/hook", &ours),
            "a tunnel or relay"
        );
        assert!(!foreign_base("http://localhost:4180/mcp", &ours));
        assert!(!foreign_base("http://[::1]:4180/mcp", &ours));
        let hub = ["https://Hub.Example/mcp"];
        assert!(
            !foreign_base("https://hub.example/hook", &hub),
            "this hub's own base"
        );
        assert!(foreign_base("https://other.example/hook", &hub));
        assert!(foreign_base("http://127.0.0.1.evil.example/mcp", &hub));
        assert!(!foreign_base("not a url", &hub));
        assert!(!foreign_base("", &hub));
    }
}
