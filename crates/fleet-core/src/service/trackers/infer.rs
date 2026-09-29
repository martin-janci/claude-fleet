//! "Paste any ticket or issue URL" (work graph M6.6, M11.4), on the backend:
//! what a pasted URL names — the provider, the site to add, the key it points
//! at, and for a GitHub Enterprise instance its `gh --hostname`. The Connect
//! flow (`pages::flows`, declarative pages P4b) asks this, so the logic runs
//! where the tracker is added; `src/lib/trackers.ts` `inferProvider` is the
//! same rules for the frontend's own previews, and both are held to the
//! same cases.
//!
//! Jira Data Center cannot be told from a URL: it is picked by hand. A
//! GitHub-shaped issue URL (`/<owner>/<repo>/issues/<n>`) on any other host is
//! offered as GitHub Enterprise Server, a guess the person confirms, since
//! Gitea and friends share the shape. The site is fenced again when it is
//! added (`normalize_provider_site`, `validate_ghes_hostname`).

use crate::store::ghes_host_ok;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Inferred {
    pub provider: &'static str,
    pub site: String,
    /// The key the URL points at, or `""` for a site.
    pub key: String,
    /// GitHub Enterprise: the instance, `host[:port]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

fn inferred(provider: &'static str, site: String, key: String) -> Option<Inferred> {
    Some(Inferred {
        provider,
        site,
        key,
        hostname: None,
    })
}

/// `[A-Za-z0-9_][A-Za-z0-9_.-]{0,99}`: a GitHub owner or repository name.
fn gh_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 100
        && (b[0].is_ascii_alphanumeric() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

fn digits(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
}

/// `[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?.atlassian.net`: one label before it.
fn atlassian_host(host: &str) -> bool {
    host.strip_suffix(".atlassian.net").is_some_and(|l| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// A Jira-style key, `ABC-123`: a letter, 1–9 more letters, digits or `_`,
/// a dash and 1–7 digits.
fn jira_style_key(s: &str) -> bool {
    let Some((p, n)) = s.split_once('-') else {
        return false;
    };
    let pb = p.as_bytes();
    pb.len() >= 2
        && pb.len() <= 10
        && pb[0].is_ascii_alphabetic()
        && pb.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        && digits(n, 7)
}

/// What `text` names, or `None` for anything fleet cannot recognise.
pub fn infer(text: &str) -> Option<Inferred> {
    if let Some((site, key)) = super::jira::parse_ticket_url(text) {
        return inferred("jira", site, key);
    }
    let t = text.trim();
    let scheme_end = t.find("://")?;
    if !t[..scheme_end].eq_ignore_ascii_case("https") {
        return None;
    }
    let rest = &t[scheme_end + 3..];
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => {
            if !digits(p, 5) {
                return None;
            }
            (h, p)
        }
        _ => (authority, ""),
    };
    let host = host.to_ascii_lowercase();
    let path = rest[auth_end..].split(['?', '#']).next().unwrap_or("");
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // An enterprise instance may listen on its own port; nothing else may.
    if !port.is_empty() {
        return ghes(&host, port, &segs);
    }
    if atlassian_host(&host) && segs.is_empty() {
        return inferred("jira", format!("https://{host}"), String::new());
    }
    if host == "github.com" || host == "www.github.com" {
        let Some(owner) = segs.first() else {
            return inferred("github", "https://github.com".into(), String::new());
        };
        if !gh_name(owner) {
            return None;
        }
        let key = match (segs.get(1), segs.get(2), segs.get(3)) {
            (Some(r), Some(&"issues"), Some(n)) if gh_name(r) && digits(n, 9) => {
                format!("{owner}/{r}#{n}").to_ascii_lowercase()
            }
            _ => String::new(),
        };
        return inferred(
            "github",
            format!("https://github.com/{}", owner.to_ascii_lowercase()),
            key,
        );
    }
    if host == "app.asana.com" {
        let mut ws = "";
        let mut task = "";
        if segs.first() == Some(&"0") && segs.len() >= 3 {
            task = segs[2];
        } else if segs.first() == Some(&"1") && segs.get(1).is_some_and(|s| digits(s, 24)) {
            ws = segs[1];
            if let Some(i) = segs.iter().position(|s| *s == "task") {
                task = segs.get(i + 1).copied().unwrap_or("");
            }
        } else if segs.len() == 1 && digits(segs[0], 24) {
            ws = segs[0];
        } else if !segs.is_empty() {
            return None;
        }
        let site = if ws.is_empty() {
            "https://app.asana.com".to_string()
        } else {
            format!("https://app.asana.com/{ws}")
        };
        let key = if digits(task, 24) {
            format!("asana:{task}")
        } else {
            String::new()
        };
        return inferred("asana", site, key);
    }
    if host == "linear.app" {
        let ws = *segs.first()?;
        if ws.is_empty()
            || ws.len() > 64
            || !ws
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return None;
        }
        let key = match (segs.get(1), segs.get(2)) {
            (Some(&"issue"), Some(k)) if jira_style_key(k) => k.to_ascii_uppercase(),
            _ => String::new(),
        };
        return inferred(
            "linear",
            format!("https://linear.app/{}", ws.to_ascii_lowercase()),
            key,
        );
    }
    ghes(&host, "", &segs)
}

/// A GitHub Enterprise issue URL: `/<owner>/<repo>/issues/<n>` on a host
/// that could be an instance.
fn ghes(host: &str, port: &str, segs: &[&str]) -> Option<Inferred> {
    if !ghes_host_ok(host)
        || host.ends_with(".atlassian.net")
        || host == "app.asana.com"
        || host == "linear.app"
    {
        return None;
    }
    let (o, r, kind, n) = (segs.first()?, segs.get(1)?, segs.get(2)?, segs.get(3)?);
    if !gh_name(o) || !gh_name(r) || *kind != "issues" || !digits(n, 9) {
        return None;
    }
    Some(Inferred {
        provider: "github",
        site: format!("https://{host}/{}", o.to_ascii_lowercase()),
        key: format!("{host}/{o}/{r}#{n}").to_ascii_lowercase(),
        hostname: Some(if port.is_empty() {
            host.to_string()
        } else {
            format!("{host}:{port}")
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(provider: &'static str, site: &str, key: &str) -> Option<Inferred> {
        inferred(provider, site.into(), key.into())
    }

    /// The same cases as `src/lib/trackers.test.ts` `inferProvider`.
    #[test]
    fn names_the_provider_the_site_and_the_key() {
        assert_eq!(
            infer("https://acme.atlassian.net/browse/abc-12"),
            i("jira", "https://acme.atlassian.net", "ABC-12")
        );
        assert_eq!(
            infer("https://acme.atlassian.net"),
            i("jira", "https://acme.atlassian.net", "")
        );
        assert_eq!(
            infer("https://github.com/Acme/API/issues/42"),
            i("github", "https://github.com/acme", "acme/api#42")
        );
        assert_eq!(
            infer("https://github.com"),
            i("github", "https://github.com", "")
        );
        assert_eq!(
            infer("https://app.asana.com/0/1200000000001001/1207000000000001"),
            i("asana", "https://app.asana.com", "asana:1207000000000001")
        );
        assert_eq!(
            infer("https://app.asana.com/1/1200000000000001/project/1200000000001002/task/1207000000000002"),
            i(
                "asana",
                "https://app.asana.com/1200000000000001",
                "asana:1207000000000002"
            )
        );
        assert_eq!(
            infer("https://linear.app/Acme/issue/eng-101/ship-sso"),
            i("linear", "https://linear.app/acme", "ENG-101")
        );
    }

    #[test]
    fn refuses_lookalikes_and_plaintext_and_cannot_guess_data_center() {
        for bad in [
            "http://github.com/acme",
            "https://github.com.evil.com/acme/api/issues/1",
            "https://user:pw@app.asana.com/0/1/2",
            "https://linear.app:8443/acme",
            "https://jira.corp.example/browse/PLAT-2",
            "not a url",
            "",
        ] {
            assert_eq!(infer(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_github_shaped_issue_url_elsewhere_is_offered_as_enterprise() {
        assert_eq!(
            infer("https://GHE.corp.example/Acme/API/issues/42"),
            Some(Inferred {
                provider: "github",
                site: "https://ghe.corp.example/acme".into(),
                key: "ghe.corp.example/acme/api#42".into(),
                hostname: Some("ghe.corp.example".into()),
            })
        );
        assert_eq!(
            infer("https://ghe.corp.example:8443/acme/api/issues/7"),
            Some(Inferred {
                provider: "github",
                site: "https://ghe.corp.example/acme".into(),
                key: "ghe.corp.example/acme/api#7".into(),
                hostname: Some("ghe.corp.example:8443".into()),
            })
        );
        for bad in [
            "https://ghe.corp.example/acme",
            "https://ghe.corp.example/acme/api/pull/7",
            "https://ghe.corp.example/.x/api/issues/7",
            "https://127.0.0.1/acme/api/issues/7",
            "https://[::1]/acme/api/issues/7",
            "https://localhost/acme/api/issues/7",
            "https://ghe/acme/api/issues/7",
            "http://ghe.corp.example/acme/api/issues/7",
            "https://u:p@ghe.corp.example/acme/api/issues/7",
            "https://acme.atlassian.net:8443/acme/api/issues/7",
            "https://github.com:8443/acme/api/issues/7",
        ] {
            assert_eq!(infer(bad), None, "{bad}");
        }
    }
}
