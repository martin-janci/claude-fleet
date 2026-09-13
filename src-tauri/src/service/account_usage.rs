//! Per-account 5-hour and weekly usage (spec
//! `docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md`).
//!
//! Usage belongs to a Claude account, not a host. The only source is the
//! undocumented `GET https://api.anthropic.com/api/oauth/usage`, which needs
//! the account's OAuth access token. That token must never leave the host it
//! lives on, so fleet does not fetch the endpoint itself: it runs
//! [`usage_script`] ON a host logged in to the account (over SSH, via
//! `bash -lc`). The script reads the token there, hands it to `curl` through a
//! private header file, and prints only machine-readable markers, the HTTP
//! status and the response body. Fleet's process never holds the token.
//!
//! Security invariants (enforced by tests on the script text and by a test
//! that runs the script locally against a fake `curl` and fake credentials):
//! - the token never reaches stdout, stderr, argv or a shell variable;
//! - the token is never refreshed and `.credentials.json` is never written
//!   (Claude Code refreshes it itself; racing it would corrupt the login);
//! - the macOS Keychain is never read (a macOS host simply has no
//!   credentials file and reports `no_credentials`);
//! - the `User-Agent` is the honest `claude-fleet/<version>`.
//!
//! Polling is gentle: at most one attempt per account per
//! [`USAGE_POLL_FLOOR_SECS`], doubling backoff up to [`USAGE_BACKOFF_CAP_SECS`]
//! on endpoint trouble, and `Retry-After` honoured on 429. The floor is never
//! bypassed, not even by a forced refresh.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::HostRow;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// Minimum seconds between two usage attempts for one account.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub const USAGE_POLL_FLOOR_SECS: i64 = 300;
/// Ceiling of the doubling backoff.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub const USAGE_BACKOFF_CAP_SECS: i64 = 1800;

/// The only URL the script ever calls.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// SSH connect budget for one host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-script budget (curl itself is capped at 15 s).
const WALL_CLOCK: Duration = Duration::from_secs(25);
/// Longest `snippet` kept from a response body.
const SNIPPET_MAX_CHARS: usize = 200;
/// Longest `detail` kept on a snapshot.
const DETAIL_MAX_CHARS: usize = 300;
/// A `Retry-After` beyond a day is treated as a day.
const RETRY_AFTER_MAX_SECS: i64 = 86_400;
/// The script treats an access token expiring within this many seconds as
/// already expired, so a request never races the expiry.
const ACCESS_TOKEN_SKEW_SECS: i64 = 60;

const USER_AGENT_PLACEHOLDER: &str = "@@USER_AGENT@@";

/// The script run on the host. Placeholder `@@USER_AGENT@@` is replaced by the
/// `shell::quote`d User-Agent. Read the "where is the token" notes in
/// [`usage_script`] before changing a line.
const SCRIPT_TEMPLATE: &str = r#"set +x
umask 077
cred="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"
if [ ! -f "$cred" ]; then echo __no_credentials__; exit 0; fi
if command -v python3 >/dev/null 2>&1; then json=python3
elif command -v jq >/dev/null 2>&1; then json=jq
else echo __host_unsupported__=python3_or_jq; exit 0; fi
if ! command -v curl >/dev/null 2>&1; then echo __host_unsupported__=curl; exit 0; fi
dir=$(mktemp -d 2>/dev/null) || { echo __host_unsupported__=mktemp; exit 0; }
trap 'rm -rf "$dir"' EXIT
trap 'exit 1' HUP INT TERM
if [ "$json" = python3 ]; then
py='import json, os, re, sys, time
def secs(v):
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return None
    v = int(v)
    return v // 1000 if v > 10 ** 12 else v
def label(v):
    return re.sub(r"[^A-Za-z0-9_.-]", "", v)[:32] if isinstance(v, str) else ""
try:
    with open(sys.argv[1]) as f:
        o = json.load(f).get("claudeAiOauth")
except Exception:
    o = None
if not isinstance(o, dict):
    print("__no_credentials__")
    sys.exit(0)
exp = secs(o.get("expiresAt"))
rexp = secs(o.get("refreshTokenExpiresAt"))
print("__expires_at__=" + ("" if exp is None else str(exp)))
print("__refresh_expires_at__=" + ("" if rexp is None else str(rexp)))
print("__subscription__=" + label(o.get("subscriptionType")))
print("__rate_limit_tier__=" + label(o.get("rateLimitTier")))
now = int(time.time())
tok = o.get("accessToken")
if not isinstance(tok, str) or not tok or "\r" in tok or "\n" in tok:
    print("__no_credentials__")
elif rexp is not None and rexp <= now:
    print("__login_expired__")
elif exp is not None and exp <= now + @@SKEW@@:
    print("__access_token_expired__")
else:
    try:
        fd = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as out:
            out.write("Authorization: Bearer " + tok + "\n")
    except Exception:
        print("__host_unsupported__=tempfile")'
python3 -c "$py" "$cred" "$dir/auth" 2>/dev/null
else
meta=$(jq -r 'def secs: if type == "number" then (if . > 1000000000000 then . / 1000 else . end | floor | tostring) else "" end;
def safelabel: if type == "string" then (gsub("[^A-Za-z0-9_.-]"; "") | .[0:32]) else "" end;
(.claudeAiOauth // {}) as $o
| "__has_token__=" + (if ($o.accessToken | type) == "string" and ($o.accessToken | length) > 0 then "1" else "0" end),
  "__expires_at__=" + ($o.expiresAt | secs),
  "__refresh_expires_at__=" + ($o.refreshTokenExpiresAt | secs),
  "__subscription__=" + ($o.subscriptionType | safelabel),
  "__rate_limit_tier__=" + ($o.rateLimitTier | safelabel)' "$cred" 2>/dev/null)
field() { printf '%s\n' "$meta" | sed -n "s/^__$1__=//p" | head -n 1; }
printf '%s\n' "$meta" | grep -v '^__has_token__='
now=$(date +%s)
exp=$(field expires_at)
rexp=$(field refresh_expires_at)
case "$exp" in *[!0-9]*) exp= ;; esac
case "$rexp" in *[!0-9]*) rexp= ;; esac
if [ "$(field has_token)" != 1 ]; then echo __no_credentials__; exit 0; fi
if [ -n "$rexp" ] && [ "$rexp" -le "$now" ]; then echo __login_expired__; exit 0; fi
if [ -n "$exp" ] && [ "$exp" -le "$((now + @@SKEW@@))" ]; then echo __access_token_expired__; exit 0; fi
jq -r '.claudeAiOauth.accessToken | select(type == "string" and length > 0 and (test("[\r\n]") | not)) | "Authorization: Bearer " + .' "$cred" > "$dir/auth" 2>/dev/null
fi
if [ ! -s "$dir/auth" ]; then echo __no_credentials__; exit 0; fi
code=$(curl -sS --max-time 15 -H @"$dir/auth" -H 'anthropic-beta: oauth-2025-04-20' -A @@USER_AGENT@@ -D "$dir/headers" -o "$dir/body" -w '%{http_code}' @@URL@@ 2>"$dir/err")
rm -f "$dir/auth"
ra=$(tr -d '\r' < "$dir/headers" 2>/dev/null | sed -n 's/^[Rr][Ee][Tt][Rr][Yy]-[Aa][Ff][Tt][Ee][Rr]:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*$/\1/p' | tail -n 1)
case "$code" in [0-9][0-9][0-9]) ;; *) code=000 ;; esac
echo "__http_status__=$code"
if [ -n "$ra" ]; then echo "__retry_after__=$ra"; fi
echo __body__
if [ "$code" = 000 ]; then head -c 2000 "$dir/err" 2>/dev/null; else head -c 65536 "$dir/body" 2>/dev/null; fi
exit 0
"#;

/// The honest User-Agent: `claude-fleet/<version>`. Never Claude Code's.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub fn user_agent() -> String {
    format!("claude-fleet/{}", env!("CARGO_PKG_VERSION"))
}

