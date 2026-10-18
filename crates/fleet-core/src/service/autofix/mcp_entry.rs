//! Detector 2: the `claude-fleet` entry in a host's global `~/.claude.json`
//! is missing, points at another URL, lacks the `X-Fleet-Pane` header, or
//! carries a bearer that is not the host's own token.
//!
//! The runner probes each reachable host at most every
//! [`PROBE_EVERY_SECS`] ([`probe_script`]); the scan reads the cached
//! results, so it stays pure. The probe hashes the bearer ON THE HOST: the
//! token is piped straight into `sha256sum` (or hashed inside python), and
//! only the hash crosses the wire.

use super::{token_label, Finding, FindingBody, Fix, FixPlan, Safety, ScanCx, Severity};
use crate::ipc_error::IpcError;
use crate::ssh::SshExec;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// How often one host's entry is read again.
pub const PROBE_EVERY_SECS: i64 = 600;
/// One probe's SSH budget.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// sha256 of the empty string: the entry carries no bearer at all.
const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// What a host's entry looks like, as the probe reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryProbe {
    /// No `~/.claude.json`, or no `claude-fleet` entry in it.
    Missing,
    /// `~/.claude.json` is not a JSON object.
    ParseError,
    Entry {
        url: Option<String>,
        has_pane: bool,
        /// sha256 of the bearer, computed on the host.
        bearer_sha: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProbe {
    pub alias: String,
    pub at: i64,
    pub probe: EntryProbe,
}

/// The probe, run with `bash -lc`. Prints `@@AF …` lines only; never the
/// token, which goes from `jq -j` into the hash on the host's side of the
/// pipe (or is hashed inside python). `None` of jq and python3: `no_tool`,
/// and the host is not judged.
pub fn probe_script() -> String {
    r#"f="$HOME/.claude.json"
if [ ! -s "$f" ]; then echo '@@AF missing'; exit 0; fi
if command -v sha256sum >/dev/null 2>&1; then af_h() { sha256sum; }; else af_h() { shasum -a 256; }; fi
if command -v jq >/dev/null 2>&1; then
  if ! jq -e 'type == "object"' "$f" >/dev/null 2>&1; then echo '@@AF parse_error'; exit 0; fi
  printf '@@AF entry '
  jq -c '(.mcpServers // {})["claude-fleet"] // null | if type != "object" then null else {url: (.url // null), has_pane: ((.headers // {})["X-Fleet-Pane"] != null)} end' "$f"
  printf '@@AF sha '
  jq -j '((.mcpServers // {})["claude-fleet"].headers.Authorization // "") | tostring | sub("^Bearer "; "")' "$f" 2>/dev/null | af_h | cut -c1-64
elif command -v python3 >/dev/null 2>&1; then
  python3 - "$f" <<'PY'
import hashlib, json, sys
try:
    d = json.load(open(sys.argv[1]))
    assert isinstance(d, dict)
except Exception:
    print("@@AF parse_error")
    sys.exit(0)
s = d.get("mcpServers")
e = s.get("claude-fleet") if isinstance(s, dict) else None
if not isinstance(e, dict):
    print("@@AF entry null")
    sys.exit(0)
h = e.get("headers") if isinstance(e.get("headers"), dict) else {}
print("@@AF entry " + json.dumps({"url": e.get("url"), "has_pane": h.get("X-Fleet-Pane") is not None}))
a = str(h.get("Authorization") or "")
if a.startswith("Bearer "):
    a = a[7:]
print("@@AF sha " + hashlib.sha256(a.encode()).hexdigest())
PY
else
  echo '@@AF no_tool'
fi
"#
    .to_string()
}

/// PURE: read the probe's output. `None`: it could not tell (no jq or
/// python3 on the host, or output it does not recognise).
pub fn parse_probe(stdout: &str) -> Option<EntryProbe> {
    let mut entry: Option<Option<serde_json::Value>> = None;
    let mut sha: Option<String> = None;
    for line in stdout.lines() {
        let Some(rest) = line.trim_end().strip_prefix("@@AF ") else {
            continue;
        };
        match rest {
            "missing" => return Some(EntryProbe::Missing),
            "parse_error" => return Some(EntryProbe::ParseError),
            "no_tool" => return None,
            _ => {}
        }
        if let Some(j) = rest.strip_prefix("entry ") {
            let v: serde_json::Value = serde_json::from_str(j).ok()?;
            entry = Some((!v.is_null()).then_some(v));
        } else if let Some(h) = rest.strip_prefix("sha ") {
            let h = h.trim();
            if h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
                sha = Some(h.to_ascii_lowercase());
            }
        }
    }
    match entry? {
        None => Some(EntryProbe::Missing),
        Some(v) => Some(EntryProbe::Entry {
            url: v.get("url").and_then(|u| u.as_str()).map(str::to_string),
            has_pane: v.get("has_pane").and_then(|b| b.as_bool()).unwrap_or(false),
            bearer_sha: sha?,
        }),
    }
}

