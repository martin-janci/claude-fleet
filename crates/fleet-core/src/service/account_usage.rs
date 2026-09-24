//! Per-account 5-hour and weekly usage (spec
//! `docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md`).
//!
//! Usage belongs to a Claude account, not a host. The only source is the
//! undocumented `GET https://api.anthropic.com/api/oauth/usage`, which needs
//! the account's OAuth access token. That token must never leave the host it
//! lives on, so fleet does not fetch the endpoint itself: it runs
//! [`usage_script`] ON a host logged in to the account (over SSH, via
//! `bash -lc`). The script reads the token there and pipes the header
//! straight into `curl`; it prints only machine-readable markers, the HTTP
//! status and the response body. Fleet's process never holds the token.
//!
//! Security invariants (enforced by tests on the script text and by tests
//! that run the script locally against a fake `curl` and fake credentials,
//! tracing every shell variable and scanning the sandbox for the token):
//! - the token never reaches stdout, stderr, argv, the environment, a shell
//!   variable or the disk — it exists only in the reading process and the
//!   pipe into `curl`;
//! - `curl` runs with `-q` (no `~/.curlrc`), HTTPS only, no redirects;
//! - the token is never refreshed and `.credentials.json` is never written
//!   (Claude Code refreshes it itself; racing it would corrupt the login);
//! - the macOS Keychain is never read (a macOS host simply has no
//!   credentials file and reports `no_credentials`);
//! - the `User-Agent` is the honest `claude-fleet/<version>`.
//!
//! Polling is gentle: at most one attempt per account per
//! [`USAGE_POLL_FLOOR_SECS`] measured from the END of the previous attempt,
//! at most two endpoint requests per attempt, doubling backoff up to
//! [`USAGE_BACKOFF_CAP_SECS`] on endpoint or transport trouble, and
//! `Retry-After` honoured on 429. The floor is never bypassed, not even by a
//! forced refresh.

use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::HostRow;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Minimum seconds between two usage attempts for one account.
pub const USAGE_POLL_FLOOR_SECS: i64 = 300;
/// Ceiling of the doubling backoff.
pub const USAGE_BACKOFF_CAP_SECS: i64 = 1800;

/// The only URL the script ever calls.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// SSH connect budget for one host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-script budget: connect (≤ 10 s) + a slow login profile + curl's own
/// 15 s cap, with room so a timeout rarely fires after the request left.
const WALL_CLOCK: Duration = Duration::from_secs(40);
/// Most bytes kept from the host's stdout / stderr. The script's own output
/// is at most ~66 KB (markers + a 64 KB body).
const OUTPUT_CAP: usize = 128 * 1024;
/// Longest `snippet` kept from a response body.
const SNIPPET_MAX_CHARS: usize = 200;
/// Longest `detail` kept on a snapshot.
const DETAIL_MAX_CHARS: usize = 300;
/// A `Retry-After` beyond a day is treated as a day.
const RETRY_AFTER_MAX_SECS: i64 = 86_400;
/// The script treats an access token expiring within this many seconds as
/// already expired, so a request never races the expiry.
const ACCESS_TOKEN_SKEW_SECS: i64 = 60;
/// After a host's token is rejected (a request WAS sent), at most this many
/// further hosts are asked in the same attempt.
const FOLLOW_UPS_AFTER_REJECT: u8 = 1;

const USER_AGENT_PLACEHOLDER: &str = "@@USER_AGENT@@";

/// The script run on the host. Placeholders are filled by [`usage_script`];
/// read its "where is the token" notes before changing a line.
const SCRIPT_TEMPLATE: &str = r#"builtin unalias -a 2>/dev/null
builtin unset -f unset unalias builtin command set trap umask exit echo printf test [ curl python3 jq rm mktemp cat head tail tr sed grep date 2>/dev/null
set +x
set +e +u +o pipefail +o noclobber
umask 077
trap '' PIPE
unset SSLKEYLOGFILE
echo __usage_start__
cred="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"
if [ ! -f "$cred" ]; then echo __no_credentials__; exit 0; fi
if command -v python3 >/dev/null 2>&1; then json=python3
elif command -v jq >/dev/null 2>&1; then json=jq
else echo __host_unsupported__=python3_or_jq; exit 0; fi
if ! command -v curl >/dev/null 2>&1; then echo __host_unsupported__=curl; exit 0; fi
cv=$(curl -q --version 2>/dev/null | sed -n '1s/^curl \([0-9][0-9]*\)\.\([0-9][0-9]*\).*/\1 \2/p')
cmaj=${cv%% *}
cmin=${cv##* }
case "$cmaj" in ''|*[!0-9]*) cmaj=0 ;; esac
case "$cmin" in ''|*[!0-9]*) cmin=0 ;; esac
if [ "$cmaj" -lt 7 ] || { [ "$cmaj" -eq 7 ] && [ "$cmin" -lt 55 ]; }; then echo __host_unsupported__=curl_7.55; exit 0; fi
if [ "$json" = python3 ]; then
meta_py='import json, re, sys
def secs(v):
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return ""
    v = int(v)
    return str(v // 1000 if v > 10 ** 12 else v)
def label(v):
    return re.sub(r"[^A-Za-z0-9_.-]", "", v)[:32] if isinstance(v, str) else ""
try:
    with open(sys.argv[1]) as f:
        o = json.load(f).get("claudeAiOauth")
except Exception:
    o = None
if not isinstance(o, dict):
    o = {}
t = o.get("accessToken")
usable = isinstance(t, str) and len(t) > 0 and "\r" not in t and "\n" not in t
print("__has_token__=" + ("1" if usable else "0"))
print("__expires_at__=" + secs(o.get("expiresAt")))
print("__refresh_expires_at__=" + secs(o.get("refreshTokenExpiresAt")))
print("__subscription__=" + label(o.get("subscriptionType")))
print("__rate_limit_tier__=" + label(o.get("rateLimitTier")))'
meta=$(python3 -I -c "$meta_py" "$cred" 2>/dev/null)
else
meta=$(jq -r 'def secs: if type == "number" then (if . > 1000000000000 then . / 1000 else . end | floor | tostring) else "" end;
def safelabel: if type == "string" then (gsub("[^A-Za-z0-9_.-]"; "") | .[0:32]) else "" end;
(.claudeAiOauth // {}) as $o
| "__has_token__=" + (if ($o.accessToken | type) == "string" and ($o.accessToken | length) > 0 and ($o.accessToken | test("[\r\n]") | not) then "1" else "0" end),
  "__expires_at__=" + ($o.expiresAt | secs),
  "__refresh_expires_at__=" + ($o.refreshTokenExpiresAt | secs),
  "__subscription__=" + ($o.subscriptionType | safelabel),
  "__rate_limit_tier__=" + ($o.rateLimitTier | safelabel)' "$cred" 2>/dev/null)
fi
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
dir=$(mktemp -d 2>/dev/null) || { echo __host_unsupported__=mktemp; exit 0; }
trap 'rm -rf "$dir"' EXIT
trap 'exit 1' HUP INT TERM
hdr_py='import json, sys
try:
    with open(sys.argv[1]) as f:
        t = json.load(f)["claudeAiOauth"]["accessToken"]
except Exception:
    sys.exit(1)
if isinstance(t, str) and t and "\r" not in t and "\n" not in t:
    sys.stdout.write("Authorization: Bearer " + t + "\n")'
auth_header() {
  if [ "$json" = python3 ]; then
    python3 -I -c "$hdr_py" "$cred" 2>/dev/null
  else
    jq -r '.claudeAiOauth.accessToken | select(type == "string" and length > 0 and (test("[\r\n]") | not)) | "Authorization: Bearer " + .' "$cred" 2>/dev/null
  fi
}
code=$(auth_header | curl -q -sS --proto =https --proto-redir =https --max-redirs 0 --max-time 15 -H @- -H 'anthropic-beta: oauth-2025-04-20' -A @@USER_AGENT@@ -D "$dir/headers" -o "$dir/body" -w '%{http_code}' @@URL@@ 2>"$dir/err")
rc=$?
case "$code" in [0-9][0-9][0-9]) ;; *) code=000 ;; esac
ra=$(tr -d '\r' < "$dir/headers" 2>/dev/null | sed -n 's/^[Rr][Ee][Tt][Rr][Yy]-[Aa][Ff][Tt][Ee][Rr]:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*$/\1/p' | tail -n 1)
echo "__http_status__=$code"
if [ "$code" = 000 ]; then echo "__curl_exit__=$rc"; fi
if [ -n "$ra" ]; then echo "__retry_after__=$ra"; fi
echo __body__
if [ "$code" = 000 ]; then grep -a '^curl: (' "$dir/err" 2>/dev/null | head -n 2; else head -c 65536 "$dir/body" 2>/dev/null; fi
exit 0
"#;

/// The honest User-Agent: `claude-fleet/<version>`. Never Claude Code's.
pub fn user_agent() -> String {
    format!("claude-fleet/{}", crate::app_version::get())
}

/// Build the usage script for `bash -lc` on a host.
///
/// Where the token is (and is not):
/// - Lines 1–7 isolate the shell from the login profile: drop aliases and
///   functions shadowing the builtins and tools used (via `builtin`, so even
///   a function named `unset` cannot intercept it; `rm() { trash-put …; }`
///   would keep files), no xtrace, no `errexit`/`nounset`/`pipefail`/
///   `noclobber`, `umask 077`, ignore SIGPIPE (a closed channel makes writes
///   fail instead of killing bash before the EXIT trap), no `SSLKEYLOGFILE`.
/// - Line 8 prints `__usage_start__`: proof for the fetch layer that the
///   script ran, so an exit-255 run is never mistaken for "never connected".
/// - `cred=…` holds only the credentials file PATH. The curl version check
///   (≥ 7.55, needed for `-H @-`) runs before anything reads the file.
/// - Pass 1 (`meta_py` via `python3 -I`, or `jq`): prints only NON-secret
///   markers. The token is reduced to `__has_token__=1|0` inside that
///   process; `meta` never holds it. The call/no-call decision is shell
///   arithmetic on the non-secret epochs.
/// - `$dir` (`mktemp -d`, removed by `trap … EXIT`) holds only the response
///   headers, the response body and curl's stderr — never the token.
/// - Pass 2: `auth_header` (`hdr_py` via `python3 -I`, or `jq`) re-reads the
///   file and writes `Authorization: Bearer <token>` to its stdout, which is
///   the pipe into `curl … -H @-`. The token exists only in that process, the
///   kernel pipe buffer and curl — no file, no variable, no argv.
/// - `curl -q` (FIRST argument, so `~/.curlrc` cannot add `verbose`, `trace`,
///   `proxy`, `insecure`…), `--proto =https --proto-redir =https
///   --max-redirs 0`, no `-v`.
/// - Returned: markers, the HTTP status, `Retry-After` parsed from the saved
///   RESPONSE headers, and the response body — or, with no HTTP response,
///   curl's exit code and at most two `curl: (N) …` lines (never headers).
/// - Nothing refreshes the token and nothing writes `.credentials.json`.
pub fn usage_script(user_agent: &str) -> String {
    SCRIPT_TEMPLATE
        .replace("@@SKEW@@", &ACCESS_TOKEN_SKEW_SECS.to_string())
        .replace("@@URL@@", USAGE_URL)
        .replace(USER_AGENT_PLACEHOLDER, &quote(user_agent))
}

/// One usage window. `utilization` is percent USED, clamped to `0..=100`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    pub utilization: f64,
    /// Unix seconds; `None` when absent or not RFC 3339.
    pub resets_at: Option<i64>,
}

