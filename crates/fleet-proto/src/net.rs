//! Where a hub is, and whether that is this machine.
//!
//! Three crates ask the same two questions — the agent before it dials, the
//! desktop before it sends its client token, the hub before it binds — and
//! each used to answer them in its own words. The copies disagreed, which for
//! a *security* predicate is the worst shape a duplicate can take: the
//! narrowest copy decides what is refused and the widest decides what is
//! allowed, and nothing says which is which. They live here because
//! `fleet-agent` may depend on this crate and on no other in the workspace.
//!
//! Pure string and `std::net` work: no transport, no tokio, no `url` crate.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

/// True when `host` names *this machine* — the one the caller is running on —
/// with no network hop in between.
///
/// `host` is a bare host: a DNS name, an IPv4 literal, or an IPv6 literal
/// either bare or in brackets (`[::1]`, as a `Host` header or a URL authority
/// spells it). Surrounding whitespace and the brackets are stripped; no port
/// is accepted here, because a caller that has an authority must split it
/// first (a name can contain no colon, so guessing would be ambiguous for
/// IPv6).
///
/// True for exactly three things:
///
/// - `localhost`, matched exactly, ASCII case-insensitively;
/// - any IPv4 literal in `127.0.0.0/8` — the whole block, not just
///   `127.0.0.1`;
/// - the IPv6 literal `::1`.
///
/// Everything else is false, and three of those deserve their reasons:
///
/// - **`foo.localhost` and any other `*.localhost` name is NOT loopback.**
///   RFC 6761 says resolvers *should* keep the `localhost.` subtree on the
///   machine, but "should" is not "must", and it is not what every resolver
///   does: musl's does not special-case the name at all, and a DNS server
///   that answers for `<anything>.localhost` therefore gets to decide where
///   the packet — and any credential in it — goes. A predicate that gates
///   sending a token in the clear cannot rest on a name whose destination an
///   attacker can influence.
/// - **An IPv4-mapped address (`::ffff:127.0.0.1`) is NOT loopback.** It is a
///   *routable* v6 destination that some stacks map back to v4 and others do
///   not; whether a packet ever reaches the loopback interface depends on the
///   host's configuration, so the address does not promise what the name of
///   this function promises. (`Ipv6Addr::is_loopback` agrees: only `::1`.)
/// - **`0.0.0.0` is NOT loopback.** It is the unspecified address — "every
///   interface" as a bind, and not a destination at all.
///
/// Callers keep their own *policy* (the agent's `--insecure`, the desktop's
/// opt-in setting, the hub's refusal to serve a routable bind in the clear).
/// Only the question "is this the local machine" is answered here.
pub fn is_loopback(host: &str) -> bool {
    let host = host.trim();
    // `[::1]`: an authority's brackets, if the caller did not strip them.
    // Brackets mean IPv6 and nothing else — `[127.0.0.1]` is not a legal
    // authority, and accepting it here would widen a `Host`-header check
    // beyond the three forms this function promises.
    if let Some(rest) = host.strip_prefix('[') {
        return rest
            .strip_suffix(']')
            .and_then(|inner| inner.parse::<Ipv6Addr>().ok())
            .is_some_and(|v6| is_loopback_ip(&IpAddr::V6(v6)));
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| is_loopback_ip(&ip))
}

/// [`is_loopback`] for an address that is already parsed — what a bind check
/// has. Deliberately the same rule: `std`'s own `is_loopback` is
/// `127.0.0.0/8` for v4 and `::1` alone for v6, which is the definition
/// [`is_loopback`] is built on.
pub fn is_loopback_ip(ip: &IpAddr) -> bool {
    ip.is_loopback()
}