/// Build the usage script for `bash -lc` on a host.
///
/// Where the token is (and is not):
/// - `cred=…` holds only the credentials file PATH.
/// - python path: one `python3 -c "$py"` process reads the file, prints the
///   NON-secret markers (`__expires_at__`, `__refresh_expires_at__`,
///   `__subscription__`, `__rate_limit_tier__`), decides login / access-token
///   expiry on the host's own clock, and — only when the endpoint should be
///   called — writes `Authorization: Bearer <token>` straight into
///   `$dir/auth` (`O_EXCL`, mode 0600). The token lives only in that
///   process's memory and that file; `$py` is program text, its argv is two
///   paths.
/// - jq path (no python3): `meta` holds only the non-secret markers (the
///   token is reduced to `__has_token__=1|0` inside jq); the expiry decision
///   is shell arithmetic on the non-secret epochs; a second `jq` writes the
///   header line straight into `$dir/auth` via a redirect.
/// - `umask 077` precedes everything; `$dir` is `mktemp -d` (0700) and
///   `trap 'rm -rf "$dir"' EXIT` removes it however the script ends (a
///   HUP/INT/TERM, e.g. the SSH wall clock, becomes an exit that runs it);
///   `$dir/auth` is also removed right after `curl` returns.
/// - `curl` gets the token ONLY through `-H @"$dir/auth"` (curl reads the
///   file itself; `ps` shows only the path). No `-v`, no `--trace`.
/// - stderr of python/jq is discarded; curl's stderr (connection errors,
///   never headers) is shown only when there was no HTTP response.
/// - stdout carries markers, the HTTP status, `Retry-After` from the saved
///   RESPONSE headers, and the response body — none of which contain the
///   token.
/// - Nothing refreshes the token and nothing writes `.credentials.json`.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub fn usage_script(user_agent: &str) -> String {
    SCRIPT_TEMPLATE
        .replace("@@SKEW@@", &ACCESS_TOKEN_SKEW_SECS.to_string())
        .replace("@@URL@@", USAGE_URL)
        .replace(USER_AGENT_PLACEHOLDER, &quote(user_agent))
}

/// One usage window. `utilization` is percent USED, clamped to `0..=100`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub struct Window {
    pub utilization: f64,
    /// Unix seconds; `None` when absent or not RFC 3339.
    pub resets_at: Option<i64>,
}

/// The endpoint's buckets. Any may be absent.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub struct AccountUsage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub seven_day_opus: Option<Window>,
    pub seven_day_sonnet: Option<Window>,
}

/// What one host's script run said.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub enum UsageOutcome {
    Ok {
        usage: AccountUsage,
        subscription: Option<String>,
    },
    /// No credentials file (a macOS host keeps its token in the Keychain,
    /// which fleet never reads) or no usable token in it. Try another host.
    NoCredentials,
    /// The access token expired but the login is valid; Claude Code refreshes
    /// it the next time it runs there. Benign — try another host.
    AccessTokenExpired,
    /// The refresh token expired: that host needs `claude /login`.
    LoginExpired,
    /// HTTP 401/403 despite a non-expired token.
    TokenRejected,
    /// HTTP 429.
    RateLimited { retry_after_secs: Option<i64> },
    /// Any other non-2xx, no HTTP response, or a 2xx with an unexpected
    /// shape. `snippet` is at most 200 characters of the response body (or
    /// of curl's own error when there was no response).
    Unavailable {
        status: Option<u16>,
        snippet: String,
    },
    /// The host cannot run the check (no python3/jq, no curl, no temp dir, or
    /// output without any usage marker). Try another host.
    HostUnsupported { detail: String },
}

impl UsageOutcome {
    pub fn kind(&self) -> UsageOutcomeKind {
        match self {
            Self::Ok { .. } => UsageOutcomeKind::Ok,
            Self::NoCredentials => UsageOutcomeKind::NoCredentials,
            Self::AccessTokenExpired => UsageOutcomeKind::AccessTokenExpired,
            Self::LoginExpired => UsageOutcomeKind::LoginExpired,
            Self::TokenRejected => UsageOutcomeKind::TokenRejected,
            Self::RateLimited { .. } => UsageOutcomeKind::RateLimited,
            Self::Unavailable { .. } => UsageOutcomeKind::Unavailable,
            Self::HostUnsupported { .. } => UsageOutcomeKind::HostUnsupported,
        }
    }

    /// A problem with the HOST (its credentials or tooling), so another host
    /// on the same account may still answer. `Ok`, `RateLimited` and
    /// `Unavailable` are about the account or the endpoint: asking another
    /// host would only multiply requests.
    fn is_host_specific(&self) -> bool {
        matches!(
            self,
            Self::NoCredentials
                | Self::AccessTokenExpired
                | Self::LoginExpired
                | Self::TokenRejected
                | Self::HostUnsupported { .. }
        )
    }
}

/// The wire status of an account's usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub enum UsageOutcomeKind {
    Ok,
    NoCredentials,
    AccessTokenExpired,
    LoginExpired,
    TokenRejected,
    RateLimited,
    Unavailable,
    HostUnsupported,
    NoOnlineHost,
    NeverFetched,
}

impl UsageOutcomeKind {
    /// Short human text used in `detail` notes.
    fn describe(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoCredentials => "no credentials file",
            Self::AccessTokenExpired => "access token expired",
            Self::LoginExpired => "login expired",
            Self::TokenRejected => "token rejected",
            Self::RateLimited => "rate limited",
            Self::Unavailable => "usage endpoint unavailable",
            Self::HostUnsupported => "host cannot run the usage check",
            Self::NoOnlineHost => "no online host",
            Self::NeverFetched => "never fetched",
        }
    }

    /// When every host failed for a host-specific reason, the one to report:
    /// the most actionable first.
    fn host_state_rank(self) -> u8 {
        match self {
            Self::TokenRejected => 5,
            Self::LoginExpired => 4,
            Self::AccessTokenExpired => 3,
            Self::HostUnsupported => 2,
            Self::NoCredentials => 1,
            _ => 0,
        }
    }
}

const TERMINAL_NO_CREDENTIALS: &str = "__no_credentials__";
const TERMINAL_LOGIN_EXPIRED: &str = "__login_expired__";
const TERMINAL_ACCESS_EXPIRED: &str = "__access_token_expired__";
const MARK_HOST_UNSUPPORTED: &str = "__host_unsupported__=";
const MARK_HTTP_STATUS: &str = "__http_status__=";
const MARK_RETRY_AFTER: &str = "__retry_after__=";
const MARK_SUBSCRIPTION: &str = "__subscription__=";
const MARK_BODY: &str = "__body__";

/// Classify the script's stdout.
///
/// Only exact marker lines before `__body__` are read; anything else there (a
/// login-shell banner, an unknown marker) is ignored and never copied into
/// the outcome. The first terminal marker wins. The only free text that can
/// reach the outcome is `snippet`, taken from what follows `__body__` — the
/// endpoint's response body (or curl's connection error when there was no
/// response), which never contains the request's Authorization header.
///
/// `_now_unix` is accepted for the fetch layer's uniform signature; expiry
/// decisions are made on the host with the host's clock.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub fn parse_usage_output(stdout: &str, _now_unix: i64) -> UsageOutcome {
    let mut terminal: Option<UsageOutcome> = None;
    let mut status: Option<u16> = None;
    let mut saw_status = false;
    let mut retry_after: Option<i64> = None;
    let mut subscription: Option<String> = None;
    let mut body = String::new();

    let mut rest = stdout;
    loop {
        let (line, next) = match rest.find('\n') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let l = line.trim_end_matches('\r');
        if l == MARK_BODY {
            body = next.unwrap_or("").to_string();
            break;
        }
        if terminal.is_none() {
            if l == TERMINAL_NO_CREDENTIALS {
                terminal = Some(UsageOutcome::NoCredentials);
            } else if l == TERMINAL_LOGIN_EXPIRED {
                terminal = Some(UsageOutcome::LoginExpired);
            } else if l == TERMINAL_ACCESS_EXPIRED {
                terminal = Some(UsageOutcome::AccessTokenExpired);
            } else if let Some(what) = l.strip_prefix(MARK_HOST_UNSUPPORTED) {
                terminal = Some(UsageOutcome::HostUnsupported {
                    detail: match safe_label(what) {
                        Some(w) => format!("missing {w}"),
                        None => "missing a required tool".to_string(),
                    },
                });
            }
        }
        if let Some(v) = l.strip_prefix(MARK_HTTP_STATUS) {
            if !saw_status {
                saw_status = true;
                status = v.trim().parse::<u16>().ok().filter(|c| *c != 0);
            }
        } else if let Some(v) = l.strip_prefix(MARK_RETRY_AFTER) {
            retry_after = parse_retry_after(v);
        } else if let Some(v) = l.strip_prefix(MARK_SUBSCRIPTION) {
            subscription = safe_label(v);
        }
        match next {
            Some(n) => rest = n,
            None => break,
        }
    }

    if let Some(t) = terminal {
        return t;
    }
    if !saw_status {
        return UsageOutcome::HostUnsupported {
            detail: "no usage markers in the host's output".to_string(),
        };
    }
    let snippet = snippet_of(&body);
    match status {
        None => UsageOutcome::Unavailable {
            status: None,
            snippet,
        },
        Some(401) | Some(403) => UsageOutcome::TokenRejected,
        Some(429) => UsageOutcome::RateLimited {
            retry_after_secs: retry_after,
        },
        Some(code) if (200..300).contains(&code) => match parse_usage_body(&body) {
            Some(usage) => UsageOutcome::Ok {
                usage,
                subscription,
            },
            None => UsageOutcome::Unavailable {
                status: Some(code),
                snippet,
            },
        },
        Some(code) => UsageOutcome::Unavailable {
            status: Some(code),
            snippet,
        },
    }
}