/// The endpoint's buckets. Any may be absent.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct AccountUsage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub seven_day_opus: Option<Window>,
    pub seven_day_sonnet: Option<Window>,
}

/// What one host's script run said.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UsageOutcome {
    Ok {
        usage: AccountUsage,
        subscription: Option<String>,
    },
    /// No credentials file (a macOS host keeps its token in the Keychain,
    /// which fleet never reads) or no usable token in it. No request sent.
    NoCredentials,
    /// The access token expired but the login is valid; Claude Code refreshes
    /// it the next time it runs there. No request sent.
    AccessTokenExpired,
    /// The refresh token expired: that host needs `claude /login`. No request
    /// sent.
    LoginExpired,
    /// HTTP 401/403 despite a non-expired token. A request WAS sent.
    TokenRejected,
    /// HTTP 429.
    RateLimited { retry_after_secs: Option<i64> },
    /// Any other non-2xx, no HTTP response, or a 2xx with an unexpected
    /// shape. `snippet` is at most 200 sanitised characters of the response
    /// body (or, with no response, curl's exit code and error line).
    Unavailable {
        status: Option<u16>,
        snippet: String,
    },
    /// The host cannot run the check (no python3/jq, no or too old curl, no
    /// temp dir, or output without any usage marker). No request sent.
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
const MARK_CURL_EXIT: &str = "__curl_exit__=";
const MARK_RETRY_AFTER: &str = "__retry_after__=";
const MARK_SUBSCRIPTION: &str = "__subscription__=";
const MARK_BODY: &str = "__body__";
/// Printed by the script right after its isolation lines: proof it ran.
const MARK_USAGE_START: &str = "__usage_start__";

/// Classify the script's stdout.
///
/// Only exact marker lines before `__body__` are read; anything else there (a
/// login-shell banner, an unknown marker) is ignored and never copied into
/// the outcome. The first terminal marker wins. The only free text that can
/// reach the outcome is `snippet`, taken from what follows `__body__` — the
/// endpoint's response body (or curl's `curl: (N) …` error lines when there
/// was no response), neither of which contains the request's Authorization
/// header. Numeric markers are validated; labels must be short and plain.
///
/// Expiry decisions are made on the host with the host's clock.
pub fn parse_usage_output(stdout: &str) -> UsageOutcome {
    let mut terminal: Option<UsageOutcome> = None;
    let mut status: Option<u16> = None;
    let mut saw_status = false;
    let mut curl_exit: Option<u16> = None;
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
                    detail: match safe_label(what).as_deref() {
                        Some("curl_7.55") => "curl is older than 7.55".to_string(),
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
        } else if let Some(v) = l.strip_prefix(MARK_CURL_EXIT) {
            curl_exit = parse_small_number(v).and_then(|n| u16::try_from(n).ok());
        } else if let Some(v) = l.strip_prefix(MARK_RETRY_AFTER) {
            retry_after = parse_small_number(v);
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
    match status {
        None => {
            let body = snippet_of(&body);
            let snippet = match curl_exit {
                Some(n) if body.is_empty() => format!("curl exit {n}"),
                Some(n) => format!("curl exit {n}: {body}"),
                None => body,
            };
            UsageOutcome::Unavailable {
                status: None,
                snippet: snippet.chars().take(SNIPPET_MAX_CHARS).collect(),
            }
        }
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
                snippet: snippet_of(&body),
            },
        },
        Some(code) => UsageOutcome::Unavailable {
            status: Some(code),
            snippet: snippet_of(&body),
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

/// Up to 10 ASCII digits, else `None`.
fn parse_small_number(v: &str) -> Option<i64> {
    let v = v.trim();
    if v.is_empty() || v.len() > 10 || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse::<i64>().ok()
}

/// Text from a host that reaches the UI or the control API: control
/// characters (ESC/ANSI, NUL, C1, DEL) and bidi overrides/isolates removed.
fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{061C}'
                        | '\u{200E}'
                        | '\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
        .collect()
}

/// At most [`SNIPPET_MAX_CHARS`] characters of `body`, whitespace collapsed,
/// sanitised.
fn snippet_of(body: &str) -> String {
    sanitize_text(&body.split_whitespace().collect::<Vec<_>>().join(" "))
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

/// Hosts to ask for `account_uuid`'s usage, in order: reachable, visible hosts
/// on that account; the sticky host first; `local` last (usually a macOS host
/// whose token is in the Keychain, which fleet never reads); the rest
/// alphabetical. A hidden host is never polled — on a hub that includes a
/// `local` row copied from a desktop, which would run `bash` on the hub.
pub fn source_hosts(account_uuid: &str, hosts: &[HostRow], sticky: Option<&str>) -> Vec<String> {
    let local = crate::service::projects::LOCAL_HOST;
    let mut out: Vec<String> = hosts
        .iter()
        .filter(|h| h.reachable && !h.hidden && h.account_uuid.as_deref() == Some(account_uuid))
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

/// Time source for the usage cache. Scheduling uses the monotonic
/// `now_instant`, read inside the cache itself, so neither a caller's stale
/// timestamp nor a wall clock stepping backwards can shorten or freeze the
/// polling floor. `now_unix` only stamps display values (`fetched_at`,
/// `next_try_at`).
pub trait Clock: Send + Sync {
    fn now_instant(&self) -> Instant;
    fn now_unix(&self) -> i64;
}

/// The real clock: `Instant::now()` and `SystemTime::now()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_instant(&self) -> Instant {
        Instant::now()
    }

    fn now_unix(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

/// One account's cache entry.
#[derive(Debug, Clone, Default)]
pub struct AccountUsageEntry {
    /// Last successful answer: usage, subscription, fetched_at (unix, for
    /// display). Kept across failures so the UI can show last-known values
    /// under its staleness rules.
    pub last_ok: Option<(AccountUsage, Option<String>, i64)>,
    pub last_outcome: Option<UsageOutcomeKind>,
    pub last_detail: Option<String>,
    /// The host that last answered (`Ok`, `RateLimited`, `Unavailable`); tried
    /// first next time so the "via" label does not flap.
    pub source_host: Option<String>,
    /// The monotonic deadline before which no attempt may start. `None` =
    /// never scheduled. This, not `next_try_at`, is what `due` checks.
    pub next_try: Option<Instant>,
    /// `next_try` as unix seconds, computed when it was set. Display only.
    pub next_try_at: i64,
    /// 0 = no backoff in force.
    pub backoff_secs: i64,
}

/// In-memory usage cache, per account uuid, with its clock.
pub struct UsageCache {
    pub entries: HashMap<String, AccountUsageEntry>,
    clock: Arc<dyn Clock>,
}

impl Default for UsageCache {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }
}

/// What a fetch attempt ended with, before it is written to the cache.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchResult {
    /// A host answered with `Ok`, `RateLimited` or `Unavailable`. `notes`
    /// lists hosts skipped before it.
    Answered {
        host: String,
        outcome: UsageOutcome,
        notes: Vec<String>,
    },
    /// No host answered. `host_state` is the most actionable host-specific
    /// outcome seen (`None` when there was none); `transport_error` is true
    /// when any host failed in transport.
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

fn cap_detail(s: &str) -> String {
    sanitize_text(s).chars().take(DETAIL_MAX_CHARS).collect()
}

fn join_notes(notes: &[String]) -> Option<String> {
    (!notes.is_empty()).then(|| cap_detail(&notes.join("; ")))
}

fn secs(delay: i64) -> Duration {
    Duration::from_secs(u64::try_from(delay).unwrap_or(0))
}

impl UsageCache {
    /// A cache on the real clock.
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache on `clock` (tests inject a manually advanced one).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            entries: HashMap::new(),
            clock,
        }
    }

    /// True when `account` may be attempted now: never scheduled, or the
    /// monotonic deadline has passed.
    pub fn due(&self, account: &str) -> bool {
        match self.entries.get(account).and_then(|e| e.next_try) {
            None => true,
            Some(deadline) => self.clock.now_instant() >= deadline,
        }
    }

    /// Push `account`'s deadline to at least now + `delay` seconds.
    fn schedule_at_least(&mut self, account: &str, delay: i64) {
        let (now_i, now_u) = (self.clock.now_instant(), self.clock.now_unix());
        let e = self.entries.entry(account.to_string()).or_default();
        let deadline = now_i + secs(delay);
        if e.next_try.is_none_or(|d| d < deadline) {
            e.next_try = Some(deadline);
            e.next_try_at = now_u + delay;
        }
    }

    /// Write one fetch result and schedule the next try, `delay` seconds from
    /// NOW as read from the cache's clock — i.e. from the END of the attempt,
    /// so the floor separates requests, not attempt starts.
    pub fn record(&mut self, account: &str, result: FetchResult) {
        let (now_i, now_u) = (self.clock.now_instant(), self.clock.now_unix());
        let e = self.entries.entry(account.to_string()).or_default();
        let schedule = |e: &mut AccountUsageEntry, delay: i64| {
            e.next_try = Some(now_i + secs(delay));
            e.next_try_at = now_u + delay;
        };
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
                        e.last_ok = Some((usage, subscription, now_u));
                        e.backoff_secs = 0;
                        schedule(e, USAGE_POLL_FLOOR_SECS);
                        e.last_detail = join_notes(&notes);
                    }
                    UsageOutcome::RateLimited { retry_after_secs } => {
                        e.backoff_secs = next_backoff(e.backoff_secs);
                        let ra = retry_after_secs.unwrap_or(0).clamp(0, RETRY_AFTER_MAX_SECS);
                        let delay = ra.max(e.backoff_secs);
                        schedule(e, delay);
                        e.last_detail = Some(match retry_after_secs {
                            Some(s) => {
                                format!("rate limited by the usage endpoint (Retry-After: {s}s)")
                            }
                            None => "rate limited by the usage endpoint".to_string(),
                        });
                    }
                    UsageOutcome::Unavailable { status, snippet } => {
                        e.backoff_secs = next_backoff(e.backoff_secs);
                        let delay = e.backoff_secs;
                        schedule(e, delay);
                        let head = match status {
                            Some(code) => format!("HTTP {code}"),
                            None => "no HTTP response".to_string(),
                        };
                        e.last_detail = Some(cap_detail(&if snippet.is_empty() {
                            head
                        } else {
                            format!("{head}: {snippet}")
                        }));
                    }
                    // Host-specific outcomes never arrive as `Answered`; treat
                    // one defensively like an all-hosts host-state failure.
                    other => {
                        schedule(e, USAGE_POLL_FLOOR_SECS);
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
                    let delay = e.backoff_secs;
                    schedule(e, delay);
                } else {
                    // Host-state problems: no escalating backoff.
                    schedule(e, USAGE_POLL_FLOOR_SECS);
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
    /// Unix second the next attempt becomes allowed, as computed when it was
    /// scheduled (display only; scheduling itself is monotonic).
    pub next_try_at: i64,
}

impl AccountUsageSnapshot {
    /// Whether this snapshot says anything new compared with `before`.
    ///
    /// Everything except [`Self::next_try_at`], which moves on every attempt
    /// whether or not the answer changed — it is a display value, and the
    /// schedule that produces it is monotonic. `PartialEq` stays derived and
    /// keeps comparing it, because the scheduling tests assert on it.
    ///
    /// Without this distinction an account whose usage, status, subscription
    /// and detail are all identical compares unequal forever, so
    /// `fetch_and_emit` — documented as "emit iff changed" — emitted on every
    /// poll: roughly 6 KB/h to each connected client per account, and a
    /// desktop repainting a usage panel that had not changed.
    pub fn is_newsworthy_change(&self, before: &Self) -> bool {
        let Self {
            account_uuid,
            usage,
            subscription,
            fetched_at,
            source_host,
            status,
            detail,
            next_try_at: _,
        } = self;
        account_uuid != &before.account_uuid
            || usage != &before.usage
            || subscription != &before.subscription
            || fetched_at != &before.fetched_at
            || source_host != &before.source_host
            || status != &before.status
            || detail != &before.detail
    }
}

fn lock(cache: &Mutex<UsageCache>) -> std::sync::MutexGuard<'_, UsageCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// First non-empty line of `s`, trimmed, sanitised, at most 160 characters.
fn first_line(s: &str) -> String {
    sanitize_text(
        s.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or(""),
    )
    .chars()
    .take(160)
    .collect()
}

/// Whether an exit-255 run never reached the host, so falling back to the
/// next host cannot duplicate a request. ssh's stderr also carries the remote
/// login profile's and the tools' stderr, so both must hold:
/// - stdout lacks `__usage_start__` (the script prints it right after its
///   isolation lines, so its presence proves the script ran), and
/// - the LAST non-empty stderr line is one of ssh's own connect-failure
///   messages (matched as a prefix, not anywhere in the text).
///
/// The ssh wording lives in `ssh_diag::classify`.
fn connection_never_established(stdout: &str, stderr: &str) -> bool {
    if stdout
        .lines()
        .any(|l| l.trim_end_matches('\r') == MARK_USAGE_START)
    {
        return false;
    }
    crate::ssh_diag::classify::connect_failure_kind(stderr).is_some()
}

/// How one host's run ended, for the fallback decision.
enum HostRun {
    /// The script reported an outcome.
    Outcome(UsageOutcome),
    /// SSH never connected: nothing ran on the host, try the next one.
    NeverConnected(String),
    /// The run failed after the connection came up (timeout, dropped
    /// session, killed script, oversized output), or it cannot be proven
    /// otherwise: the request may already have left, so stop.
    FailedAfterConnect(String),
}

fn classify_run(res: Result<std::process::Output, IpcError>) -> HostRun {
    match res {
        Err(e) if codes::may_have_run(&e.code) || e.code == codes::E_CANCELLED => {
            HostRun::FailedAfterConnect(first_line(&e.message))
        }
        // A spawn failure, or an agent that is not connected: nothing ran.
        Err(e) => HostRun::NeverConnected(first_line(&e.message)),
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let code = out.status.code().unwrap_or(-1);
            let describe = || {
                let line = first_line(&stderr);
                if line.is_empty() {
                    format!("exit {code}")
                } else {
                    format!("exit {code}: {line}")
                }
            };
            if !out.status.success() {
                if code == 255 && connection_never_established(&stdout, &stderr) {
                    HostRun::NeverConnected(describe())
                } else {
                    HostRun::FailedAfterConnect(describe())
                }
            } else if out.stdout.len() >= OUTPUT_CAP {
                HostRun::FailedAfterConnect("output exceeded the cap".to_string())
            } else {
                HostRun::Outcome(parse_usage_output(&stdout))
            }
        }
    }
}