/// The four schemes a hub address can carry. Which of them a given caller
/// *accepts* is that caller's policy: the desktop's hand-written HTTP speaks
/// only `http`/`https`, the agent dials a WebSocket and takes all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
    Ws,
    Wss,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Ws => "ws",
            Self::Wss => "wss",
        }
    }

    /// Whether the transport is encrypted — the only thing the *mechanism*
    /// needs to know about a scheme.
    pub fn is_tls(self) -> bool {
        matches!(self, Self::Https | Self::Wss)
    }

    pub fn is_websocket(self) -> bool {
        matches!(self, Self::Ws | Self::Wss)
    }

    /// The same transport as a WebSocket URL: `https` → `wss`, `http` → `ws`.
    pub fn websocket(self) -> Self {
        match self {
            Self::Http | Self::Ws => Self::Ws,
            Self::Https | Self::Wss => Self::Wss,
        }
    }

    /// The port that needs no writing down.
    pub fn default_port(self) -> u16 {
        if self.is_tls() {
            443
        } else {
            80
        }
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A hub address, split into the pieces every caller needs: where to open the
/// socket, what to put in a `Host` header, and what path prefix the hub is
/// mounted under.
///
/// Built only by [`Endpoint::parse`] — the fields are private so that no
/// caller can assemble one that skipped the checks, which is the point of
/// having a single parser at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    scheme: Scheme,
    host: String,
    port: u16,
    authority: String,
    authority_as_written: String,
    path: String,
}