/// A short non-secret label (`max`, `pro`, `default_claude_max_20x`…):
/// `[A-Za-z0-9_.-]{1,32}`, else `None`. Long or odd values are dropped rather
/// than truncated so nothing token-shaped can pass through.
fn safe_label(v: &str) -> Option<String> {
    let v = v.trim();
    let ok = !v.is_empty()
        && v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    ok.then(|| v.to_string())
}

fn parse_retry_after(v: &str) -> Option<i64> {
    let v = v.trim();
    if v.is_empty() || v.len() > 10 || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse::<i64>().ok()
}

/// At most [`SNIPPET_MAX_CHARS`] characters of `body`, whitespace collapsed.
fn snippet_of(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SNIPPET_MAX_CHARS)
        .collect()
}

fn parse_usage_body(body: &str) -> Option<AccountUsage> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let obj = v.as_object()?;
    // Recognised = a well-formed window or an explicit `null`.
    let recognised = |k: &str| {
        matches!(obj.get(k), Some(serde_json::Value::Null)) || window_of(obj.get(k)).is_some()
    };
    if !recognised("five_hour") && !recognised("seven_day") {
        return None;
    }
    Some(AccountUsage {
        five_hour: window_of(obj.get("five_hour")),
        seven_day: window_of(obj.get("seven_day")),
        seven_day_opus: window_of(obj.get("seven_day_opus")),
        seven_day_sonnet: window_of(obj.get("seven_day_sonnet")),
    })
}

fn window_of(v: Option<&serde_json::Value>) -> Option<Window> {
    let obj = v?.as_object()?;
    let utilization = obj.get("utilization")?.as_f64()?;
    let resets_at = obj
        .get("resets_at")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_rfc3339);
    Some(Window {
        utilization: utilization.clamp(0.0, 100.0),
        resets_at,
    })
}

/// RFC 3339 (`2026-02-06T22:00:00+00:00`, `…00.123Z`) → unix seconds.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    if !s.is_ascii() || s.len() < 20 {
        return None;
    }
    let digits = |a: usize, b: usize| -> Option<i64> {
        let part = &s[a..b];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute, second) = (digits(11, 13)?, digits(14, 16)?, digits(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut i = 19;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let (oh, om) = (digits(i + 1, i + 3)?, digits(i + 4, i + 6)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let off = oh * 3600 + om * 60;
            if *sign == b'+' {
                off
            } else {
                -off
            }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Hosts to ask for `account_uuid`'s usage, in order: reachable hosts on that
/// account; the sticky host first; `local` last (usually a macOS host whose
/// token is in the Keychain, which fleet never reads); the rest alphabetical.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub fn source_hosts(account_uuid: &str, hosts: &[HostRow], sticky: Option<&str>) -> Vec<String> {
    let local = crate::service::projects::LOCAL_HOST;
    let mut out: Vec<String> = hosts
        .iter()
        .filter(|h| h.reachable && h.account_uuid.as_deref() == Some(account_uuid))
        .map(|h| h.alias.clone())
        .collect();
    out.sort_by(|a, b| {
        let rank = |h: &str| {
            if Some(h) == sticky {
                0
            } else if h == local {
                2
            } else {
                1
            }
        };
        rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
    });
    out.dedup();
    out
}

/// One account's cache entry.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub struct AccountUsageEntry {
    /// Last successful answer: usage, subscription, fetched_at. Kept across
    /// failures so the UI can show last-known values under its staleness
    /// rules.
    pub last_ok: Option<(AccountUsage, Option<String>, i64)>,
    pub last_outcome: Option<UsageOutcomeKind>,
    pub last_detail: Option<String>,
    /// The host that last answered (`Ok`, `RateLimited`, `Unavailable`); tried
    /// first next time so the "via" label does not flap.
    pub source_host: Option<String>,
    pub next_try_at: i64,
    /// 0 = no backoff in force.
    pub backoff_secs: i64,
}

/// In-memory usage cache, per account uuid.
#[derive(Debug, Default)]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub struct UsageCache {
    pub entries: HashMap<String, AccountUsageEntry>,
}

/// What a fetch attempt ended with, before it is written to the cache.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub enum FetchResult {
    /// A host answered with `Ok`, `RateLimited` or `Unavailable`. `notes`
    /// lists hosts skipped before it.
    Answered {
        host: String,
        outcome: UsageOutcome,
        notes: Vec<String>,
    },
    /// Every candidate failed. `host_state` is the most actionable
    /// host-specific outcome seen (`None` when all failures were transport
    /// errors); `transport_error` is true when any host failed in transport.
    AllFailed {
        host_state: Option<UsageOutcomeKind>,
        transport_error: bool,
        notes: Vec<String>,
    },
    /// No reachable host is logged in to the account. No request was made.
    NoOnlineHost,
}

fn next_backoff(current: i64) -> i64 {
    if current <= 0 {
        USAGE_POLL_FLOOR_SECS
    } else {
        (current * 2).min(USAGE_BACKOFF_CAP_SECS)
    }
}

fn cap_detail(s: String) -> String {
    if s.chars().count() <= DETAIL_MAX_CHARS {
        s
    } else {
        s.chars().take(DETAIL_MAX_CHARS).collect()
    }
}

fn join_notes(notes: &[String]) -> Option<String> {
    (!notes.is_empty()).then(|| cap_detail(notes.join("; ")))
}

