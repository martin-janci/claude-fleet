//! `X-Fleet-Client` (update design §6.3): every client says what it runs on
//! every request, so the hub knows a desktop's or a phone's version without
//! it ever calling `/update`. Today only fleet-mobile sends it (slice S8);
//! the desktop's hub transport does not yet, so the hub records a desktop
//! only once it calls `/update/check` or reports.
//!
//! ```text
//! X-Fleet-Client: desktop/0.4.1 (macos-aarch64; build 1a2b3c4; contract 5-6)
//! ```
//!
//! The parts in parentheses are optional and in any order; an unknown one is
//! ignored, so a newer client can add parts without an older hub refusing the
//! whole header. What it says is trusted for display and compatibility only,
//! like [`Speaks`](crate::wire::Speaks): the credential says *who* is asking.

use crate::model::{Component, Platform, Window};
use crate::Version;

/// The header's name.
pub const CLIENT_HEADER: &str = "X-Fleet-Client";
/// Longest header this parses; anything longer is not one of ours.
pub const MAX_CLIENT_HEADER_LEN: usize = 256;

/// One `X-Fleet-Client` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHeader {
    pub component: Component,
    pub version: Version,
    /// `os` and `arch`; `variant` stays empty (the header does not carry it).
    pub platform: Option<Platform>,
    /// The short commit it was built from.
    pub build: Option<String>,
    /// A client's `[MIN_HUB_CONTRACT, MAX_HUB_CONTRACT]`.
    pub contract_accepts: Option<Window>,
}

fn component(s: &str) -> Option<Component> {
    Some(match s {
        "desktop" => Component::Desktop,
        "android" => Component::Android,
        "ios" => Component::Ios,
        "hub" => Component::Hub,
        "agent" => Component::Agent,
        _ => return None,
    })
}

/// A word the header may carry: short, and nothing that could be markup.
fn word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

impl ClientHeader {
    /// `None` for anything that is not a well-formed header: a client too old
    /// to send one looks the same as a garbled one, and neither is an error.
    pub fn parse(raw: &str) -> Option<ClientHeader> {
        if raw.len() > MAX_CLIENT_HEADER_LEN {
            return None;
        }
        let raw = raw.trim();
        let (head, rest) = match raw.split_once(' ') {
            Some((h, r)) => (h, r.trim()),
            None => (raw, ""),
        };
        let (c, v) = head.split_once('/')?;
        let mut h = ClientHeader {
            component: component(c)?,
            version: Version::parse(v).ok()?,
            platform: None,
            build: None,
            contract_accepts: None,
        };
        if rest.is_empty() {
            return Some(h);
        }
        let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
        for part in inner.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            if let Some(b) = part.strip_prefix("build ") {
                h.build = word(b.trim()).then(|| b.trim().to_string());
            } else if let Some(w) = part.strip_prefix("contract ") {
                h.contract_accepts = w.trim().split_once('-').and_then(|(a, b)| {
                    let (a, b) = (a.parse().ok()?, b.parse().ok()?);
                    (a <= b).then_some(Window::new(a, b))
                });
            } else if let Some((os, arch)) = part.split_once('-') {
                if word(os) && word(arch) && !part.contains(' ') {
                    h.platform = Some(Platform::new(os, arch, ""));
                }
            }
        }
        Some(h)
    }

    /// The header value a client sends.
    pub fn to_header_value(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = &self.platform {
            parts.push(format!("{}-{}", p.os, p.arch));
        }
        if let Some(b) = &self.build {
            parts.push(format!("build {b}"));
        }
        if let Some(w) = &self.contract_accepts {
            parts.push(format!("contract {}-{}", w.min, w.max));
        }
        let head = format!("{}/{}", self.component.as_str(), self.version);
        if parts.is_empty() {
            head
        } else {
            format!("{head} ({})", parts.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_header_round_trips() {
        let raw = "desktop/0.4.1 (macos-aarch64; build 1a2b3c4; contract 5-6)";
        let h = ClientHeader::parse(raw).unwrap();
        assert_eq!(h.component, Component::Desktop);
        assert_eq!(h.version, Version::new(0, 4, 1));
        assert_eq!(h.platform, Some(Platform::new("macos", "aarch64", "")));
        assert_eq!(h.build.as_deref(), Some("1a2b3c4"));
        assert_eq!(h.contract_accepts, Some(Window::new(5, 6)));
        assert_eq!(h.to_header_value(), raw);
    }

    #[test]
    fn parts_are_optional_in_any_order_and_unknown_ones_ignored() {
        let h =
            ClientHeader::parse("android/0.2.36 (contract 1-5; screen 1080x2400; linux-x86_64)")
                .unwrap();
        assert_eq!(h.component, Component::Android);
        assert_eq!(h.contract_accepts, Some(Window::new(1, 5)));
        assert_eq!(h.platform, Some(Platform::new("linux", "x86_64", "")));
        assert_eq!(h.build, None);
        let bare = ClientHeader::parse("desktop/0.4.1-rc.2").unwrap();
        assert_eq!(bare.version.to_string(), "0.4.1-rc.2");
        assert_eq!(bare.to_header_value(), "desktop/0.4.1-rc.2");
    }

    #[test]
    fn garbage_is_no_header() {
        for raw in [
            "",
            "desktop",
            "toaster/1.0.0",
            "desktop/one",
            "desktop/0.4.1 macos-aarch64",
            "desktop/0.4.1 (unclosed",
        ] {
            assert_eq!(ClientHeader::parse(raw), None, "{raw:?}");
        }
        assert_eq!(ClientHeader::parse(&"x".repeat(300)), None);
        // A bad part is dropped, not the header.
        let h = ClientHeader::parse("desktop/0.4.1 (contract 6-5; build <b>)").unwrap();
        assert_eq!((h.contract_accepts, h.build), (None, None));
    }
}