/// Probe one host. `Ok(None)`: the host answered but could not be judged.
pub async fn probe(ssh: &dyn SshExec, host: &str) -> Result<Option<EntryProbe>, IpcError> {
    crate::validate::host_alias(host)?;
    let out = crate::ssh::run_shell(ssh, host, &probe_script(), PROBE_TIMEOUT).await?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(parse_probe(&String::from_utf8_lossy(&out.stdout)))
}

/// Per host: when it was last probed, and what was seen (`None`: it could
/// not be judged). Kept in memory; a hub restart probes again.
type Seen = BTreeMap<String, (i64, Option<EntryProbe>)>;
static CACHE: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Every cached probe that saw something.
pub fn cached() -> Vec<HostProbe> {
    CACHE
        .lock()
        .map(|c| {
            c.iter()
                .filter_map(|(alias, (at, p))| {
                    p.clone().map(|probe| HostProbe {
                        alias: alias.clone(),
                        at: *at,
                        probe,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `alias` is due a probe (a failed one counts: it is not retried
/// every pass).
pub fn due(alias: &str, now: i64) -> bool {
    CACHE.lock().is_ok_and(|c| {
        c.get(alias)
            .is_none_or(|(at, _)| now - at >= PROBE_EVERY_SECS)
    })
}

/// Record a probe of `alias` at `at`; `None`: it could not be judged, and
/// the host is left out of the scan until it can.
pub fn remember(alias: &str, at: i64, probe: Option<EntryProbe>) {
    if let Ok(mut c) = CACHE.lock() {
        c.insert(alias.to_string(), (at, probe));
    }
}

fn rewrite(alias: &str, safety: Safety, why: &str) -> Fix {
    Fix {
        summary: format!("Repair fleet's entry in {alias}'s ~/.claude.json"),
        changes: vec![
            format!(
                "Rewrites the `claude-fleet` server in {alias}'s ~/.claude.json with this hub's \
                 address, the session header and {alias}'s own token. {why}"
            ),
            "Keeps every other key in the file, and saves the old file as \
             ~/.claude.json.fleet-bak first."
                .into(),
        ],
        safety,
        reversible: true,
        auto: safety == Safety::Safe,
        plan: FixPlan::RewriteMcpEntry {
            host: alias.to_string(),
        },
    }
}

/// Whose token the entry carries, in words, never the full hash.
fn whose(cx: &ScanCx, alias: &str, sha: &str) -> String {
    if sha == EMPTY_SHA {
        return "the entry sends no token at all".into();
    }
    let short = &sha[..sha.len().min(12)];
    if let Some((h, _)) = cx
        .host_token_shas
        .iter()
        .find(|(h, t)| h != alias && t == sha)
    {
        return format!("it carries {h}'s host token (sha256 {short}…), not {alias}'s");
    }
    if cx.operator_sha.as_deref() == Some(sha) {
        return format!("it carries Control's token (sha256 {short}…)");
    }
    if let Some(t) = cx.client_tokens.iter().find(|t| t.token_sha256 == sha) {
        return format!("it carries the client token {}", token_label(&t.name, sha));
    }
    if let Some(t) = cx
        .revoked_client_tokens
        .iter()
        .find(|t| t.token_sha256 == sha)
    {
        return format!(
            "it carries the revoked client token {}",
            token_label(&t.name, sha)
        );
    }
    format!("it carries a token this hub does not know (sha256 {short}…), perhaps an old one of {alias}'s")
}

fn finding(
    alias: &str,
    variant: &str,
    title: String,
    explanation: &str,
    evidence: Vec<String>,
    fix: Option<Fix>,
) -> Finding {
    Finding {
        kind: format!("host_mcp_entry.{variant}"),
        subject: format!("host:{alias}"),
        host_alias: Some(alias.to_string()),
        severity: Severity::Degraded,
        body: FindingBody {
            title,
            explanation: explanation.into(),
            evidence,
            fix,
            action: None,
        },
    }
}

pub fn scan(cx: &ScanCx) -> Vec<Finding> {
    let Some(url) = cx.mcp_url.as_deref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in &cx.mcp_probes {
        let alias = p.alias.as_str();
        let reachable = cx.health.iter().any(|h| h.alias == alias && h.reachable);
        let Some((_, own)) = cx.host_token_shas.iter().find(|(h, _)| h == alias) else {
            continue;
        };
        if !reachable {
            continue;
        }
        match &p.probe {
            EntryProbe::Missing => out.push(finding(
                alias,
                "missing",
                format!("Claude Code on {alias} is not connected to fleet"),
                "The fleet server is missing from this host's ~/.claude.json, so sessions there \
                 cannot use fleet's tools (they cannot see other sessions, report status or ask \
                 for help). Adding the entry back connects them again; sessions pick it up when \
                 they next start.",
                vec![format!(
                    "{alias}'s ~/.claude.json has no `claude-fleet` server"
                )],
                Some(rewrite(alias, Safety::Safe, "Nothing else changes.")),
            )),
            EntryProbe::ParseError => out.push(finding(
                alias,
                "parse_error",
                format!("{alias}'s ~/.claude.json cannot be read"),
                "The file is not valid JSON, so fleet cannot check or repair its entry, and \
                 Claude Code may not start properly there. Fleet never overwrites a file it \
                 cannot read: open ~/.claude.json on that host and fix it (a backup may be in \
                 ~/.claude.json.fleet-bak), and this check runs again within 10 minutes.",
                vec![format!("{alias}'s ~/.claude.json is not a JSON object")],
                None,
            )),
            EntryProbe::Entry {
                url: got,
                has_pane,
                bearer_sha,
            } => {
                if bearer_sha != own {
                    out.push(finding(
                        alias,
                        "token",
                        format!("Claude Code on {alias} signs in to fleet with the wrong token"),
                        "Fleet tells hosts apart by their token. This host's entry carries a token \
                         that is not its own, so its sessions act as someone else, or are refused. \
                         The fix puts this host's own token back. It changes who the host's \
                         sessions are to fleet, so it waits for you.",
                        vec![whose(cx, alias, bearer_sha)],
                        Some(rewrite(
                            alias,
                            Safety::NeedsApproval,
                            "The token it carries now stops working on this host.",
                        )),
                    ));
                    continue;
                }
                let mut evidence = Vec::new();
                if got.as_deref() != Some(url) {
                    evidence.push(format!(
                        "it points at {}, this hub is at {url}",
                        got.as_deref().unwrap_or("no address")
                    ));
                }
                if !has_pane {
                    evidence.push(
                        "it does not send the session header (X-Fleet-Pane), so fleet cannot tell \
                         which session a call comes from"
                            .into(),
                    );
                }
                if !evidence.is_empty() {
                    out.push(finding(
                        alias,
                        "header",
                        format!("{alias}'s fleet connection is out of date"),
                        "The fleet entry in this host's ~/.claude.json was written by an older \
                         fleet or for another address. Sessions there may not reach fleet, or \
                         fleet cannot tell which session is calling, so rules about a session's \
                         own work refuse them. Rewriting the entry brings it up to date.",
                        evidence,
                        Some(rewrite(alias, Safety::Safe, "The token stays the same.")),
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::health::HostHealthRow;
    use crate::store::ClientTokenRow;

    const URL: &str = "https://fleet.example/mcp";
    const H1_SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2_SHA: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn cx(probe: EntryProbe) -> ScanCx {
        ScanCx {
            now: 1000,
            health: vec![HostHealthRow {
                alias: "h1".into(),
                reachable: true,
                ..Default::default()
            }],
            mcp_url: Some(URL.into()),
            mcp_probes: vec![HostProbe {
                alias: "h1".into(),
                at: 900,
                probe,
            }],
            host_token_shas: vec![("h1".into(), H1_SHA.into()), ("h2".into(), H2_SHA.into())],
            ..Default::default()
        }
    }

    fn entry(url: &str, pane: bool, sha: &str) -> EntryProbe {
        EntryProbe::Entry {
            url: Some(url.into()),
            has_pane: pane,
            bearer_sha: sha.into(),
        }
    }

    fn only(cx: &ScanCx) -> Finding {
        let mut f = scan(cx);
        assert_eq!(f.len(), 1, "{f:?}");
        f.remove(0)
    }

    #[test]
    fn a_healthy_entry_is_no_finding() {
        assert!(scan(&cx(entry(URL, true, H1_SHA))).is_empty());
    }

    #[test]
    fn an_entry_without_the_pane_header_is_a_safe_rewrite() {
        let f = only(&cx(entry(URL, false, H1_SHA)));
        assert_eq!(f.kind, "host_mcp_entry.header");
        assert_eq!(f.subject, "host:h1");
        let fix = f.body.fix.unwrap();
        assert_eq!(fix.safety, Safety::Safe);
        assert!(fix.auto);
        assert_eq!(fix.plan, FixPlan::RewriteMcpEntry { host: "h1".into() });
    }

    #[test]
    fn an_entry_with_another_url_is_a_safe_rewrite() {
        let f = only(&cx(entry("http://127.0.0.1:4180/mcp", true, H1_SHA)));
        assert_eq!(f.kind, "host_mcp_entry.header");
        assert!(
            f.body.evidence.iter().any(|e| e.contains("127.0.0.1:4180")),
            "{:?}",
            f.body.evidence
        );
        assert_eq!(f.body.fix.unwrap().safety, Safety::Safe);
    }

    #[test]
    fn another_hosts_token_needs_approval_and_names_whose_it_is() {
        let f = only(&cx(entry(URL, true, H2_SHA)));
        assert_eq!(f.kind, "host_mcp_entry.token");
        let fix = f.body.fix.as_ref().unwrap();
        assert_eq!(fix.safety, Safety::NeedsApproval);
        assert!(!fix.auto);
        assert_eq!(fix.plan, FixPlan::RewriteMcpEntry { host: "h1".into() });
        let ev = f.body.evidence.join("\n");
        assert!(ev.contains("h2's host token"), "{ev}");
        assert!(!ev.contains(H2_SHA), "never the full hash: {ev}");
    }

    #[test]
    fn the_operators_token_on_a_host_is_named() {
        let mut c = cx(entry(URL, true, "ab".repeat(32).as_str()));
        c.operator_sha = Some("ab".repeat(32));
        let f = only(&c);
        assert_eq!(f.kind, "host_mcp_entry.token");
        assert!(f.body.evidence.join(" ").contains("Control's token"));
    }

    #[test]
    fn a_revoked_client_token_is_named() {
        let mut c = cx(entry(URL, true, "cd".repeat(32).as_str()));
        c.revoked_client_tokens = vec![ClientTokenRow {
            id: 7,
            name: "old-laptop".into(),
            token_sha256: "cd".repeat(32),
            mode: "full".into(),
            created_at: 0,
            last_seen_at: None,
            revoked_at: Some(5),
            trusted_at: None,
            org_id: None,
            assets_admin_at: None,
            person_id: None,
        }];
        let ev = only(&c).body.evidence.join(" ");
        assert!(ev.contains("old-laptop") && ev.contains("revoked"), "{ev}");
    }

    #[test]
    fn an_entry_with_no_bearer_is_a_token_finding() {
        let f = only(&cx(entry(URL, true, EMPTY_SHA)));
        assert_eq!(f.kind, "host_mcp_entry.token");
        assert!(f.body.evidence.join(" ").contains("no token"));
    }

    #[test]
    fn a_missing_entry_is_a_safe_rewrite() {
        let f = only(&cx(EntryProbe::Missing));
        assert_eq!(f.kind, "host_mcp_entry.missing");
        let fix = f.body.fix.unwrap();
        assert_eq!((fix.safety, fix.auto), (Safety::Safe, true));
    }

    #[test]
    fn an_unreadable_file_is_explained_and_never_rewritten() {
        let f = only(&cx(EntryProbe::ParseError));
        assert_eq!(f.kind, "host_mcp_entry.parse_error");
        assert!(f.body.fix.is_none());
    }

    #[test]
    fn an_unreachable_host_a_host_without_a_token_or_an_unknown_url_is_not_judged() {
        let mut c = cx(EntryProbe::Missing);
        c.health[0].reachable = false;
        assert!(scan(&c).is_empty(), "unreachable");
        let mut c = cx(EntryProbe::Missing);
        c.host_token_shas.retain(|(h, _)| h != "h1");
        assert!(scan(&c).is_empty(), "no stored token to write");
        let mut c = cx(entry(URL, false, H1_SHA));
        c.mcp_url = None;
        assert!(scan(&c).is_empty(), "no URL to compare with");
    }

    #[test]
    fn the_probe_output_parses() {
        let sha = "a".repeat(64);
        assert_eq!(
            parse_probe(&format!(
                "motd noise\n@@AF entry {{\"url\":\"{URL}\",\"has_pane\":true}}\n@@AF sha {sha}\n"
            )),
            Some(entry(URL, true, &sha))
        );
        assert_eq!(parse_probe("@@AF missing\n"), Some(EntryProbe::Missing));
        assert_eq!(
            parse_probe(&format!("@@AF entry null\n@@AF sha {EMPTY_SHA}\n")),
            Some(EntryProbe::Missing)
        );
        assert_eq!(
            parse_probe("@@AF parse_error\n"),
            Some(EntryProbe::ParseError)
        );
        assert_eq!(parse_probe("@@AF no_tool\n"), None);
        assert_eq!(parse_probe("@@AF entry {\"url\":\"u\"}\n"), None, "no hash");
        assert_eq!(parse_probe(""), None);
    }

    #[test]
    fn the_probe_script_never_prints_the_bearer() {
        let script = probe_script();
        for line in script.lines().filter(|l| l.contains("Authorization")) {
            assert!(
                line.contains("| af_h") || !line.contains("jq"),
                "the jq bearer read must pipe into the hash: {line}"
            );
        }
    }

    /// Runs the real script against a fixture `~/.claude.json` (needs bash
    /// and jq or python3; skipped when neither tool exists). Unix only, like
    /// the other host-script tests: the script runs on a Unix host, and a
    /// Windows runner's `bash` cannot read the native `HOME` path.
    #[cfg(unix)]
    #[test]
    fn the_probe_hashes_the_bearer_on_the_host_and_prints_only_the_hash() {
        let has = |t: &str| {
            crate::proc::std_command("sh")
                .args(["-c", &format!("command -v {t}")])
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !(has("bash") && (has("jq") || has("python3"))) {
            eprintln!("skipped: no bash, or neither jq nor python3");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let secret = "SECRET-host-token-0123456789";
        std::fs::write(
            home.path().join(".claude.json"),
            serde_json::json!({"mcpServers": {"claude-fleet": {
                "type": "http", "url": URL,
                "headers": {"Authorization": format!("Bearer {secret}"), "X-Fleet-Pane": "${TMUX_PANE:-}"}
            }}})
            .to_string(),
        )
        .unwrap();
        let out = crate::proc::std_command("bash")
            .args(["-c", &probe_script()])
            .env("HOME", home.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains(secret), "{stdout}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains(secret));
        assert_eq!(
            parse_probe(&stdout),
            Some(entry(URL, true, &crate::mcp::auth::sha256_hex(secret)))
        );
    }
}