#[allow(dead_code)] // Wired into the poller and commands in Task 4.
impl UsageCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when `account` may be attempted now (never attempted, or the
    /// floor/backoff has passed).
    pub fn due(&self, account: &str, now: i64) -> bool {
        self.entries
            .get(account)
            .is_none_or(|e| now >= e.next_try_at)
    }

    /// Write one fetch result and schedule the next try.
    pub fn record(&mut self, account: &str, result: FetchResult, now: i64) {
        let e = self.entries.entry(account.to_string()).or_default();
        match result {
            FetchResult::NoOnlineHost => {
                // No request was made: leave the schedule alone so the
                // account is fetched as soon as a host comes back.
                e.last_outcome = Some(UsageOutcomeKind::NoOnlineHost);
                e.last_detail = Some("no reachable host is logged in to this account".to_string());
            }
            FetchResult::Answered {
                host,
                outcome,
                notes,
            } => {
                e.source_host = Some(host);
                e.last_outcome = Some(outcome.kind());
                match outcome {
                    UsageOutcome::Ok {
                        usage,
                        subscription,
                    } => {
                        e.last_ok = Some((usage, subscription, now));
                        e.backoff_secs = 0;
                        e.next_try_at = now + USAGE_POLL_FLOOR_SECS;
                        e.last_detail = join_notes(&notes);
                    }
                    UsageOutcome::RateLimited { retry_after_secs } => {
                        e.backoff_secs = next_backoff(e.backoff_secs);
                        let ra = retry_after_secs.unwrap_or(0).clamp(0, RETRY_AFTER_MAX_SECS);
                        e.next_try_at = now + ra.max(e.backoff_secs);
                        e.last_detail = Some(match retry_after_secs {
                            Some(s) => {
                                format!("rate limited by the usage endpoint (Retry-After: {s}s)")
                            }
                            None => "rate limited by the usage endpoint".to_string(),
                        });
                    }
                    UsageOutcome::Unavailable { status, snippet } => {
                        e.backoff_secs = next_backoff(e.backoff_secs);
                        e.next_try_at = now + e.backoff_secs;
                        let head = match status {
                            Some(code) => format!("HTTP {code}"),
                            None => "no HTTP response".to_string(),
                        };
                        e.last_detail = Some(cap_detail(if snippet.is_empty() {
                            head
                        } else {
                            format!("{head}: {snippet}")
                        }));
                    }
                    // Host-specific outcomes never arrive as `Answered`; treat
                    // one defensively like an all-hosts host-state failure.
                    other => {
                        e.next_try_at = now + USAGE_POLL_FLOOR_SECS;
                        e.last_detail = Some(other.kind().describe().to_string());
                    }
                }
            }
            FetchResult::AllFailed {
                host_state,
                transport_error,
                notes,
            } => {
                e.last_outcome = Some(host_state.unwrap_or(UsageOutcomeKind::NoOnlineHost));
                e.last_detail = join_notes(&notes);
                if transport_error {
                    e.backoff_secs = next_backoff(e.backoff_secs);
                    e.next_try_at = now + e.backoff_secs;
                } else {
                    // Host-state problems: no escalating backoff.
                    e.next_try_at = now + USAGE_POLL_FLOOR_SECS;
                }
            }
        }
    }

    /// What the UI needs for `account`.
    pub fn snapshot(&self, account: &str) -> AccountUsageSnapshot {
        match self.entries.get(account) {
            None => AccountUsageSnapshot {
                account_uuid: account.to_string(),
                usage: None,
                subscription: None,
                fetched_at: None,
                source_host: None,
                status: UsageOutcomeKind::NeverFetched,
                detail: None,
                next_try_at: 0,
            },
            Some(e) => AccountUsageSnapshot {
                account_uuid: account.to_string(),
                usage: e.last_ok.as_ref().map(|(u, _, _)| u.clone()),
                subscription: e.last_ok.as_ref().and_then(|(_, s, _)| s.clone()),
                fetched_at: e.last_ok.as_ref().map(|(_, _, t)| *t),
                source_host: e.source_host.clone(),
                status: e.last_outcome.unwrap_or(UsageOutcomeKind::NeverFetched),
                detail: e.last_detail.clone(),
                next_try_at: e.next_try_at,
            },
        }
    }
}

/// One account's usage as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub struct AccountUsageSnapshot {
    pub account_uuid: String,
    /// Last-known usage (from the last `ok`), even when `status` is a failure.
    pub usage: Option<AccountUsage>,
    pub subscription: Option<String>,
    /// When `usage` was fetched (unix seconds).
    pub fetched_at: Option<i64>,
    pub source_host: Option<String>,
    pub status: UsageOutcomeKind,
    pub detail: Option<String>,
    /// Earliest unix second the next attempt is allowed.
    pub next_try_at: i64,
}

fn lock(cache: &Mutex<UsageCache>) -> std::sync::MutexGuard<'_, UsageCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// First line of `s`, trimmed, at most 160 characters.
fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(160)
        .collect()
}