impl Endpoint {
    /// Parse an absolute `http`/`https`/`ws`/`wss` URL.
    ///
    /// Strict on purpose — every one of these refusals is something a caller
    /// here would otherwise have had to notice for itself:
    ///
    /// - **no userinfo** (`https://user:pw@hub`): this value is logged, shown
    ///   in Settings and folded into diagnostics blobs, and it is not an
    ///   authentication channel (a bearer token is);
    /// - **no query and no fragment**: the desktop concatenates this URL with
    ///   `/mcp` or `/events`, which turns `https://hub/?t=1` into
    ///   `https://hub/?t=1/mcp`, and the agent appends `/agent` to it;
    /// - **an IPv6 literal must be bracketed**: `http://::1` is otherwise
    ///   read as host `::` on port 1;
    /// - **ASCII only, no controls, no spaces**: `host`, `authority` and
    ///   `path` go straight into a hand-written HTTP request line, where a
    ///   CR or an LF is header injection. A non-ASCII hostname is refused
    ///   rather than guessed at — write it in punycode.
    ///
    /// What it does NOT decide is whether a plaintext scheme is acceptable.
    /// That is policy and it differs per caller; ask [`Endpoint::is_tls`] and
    /// [`is_loopback`] and answer it where the policy lives.
    pub fn parse(url: &str) -> Result<Self, String> {
        let raw = url.trim();
        let (scheme_str, rest) = raw
            .split_once("://")
            .ok_or_else(|| "expected scheme://host[:port][/path]".to_string())?;
        let scheme = match scheme_str.to_ascii_lowercase().as_str() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            "ws" => Scheme::Ws,
            "wss" => Scheme::Wss,
            other => return Err(format!("{other}:// is not a hub address")),
        };
        if rest.contains(['?', '#']) {
            return Err("a hub URL has no query or fragment".to_string());
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => rest.split_at(i),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err("no host".to_string());
        }
        if authority.contains('@') {
            return Err("a hub URL carries no userinfo".to_string());
        }
        let authority_as_written = authority.to_string();
        let (host, port, bracketed) = split_authority(authority, scheme.default_port())?;
        let host = normalise_host(&host, bracketed)?;
        let path = normalise_path(path)?;
        let authority = match (bracketed, port == scheme.default_port()) {
            (true, true) => format!("[{host}]"),
            (true, false) => format!("[{host}]:{port}"),
            (false, true) => host.clone(),
            (false, false) => format!("{host}:{port}"),
        };
        Ok(Self {
            scheme,
            host,
            port,
            authority,
            authority_as_written,
            path,
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The name to connect to and to check the certificate against.
    /// **Unbracketed**, so an IPv6 literal resolves and parses as a TLS
    /// server name; `[::1]` does neither.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_tls(&self) -> bool {
        self.scheme.is_tls()
    }

    /// The **canonical** authority: brackets REQUIRED around an IPv6 literal
    /// (`[::1]:8787`), host lower-cased, and the port present only when it is
    /// not the scheme's default.
    ///
    /// This is what the desktop puts in its `Host` header, and it is the form
    /// it has always sent — the `url` crate it used before this parser
    /// existed drops a scheme-default port the same way. A hub matches its
    /// `allowed_hosts` against exactly this string, so the form is not a
    /// cosmetic choice: see [`Endpoint::authority_as_written`] for the other
    /// half of that.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// The authority **exactly as the URL spelled it** — original case,
    /// original IPv6 spelling, and a scheme-default port still present if it
    /// was typed.
    ///
    /// `fleet-agent` builds its WebSocket URL from this, and therefore sends
    /// it as its `Host` header. It must, for compatibility: a hub's
    /// `allowed_hosts` is an exact string match that only relaxes one way
    /// (an entry spelled `hub` accepts `Host: hub` and `Host: hub:443`, but
    /// an entry spelled `hub:443` accepts only `hub:443`), and an operator
    /// who wrote `--public-url https://hub:443` has `hub:443` in that list.
    /// An agent that started eliding the port would be refused by a hub it
    /// reached the day before.
    ///
    /// It is as safe as [`Endpoint::authority`] to put in a request line:
    /// the same host and port validation ran over it, so it carries no
    /// control character, no space and no userinfo.
    pub fn authority_as_written(&self) -> &str {
        &self.authority_as_written
    }

    /// The path prefix the hub is mounted under, with no trailing slash:
    /// `/fleet`, or `""` for a hub at the root. Append `/mcp`, `/events` or
    /// `/agent` to it.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The path as an HTTP request line spells it, which is never empty.
    pub fn request_target(&self) -> String {
        if self.path.is_empty() {
            "/".to_string()
        } else {
            self.path.clone()
        }
    }

    /// True when this endpoint is on the machine the caller is running on —
    /// [`is_loopback`] of its host.
    pub fn is_loopback(&self) -> bool {
        is_loopback(&self.host)
    }
}

/// `host[:port]`, or `[v6][:port]`, into its halves — plus whether the host
/// arrived in brackets, which is what tells an IPv6 literal from a name.
fn split_authority(authority: &str, default_port: u16) -> Result<(String, u16, bool), String> {
    // `str::parse::<u16>` accepts a leading sign, which the `url` crate — and
    // therefore every URL this fleet stores — does not. Digits only. Leading
    // zeros are deliberately left alone: `url` accepts those, so refusing
    // them would be a new divergence rather than the end of one.
    let parse_port = |p: &str| match p.bytes().all(|b| b.is_ascii_digit()) {
        true => p.parse::<u16>().ok(),
        false => None,
    };
    let bad_port = |p: &str| format!("{p:?} is not a port");
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6
            .split_once(']')
            .ok_or_else(|| "an IPv6 literal is missing its closing bracket".to_string())?;
        let port = match after.strip_prefix(':') {
            Some(p) => parse_port(p).ok_or_else(|| bad_port(p))?,
            None if after.is_empty() => default_port,
            None => return Err(format!("{after:?} after an IPv6 literal")),
        };
        return Ok((host.to_string(), port, true));
    }
    match authority.rsplit_once(':') {
        Some((host, p)) => Ok((
            host.to_string(),
            parse_port(p).ok_or_else(|| bad_port(p))?,
            false,
        )),
        None => Ok((authority.to_string(), default_port, false)),
    }
}