/// Fetch `account_uuid`'s usage through its hosts, respecting the floor.
///
/// Time comes only from the cache's [`Clock`]; there is no caller-supplied
/// timestamp to be stale. Not due → the cached snapshot, with no SSH call —
/// also when `force` is true: the spec forbids a refresh that bypasses the
/// floor. Otherwise each source host is asked in order. Fallback to the next
/// host happens only when no request can have left: the script reported
/// `no_credentials`, `access_token_expired`, `login_expired` or
/// `host_unsupported`, or SSH provably never connected. After
/// `token_rejected` (a request was sent) at most one more host is asked.
/// Everything else stops the attempt: `ok`, `rate_limited`, `unavailable`, an
/// SSH timeout, or any failure after the connection came up. The next try is
/// scheduled from the attempt's end.
///
/// The cache mutex is never held across an `.await`: the attempt is reserved
/// under one lock, the SSH calls run unlocked, and the result is written
/// under a second lock.
pub async fn fetch_account_usage_with(
    account_uuid: &str,
    hosts: &[HostRow],
    ssh: &dyn SshExec,
    cache: &Mutex<UsageCache>,
    force: bool,
) -> AccountUsageSnapshot {
    // `force` only expresses the caller's intent; it never bypasses the floor.
    let _ = force;
    let candidates = {
        let mut c = lock(cache);
        if !c.due(account_uuid) {
            return c.snapshot(account_uuid);
        }
        let sticky = c
            .entries
            .get(account_uuid)
            .and_then(|e| e.source_host.clone());
        let candidates = source_hosts(account_uuid, hosts, sticky.as_deref());
        if candidates.is_empty() {
            c.record(account_uuid, FetchResult::NoOnlineHost);
            return c.snapshot(account_uuid);
        }
        // Reserve the attempt so a concurrent caller sees "not due" instead
        // of issuing a second request inside the floor.
        c.schedule_at_least(account_uuid, USAGE_POLL_FLOOR_SECS);
        candidates
    };

    let script = usage_script(&user_agent());
    let quoted = quote(&script);
    let mut notes: Vec<String> = Vec::new();
    let mut host_state: Option<UsageOutcomeKind> = None;
    let mut transport_error = false;
    let mut answered: Option<(String, UsageOutcome)> = None;
    let mut follow_ups_left: Option<u8> = None;

    for host in &candidates {
        match follow_ups_left.as_mut() {
            Some(0) => break,
            Some(n) => *n -= 1,
            None => {}
        }
        let res = ssh
            .run_bounded_capped(
                host,
                &["bash", "-lc", &quoted],
                CONNECT_TIMEOUT,
                WALL_CLOCK,
                OUTPUT_CAP,
            )
            .await;
        match classify_run(res) {
            HostRun::NeverConnected(msg) => {
                transport_error = true;
                notes.push(format!("{host}: {msg}"));
            }
            HostRun::FailedAfterConnect(msg) => {
                transport_error = true;
                notes.push(format!("{host}: {msg}"));
                break;
            }
            HostRun::Outcome(outcome) if outcome.is_host_specific() => {
                let kind = outcome.kind();
                let text = match &outcome {
                    UsageOutcome::HostUnsupported { detail } => detail.clone(),
                    _ => kind.describe().to_string(),
                };
                notes.push(format!("{host}: {text}"));
                if host_state.is_none_or(|k| kind.host_state_rank() > k.host_state_rank()) {
                    host_state = Some(kind);
                }
                if kind == UsageOutcomeKind::TokenRejected && follow_ups_left.is_none() {
                    follow_ups_left = Some(FOLLOW_UPS_AFTER_REJECT);
                }
            }
            HostRun::Outcome(outcome) => {
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
    // `record` reads the clock now, at the end of the attempt.
    c.record(account_uuid, result);
    c.snapshot(account_uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use std::path::{Path, PathBuf};

    /// A fake access token. `CANARY` is its distinctive middle; it must never
    /// be found anywhere but the fake credentials file.
    const CANARY: &str = "LeakCanary7Qz";
    const FAKE_TOKEN: &str = "sk-ant-oat01-LeakCanary7Qz-0123456789abcdefFAKE";
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
            transport: "ssh".to_string(),
        }
    }

    /// The usage request is not idempotent — a second one against the same
    /// account is a second billed call — so a failure that may already have
    /// left the hub must stop the sweep, and one that provably did not must
    /// let it move to the next host. An agent host maps onto the SSH codes
    /// one for one: `E_AGENT_OFFLINE` is `E_SSH` (no connection, nothing
    /// left), `E_TIMEOUT` is `E_SSH_TIMEOUT` (it left and never came back),
    /// and `E_AGENT_PROTOCOL` is only ever raised after the frame was sent.
    #[test]
    fn an_agent_failure_classifies_like_its_ssh_twin() {
        let run = |code: &str| classify_run(Err(IpcError::new(code, "x")));
        for code in [
            codes::E_SSH_TIMEOUT,
            codes::E_TIMEOUT,
            codes::E_AGENT_PROTOCOL,
        ] {
            assert!(
                matches!(run(code), HostRun::FailedAfterConnect(_)),
                "{code} must stop the sweep"
            );
        }
        for code in [codes::E_SSH, codes::E_AGENT_OFFLINE] {
            assert!(
                matches!(run(code), HostRun::NeverConnected(_)),
                "{code} must let the sweep try the next host"
            );
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
        let out = parse_usage_output(&ok_output(OK_BODY));
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
        let UsageOutcome::Ok { usage, .. } = parse_usage_output(&ok_output(body)) else {
            panic!()
        };
        assert_eq!(usage.five_hour.unwrap().resets_at, None);
        assert_eq!(usage.seven_day.unwrap().resets_at, None);
    }

    #[test]
    fn clamps_utilization() {
        let body = r#"{"five_hour":{"utilization":142.5,"resets_at":null},"seven_day":{"utilization":-3}}"#;
        let UsageOutcome::Ok { usage, .. } = parse_usage_output(&ok_output(body)) else {
            panic!()
        };
        assert_eq!(usage.five_hour.unwrap().utilization, 100.0);
        assert_eq!(usage.seven_day.unwrap().utilization, 0.0);
    }

    #[test]
    fn missing_bucket_is_tolerated() {
        let body = r#"{"seven_day":{"utilization":50,"resets_at":"2026-02-12T20:00:00Z"},"new_field":[1,2]}"#;
        let UsageOutcome::Ok { usage, .. } = parse_usage_output(&ok_output(body)) else {
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
            let out = parse_usage_output(&ok_output(body));
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
            parse_usage_output(&format!("{banner}__no_credentials__\n")),
            UsageOutcome::NoCredentials
        );
        assert_eq!(
            parse_usage_output(
                "__expires_at__=1\n__refresh_expires_at__=2\n__subscription__=pro\n__login_expired__\n"),
            UsageOutcome::LoginExpired
        );
        assert_eq!(
            parse_usage_output(
                "__expires_at__=1\n__refresh_expires_at__=2099999999\n__access_token_expired__\n__no_credentials__\n"),
            UsageOutcome::AccessTokenExpired,
            "the first terminal marker wins"
        );
        assert_eq!(
            parse_usage_output("__host_unsupported__=curl\n"),
            UsageOutcome::HostUnsupported {
                detail: "missing curl".into()
            }
        );
        assert_eq!(
            parse_usage_output("__host_unsupported__=curl_7.55\n"),
            UsageOutcome::HostUnsupported {
                detail: "curl is older than 7.55".into()
            }
        );
        assert!(matches!(
            parse_usage_output("bash: something odd\n"),
            UsageOutcome::HostUnsupported { .. }
        ));
    }

    #[test]
    fn http_statuses() {
        let with = |status: &str, extra: &str, body: &str| {
            parse_usage_output(&format!(
                "__subscription__=max\n__http_status__={status}\n{extra}__body__\n{body}"
            ))
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
                "__curl_exit__=6\n",
                "curl: (6) Could not resolve host: api.anthropic.com\n"
            ),
            UsageOutcome::Unavailable {
                status: None,
                snippet: "curl exit 6: curl: (6) Could not resolve host: api.anthropic.com".into()
            }
        );
        assert_eq!(
            with("000", "__curl_exit__=28\n", ""),
            UsageOutcome::Unavailable {
                status: None,
                snippet: "curl exit 28".into()
            }
        );
        let long = "x".repeat(1000);
        let UsageOutcome::Unavailable { snippet, .. } = with("500", "", &long) else {
            panic!()
        };
        assert_eq!(snippet.chars().count(), 200);
    }

    #[test]
    fn snippet_and_detail_drop_control_and_bidi_characters() {
        let body = "bad\u{1b}[31mred\u{0}nul\u{202e}rtl\u{2066}iso\u{9b}c1 end";
        let UsageOutcome::Unavailable { snippet, .. } =
            parse_usage_output(&format!("__http_status__=502\n__body__\n{body}"))
        else {
            panic!()
        };
        assert_eq!(snippet, "bad[31mrednulrtlisoc1 end");
        let mut c = UsageCache::new();
        c.record(
            "a",
            FetchResult::AllFailed {
                host_state: None,
                transport_error: true,
                notes: vec![format!(
                    "h: {}",
                    first_line("exit 255: \u{1b}]0;x\u{7}boom\u{202e}")
                )],
            },
        );
        assert_eq!(
            c.snapshot("a").detail.as_deref(),
            Some("h: exit 255: ]0;xboom")
        );
    }

    /// The token can only reach an outcome through the body, and the body is
    /// the endpoint's response, which never echoes the request's
    /// Authorization header. A token-shaped string anywhere else in the
    /// output (a banner, an unknown marker, an over-long subscription, a bad
    /// Retry-After or curl exit) must never surface.
    #[test]
    fn token_like_text_outside_the_body_never_surfaces() {
        let noise = format!(
            "Authorization: Bearer {FAKE_TOKEN}\n__access_token__={FAKE_TOKEN}\n__subscription__={FAKE_TOKEN}\n__retry_after__={FAKE_TOKEN}\n__curl_exit__={FAKE_TOKEN}\n"
        );
        let cases = [
            format!("{noise}__http_status__=200\n__body__\n{OK_BODY}"),
            format!("{noise}__http_status__=429\n__body__\n{{}}"),
            format!("{noise}__http_status__=502\n__body__\nBad gateway"),
            format!("{noise}__http_status__=000\n__body__\n"),
            format!("{noise}__http_status__=401\n__body__\n{{}}"),
            format!("{noise}__host_unsupported__=x{FAKE_TOKEN}\n"),
            format!("{noise}__no_credentials__\n"),
        ];
        assert!(matches!(
            parse_usage_output(&cases[0]),
            UsageOutcome::Ok { .. }
        ));
        for out in &cases {
            let outcome = parse_usage_output(out);
            let rendered = format!("{outcome:?} {}", serde_json::to_string(&outcome).unwrap());
            assert!(!rendered.contains(CANARY), "{rendered}");
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
            cache.record("acct", result);
            let snap = serde_json::to_string(&cache.snapshot("acct")).unwrap();
            assert!(!snap.contains(CANARY), "{snap}");
        }
        // The only path in: the body itself (documented, and impossible in
        // practice because the endpoint does not echo request headers).
        let outcome =
            parse_usage_output(&format!("__http_status__=500\n__body__\necho {FAKE_TOKEN}"));
        let UsageOutcome::Unavailable { snippet, .. } = outcome else {
            panic!()
        };
        assert!(snippet.contains(CANARY));
    }

    // ── script text ────────────────────────────────────────────────────────

    fn script() -> String {
        usage_script("claude-fleet/9.9.9")
    }

    #[test]
    fn script_isolates_the_shell_first() {
        let s = script();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[0], "builtin unalias -a 2>/dev/null");
        // `builtin` so a profile function named `unset` cannot intercept it.
        assert!(lines[1].starts_with("builtin unset -f "));
        for f in [
            "unset", "builtin", "command", "set", "trap", "curl", "python3", "jq", "rm", "mktemp",
            "cat", "head", "sed", "grep",
        ] {
            assert!(
                lines[1].split_whitespace().any(|w| w == f),
                "unset -f misses {f}"
            );
        }
        assert_eq!(lines[2], "set +x");
        assert_eq!(lines[3], "set +e +u +o pipefail +o noclobber");
        assert_eq!(lines[4], "umask 077");
        assert_eq!(lines[5], "trap '' PIPE");
        assert_eq!(lines[6], "unset SSLKEYLOGFILE");
        assert_eq!(lines[7], "echo __usage_start__");
        assert!(!s.contains("trap - DEBUG"));
        assert!(!s.contains("set -x"));
        assert!(!s.contains("-o xtrace"));
        // Every python run is isolated (-I: no PYTHON* env, no user site).
        assert_eq!(s.matches("python3 -").count(), 2);
        assert_eq!(s.matches("python3 -I -c ").count(), 2);
    }

    #[test]
    fn script_never_prints_the_token() {
        let s = script();
        for line in s.lines() {
            let l = line.to_ascii_lowercase();
            let prints = l.contains("echo") || l.contains("printf") || l.contains("print(");
            if prints {
                for needle in ["accesstoken", "(t)", "+ t)", "+ t ", "bearer", "$token"] {
                    assert!(!l.contains(needle), "prints a token: {line}");
                }
            }
        }
        // No shell variable ever holds the token.
        assert!(!s.contains("token="));
        assert!(!s.contains("TOKEN="));
        // Only the non-secret `__has_token__` flag leaves pass 1.
        let meta_start = s.find("meta_py='").unwrap();
        let meta_end = s.find("\nfield() {").unwrap();
        let meta = &s[meta_start..meta_end];
        assert!(!meta.contains("Bearer"));
        assert!(meta.contains("__has_token__="));
    }

    #[test]
    fn curl_gets_the_token_only_from_a_pipe() {
        let s = script();
        let curl: Vec<&str> = s.lines().filter(|l| l.contains("curl -q -sS")).collect();
        assert_eq!(curl.len(), 1, "{curl:?}");
        let curl = curl[0];
        // `-q` must be curl's FIRST argument or ~/.curlrc still applies.
        assert!(
            curl.contains("$(auth_header | curl -q -sS --proto =https --proto-redir =https --max-redirs 0 --max-time 15 -H @- "),
            "{curl}"
        );
        assert!(!curl.to_ascii_lowercase().contains("bearer"));
        assert!(!curl.contains(" -v") && !curl.contains("--trace") && !curl.contains("-k "));
        assert!(!curl.contains("-L") && !curl.contains("--location"));
        assert!(curl.contains("-H 'anthropic-beta: oauth-2025-04-20'"));
        // Every curl invocation starts with -q.
        for l in s.lines() {
            for (i, _) in l.match_indices("curl ") {
                let before = &l[..i];
                if before.ends_with("command -v ")
                    || l.starts_with("builtin unset -f ")
                    // the `sed` pattern matching `curl --version` output
                    || before.ends_with('^')
                {
                    continue;
                }
                assert!(l[i..].starts_with("curl -q "), "{l}");
            }
        }
        // The token never touches disk: no header file, no here-string.
        assert!(!s.contains("$dir/auth"));
        assert!(!s.contains("<<<"));
        assert!(!s.contains("os.open"));
        // The header is written only by `auth_header`, used once, into curl.
        let writers: Vec<&str> = s.lines().filter(|l| l.contains("Bearer")).collect();
        assert_eq!(writers.len(), 2, "{writers:?}");
        assert!(writers[0].contains("sys.stdout.write("));
        assert!(writers[1].trim_start().starts_with("jq -r "));
        for w in &writers {
            assert!(!w.contains('>') || w.contains("2>/dev/null"), "{w}");
            assert!(!w.contains("> \""), "{w}");
        }
        assert_eq!(s.matches("auth_header").count(), 2);
        // SSLKEYLOGFILE unset and SIGPIPE ignored before curl runs.
        let call = s.find("code=$(auth_header").unwrap();
        assert!(s.find("unset SSLKEYLOGFILE").unwrap() < call);
        assert!(s.find("trap '' PIPE").unwrap() < call);
    }

    #[test]
    fn curl_stderr_is_never_returned_wholesale() {
        let s = script();
        let reads: Vec<&str> = s
            .lines()
            .filter(|l| l.contains("\"$dir/err\"") && !l.contains("2>\"$dir/err\""))
            .collect();
        assert_eq!(reads.len(), 1, "{reads:?}");
        assert!(reads[0].contains("grep -a '^curl: (' \"$dir/err\""));
        assert!(!s.contains("cat \"$dir/err\"") && !s.contains("head -c 2000 \"$dir/err\""));
    }

    #[test]
    fn script_checks_curl_version_before_reading_credentials() {
        let s = script();
        let version = s.find("curl -q --version").unwrap();
        assert!(s.contains("__host_unsupported__=curl_7.55"));
        assert!(version < s.find("meta_py=").unwrap());
    }

    #[test]
    fn umask_and_trap_precede_the_temp_files() {
        let s = script();
        let umask = s.find("umask 077").unwrap();
        let mktemp = s.find("mktemp -d").unwrap();
        let trap = s.find("trap 'rm -rf \"$dir\"' EXIT").unwrap();
        let first_use = s.find("\"$dir/").unwrap();
        assert!(umask < mktemp && mktemp < trap && trap < first_use);
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
        assert!(!s.contains("\"w\""));
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
    // PATH holds only symlinks to the needed tools, logging shims for
    // python3/jq, and a fake `curl` — the real curl is unreachable, so no
    // request can leave this machine. `BASH_ENV` installs a DEBUG trap that
    // dumps every shell variable before every command (functions and
    // subshells included), plus hostile `rm`/`curl` functions the script
    // must remove. After every run the whole sandbox except the fake
    // credentials file is scanned for the token.

    struct Sandbox {
        dir: tempfile::TempDir,
    }

    const FAKE_CURL: &str = r#"#!/bin/sh
log="$FAKE_LOG"
if [ "$1" = -q ]; then echo yes > "$log/q_first"; fi
for a in "$@"; do
  if [ "$a" = --version ]; then echo "curl ${FAKE_CURL_VERSION:-8.4.0} (fake) libcurl/8.4.0"; exit 0; fi
done
verbose=
if [ "$1" != -q ] && grep -q verbose "$HOME/.curlrc" 2>/dev/null; then verbose=1; fi
: > "$log/argv"
for a in "$@"; do printf '%s\n' "$a" >> "$log/argv"; done
env >> "$log/curl_env"
src= hdr= body=
while [ $# -gt 0 ]; do
  case "$1" in
    -H) case "$2" in @-) src=-;; @*) src="${2#@}";; esac; shift 2;;
    -D) hdr="$2"; shift 2;;
    -o) body="$2"; shift 2;;
    -A|-w|--max-time|--proto|--proto-redir|--max-redirs) shift 2;;
    *) shift;;
  esac