/// Fetch `account_uuid`'s usage through its hosts, respecting the floor.
///
/// Not due → the cached snapshot, with no SSH call — also when `force` is
/// true: the spec forbids a refresh that bypasses the floor. Otherwise each
/// source host is asked in order; host-specific failures and transport errors
/// move on to the next host, while `Ok`, `RateLimited` and `Unavailable` stop
/// (they concern the account or the endpoint). The cache mutex is never held
/// across an `.await`: the attempt is reserved under one lock, the SSH calls
/// run unlocked, and the result is written under a second lock.
#[allow(dead_code)] // Wired into the poller and commands in Task 4.
pub async fn fetch_account_usage_with(
    account_uuid: &str,
    hosts: &[HostRow],
    ssh: &dyn SshExec,
    cache: &Mutex<UsageCache>,
    now: i64,
    force: bool,
) -> AccountUsageSnapshot {
    // `force` only expresses the caller's intent; it never bypasses the floor.
    let _ = force;
    let candidates = {
        let mut c = lock(cache);
        if !c.due(account_uuid, now) {
            return c.snapshot(account_uuid);
        }
        let sticky = c
            .entries
            .get(account_uuid)
            .and_then(|e| e.source_host.clone());
        let candidates = source_hosts(account_uuid, hosts, sticky.as_deref());
        if candidates.is_empty() {
            c.record(account_uuid, FetchResult::NoOnlineHost, now);
            return c.snapshot(account_uuid);
        }
        // Reserve the attempt so a concurrent caller sees "not due" instead
        // of issuing a second request inside the floor.
        let e = c.entries.entry(account_uuid.to_string()).or_default();
        e.next_try_at = e.next_try_at.max(now + USAGE_POLL_FLOOR_SECS);
        candidates
    };

    let script = usage_script(&user_agent());
    let quoted = quote(&script);
    let mut notes: Vec<String> = Vec::new();
    let mut host_state: Option<UsageOutcomeKind> = None;
    let mut transport_error = false;
    let mut answered: Option<(String, UsageOutcome)> = None;

    for host in &candidates {
        let res: Result<UsageOutcome, IpcError> = match ssh
            .run_bounded(host, &["bash", "-lc", &quoted], CONNECT_TIMEOUT, WALL_CLOCK)
            .await
        {
            Err(e) => Err(e),
            Ok(out) if out.status.success() => Ok(parse_usage_output(
                &String::from_utf8_lossy(&out.stdout),
                now,
            )),
            Ok(out) => {
                let stderr = first_line(&String::from_utf8_lossy(&out.stderr));
                let code = out.status.code().unwrap_or(-1);
                Err(IpcError::new(
                    "E_SSH",
                    if stderr.is_empty() {
                        format!("exit {code}")
                    } else {
                        format!("exit {code}: {stderr}")
                    },
                ))
            }
        };
        match res {
            Err(e) => {
                transport_error = true;
                notes.push(format!("{host}: {}", first_line(&e.message)));
            }
            Ok(outcome) if outcome.is_host_specific() => {
                let kind = outcome.kind();
                let text = match &outcome {
                    UsageOutcome::HostUnsupported { detail } => detail.clone(),
                    _ => kind.describe().to_string(),
                };
                notes.push(format!("{host}: {text}"));
                if host_state.is_none_or(|k| kind.host_state_rank() > k.host_state_rank()) {
                    host_state = Some(kind);
                }
            }
            Ok(outcome) => {
                answered = Some((host.clone(), outcome));
                break;
            }
        }
    }

    let result = match answered {
        Some((host, outcome)) => FetchResult::Answered {
            host,
            outcome,
            notes,
        },
        None => FetchResult::AllFailed {
            host_state,
            transport_error,
            notes,
        },
    };
    let mut c = lock(cache);
    c.record(account_uuid, result, now);
    c.snapshot(account_uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    const FAKE_TOKEN: &str = "sk-ant-oat01-FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE_do_not_leak";
    const NOW: i64 = 1_770_000_000;

    fn host(alias: &str, account: Option<&str>, reachable: bool) -> HostRow {
        HostRow {
            alias: alias.to_string(),
            ssh_alias: None,
            reachable,
            claude_version: None,
            tmux_version: None,
            hidden: false,
            last_pinged_at: None,
            account_uuid: account.map(str::to_string),
            provisioned: true,
        }
    }

    const OK_BODY: &str = r#"{"five_hour":{"utilization":35.0,"resets_at":"2026-02-06T22:00:00+00:00"},"seven_day":{"utilization":14.0,"resets_at":"2026-02-12T20:00:00+00:00"},"seven_day_opus":{"utilization":12.0,"resets_at":"2026-02-12T20:00:00+00:00"},"seven_day_sonnet":null,"extra_usage":{"is_enabled":false}}"#;

    fn ok_output(body: &str) -> String {
        format!(
            "__expires_at__=1999999999\n__refresh_expires_at__=2099999999\n__subscription__=max\n__rate_limit_tier__=default_claude_max_20x\n__http_status__=200\n__body__\n{body}"
        )
    }

    // ── parser ─────────────────────────────────────────────────────────────

    #[test]
    fn parses_ok_with_all_buckets_and_rfc3339_offsets() {
        let out = parse_usage_output(&ok_output(OK_BODY), NOW);
        let UsageOutcome::Ok {
            usage,
            subscription,
        } = out
        else {
            panic!("expected Ok, got {out:?}");
        };
        assert_eq!(subscription.as_deref(), Some("max"));
        let five = usage.five_hour.unwrap();
        assert_eq!(five.utilization, 35.0);
        // 2026-02-06T22:00:00Z
        assert_eq!(five.resets_at, Some(1_770_415_200));
        assert_eq!(usage.seven_day.unwrap().utilization, 14.0);
        assert!(usage.seven_day_opus.is_some());
        assert!(usage.seven_day_sonnet.is_none(), "null bucket → None");
    }

    #[test]
    fn rfc3339_variants() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-02-06T23:30:00+01:30"),
            Some(1_770_415_200)
        );
        assert_eq!(
            parse_rfc3339("2026-02-06T20:00:00-02:00"),
            Some(1_770_415_200)
        );
        assert_eq!(
            parse_rfc3339("2026-02-06T22:00:00.123456+00:00"),
            Some(1_770_415_200)
        );
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for bad in [
            "",
            "tomorrow",
            "2026-02-06 22:00",
            "2026-13-06T22:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-02-06T22:00:00",
            "2026-02-06T22:00:00+0000",
            "2026-02-06T22:00:00.Z",
            "+026-02-06T22:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn unparseable_resets_at_is_none_not_an_error() {
        let body =
            r#"{"five_hour":{"utilization":10,"resets_at":"soon"},"seven_day":{"utilization":5}}"#;
        let UsageOutcome::Ok { usage, .. } = parse_usage_output(&ok_output(body), NOW) else {
            panic!()
        };
        assert_eq!(usage.five_hour.unwrap().resets_at, None);
        assert_eq!(usage.seven_day.unwrap().resets_at, None);
    }

    #[test]
    fn clamps_utilization() {
        let body = r#"{"five_hour":{"utilization":142.5,"resets_at":null},"seven_day":{"utilization":-3}}"#;
        let UsageOutcome::Ok { usage, .. } = parse_usage_output(&ok_output(body), NOW) else {
            panic!()
        };
        assert_eq!(usage.five_hour.unwrap().utilization, 100.0);
        assert_eq!(usage.seven_day.unwrap().utilization, 0.0);
    }

    #[test]
    fn missing_bucket_is_tolerated() {
        let body = r#"{"seven_day":{"utilization":50,"resets_at":"2026-02-12T20:00:00Z"},"new_field":[1,2]}"#;
        let UsageOutcome::Ok { usage, .. } = parse_usage_output(&ok_output(body), NOW) else {
            panic!()
        };
        assert!(usage.five_hour.is_none());
        assert_eq!(usage.seven_day.unwrap().utilization, 50.0);
    }

    #[test]
    fn unexpected_2xx_shape_is_unavailable() {
        for body in [
            r#"{"limits":[]}"#,
            "not json",
            "[1,2,3]",
            r#"{"five_hour":"35%","seven_day":7}"#,
            "",
        ] {
            let out = parse_usage_output(&ok_output(body), NOW);
            assert!(
                matches!(
                    out,
                    UsageOutcome::Unavailable {
                        status: Some(200),
                        ..
                    }
                ),
                "{body}: {out:?}"
            );
        }
    }

    #[test]
    fn terminal_markers() {
        let banner = "Welcome to vps\n";
        assert_eq!(
            parse_usage_output(&format!("{banner}__no_credentials__\n"), NOW),
            UsageOutcome::NoCredentials
        );
        assert_eq!(
            parse_usage_output(
                "__expires_at__=1\n__refresh_expires_at__=2\n__subscription__=pro\n__login_expired__\n",
                NOW
            ),
            UsageOutcome::LoginExpired
        );
        assert_eq!(
            parse_usage_output(
                "__expires_at__=1\n__refresh_expires_at__=2099999999\n__access_token_expired__\n__no_credentials__\n",
                NOW
            ),
            UsageOutcome::AccessTokenExpired,
            "the first terminal marker wins"
        );
        assert_eq!(
            parse_usage_output("__host_unsupported__=curl\n", NOW),
            UsageOutcome::HostUnsupported {
                detail: "missing curl".into()
            }
        );
        assert!(matches!(
            parse_usage_output("bash: something odd\n", NOW),
            UsageOutcome::HostUnsupported { .. }
        ));
    }

    #[test]
    fn http_statuses() {
        let with = |status: &str, extra: &str, body: &str| {
            parse_usage_output(
                &format!("__subscription__=max\n__http_status__={status}\n{extra}__body__\n{body}"),
                NOW,
            )
        };
        assert_eq!(with("401", "", "{}"), UsageOutcome::TokenRejected);
        assert_eq!(with("403", "", "{}"), UsageOutcome::TokenRejected);
        assert_eq!(
            with("429", "__retry_after__=120\n", "{}"),
            UsageOutcome::RateLimited {
                retry_after_secs: Some(120)
            }
        );
        assert_eq!(
            with("429", "", ""),
            UsageOutcome::RateLimited {
                retry_after_secs: None
            }
        );
        assert_eq!(
            with("429", "__retry_after__=Wed, 21 Oct 2026 07:28:00 GMT\n", ""),
            UsageOutcome::RateLimited {
                retry_after_secs: None
            }
        );
        assert_eq!(
            with("404", "", "{\"error\":  {\"type\":\"not_found\"}}"),
            UsageOutcome::Unavailable {
                status: Some(404),
                snippet: "{\"error\": {\"type\":\"not_found\"}}".into()
            }
        );
        assert_eq!(
            with(
                "000",
                "",
                "curl: (6) Could not resolve host: api.anthropic.com\n"
            ),
            UsageOutcome::Unavailable {
                status: None,
                snippet: "curl: (6) Could not resolve host: api.anthropic.com".into()
            }
        );
        let long = "x".repeat(1000);
        let UsageOutcome::Unavailable { snippet, .. } = with("500", "", &long) else {
            panic!()
        };
        assert_eq!(snippet.chars().count(), 200);
    }

    /// The token can only reach an outcome through the body, and the body is
    /// the endpoint's response, which never echoes the request's
    /// Authorization header. A token-shaped string anywhere else in the
    /// output (a banner, an unknown marker, an over-long subscription, a bad
    /// Retry-After) must never surface.
    #[test]
    fn token_like_text_outside_the_body_never_surfaces() {
        let noise = format!(
            "Authorization: Bearer {FAKE_TOKEN}\n__access_token__={FAKE_TOKEN}\n__subscription__={FAKE_TOKEN}\n__retry_after__={FAKE_TOKEN}\n"
        );
        let cases = [
            format!("{noise}__http_status__=200\n__body__\n{OK_BODY}"),
            format!("{noise}__http_status__=429\n__body__\n{{}}"),
            format!("{noise}__http_status__=502\n__body__\nBad gateway"),
            format!("{noise}__http_status__=401\n__body__\n{{}}"),
            format!("{noise}__host_unsupported__=x{FAKE_TOKEN}\n"),
            format!("{noise}__no_credentials__\n"),
        ];
        assert!(matches!(
            parse_usage_output(&cases[0], NOW),
            UsageOutcome::Ok { .. }
        ));
        for out in &cases {
            let outcome = parse_usage_output(out, NOW);
            let rendered = format!("{outcome:?} {}", serde_json::to_string(&outcome).unwrap());
            assert!(!rendered.contains("FAKEFAKE"), "{rendered}");
            // And through the cache into the snapshot's detail.
            let mut cache = UsageCache::new();
            let result = if outcome.is_host_specific() {
                FetchResult::AllFailed {
                    host_state: Some(outcome.kind()),
                    transport_error: false,
                    notes: vec![format!("h: {}", outcome.kind().describe())],
                }
            } else {
                FetchResult::Answered {
                    host: "h".into(),
                    outcome,
                    notes: vec![],
                }
            };
            cache.record("acct", result, NOW);
            let snap = serde_json::to_string(&cache.snapshot("acct")).unwrap();
            assert!(!snap.contains("FAKEFAKE"), "{snap}");
        }
        // The only path in: the body itself (documented, and impossible in
        // practice because the endpoint does not echo request headers).
        let outcome = parse_usage_output(
            &format!("__http_status__=500\n__body__\necho {FAKE_TOKEN}"),
            NOW,
        );
        let UsageOutcome::Unavailable { snippet, .. } = outcome else {
            panic!()
        };
        assert!(snippet.contains("FAKEFAKE"));
    }

    // ── script text ────────────────────────────────────────────────────────

    fn script() -> String {
        usage_script("claude-fleet/9.9.9")
    }

    #[test]
    fn script_has_no_xtrace() {
        let s = script();
        assert!(!s.contains("set -x"));
        assert!(!s.contains("-o xtrace"));
        assert!(s.starts_with("set +x\n"));
    }

    #[test]
    fn script_never_prints_the_token() {
        let s = script();
        for line in s.lines() {
            let l = line.to_ascii_lowercase();
            let prints = l.contains("echo") || l.contains("printf") || l.contains("print(");
            if prints {
                for needle in ["accesstoken", "tok)", "tok ", "+ tok", "bearer", "$token"] {
                    assert!(!l.contains(needle), "prints a token: {line}");
                }
            }
        }
        // No shell variable ever holds the token.
        assert!(!s.contains("token="));
        assert!(!s.contains("TOKEN="));
        // Only the non-secret `__has_token__` flag leaves jq's metadata pass.
        let meta_start = s.find("meta=$(jq").unwrap();
        let meta_end = meta_start + s[meta_start..].find("\"$cred\"").unwrap();
        let meta = &s[meta_start..meta_end];
        assert!(!meta.contains("Bearer"));
        assert!(meta.contains("($o.accessToken | length) > 0"));
    }

    #[test]
    fn curl_gets_the_token_only_from_a_header_file() {
        let s = script();
        let curl: Vec<&str> = s.lines().filter(|l| l.contains("curl -")).collect();
        assert_eq!(curl.len(), 1, "{curl:?}");
        let curl = curl[0];
        assert!(curl.contains("-H @\"$dir/auth\""));
        assert!(!curl.to_ascii_lowercase().contains("bearer"));
        assert!(!curl.contains("authorization"));
        assert!(!curl.contains(" -v") && !curl.contains("--trace"));
        assert!(curl.contains("--max-time 15"));
        assert!(curl.contains("-H 'anthropic-beta: oauth-2025-04-20'"));
        // The header file is written only by python (`os.open`) or jq's
        // redirect, and both go straight to "$dir/auth".
        let writers: Vec<&str> = s.lines().filter(|l| l.contains("Bearer")).collect();
        assert_eq!(writers.len(), 2, "{writers:?}");
        assert!(writers[0].contains("out.write("));
        assert!(writers[1].contains("> \"$dir/auth\""));
    }

    #[test]
    fn umask_and_trap_precede_the_header_file() {
        let s = script();
        let umask = s.find("umask 077").unwrap();
        let mktemp = s.find("mktemp -d").unwrap();
        let trap = s.find("trap 'rm -rf \"$dir\"' EXIT").unwrap();
        let auth = s.find("$dir/auth").unwrap();
        assert!(umask < mktemp && mktemp < trap && trap < auth);
        assert!(s.contains("0o600"));
        assert!(s.contains("rm -f \"$dir/auth\""));
    }

    #[test]
    fn script_calls_only_the_usage_url_and_never_refreshes() {
        let s = script();
        let urls: Vec<&str> = s.split_whitespace().filter(|w| w.contains("://")).collect();
        assert_eq!(urls, vec![USAGE_URL]);
        assert!(!s.contains("oauth/token"));
        assert!(!s.contains("grant_type"));
        for u in &urls {
            assert!(!u.contains("refresh"));
        }
        // Never writes the credentials file; never touches the Keychain.
        assert!(!s.contains("> \"$cred\"") && !s.contains(">\"$cred\""));
        assert!(!s.contains("open(sys.argv[1], \"w\")"));
        assert!(!s.contains("security "));
        assert!(!s.to_ascii_lowercase().contains("keychain"));
        assert!(s.contains("${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"));
    }

    #[test]
    fn user_agent_is_quoted_and_honest() {
        assert!(user_agent().starts_with("claude-fleet/"));
        assert!(script().contains("-A 'claude-fleet/9.9.9'"));
        let odd = usage_script("claude-fleet/1 it's $(x)");
        assert!(odd.contains(&format!("-A {}", quote("claude-fleet/1 it's $(x)"))));
        assert!(!odd.to_ascii_lowercase().contains("claude-code/"));
        assert!(!odd.contains(USER_AGENT_PLACEHOLDER));
        assert!(!odd.contains("@@"));
    }

    // ── the script, run locally against a fake curl ────────────────────────
    //
    // Runs `bash -c <script>` with an EMPTY environment: HOME and
    // CLAUDE_CONFIG_DIR point into a temp dir holding FAKE credentials, and
    // PATH holds only symlinks to the needed tools plus a fake `curl` — the
    // real curl is unreachable, so no request can leave this machine.

    struct Sandbox {
        dir: tempfile::TempDir,
    }

    const FAKE_CURL: &str = r#"#!/bin/sh
log="$FAKE_LOG"
: > "$log/argv"
for a in "$@"; do printf '%s\n' "$a" >> "$log/argv"; done
while [ $# -gt 0 ]; do
  case "$1" in
    -H) case "$2" in @*) f="${2#@}"; cat "$f" > "$log/auth_seen"; dirname "$f" > "$log/tmpdir"; find "$f" -perm 0600 > "$log/auth_mode";; esac; shift 2;;
    -D) hdr="$2"; shift 2;;
    -o) body="$2"; shift 2;;
    -A|-w|--max-time) shift 2;;
    *) shift;;
  esac