/// Lower-cased, and an IPv6 literal put in its canonical form, so that two
/// spellings of one address compare equal and [`is_loopback`] sees the
/// address rather than the spelling.
///
/// The character set is the ONLY shape check on a name — no label lengths, no
/// empty-label rule, nothing that would have to decide whether a trailing dot
/// is an absolute name or a typo. Whether `hub..example` exists is the
/// resolver's answer to give; what matters here is that nothing which cannot
/// safely reach a request line gets through.
///
/// `bracketed` says the authority wrote the host as `[…]`, and it is the
/// whole of what distinguishes `http://[::1]` from `http://::1` — the latter
/// splits into host `::` on port 1, which is nobody's intent.
fn normalise_host(host: &str, bracketed: bool) -> Result<String, String> {
    if host.is_empty() {
        return Err("no host".to_string());
    }
    if bracketed {
        return match host.parse::<Ipv6Addr>() {
            Ok(v6) => Ok(v6.to_string()),
            Err(_) => Err(format!("{host:?} in brackets is not an IPv6 literal")),
        };
    }
    if host.contains(':') || host.parse::<Ipv6Addr>().is_ok() {
        return Err(format!(
            "{host:?}: an IPv6 literal belongs in brackets, as [{host}]"
        ));
    }
    if !host.bytes().all(ok_in_host) {
        return Err(format!(
            "{host:?} has a character a host name cannot (ASCII letters, digits, '-', '.' \
             and '_' only; write an international name in punycode)"
        ));
    }
    Ok(host.to_ascii_lowercase())
}

fn ok_in_host(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
}