done
dirname "$hdr" > "$log/tmpdir"
if [ -n "$verbose" ]; then
  echo "* Trying 1.2.3.4:443..." >&2
  if [ "$src" = - ]; then sed 's/^/> /' >&2; elif [ -n "$src" ]; then sed 's/^/> /' "$src" >&2; fi
elif [ "$src" = - ]; then
  cksum > "$log/auth_cksum"
elif [ -n "$src" ]; then
  cksum < "$src" > "$log/auth_cksum"
fi
if [ -n "$FAKE_SLEEP" ]; then echo $$ > "$log/curl_pid"; exec sleep "$FAKE_SLEEP"; fi
if [ "$FAKE_STATUS" = 000 ]; then
  echo "curl: (6) Could not resolve host: api.anthropic.com" >&2
  printf 000
  exit 6
fi
printf 'HTTP/2 %s\r\n%s\r\n\r\n' "$FAKE_STATUS" "$FAKE_HEADER" > "$hdr"
printf '%s' "$FAKE_BODY" > "$body"
printf '%s' "$FAKE_STATUS"
"#;

    /// BASH_ENV for the sandboxed bash: trace every variable, and plant
    /// hostile functions the script must unset before use.
    const BASH_ENV: &str = r#"set -o functrace
trap 'declare -p >>"$FAKE_LOG/vars" 2>/dev/null' DEBUG
rm() { :; }
curl() { echo HIJACKED; }
"#;

    const SHIM: &str = "#!/bin/sh\n{ echo \"== $0\"; printf '%s\\n' \"$@\"; env; } >> \"$FAKE_LOG/tool_calls\"\nexec @@REAL@@ \"$@\"\n";

    fn find_tool(name: &str) -> Option<PathBuf> {
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(name))
            .find(|p| p.is_file())
    }

    /// Recursively list regular files under `dir` whose bytes contain
    /// `needle`, skipping symlinks and `skip`.
    fn files_containing(dir: &Path, needle: &[u8], skip: &Path) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return hits;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(md) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if md.file_type().is_symlink() || p == skip {
                continue;
            }
            if md.is_dir() {
                hits.extend(files_containing(&p, needle, skip));
            } else if let Ok(bytes) = std::fs::read(&p) {
                if bytes.windows(needle.len()).any(|w| w == needle) {
                    hits.push(p);
                }
            }
        }
        hits
    }

    impl Sandbox {
        /// `None` when a needed tool is missing on this machine: the test
        /// skips, except under CI where it must fail loudly.
        fn new(json_tool: &str) -> Option<Self> {
            use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("bin");
            for d in ["bin", "cfg", "tmp", "log", "home"] {
                std::fs::create_dir(dir.path().join(d)).unwrap();
            }
            // The probe guard goes straight after the shebang, so the
            // probe exec inside `write_exec` never reaches a body's own
            // side effects (these shims append to `$FAKE_LOG`, and tests
            // assert on exactly what landed there).
            let exe = |name: &str, body: &str| {
                let (shebang, rest) = body.split_once('\n').expect("a shim starts with a shebang");
                write_exec(&bin, name, &format!("{shebang}\n{PROBE_GUARD}{rest}"));
            };
            let needed = [
                "cat", "sed", "grep", "head", "tail", "tr", "date", "rm", "dirname", "cksum",
                "sleep", "env",
            ];
            for t in needed.iter().copied().chain(["bash", "mktemp"]) {
                if find_tool(t).is_none() {
                    return missing(t);
                }
            }
            for t in needed {
                std::os::unix::fs::symlink(find_tool(t).unwrap(), bin.join(t)).unwrap();
            }
            let Some(real) = find_tool(json_tool) else {
                return missing(json_tool);
            };
            exe(
                json_tool,
                &SHIM.replace("@@REAL@@", &quote(&real.to_string_lossy())),
            );
            exe("curl", FAKE_CURL);
            // Some `mktemp`s (macOS) ignore $TMPDIR; pin the script's temp
            // dir inside the sandbox so the leak scan covers it.
            exe(
                "mktemp",
                &format!(
                    "#!/bin/sh\n[ \"$1\" = -d ] || exit 1\nexec {} -d \"$TMPDIR/tmp.XXXXXXXX\"\n",
                    quote(&find_tool("mktemp").unwrap().to_string_lossy())
                ),
            );
            std::fs::write(dir.path().join("bash_env"), BASH_ENV).unwrap();
            Some(Self { dir })
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn write_credentials(&self, json: &str) {
            std::fs::write(self.path("cfg/.credentials.json"), json).unwrap();
        }

        fn log(&self, name: &str) -> Option<String> {
            std::fs::read_to_string(self.path("log").join(name)).ok()
        }

        fn clear_log(&self, name: &str) {
            let _ = std::fs::remove_file(self.path("log").join(name));
        }

        fn command(&self, env: &[(&str, &str)], script: &str) -> std::process::Command {
            let p = self.dir.path();
            let mut cmd = std::process::Command::new(find_tool("bash").unwrap());
            cmd.arg("-c")
                .arg(script)
                .env_clear()
                .env("PATH", p.join("bin"))
                .env("HOME", p.join("home"))
                .env("CLAUDE_CONFIG_DIR", p.join("cfg"))
                .env("TMPDIR", p.join("tmp"))
                .env("FAKE_LOG", p.join("log"))
                .env("BASH_ENV", p.join("bash_env"))
                .env("FAKE_STATUS", "200")
                .env("FAKE_BODY", OK_BODY);
            for (k, v) in env {
                cmd.env(k, v);
            }
            cmd
        }

        /// The token must be nowhere in the sandbox but the credentials file.
        fn assert_no_token_on_disk(&self) {
            let hits = files_containing(
                self.dir.path(),
                CANARY.as_bytes(),
                &self.path("cfg/.credentials.json"),
            );
            assert!(hits.is_empty(), "token found in {hits:?}");
        }

        fn run_script(&self, script: &str, env: &[(&str, &str)]) -> String {
            self.clear_log("argv");
            self.clear_log("auth_cksum");
            self.clear_log("q_first");
            let out = self.command(env, script).output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(out.status.success(), "stdout={stdout} stderr={stderr}");
            assert!(!stdout.contains(CANARY), "token on stdout: {stdout}");
            assert!(!stderr.contains(CANARY), "token on stderr: {stderr}");
            // `declare -p` prints `declare -x NAME="v"` on bash 4+ but a bare
            // `NAME=v` on the bash 3.2 that ships with macOS, so match on a
            // variable the dump always carries rather than on the format.
            let vars = self.log("vars").unwrap_or_default();
            assert!(
                vars.contains("BASH="),
                "the DEBUG trace ran: bash={:?}, vars_len={} vars_head={:?} stdout={stdout}",
                find_tool("bash"),
                vars.len(),
                vars.chars().take(300).collect::<String>(),
            );
            self.assert_no_token_on_disk();
            stdout
        }

        fn run(&self, env: &[(&str, &str)]) -> String {
            self.run_script(&usage_script("claude-fleet/test"), env)
        }
    }

    fn missing(tool: &str) -> Option<Sandbox> {
        if std::env::var_os("CI").is_some() {
            panic!("{tool} is required for the usage-script tests under CI");
        }
        eprintln!("skipping: {tool} is not installed");
        None
    }

    fn creds(expires_at: i64, refresh_expires_at: i64) -> String {
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{FAKE_TOKEN}","refreshToken":"sk-ant-ort01-refresh-FAKE","expiresAt":{expires_at},"refreshTokenExpiresAt":{refresh_expires_at},"scopes":["user:inference"],"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}}}"#
        )
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    /// POSIX `cksum` of `data`, computed by the system tool.
    fn cksum_of(data: &str) -> String {
        use std::io::Write;
        let mut child = std::process::Command::new(find_tool("cksum").unwrap())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(data.as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    }

    fn run_script_paths(json_tool: &str) {
        let Some(sb) = Sandbox::new(json_tool) else {
            return;
        };
        let now = unix_now();

        // No credentials file: nothing else happens.
        let out = sb.run(&[]);
        assert_eq!(out, "__usage_start__\n__no_credentials__\n");
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Login expired (refresh token in the past, epoch in ms).
        sb.write_credentials(&creds((now + 3600) * 1000, (now - 10) * 1000));
        let out = sb.run(&[]);
        assert_eq!(
            parse_usage_output(&out),
            UsageOutcome::LoginExpired,
            "{out}"
        );
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Access token expired (seconds epoch), login still valid.
        sb.write_credentials(&creds(now - 5, now + 86_400));
        let out = sb.run(&[]);
        assert_eq!(
            parse_usage_output(&out),
            UsageOutcome::AccessTokenExpired,
            "{out}"
        );
        assert!(
            out.contains(&format!("__expires_at__={}", now - 5)),
            "{out}"
        );
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Valid: curl runs with -q first and gets the header on stdin.
        let valid = creds((now + 3600) * 1000, (now + 86_400) * 1000);
        sb.write_credentials(&valid);
        let out = sb.run(&[]);
        let outcome = parse_usage_output(&out);
        assert!(
            matches!(&outcome, UsageOutcome::Ok { subscription: Some(s), .. } if s == "max"),
            "{out}"
        );
        assert_eq!(sb.log("q_first").as_deref(), Some("yes\n"));
        let argv = sb.log("argv").unwrap();
        assert!(argv.contains("claude-fleet/test"));
        assert!(argv.contains("@-\n"), "{argv}");
        assert!(argv.contains("https://api.anthropic.com/api/oauth/usage"));
        assert_eq!(
            sb.log("auth_cksum").unwrap(),
            cksum_of(&format!("Authorization: Bearer {FAKE_TOKEN}\n")),
            "curl received exactly the Authorization header"
        );
        let tmpdir = sb.log("tmpdir").unwrap();
        assert!(
            Path::new(tmpdir.trim()).starts_with(sb.path("tmp")),
            "the leak scan covers the script's temp dir: {tmpdir}"
        );
        assert!(
            !Path::new(tmpdir.trim()).exists(),
            "trap removed the temp dir despite a hostile rm() function"
        );
        assert_eq!(
            std::fs::read_to_string(sb.path("cfg/.credentials.json")).unwrap(),
            valid,
            "credentials file untouched"
        );
        let calls = sb.log("tool_calls").unwrap();
        assert!(calls.contains(&format!("== {}", sb.path("bin").join(json_tool).display())));

        // 429 with Retry-After in the response headers.
        let out = sb.run(&[
            ("FAKE_STATUS", "429"),
            ("FAKE_HEADER", "Retry-After: 90"),
            ("FAKE_BODY", "{}"),
        ]);
        assert_eq!(
            parse_usage_output(&out),
            UsageOutcome::RateLimited {
                retry_after_secs: Some(90)
            },
            "{out}"
        );

        // A hostile ~/.curlrc asking for `verbose`, and no HTTP response:
        // with -q the config is ignored and only the `curl: (N)` line returns.
        std::fs::write(sb.path("home/.curlrc"), "verbose\n").unwrap();
        let out = sb.run(&[("FAKE_STATUS", "000")]);
        assert_eq!(
            parse_usage_output(&out),
            UsageOutcome::Unavailable {
                status: None,
                snippet: "curl exit 6: curl: (6) Could not resolve host: api.anthropic.com".into()
            },
            "{out}"
        );
        std::fs::remove_file(sb.path("home/.curlrc")).unwrap();

        // curl older than 7.55 cannot read -H @-: host_unsupported, no call.
        let out = sb.run(&[("FAKE_CURL_VERSION", "7.29.0")]);
        assert_eq!(
            parse_usage_output(&out),
            UsageOutcome::HostUnsupported {
                detail: "curl is older than 7.55".into()
            },
            "{out}"
        );
        assert!(sb.log("argv").is_none(), "curl must not run");

        // A credentials file without claudeAiOauth.
        sb.write_credentials(r#"{"somethingElse":{}}"#);
        let out = sb.run(&[]);
        assert_eq!(parse_usage_output(&out), UsageOutcome::NoCredentials);
        assert!(sb.log("argv").is_none(), "curl must not run");

        // Nothing token-bearing in the tool shims' argv/env logs either (the
        // sandbox scan covers them) and the variable trace saw the pipeline.
        let vars = sb.log("vars").unwrap();
        assert!(
            vars.contains("meta="),
            "trace covers the script's variables"
        );
    }

    #[test]
    fn script_runs_end_to_end_with_python3_against_a_fake_curl() {
        run_script_paths("python3");
    }

    #[test]
    fn script_runs_end_to_end_with_jq_against_a_fake_curl() {
        run_script_paths("jq");
    }

    /// The leak detectors themselves work: a script that keeps the token in
    /// a shell variable, or writes it to a temp file, is caught.
    #[test]
    fn leak_detectors_catch_a_token_in_a_variable_or_on_disk() {
        let Some(sb) = Sandbox::new("jq") else {
            return;
        };
        sb.write_credentials(&creds(
            (unix_now() + 3600) * 1000,
            (unix_now() + 86_400) * 1000,
        ));
        let in_var = "h=$(jq -r .claudeAiOauth.accessToken \"$CLAUDE_CONFIG_DIR/.credentials.json\"); true; echo ok";
        let out = sb.command(&[], in_var).output().unwrap();
        assert!(out.status.success());
        let hits = files_containing(
            sb.dir.path(),
            CANARY.as_bytes(),
            &sb.path("cfg/.credentials.json"),
        );
        assert_eq!(hits, vec![sb.path("log/vars")], "the DEBUG trace caught it");
        std::fs::remove_file(sb.path("log/vars")).unwrap();

        let on_disk = "jq -r .claudeAiOauth.accessToken \"$CLAUDE_CONFIG_DIR/.credentials.json\" > \"$TMPDIR/auth\"";
        sb.command(&[], on_disk).output().unwrap();
        let hits = files_containing(
            sb.dir.path(),
            CANARY.as_bytes(),
            &sb.path("cfg/.credentials.json"),
        );
        assert_eq!(hits, vec![sb.path("tmp/auth")]);
    }

    /// SIGKILL while curl is in flight (OOM killer): no token may be left on
    /// disk. This failed when the header went through a temp file.
    #[test]
    fn sigkill_during_the_request_leaves_no_token_on_disk() {
        for tool in ["python3", "jq"] {
            let Some(sb) = Sandbox::new(tool) else {
                continue;
            };
            let now = unix_now();
            sb.write_credentials(&creds((now + 3600) * 1000, (now + 86_400) * 1000));
            let mut child = sb
                .command(&[("FAKE_SLEEP", "30")], &usage_script("claude-fleet/test"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let pid_file = sb.path("log/curl_pid");
            let started = Instant::now();
            // The deadline is liveness only, so it is generous: a loaded
            // machine takes its time over a traced bash, and a script that
            // gives up before curl is caught by its exit, not by the wait.
            while !pid_file.exists() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("{tool}: script exited ({status}) before fake curl started");
                }
                assert!(
                    started.elapsed() < Duration::from_secs(120),
                    "fake curl never started"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            // Let the fake curl finish reading the header and exec sleep.
            std::thread::sleep(Duration::from_millis(200));
            child.kill().unwrap();
            child.wait().unwrap();
            let curl_pid = std::fs::read_to_string(&pid_file).unwrap();
            if let Some(kill) = find_tool("kill") {
                let _ = std::process::Command::new(kill)
                    .args(["-9", curl_pid.trim()])
                    .status();
            }
            let tmpdir = sb.log("tmpdir").unwrap();
            assert!(Path::new(tmpdir.trim()).starts_with(sb.path("tmp")));
            sb.assert_no_token_on_disk();
            assert!(
                sb.log("auth_cksum").is_some(),
                "{tool}: curl had the header"
            );
        }
    }

    /// L-2: a hostile login profile (`set -euo pipefail`, a function named
    /// `unset`, shadowing `rm`/`curl`, the variable trace) cannot change the
    /// script's control flow or make it leak the token.
    #[test]
    fn a_hostile_profile_does_not_change_the_outcome_or_leak() {
        for tool in ["python3", "jq"] {
            let Some(sb) = Sandbox::new(tool) else {
                continue;
            };
            let hostile = sb.path("bash_env_hostile");
            std::fs::write(
                &hostile,
                format!("{BASH_ENV}set -euo pipefail\nunset() {{ echo HIJACKED-unset; }}\n"),
            )
            .unwrap();
            let env = [("BASH_ENV", hostile.to_str().unwrap())];
            let now = unix_now();

            // No credentials file: the failing `[ -f ]` test must not abort.
            let out = sb.run(&env);
            assert_eq!(out, "__usage_start__\n__no_credentials__\n", "{tool}");

            // Access token expired: unset variables and failing tests abound.
            sb.write_credentials(&creds(now - 5, now + 86_400));
            let out = sb.run(&env);
            assert_eq!(
                parse_usage_output(&out),
                UsageOutcome::AccessTokenExpired,
                "{tool}: {out}"
            );

            // Valid: the full request path, token never on disk (the scan in
            // `run` covers curl's env log too).
            sb.write_credentials(&creds((now + 3600) * 1000, (now + 86_400) * 1000));
            let out = sb.run(&env);
            assert!(out.starts_with("__usage_start__\n"), "{tool}: {out}");
            assert!(!out.contains("HIJACKED"), "{tool}: {out}");
            assert!(
                matches!(parse_usage_output(&out), UsageOutcome::Ok { .. }),
                "{tool}: {out}"
            );
            assert_eq!(
                sb.log("auth_cksum").unwrap(),
                cksum_of(&format!("Authorization: Bearer {FAKE_TOKEN}\n"))
            );
            assert!(sb.log("curl_env").is_some_and(|e| e.contains("PATH=")));
            let tmpdir = sb.log("tmpdir").unwrap();
            assert!(
                !Path::new(tmpdir.trim()).exists(),
                "{tool}: temp dir removed"
            );

            // No HTTP response: curl exits non-zero and writes no headers
            // file — under the profile's errexit/pipefail this would abort
            // the script before it reports anything.
            let out = sb.run(&[env[0], ("FAKE_STATUS", "000")]);
            assert_eq!(
                parse_usage_output(&out),
                UsageOutcome::Unavailable {
                    status: None,
                    snippet: "curl exit 6: curl: (6) Could not resolve host: api.anthropic.com"
                        .into()
                },
                "{tool}: {out}"
            );
        }
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

    #[test]
    fn a_hidden_reachable_host_is_not_a_source() {
        // A hub hides a `local` row copied from a desktop; polling it would run
        // `bash` on the hub itself.
        let mut hidden = host("local", Some("A"), true);
        hidden.hidden = true;
        let mut hidden_remote = host("parked", Some("A"), true);
        hidden_remote.hidden = true;
        let hosts = vec![hidden, hidden_remote, host("alpha", Some("A"), true)];
        assert_eq!(source_hosts("A", &hosts, None), vec!["alpha"]);
        assert_eq!(
            source_hosts("A", &hosts, Some("parked")),
            vec!["alpha"],
            "a hidden sticky host is not tried either"
        );
    }

    // ── clock ──────────────────────────────────────────────────────────────

    /// A manually advanced clock. `advance` moves both the monotonic and the
    /// wall clock; `set_unix` moves only the wall clock (a stale or stepped
    /// wall clock must not affect scheduling).
    struct FakeClock {
        base: Instant,
        state: Mutex<(Duration, i64)>,
    }

    impl FakeClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                base: Instant::now(),
                state: Mutex::new((Duration::ZERO, NOW)),
            })
        }

        fn advance(&self, secs: u64) {
            let mut st = self.state.lock().unwrap();
            st.0 += Duration::from_secs(secs);
            st.1 += secs as i64;
        }

        fn set_unix(&self, unix: i64) {
            self.state.lock().unwrap().1 = unix;
        }
    }

    impl Clock for FakeClock {
        fn now_instant(&self) -> Instant {
            self.base + self.state.lock().unwrap().0
        }

        fn now_unix(&self) -> i64 {
            self.state.lock().unwrap().1
        }
    }

    fn fake_cache() -> (Arc<FakeClock>, Mutex<UsageCache>) {
        let clock = FakeClock::new();
        let cache = Mutex::new(UsageCache::with_clock(clock.clone()));
        (clock, cache)
    }

    /// An `SshExec` whose `run_bounded` takes `took` seconds of fake time,
    /// so a test can prove the next try is scheduled from the attempt's END.
    struct SlowSsh {
        inner: FakeSsh,
        clock: Arc<FakeClock>,
        took: u64,
    }

    #[async_trait::async_trait]
    impl SshExec for SlowSsh {
        async fn run(
            &self,
            host: &str,
            args: &[&str],
            timeout: Duration,
        ) -> Result<std::process::Output, IpcError> {
            self.inner.run(host, args, timeout).await
        }

        async fn run_bounded(
            &self,
            host: &str,
            args: &[&str],
            connect_timeout: Duration,
            wall_clock: Duration,
        ) -> Result<std::process::Output, IpcError> {
            self.clock.advance(self.took);
            self.inner
                .run_bounded(host, args, connect_timeout, wall_clock)
                .await
        }

        async fn run_cancellable(
            &self,
            host: &str,
            args: &[&str],
            timeout: Duration,
            token: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, IpcError> {
            self.inner.run_cancellable(host, args, timeout, token).await
        }

        async fn run_bounded_cancellable(
            &self,
            host: &str,
            args: &[&str],
            connect_timeout: Duration,
            wall_clock: Duration,
            token: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, IpcError> {
            self.inner
                .run_bounded_cancellable(host, args, connect_timeout, wall_clock, token)
                .await
        }

        async fn upload_file(
            &self,
            host: &str,
            local_path: &Path,
            remote_path: &str,
            timeout: Duration,
        ) -> Result<(), IpcError> {
            self.inner
                .upload_file(host, local_path, remote_path, timeout)
                .await
        }

        async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
            self.inner.remote_home(host).await
        }
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
            Reply::ok("__usage_start__\n__expires_at__=1\n__access_token_expired__\n"),
        )
        .on_host(
            "b",
            Match::script_contains("api/oauth/usage"),
            Reply::ok(&ok_output(OK_BODY)),
        );
        let (clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::Ok);
        assert_eq!(snap.source_host.as_deref(), Some("b"));
        assert_eq!(snap.subscription.as_deref(), Some("max"));
        assert_eq!(snap.fetched_at, Some(NOW));
        assert_eq!(snap.next_try_at, NOW + 300);
        assert_eq!(snap.detail.as_deref(), Some("a: access token expired"));
        assert_eq!(usage_calls(&fake, "a"), 1);
        assert_eq!(usage_calls(&fake, "b"), 1);
        let call = &fake.calls_for("b")[0];
        assert_eq!(call.args[0], "bash");
        assert_eq!(call.args[1], "-lc");
        assert!(call
            .script()
            .unwrap()
            .contains(&format!("-A {}", quote(&user_agent()))));

        // Next due fetch: b (sticky) is asked first, a is not asked at all.
        fake.clear_calls();
        clock.advance(300);
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
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
        let (_clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::RateLimited);
        assert_eq!(snap.source_host.as_deref(), Some("a"));
        assert_eq!(snap.next_try_at, NOW + 300);
        assert!(fake.calls_for("b").is_empty());
    }

    #[tokio::test]
    async fn moves_on_only_when_ssh_never_connected_and_stops_at_unavailable() {
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
        let (_clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
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
    async fn an_ssh_timeout_stops_the_fallback() {
        let hosts = vec![host("a", Some("acct"), true), host("b", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.hanging("a")
            .on_host("b", Match::Any, Reply::ok(&ok_output(OK_BODY)))
            .set_wall_clock(Duration::from_millis(50));
        let (_clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert!(fake.calls_for("b").is_empty(), "the request may have left");
        assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        assert!(
            snap.detail.as_deref().unwrap().starts_with("a: "),
            "{snap:?}"
        );
    }

    #[tokio::test]
    async fn the_next_try_is_scheduled_from_the_end_of_the_attempt() {
        let hosts = vec![host("a", Some("acct"), true)];
        let inner = FakeSsh::new();
        inner.on(Match::Any, Reply::ok(&ok_output(OK_BODY)));
        let (clock, cache) = fake_cache();
        let ssh = SlowSsh {
            inner: inner.clone(),
            clock: clock.clone(),
            took: 80,
        };
        let snap = fetch_account_usage_with("acct", &hosts, &ssh, &cache, false).await;
        assert_eq!(snap.fetched_at, Some(NOW + 80));
        assert_eq!(snap.next_try_at, NOW + 80 + 300);
        clock.advance(300 - 80); // 300 s after the attempt STARTED
        fetch_account_usage_with("acct", &hosts, &ssh, &cache, false).await;
        assert_eq!(inner.calls().len(), 1, "80 s short of the floor");
        clock.advance(79);
        fetch_account_usage_with("acct", &hosts, &ssh, &cache, false).await;
        assert_eq!(inner.calls().len(), 1, "1 s short of the floor");
        clock.advance(1);
        fetch_account_usage_with("acct", &hosts, &ssh, &cache, false).await;
        assert_eq!(inner.calls().len(), 2);
    }

    /// M-1: with a caller-supplied timestamp, a stale `now` (a poller that
    /// read the clock once per sweep) shortened the floor. Time now comes
    /// only from the cache's monotonic clock; a wall clock that lags or
    /// steps back changes nothing.
    #[tokio::test]
    async fn a_stale_or_stepped_wall_clock_cannot_shorten_the_floor() {
        let hosts = vec![host("a", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok(&ok_output(OK_BODY)));
        let (clock, cache) = fake_cache();
        fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(fake.calls().len(), 1);
        // 100 s of real (monotonic) time pass, but the wall clock reads an
        // hour in the past — as a stale or stepped-back clock would.
        clock.advance(100);
        clock.set_unix(NOW - 3600);
        fetch_account_usage_with("acct", &hosts, &fake, &cache, true).await;
        // And far in the future.
        clock.set_unix(NOW + 86_400);
        fetch_account_usage_with("acct", &hosts, &fake, &cache, true).await;
        assert_eq!(fake.calls().len(), 1, "the floor is monotonic");
        clock.advance(200);
        fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(fake.calls().len(), 2);
    }

    #[tokio::test]
    async fn a_failure_after_the_connection_came_up_stops_the_fallback() {
        let hosts = vec![host("a", Some("acct"), true), host("b", Some("acct"), true)];
        for reply in [
            Reply::fail(255, "Connection to a closed by remote host.\r\n"),
            Reply::fail(255, "client_loop: send disconnect: Broken pipe\r\n"),
            // A profile line that merely LOOKS like a connect failure, after
            // the script started.
            Reply::Exit {
                code: 255,
                stdout: b"__usage_start__\n".to_vec(),
                stderr: b"proxy check: ssh: connect to host proxy port 3128: Connection refused\nConnection refused\n".to_vec(),
            },
            // Even with a genuine-looking last line, the start marker wins.
            Reply::Exit {
                code: 255,
                stdout: b"__usage_start__\n".to_vec(),
                stderr: b"ssh: connect to host a port 22: Connection refused\n".to_vec(),
            },
            // No marker, but ssh's message is not the LAST stderr line.
            Reply::fail(
                255,
                "ssh: connect to host a port 22: Connection refused\nsomething else\n",
            ),
            Reply::fail(1, ""),
            // Output at the cap: markers may have been cut off.
            Reply::ok(&"x".repeat(OUTPUT_CAP + 10)),
        ] {
            let fake = FakeSsh::new();
            fake.on_host("a", Match::Any, reply.clone())
                .on_host("b", Match::Any, Reply::ok(&ok_output(OK_BODY)));
            let (_clock, cache) = fake_cache();
            let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
            assert!(fake.calls_for("b").is_empty(), "{reply:?} → {snap:?}");
            assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        }
    }

    #[tokio::test]
    async fn a_genuine_connect_failure_falls_back() {
        let hosts = vec![host("a", Some("acct"), true), host("b", Some("acct"), true)];
        for stderr in [
            "ssh: connect to host a port 22: Connection refused\r\n",
            "Warning: Permanently added 'a' to known hosts.\nssh: Could not resolve hostname a: Name or service not known\n",
            "Permission denied (publickey).\n",
            "Host key verification failed.\n",
            "kex_exchange_identification: read: Connection reset by peer\n",
            "Connection closed by 10.0.0.1 port 22\n",
        ] {
            let fake = FakeSsh::new();
            fake.on_host("a", Match::Any, Reply::fail(255, stderr))
                .on_host("b", Match::Any, Reply::ok(&ok_output(OK_BODY)));
            let (_clock, cache) = fake_cache();
            let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
            assert_eq!(snap.status, UsageOutcomeKind::Ok, "{stderr}");
            assert_eq!(snap.source_host.as_deref(), Some("b"));
        }
    }

    #[test]
    fn connect_failure_detection_is_strict() {
        let yes = [
            ("", "ssh: connect to host h port 22: Connection refused"),
            (
                "",
                "ssh: Could not resolve hostname h: nodename nor servname provided",
            ),
            ("", "Permission denied (publickey)."),
            ("", "Host key verification failed."),
            (
                "",
                "kex_exchange_identification: read: Connection reset by peer",
            ),
            ("", "Connection closed by 10.0.0.1 port 22"),
            (
                "banner\n",
                "debug noise\nssh: connect to host h port 22: Operation timed out\r\n",
            ),
        ];
        for (out, err) in yes {
            assert!(connection_never_established(out, err), "{err}");
        }
        let no = [
            (
                "__usage_start__\n",
                "ssh: connect to host h port 22: Connection refused",
            ),
            ("", "Connection refused"),
            ("", "Connection to h closed by remote host."),
            ("", "client_loop: send disconnect: Broken pipe"),
            ("", "Connection closed by remote"),
            (
                "",
                "ssh: connect to host h port 22: Connection refused\nlater line",
            ),
            ("", ""),
        ];
        for (out, err) in no {
            assert!(!connection_never_established(out, err), "{out:?} {err}");
        }
    }

    #[tokio::test]
    async fn token_rejected_allows_exactly_one_follow_up_host() {
        let hosts = vec![
            host("a", Some("acct"), true),
            host("b", Some("acct"), true),
            host("c", Some("acct"), true),
        ];
        let rejected = "__http_status__=401\n__body__\n{}";
        // a rejected, b rejected → c is never asked.
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok(rejected)).on_host(
            "c",
            Match::Any,
            Reply::ok(&ok_output(OK_BODY)),
        );
        let (_clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::TokenRejected);
        assert_eq!(fake.calls_for("b").len(), 1);
        assert!(fake.calls_for("c").is_empty());

        // a rejected, b has no credentials (no request) → still only one
        // follow-up: c is not asked.
        let fake = FakeSsh::new();
        fake.on_host("a", Match::Any, Reply::ok(rejected))
            .on_host("b", Match::Any, Reply::ok("__no_credentials__\n"))
            .on_host("c", Match::Any, Reply::ok(&ok_output(OK_BODY)));
        let (_clock, cache) = fake_cache();
        fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert!(fake.calls_for("c").is_empty());

        // a rejected, b answers → ok via b.
        let fake = FakeSsh::new();
        fake.on_host("a", Match::Any, Reply::ok(rejected)).on_host(
            "b",
            Match::Any,
            Reply::ok(&ok_output(OK_BODY)),
        );
        let (_clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::Ok);
        assert_eq!(snap.source_host.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn every_host_in_a_pre_request_state_retries_at_the_floor() {
        let hosts = vec![
            host("local", Some("acct"), true),
            host("trn", Some("acct"), true),
        ];
        let fake = FakeSsh::new();
        fake.on_host("local", Match::Any, Reply::ok("__no_credentials__\n"))
            .on_host("trn", Match::Any, Reply::ok("__login_expired__\n"));
        let (clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::LoginExpired);
        assert_eq!(snap.next_try_at, NOW + 300);
        assert_eq!(
            snap.detail.as_deref(),
            Some("trn: login expired; local: no credentials file")
        );
        assert_eq!(cache.lock().unwrap().entries["acct"].backoff_secs, 0);
        clock.advance(300);
        fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        clock.advance(300);
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.next_try_at, NOW + 900, "no escalation");
    }

    #[tokio::test]
    async fn all_hosts_unreachable_backs_off() {
        let hosts = vec![host("a", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.unreachable("a");
        let (clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        assert_eq!(snap.next_try_at, NOW + 300);
        assert!(snap.detail.unwrap().starts_with("a: exit 255"));
        clock.advance(300);
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.next_try_at, NOW + 300 + 600);
    }

    #[tokio::test]
    async fn no_reachable_host_records_no_online_host_without_a_call() {
        let hosts = vec![
            host("a", Some("acct"), false),
            host("b", Some("other"), true),
        ];
        let fake = FakeSsh::new();
        let (_clock, cache) = fake_cache();
        let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, false).await;
        assert_eq!(snap.status, UsageOutcomeKind::NoOnlineHost);
        assert!(fake.calls().is_empty());
        assert_eq!(snap.next_try_at, 0, "no request made; schedule untouched");
        assert!(cache.lock().unwrap().due("acct"));
    }

    #[tokio::test]
    async fn floor_holds_even_when_forced() {
        let hosts = vec![host("a", Some("acct"), true)];
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok(&ok_output(OK_BODY)));
        let (clock, cache) = fake_cache();
        let first = fetch_account_usage_with("acct", &hosts, &fake, &cache, true).await;
        assert_eq!(fake.calls().len(), 1);
        for dt in [1, 59, 239] {
            clock.advance(dt);
            let snap = fetch_account_usage_with("acct", &hosts, &fake, &cache, true).await;
            assert_eq!(snap, first);
        }
        assert_eq!(fake.calls().len(), 1, "no ssh call inside the floor");
        clock.advance(1);
        fetch_account_usage_with("acct", &hosts, &fake, &cache, true).await;
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn never_fetched_snapshot() {
        let (_clock, cache) = fake_cache();
        let c = cache.lock().unwrap();
        let snap = c.snapshot("acct");
        assert_eq!(snap.status, UsageOutcomeKind::NeverFetched);
        assert!(snap.usage.is_none());
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["status"], "never_fetched");
        assert!(json["usage"].is_null());
        assert!(c.due("acct"));
        assert!(UsageCache::new().due("acct"), "the real-clock cache works");
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
        } = parse_usage_output(&ok_output(OK_BODY))
        else {
            panic!()
        };
        answered(UsageOutcome::Ok {
            usage,
            subscription,
        })
    }

    fn fake_plain_cache() -> (Arc<FakeClock>, UsageCache) {
        let clock = FakeClock::new();
        let cache = UsageCache::with_clock(clock.clone());
        (clock, cache)
    }

    #[test]
    fn rate_limit_backoff_with_and_without_retry_after() {
        let (clock, mut c) = fake_plain_cache();
        c.record("a", rate_limited(None));
        assert_eq!(c.entries["a"].next_try_at, NOW + 300);
        clock.advance(299);
        assert!(!c.due("a"));
        clock.advance(1);
        assert!(c.due("a"));
        // Second 429 at NOW+300: backoff doubles to 600.
        c.record("a", rate_limited(None));
        assert_eq!(c.entries["a"].next_try_at, NOW + 300 + 600);
        clock.advance(599);
        assert!(!c.due("a"));
        clock.advance(1);
        assert!(c.due("a"));
        // Retry-After larger than the backoff (1200) wins.
        let t = NOW + 900;
        c.record("a", rate_limited(Some(3000)));
        assert_eq!(c.entries["a"].backoff_secs, 1200);
        assert_eq!(c.entries["a"].next_try_at, t + 3000);
        clock.advance(2999);
        assert!(!c.due("a"));
        clock.advance(1);
        assert!(c.due("a"));
        // Retry-After smaller than the backoff (1800) loses.
        let t = t + 3000;
        c.record("a", rate_limited(Some(10)));
        assert_eq!(c.entries["a"].backoff_secs, 1800);
        assert_eq!(c.entries["a"].next_try_at, t + 1800);
        clock.advance(1799);
        assert!(!c.due("a"));
        clock.advance(1);
        assert!(c.due("a"));
        let snap = c.snapshot("a");
        assert_eq!(snap.status, UsageOutcomeKind::RateLimited);
        assert_eq!(
            serde_json::to_value(&snap).unwrap()["status"],
            "rate_limited"
        );
    }

    #[test]
    fn unavailable_backoff_doubles_to_the_cap_and_resets_after_ok() {
        let (_clock, mut c) = fake_plain_cache();
        let mut seen = vec![];
        for _ in 0..6 {
            c.record("a", unavailable());
            seen.push(c.entries["a"].next_try_at - NOW);
        }
        assert_eq!(seen, vec![300, 600, 1200, 1800, 1800, 1800]);
        c.record("a", ok_result());
        assert_eq!(c.entries["a"].backoff_secs, 0);
        assert_eq!(c.entries["a"].next_try_at, NOW + 300);
        c.record("a", unavailable());
        assert_eq!(c.entries["a"].next_try_at, NOW + 300, "backoff restarted");
    }

    #[test]
    fn a_clock_stepping_backwards_does_not_freeze_polling() {
        let (clock, mut c) = fake_plain_cache();
        c.record("a", ok_result());
        // The wall clock jumps back an hour; monotonic time moves on.
        clock.set_unix(NOW - 3600);
        clock.advance(299);
        assert!(!c.due("a"));
        clock.advance(1);
        assert!(c.due("a"), "due 300 s later despite the wall clock");
        // A rate-limit wait keeps its full length across a step back too.
        c.record("a", rate_limited(Some(3000)));
        clock.set_unix(NOW - 100_000);
        clock.advance(2999);
        assert!(!c.due("a"));
        clock.advance(1);
        assert!(c.due("a"));
        // And a wall clock jumping FORWARD does not make it due early.
        c.record("a", ok_result());
        clock.set_unix(NOW + 1_000_000);
        assert!(!c.due("a"));
    }

    #[test]
    fn last_known_values_survive_later_failures() {
        let (clock, mut c) = fake_plain_cache();
        c.record("a", ok_result());
        let ok = c.snapshot("a");
        clock.advance(300);
        c.record("a", unavailable());
        clock.advance(600);
        c.record("a", rate_limited(Some(60)));
        clock.advance(1100);
        c.record(
            "a",
            FetchResult::AllFailed {
                host_state: Some(UsageOutcomeKind::LoginExpired),
                transport_error: false,
                notes: vec!["h: login expired".into()],
            },
        );
        clock.advance(400);
        c.record("a", FetchResult::NoOnlineHost);
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