done
printf 'HTTP/2 %s\r\n%s\r\n\r\n' "$FAKE_STATUS" "$FAKE_HEADER" > "$hdr"
printf '%s' "$FAKE_BODY" > "$body"
printf '%s' "$FAKE_STATUS"
"#;

    fn find_tool(name: &str) -> Option<std::path::PathBuf> {
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .iter()
            .map(|d| std::path::Path::new(d).join(name))
            .find(|p| p.is_file())
    }

    impl Sandbox {
        /// `None` when a needed tool is missing on this machine (skip).
        fn new(json_tool: &str) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("bin");
            for d in ["bin", "cfg", "tmp", "log", "home"] {
                std::fs::create_dir(dir.path().join(d)).unwrap();
            }
            for t in [
                "sh", "cat", "sed", "grep", "head", "tail", "tr", "date", "mktemp", "rm",
                "dirname", "find", json_tool,
            ] {
                let src = find_tool(t)?;
                std::os::unix::fs::symlink(src, bin.join(t)).unwrap();
            }
            let curl = bin.join("curl");
            std::fs::write(&curl, FAKE_CURL).unwrap();
            std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
            Some(Self { dir })
        }

        fn write_credentials(&self, json: &str) {
            std::fs::write(self.dir.path().join("cfg/.credentials.json"), json).unwrap();
        }

        fn log(&self, name: &str) -> Option<String> {
            std::fs::read_to_string(self.dir.path().join("log").join(name)).ok()
        }

        fn run(&self, status: &str, header: &str, body: &str) -> String {
            let bash = find_tool("bash").expect("bash");
            let p = self.dir.path();
            let out = std::process::Command::new(bash)
                .arg("-c")
                .arg(usage_script("claude-fleet/test"))
                .env_clear()
                .env("PATH", p.join("bin"))
                .env("HOME", p.join("home"))
                .env("CLAUDE_CONFIG_DIR", p.join("cfg"))
                .env("TMPDIR", p.join("tmp"))
                .env("FAKE_LOG", p.join("log"))
                .env("FAKE_STATUS", status)
                .env("FAKE_HEADER", header)
                .env("FAKE_BODY", body)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(out.status.success(), "stdout={stdout} stderr={stderr}");
            assert!(!stdout.contains("FAKEFAKE"), "token on stdout: {stdout}");
            assert!(!stderr.contains("FAKEFAKE"), "token on stderr: {stderr}");
            stdout
        }
    }

    fn creds(expires_at: i64, refresh_expires_at: i64) -> String {
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{FAKE_TOKEN}","refreshToken":"sk-ant-ort01-FAKEFAKE-refresh","expiresAt":{expires_at},"refreshTokenExpiresAt":{refresh_expires_at},"scopes":["user:inference"],"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}}}"#
        )
    }

    fn run_script_paths(json_tool: &str) {
        let Some(sb) = Sandbox::new(json_tool) else {
            eprintln!("skipping: {json_tool} or a coreutil is not installed");
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // No credentials file: nothing else happens.
        let out = sb.run("200", "", OK_BODY);
        assert_eq!(out, "__no_credentials__\n");
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Login expired (refresh token in the past, epoch in ms).
        sb.write_credentials(&creds((now + 3600) * 1000, (now - 10) * 1000));
        let out = sb.run("200", "", OK_BODY);
        assert_eq!(
            parse_usage_output(&out, now),
            UsageOutcome::LoginExpired,
            "{out}"
        );
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Access token expired (seconds epoch), login still valid.
        sb.write_credentials(&creds(now - 5, now + 86_400));
        let out = sb.run("200", "", OK_BODY);
        assert_eq!(
            parse_usage_output(&out, now),
            UsageOutcome::AccessTokenExpired,
            "{out}"
        );
        assert!(
            out.contains(&format!("__expires_at__={}", now - 5)),
            "{out}"
        );
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Valid: curl runs, gets the token only through the header file.
        let creds_before = creds((now + 3600) * 1000, (now + 86_400) * 1000);
        sb.write_credentials(&creds_before);
        let out = sb.run("200", "", OK_BODY);
        let outcome = parse_usage_output(&out, now);
        assert!(
            matches!(&outcome, UsageOutcome::Ok { subscription: Some(s), .. } if s == "max"),
            "{out}"
        );
        let argv = sb.log("argv").unwrap();
        assert!(!argv.contains("FAKEFAKE"), "token in curl argv: {argv}");
        assert!(argv.contains("claude-fleet/test"));
        assert!(argv.contains("https://api.anthropic.com/api/oauth/usage"));
        assert_eq!(
            sb.log("auth_seen").unwrap(),
            format!("Authorization: Bearer {FAKE_TOKEN}\n")
        );
        assert!(
            !sb.log("auth_mode").unwrap().trim().is_empty(),
            "header file is mode 0600"
        );
        let tmpdir = sb.log("tmpdir").unwrap();
        assert!(
            !std::path::Path::new(tmpdir.trim()).exists(),
            "trap removed the temp dir"
        );
        assert_eq!(
            std::fs::read_to_string(sb.dir.path().join("cfg/.credentials.json")).unwrap(),
            creds_before,
            "credentials file untouched"
        );

        // 429 with Retry-After in the response headers.
        let out = sb.run("429", "Retry-After: 90", "{}");
        assert_eq!(
            parse_usage_output(&out, now),
            UsageOutcome::RateLimited {
                retry_after_secs: Some(90)
            },
            "{out}"
        );

        // A credentials file without claudeAiOauth.
        sb.write_credentials(r#"{"somethingElse":{}}"#);
        std::fs::remove_file(sb.dir.path().join("log/argv")).unwrap();
        let out = sb.run("200", "", OK_BODY);
        assert_eq!(parse_usage_output(&out, now), UsageOutcome::NoCredentials);
        assert!(sb.log("argv").is_none(), "curl must not run");
    }

    #[test]
    fn script_runs_end_to_end_with_python3_against_a_fake_curl() {
        run_script_paths("python3");
    }

    #[test]
    fn script_runs_end_to_end_with_jq_against_a_fake_curl() {
        run_script_paths("jq");
    }

    // ── host order ─────────────────────────────────────────────────────────

    #[test]
    fn source_host_order() {
        let hosts = vec![
            host("local", Some("A"), true),
            host("zeta", Some("A"), true),
            host("alpha", Some("A"), true),
            host("mid", Some("A"), true),
            host("down", Some("A"), false),
            host("other", Some("B"), true),
            host("none", None, true),
        ];
        assert_eq!(
            source_hosts("A", &hosts, None),
            vec!["alpha", "mid", "zeta", "local"]
        );
        assert_eq!(
            source_hosts("A", &hosts, Some("zeta")),
            vec!["zeta", "alpha", "mid", "local"]
        );
        assert_eq!(
            source_hosts("A", &hosts, Some("local")),
            vec!["local", "alpha", "mid", "zeta"]
        );
        assert_eq!(
            source_hosts("A", &hosts, Some("down")),
            vec!["alpha", "mid", "zeta", "local"],
            "an unreachable sticky host is not tried"
        );
        assert_eq!(source_hosts("B", &hosts, None), vec!["other"]);
        assert!(source_hosts("C", &hosts, None).is_empty());
    }

    // ── fetch ──────────────────────────────────────────────────────────────

    fn usage_calls(fake: &FakeSsh, h: &str) -> usize {
        fake.calls_for(h)
            .iter()
            .filter(|c| c.script().is_some_and(|s| s.contains("api/oauth/usage")))
            .count()
    }

    #[tokio::test]
    async fn falls_back_past_an_expired_access_token_and_sticks_to_the_answering_host() {
        let hosts = vec![host("a", Some("acct"), true), host("b", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.on_host(
            "a",
            Match::script_contains("api/oauth/usage"),
            Reply::ok("__expires_at__=1\n__access_token_expired__\n"),
        )
        .on_host(
            "b",
            Match::script_contains("api/oauth/usage"),
            Reply::ok(&ok_output(OK_BODY)),
        );
        let cache = Mutex::new(UsageCache::new());
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::Ok);
        assert_eq!(snap.source_host.as_deref(), Some("b"));
        assert_eq!(snap.subscription.as_deref(), Some("max"));
        assert_eq!(snap.fetched_at, Some(NOW));
        assert_eq!(snap.next_try_at, NOW + 300);
        assert_eq!(snap.detail.as_deref(), Some("a: access token expired"));
        assert_eq!(usage_calls(&fake, "a"), 1);
        assert_eq!(usage_calls(&fake, "b"), 1);
        // The script went through bash -lc with the honest UA and bounded ssh.
        let call = &fake.calls_for("b")[0];
        assert_eq!(call.args[0], "bash");
        assert_eq!(call.args[1], "-lc");
        assert!(call
            .script()
            .unwrap()
            .contains(&format!("-A {}", quote(&user_agent()))));

        // Next due fetch: b (sticky) is asked first, a is not asked at all.
        fake.clear_calls();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW + 300, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::Ok);
        assert_eq!(fake.calls().len(), 1);
        assert_eq!(fake.calls()[0].host, "b");
    }

    #[tokio::test]
    async fn stops_at_429_without_asking_the_next_host() {
        let hosts = vec![host("a", Some("acct"), true), host("b", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.on_host(
            "a",
            Match::Any,
            Reply::ok("__subscription__=max\n__http_status__=429\n__body__\n{}"),
        )
        .on_host("b", Match::Any, Reply::ok(&ok_output(OK_BODY)));
        let cache = Mutex::new(UsageCache::new());
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::RateLimited);
        assert_eq!(snap.source_host.as_deref(), Some("a"));
        assert_eq!(snap.next_try_at, NOW + 300);
        assert!(fake.calls_for("b").is_empty());
    }

    #[tokio::test]
    async fn stops_at_unavailable_and_moves_on_after_transport_errors() {
        let hosts = vec![
            host("a", Some("acct"), true),
            host("b", Some("acct"), true),
            host("c", Some("acct"), true),
            host("local", Some("acct"), true),
        ];
        let fake = FakeSsh::new();
        fake.unreachable("a")
            .on_host(
                "b",
                Match::Any,
                Reply::SpawnError {
                    message: "fd exhaustion".into(),
                },
            )
            .on_host(
                "c",
                Match::Any,
                Reply::ok("__http_status__=404\n__body__\n{\"error\":\"gone\"}"),
            );
        let cache = Mutex::new(UsageCache::new());
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::Unavailable);
        assert_eq!(snap.source_host.as_deref(), Some("c"));
        assert_eq!(
            snap.detail.as_deref(),
            Some("HTTP 404: {\"error\":\"gone\"}")
        );
        assert_eq!(snap.next_try_at, NOW + 300);
        assert!(fake.calls_for("local").is_empty(), "stopped at c");
    }

    #[tokio::test]
    async fn every_host_in_a_host_state_failure_retries_at_the_floor() {
        let hosts = vec![
            host("local", Some("acct"), true),
            host("trn", Some("acct"), true),
        ];
        let fake = FakeSsh::new();
        fake.on_host("local", Match::Any, Reply::ok("__no_credentials__\n"))
            .on_host(
                "trn",
                Match::Any,
                Reply::ok("__http_status__=401\n__body__\n{}"),
            );
        let cache = Mutex::new(UsageCache::new());
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::TokenRejected);
        assert_eq!(snap.next_try_at, NOW + 300);
        assert_eq!(
            snap.detail.as_deref(),
            Some("trn: token rejected; local: no credentials file")
        );
        assert_eq!(cache.lock().unwrap().entries["acct"].backoff_secs, 0);
        // Twice more: still the floor, no escalation.
        fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW + 300, false).await;
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW + 600, false).await;
        assert_eq!(snap.next_try_at, NOW + 900);
    }

    #[tokio::test]
    async fn all_transport_errors_back_off() {
        let hosts = vec![host("a", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.unreachable("a");
        let cache = Mutex::new(UsageCache::new());
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        assert_eq!(snap.next_try_at, NOW + 300);
        assert!(snap.detail.unwrap().starts_with("a: exit 255"));
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW + 300, false).await;
        assert_eq!(snap.next_try_at, NOW + 300 + 600);
    }

    #[tokio::test]
    async fn no_reachable_host_records_no_online_host_without_a_call() {
        let hosts = vec![
            host("a", Some("acct"), false),
            host("b", Some("other"), true),
        ];
        let fake = FakeSsh::new();
        let cache = Mutex::new(UsageCache::new());
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        assert!(fake.calls().is_empty());
        assert_eq!(snap.next_try_at, 0, "no request made; schedule untouched");
    }

    #[tokio::test]
    async fn floor_holds_even_when_forced() {
        let hosts = vec![host("a", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok(&ok_output(OK_BODY)));
        let cache = Mutex::new(UsageCache::new());
        let first = fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW, true).await;
        assert_eq!(fake.calls().len(), 1);
        for dt in [1, 60, 299] {
            let snap =
                fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW + dt, true).await;
            assert_eq!(snap, first);
        }
        assert_eq!(fake.calls().len(), 1, "no ssh call inside the floor");
        fetch_account_usage_with("acct", &hosts, &fake, &cache, NOW + 300, true).await;
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn never_fetched_snapshot() {
        let snap = UsageCache::new().snapshot("acct");
        assert_eq!(snap.status, UsageOutcomeKind::NeverFetched);
        assert!(snap.usage.is_none());
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["status"], "never_fetched");
        assert!(json["usage"].is_null());
        assert!(UsageCache::new().due("acct", 0));
    }

    // ── backoff arithmetic ─────────────────────────────────────────────────

    fn answered(outcome: UsageOutcome) -> FetchResult {
        FetchResult::Answered {
            host: "h".into(),
            outcome,
            notes: vec![],
        }
    }

    fn rate_limited(ra: Option<i64>) -> FetchResult {
        answered(UsageOutcome::RateLimited {
            retry_after_secs: ra,
        })
    }

    fn unavailable() -> FetchResult {
        answered(UsageOutcome::Unavailable {
            status: Some(503),
            snippet: String::new(),
        })
    }

    fn ok_result() -> FetchResult {
        let UsageOutcome::Ok {
            usage,
            subscription,
        } = parse_usage_output(&ok_output(OK_BODY), NOW)
        else {
            panic!()
        };
        answered(UsageOutcome::Ok {
            usage,
            subscription,
        })
    }

    #[test]
    fn rate_limit_backoff_with_and_without_retry_after() {
        let mut c = UsageCache::new();
        c.record("a", rate_limited(None), NOW);
        assert_eq!(c.entries["a"].next_try_at, NOW + 300);
        assert!(!c.due("a", NOW + 299));
        assert!(c.due("a", NOW + 300));
        c.record("a", rate_limited(None), NOW);
        assert_eq!(c.entries["a"].next_try_at, NOW + 600);
        // Retry-After larger than the backoff wins.
        c.record("a", rate_limited(Some(3000)), NOW);
        assert_eq!(c.entries["a"].backoff_secs, 1200);
        assert_eq!(c.entries["a"].next_try_at, NOW + 3000);
        // Retry-After smaller than the backoff loses.
        c.record("a", rate_limited(Some(10)), NOW);
        assert_eq!(c.entries["a"].backoff_secs, 1800);
        assert_eq!(c.entries["a"].next_try_at, NOW + 1800);
        let snap = c.snapshot("a");
        assert_eq!(snap.status, UsageOutcomeKind::RateLimited);
        assert_eq!(
            serde_json::to_value(&snap).unwrap()["status"],
            "rate_limited"
        );
    }

    #[test]
    fn unavailable_backoff_doubles_to_the_cap_and_resets_after_ok() {
        let mut c = UsageCache::new();
        let mut seen = vec![];
        for _ in 0..6 {
            c.record("a", unavailable(), NOW);
            seen.push(c.entries["a"].next_try_at - NOW);
        }
        assert_eq!(seen, vec![300, 600, 1200, 1800, 1800, 1800]);
        c.record("a", ok_result(), NOW);
        assert_eq!(c.entries["a"].backoff_secs, 0);
        assert_eq!(c.entries["a"].next_try_at, NOW + 300);
        c.record("a", unavailable(), NOW);
        assert_eq!(c.entries["a"].next_try_at, NOW + 300, "backoff restarted");
    }

    #[test]
    fn last_known_values_survive_later_failures() {
        let mut c = UsageCache::new();
        c.record("a", ok_result(), NOW);
        let ok = c.snapshot("a");
        c.record("a", unavailable(), NOW + 300);
        c.record("a", rate_limited(Some(60)), NOW + 900);
        c.record(
            "a",
            FetchResult::AllFailed {
                host_state: Some(UsageOutcomeKind::LoginExpired),
                transport_error: false,
                notes: vec!["h: login expired".into()],
            },
            NOW + 2000,
        );
        c.record("a", FetchResult::NoOnlineHost, NOW + 2400);
        let snap = c.snapshot("a");
        assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        assert_eq!(snap.usage, ok.usage);
        assert_eq!(snap.subscription.as_deref(), Some("max"));
        assert_eq!(snap.fetched_at, Some(NOW));
        assert_eq!(snap.source_host.as_deref(), Some("h"));
    }

    #[test]
    fn wire_names_are_snake_case() {
        let json = serde_json::to_value(UsageOutcome::RateLimited {
            retry_after_secs: Some(5),
        })
        .unwrap();
        assert_eq!(json["kind"], "rate_limited");
        assert_eq!(json["retry_after_secs"], 5);
        for (k, s) in [
            (UsageOutcomeKind::AccessTokenExpired, "access_token_expired"),
            (UsageOutcomeKind::NoOnlineHost, "no_online_host"),
            (UsageOutcomeKind::HostUnsupported, "host_unsupported"),
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), s);
        }
    }
}