/// Trailing slashes off, so `https://hub/` and `https://hub` are one thing.
fn normalise_path(path: &str) -> Result<String, String> {
    if !path
        .bytes()
        .all(|b| b.is_ascii() && !b.is_ascii_control() && b != b' ')
    {
        return Err(format!(
            "{path:?} is not a path (printable ASCII, no spaces; percent-encode the rest)"
        ));
    }
    Ok(path.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── loopback ───────────────────────────────────────────────────────────

    #[test]
    fn the_local_machine_by_name_and_by_address() {
        for host in [
            "localhost",
            "LocalHost",
            "LOCALHOST",
            " localhost ",
            "127.0.0.1",
            "127.0.0.53",
            "127.8.9.10",
            "127.255.255.255",
            "::1",
            "[::1]",
            "0:0:0:0:0:0:0:1",
        ] {
            assert!(is_loopback(host), "should be loopback: {host:?}");
        }
    }

    /// The three that a copy of this predicate somewhere else used to get
    /// wrong, and the ordinary remote cases.
    #[test]
    fn everything_else_is_not_the_local_machine() {
        for host in [
            // RFC 6761 is a SHOULD; a resolver that sends this to DNS lets
            // someone else choose where the packet goes.
            "foo.localhost",
            "localhost.evil.example",
            "evil.example",
            "127.0.0.1.evil.example",
            // Routable v6 that only *some* stacks fold back to v4.
            "::ffff:127.0.0.1",
            "[::ffff:127.0.0.1]",
            // The unspecified address is a bind, not a destination.
            "0.0.0.0",
            "::",
            // Brackets mean IPv6. A bracketed v4 is not a legal authority,
            // and accepting it would widen the `Host` check in `mcp::auth`
            // past the three forms this function promises.
            "[127.0.0.1]",
            "[localhost]",
            "10.0.0.5",
            "fe80::1",
            "",
            "[::1",
            "::1]",
            // An authority, not a host: the caller must split the port off.
            "127.0.0.1:4180",
        ] {
            assert!(!is_loopback(host), "should not be loopback: {host:?}");
        }
    }

    #[test]
    fn the_parsed_form_answers_the_same_way() {
        for (ip, want) in [
            ("127.0.0.1", true),
            ("127.1.2.3", true),
            ("::1", true),
            ("0.0.0.0", false),
            ("::ffff:127.0.0.1", false),
            ("10.0.0.5", false),
        ] {
            let parsed: IpAddr = ip.parse().unwrap();
            assert_eq!(is_loopback_ip(&parsed), want, "{ip}");
            assert_eq!(is_loopback(ip), want, "{ip} as a string");
        }
    }

    // ── the parser ─────────────────────────────────────────────────────────

    /// The union of what the agent's hand-rolled parser and the desktop's
    /// `url`-crate one were each tested for, against the one parser that
    /// replaced them.
    #[test]
    fn a_hub_url_splits_into_host_port_authority_and_path() {
        // url, host, port, tls, authority, path
        let cases = [
            (
                "https://hub.example",
                "hub.example",
                443,
                true,
                "hub.example",
                "",
            ),
            (
                "https://hub.example/",
                "hub.example",
                443,
                true,
                "hub.example",
                "",
            ),
            (
                "https://hub.example:8443",
                "hub.example",
                8443,
                true,
                "hub.example:8443",
                "",
            ),
            (
                "wss://hub.example/agent",
                "hub.example",
                443,
                true,
                "hub.example",
                "/agent",
            ),
            (
                "https://example.com/fleet",
                "example.com",
                443,
                true,
                "example.com",
                "/fleet",
            ),
            (
                "https://fleet.example.com/mcp",
                "fleet.example.com",
                443,
                true,
                "fleet.example.com",
                "/mcp",
            ),
            (
                "http://hub.example.com:4180/fleet/events",
                "hub.example.com",
                4180,
                false,
                "hub.example.com:4180",
                "/fleet/events",
            ),
            (
                "http://127.0.0.1:7777",
                "127.0.0.1",
                7777,
                false,
                "127.0.0.1:7777",
                "",
            ),
            ("ws://localhost", "localhost", 80, false, "localhost", ""),
            ("http://[::1]:9", "::1", 9, false, "[::1]:9", ""),
            (
                "https://[2001:db8::1]:8787/mcp",
                "2001:db8::1",
                8787,
                true,
                "[2001:db8::1]:8787",
                "/mcp",
            ),
            (
                "http://[::1]:8787/events",
                "::1",
                8787,
                false,
                "[::1]:8787",
                "/events",
            ),
            // An IPv6 hub on its scheme's default port: brackets, no port.
            ("https://[::1]", "::1", 443, true, "[::1]", ""),
            // A default port written out: the CANONICAL authority elides it,
            // which is the form the desktop has always sent. The agent needs
            // the other form — see the `authority_as_written` test below.
            (
                "https://hub.example:443",
                "hub.example",
                443,
                true,
                "hub.example",
                "",
            ),
            (
                "http://hub.example:80",
                "hub.example",
                80,
                false,
                "hub.example",
                "",
            ),
            ("https://[::1]:443", "::1", 443, true, "[::1]", ""),
            // Case and an already-canonical address both normalise.
            (
                "HTTPS://Hub.EXAMPLE/Fleet",
                "hub.example",
                443,
                true,
                "hub.example",
                "/Fleet",
            ),
        ];
        for (url, host, port, tls, authority, path) in cases {
            let at = Endpoint::parse(url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(at.host(), host, "{url}");
            assert_eq!(at.port(), port, "{url}");
            assert_eq!(at.is_tls(), tls, "{url}");
            assert_eq!(at.authority(), authority, "{url}");
            assert_eq!(at.path(), path, "{url}");
        }
    }

    #[test]
    fn a_hub_at_the_root_still_has_a_request_target() {
        assert_eq!(
            Endpoint::parse("https://hub").unwrap().request_target(),
            "/"
        );
        assert_eq!(
            Endpoint::parse("https://hub/mcp").unwrap().request_target(),
            "/mcp"
        );
    }

    #[test]
    fn a_url_that_is_not_a_hub_address_is_refused() {
        for url in [
            "",
            "hub.example",
            "ftp://hub.example",
            "not a url",
            "https://",
            // Query and fragment: the desktop concatenates, the agent appends.
            "https://hub.example?x=1",
            "https://hub.example/#frag",
            // Userinfo never survives into a logged base URL.
            "https://user:pw@hub.example",
            "https://user@hub.example",
            // A bare IPv6 literal reads as host "::" on port 1.
            "http://::1",
            "http://[::1",
            "https://hub.example:notaport",
            "https://hub.example:65536",
            // `str::parse::<u16>` would take a sign or surrounding space; the
            // `url` crate does not, and neither does the form every stored
            // URL is normalised into. (Leading zeros are left alone — `url`
            // takes those, so refusing them would be a divergence of its own.)
            "https://hub.example:+443",
            "https://hub.example:-443",
            "https://hub.example: 443",
            // Header injection into a hand-written request line.
            "https://hub.example\r\nX-Evil: 1",
            "https://hub .example",
            "https://hub.example/a b",
            "https://hüb.example",
        ] {
            assert!(Endpoint::parse(url).is_err(), "should be refused: {url:?}");
        }
    }

    #[test]
    fn the_scheme_carries_the_transport_and_its_websocket_twin() {
        assert!(Endpoint::parse("https://hub").unwrap().is_tls());
        assert!(!Endpoint::parse("http://hub").unwrap().is_tls());
        assert!(Endpoint::parse("wss://hub")
            .unwrap()
            .scheme()
            .is_websocket());
        assert!(!Endpoint::parse("https://hub")
            .unwrap()
            .scheme()
            .is_websocket());
        assert_eq!(Scheme::Https.websocket(), Scheme::Wss);
        assert_eq!(Scheme::Http.websocket(), Scheme::Ws);
        assert_eq!(Scheme::Wss.websocket(), Scheme::Wss);
        assert_eq!(Scheme::Ws.websocket(), Scheme::Ws);
    }

    /// The loopback rule reaches an endpoint the same way it reaches a bare
    /// host — including the `*.localhost` refusal.
    #[test]
    fn an_endpoint_knows_whether_it_is_this_machine() {
        for (url, want) in [
            ("http://127.0.0.1:7777", true),
            ("ws://localhost", true),
            ("http://[::1]:9", true),
            ("ws://127.8.9.10", true),
            ("http://10.0.0.5", false),
            ("ws://hub.example", false),
            ("http://[fe80::1]:9", false),
            ("http://127.0.0.1.example", false),
            ("http://hub.localhost", false),
        ] {
            assert_eq!(Endpoint::parse(url).unwrap().is_loopback(), want, "{url}");
        }
    }

    /// **The compatibility rule this parser exists to keep.** A hub matches
    /// `allowed_hosts` as an exact string that relaxes only one way, so the
    /// two callers must each keep sending the authority they always sent:
    /// the desktop the canonical one (the `url` crate elided a default
    /// port), `fleet-agent` the one the operator typed (its hand-rolled
    /// parser passed the authority through verbatim). Picking either for
    /// both would 403 somebody's working setup.
    #[test]
    fn the_authority_comes_in_two_forms_so_neither_caller_changes_what_it_sends() {
        // url, canonical (desktop), as written (agent)
        let cases = [
            ("https://hub.example:443", "hub.example", "hub.example:443"),
            ("http://hub.example:80", "hub.example", "hub.example:80"),
            (
                "wss://hub.example:443/agent",
                "hub.example",
                "hub.example:443",
            ),
            // No port written: the two agree.
            ("https://hub.example", "hub.example", "hub.example"),
            // A non-default port survives into both.
            (
                "https://hub.example:8443",
                "hub.example:8443",
                "hub.example:8443",
            ),
            // Case and IPv6 spelling: canonicalised in one, verbatim in the
            // other.
            ("https://HUB.Example", "hub.example", "HUB.Example"),
            (
                "https://[0:0:0:0:0:0:0:1]:8443",
                "[::1]:8443",
                "[0:0:0:0:0:0:0:1]:8443",
            ),
            ("https://[::1]:443", "[::1]", "[::1]:443"),
            ("https://[::1]", "[::1]", "[::1]"),
        ];
        for (url, canonical, as_written) in cases {
            let at = Endpoint::parse(url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(at.authority(), canonical, "canonical authority of {url}");
            assert_eq!(
                at.authority_as_written(),
                as_written,
                "as-written authority of {url}"
            );
        }
    }
}
