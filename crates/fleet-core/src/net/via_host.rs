//! Tracker transports that run on a fleet host over SSH (work graph M6).
//!
//! * [`GhCliTransport`] (`via_cli:<alias>`, M6.1): `gh api` on the host. The
//!   request body goes to `gh` on **stdin** (`--input -`), and `gh` adds the
//!   host's own GitHub login: fleet never reads, stores or sends a GitHub
//!   token, and a request that carries an `Authorization` header is refused.
//!
//! * [`CurlTransport`] (`via_host:<alias>`, M6.3): `curl` on the host, for a
//!   tracker only that host can reach (a VPN, an internal network) or when
//!   the operator wants requests to leave from there. The request's headers
//!   — the credential among them — and its body are piped on **stdin** into
//!   private temp files (`umask 077`, removed on exit), never in argv or
//!   the environment; curl reads them with `-H @file` / `--data-binary @file`.
//!
//! Both halves of the SSRF fence hold here as they do for
//! [`super::https::DirectTransport`]: https only, and only the hosts the
//! caller's policy allows (for `gh`, `api.github.com` alone); a request is
//! refused before anything runs on the host. Every value interpolated into
//! the script is `shell::quote`d, no flag comes from the caller, the output
//! is capped, the exchange is bounded, and the answer is parsed as HTTP —
//! never evaluated.

use super::https::{parse_target, HttpTransport, Method, Request, Response, TransportError};
use crate::ssh::SshExec;
use std::sync::Arc;
use std::time::Duration;

/// SSH connect budget for a transport call.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Headers `gh --include` prints before the body, at most.
const HEAD_MAX: usize = 64 * 1024;
/// The marker the script prints when the host has no `gh`.
const NO_GH: &str = "__fleet_no_gh__";
/// The line each script prints before anything else of its own. Both run
/// under `bash -lc`, and a login profile (`~/.bash_profile`, `/etc/profile.d`)
/// that prints to stdout — a greeting, an nvm / conda notice — lands BEFORE
/// the script's output, so the parsers anchor on this marker, never on byte
/// 0.
const BEGIN: &str = "__fleet_begin__";

/// The script's own output: what follows the first [`BEGIN`] line. Searched
/// as `marker + '\n'` anywhere, since a profile's last line may lack its
/// newline and glue itself to the marker. The whole of `raw` when the marker
/// is missing (the script never ran): the callers' error paths then read the
/// exit code and stderr, exactly as before.
pub(crate) fn after_begin(raw: &[u8]) -> &[u8] {
    let needle = format!("{BEGIN}\n");
    match super::http1::find(raw, needle.as_bytes()) {
        Some(i) => &raw[i + needle.len()..],
        None => raw,
    }
}
/// The only API host `gh` is pointed at.
pub const GITHUB_API_HOST: &str = "api.github.com";

/// `gh api` on a host (see the module docs).
pub struct GhCliTransport {
    ssh: Arc<dyn SshExec>,
    host: String,
    max_body: u64,
}

impl GhCliTransport {
    pub fn new(ssh: Arc<dyn SshExec>, host: impl Into<String>) -> Self {
        GhCliTransport {
            ssh,
            host: host.into(),
            max_body: super::https::DEFAULT_MAX_BODY,
        }
    }

    /// The script run on the host for `path` (the request target, already
    /// fenced). Built from constants and one `shell::quote`d value.
    pub fn script(method: Method, path: &str, has_body: bool) -> String {
        let mut s = format!(
            "printf '%s\\n' {BEGIN}\n\
             command -v gh >/dev/null 2>&1 || {{ printf '%s\\n' {NO_GH}; exit 0; }}\n\
             export GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 GH_SPINNER_DISABLED=1 \
             NO_COLOR=1 GH_PAGER=cat\n\
             exec gh api --include --hostname github.com --method {} {}",
            method.as_str(),
            crate::shell::quote(path)
        );
        if has_body {
            s.push_str(" --input -");
        }
        s
    }
}

/// `gh`'s exit code for "you are not logged in".
const GH_EXIT_AUTH: i32 = 4;

/// Split `gh api --include` output (`HTTP/2.0 200 OK`, `Name: value` lines,
/// a blank line, the body) into a [`Response`]. Lenient about `\r\n` vs
/// `\n`; `None` when it does not start with a status line.
pub fn parse_included(raw: &[u8]) -> Option<Response> {
    if !raw.starts_with(b"HTTP/") {
        return None;
    }
    let lf = super::http1::find(raw, b"\n\n").map(|i| (i, 2));
    let crlf = super::http1::find(raw, b"\r\n\r\n").map(|i| (i, 4));
    let (split, sep) = match (lf, crlf) {
        (Some(a), Some(b)) => {
            if a.0 < b.0 {
                a
            } else {
                b
            }
        }
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => (raw.len(), 0),
    };
    let head = String::from_utf8_lossy(&raw[..split.min(HEAD_MAX)]);
    let mut lines = head.lines().map(|l| l.trim_end_matches('\r'));
    let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Some(Response {
        status,
        headers,
        body: raw.get(split + sep..).unwrap_or_default().to_vec(),
    })
}

/// An SSH-level failure → the transport error it means.
pub(crate) fn ssh_error(host: &str, e: crate::ipc_error::IpcError) -> TransportError {
    if e.code == crate::ipc_error::codes::E_SSH_TIMEOUT {
        TransportError::Timeout
    } else {
        TransportError::Connect(format!("{host}: {}", crate::logging::redact(&e.message)))
    }
}

/// The first line of `stderr`, redacted and capped: enough to act on,
/// never a token.
pub(crate) fn first_line(stderr: &[u8]) -> String {
    let t = String::from_utf8_lossy(stderr);
    let line = t
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    crate::logging::redact(line).chars().take(200).collect()
}

#[async_trait::async_trait]
impl HttpTransport for GhCliTransport {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        let at = parse_target(&req.url)?;
        if !at.endpoint.is_tls() || at.endpoint.host() != GITHUB_API_HOST {
            return Err(TransportError::Refused(format!(
                "gh is only ever pointed at https://{GITHUB_API_HOST}"
            )));
        }
        if req.header_value("Authorization").is_some() {
            return Err(TransportError::Refused(
                "gh uses the host's own login; fleet never sends it a token".into(),
            ));
        }
        let script = Self::script(req.method, &at.target, req.body.is_some());
        let quoted = crate::shell::quote(&script);
        let args = ["bash", "-lc", quoted.as_str()];
        let cap = (self.max_body as usize) + HEAD_MAX;
        let out = self
            .ssh
            .run_with_stdin(
                &self.host,
                &args,
                req.body.clone().unwrap_or_default(),
                CONNECT_TIMEOUT,
                req.timeout + CONNECT_TIMEOUT,
                cap + 1,
            )
            .await
            .map_err(|e| ssh_error(&self.host, e))?;
        if out.stdout.len() > cap {
            return Err(TransportError::TooLarge(format!(
                "gh on {} answered more than {} MiB",
                self.host,
                self.max_body / (1024 * 1024)
            )));
        }
        let code = out.status.code().unwrap_or(-1);
        if code == 255 {
            return Err(TransportError::Connect(format!(
                "ssh {}: {}",
                self.host,
                first_line(&out.stderr)
            )));
        }
        let stdout = after_begin(&out.stdout);
        if stdout.starts_with(NO_GH.as_bytes()) {
            return Err(TransportError::Connect(format!(
                "gh is not installed on {}; install the GitHub CLI there and run `gh auth login`",
                self.host
            )));
        }
        if let Some(resp) = parse_included(stdout) {
            return Ok(resp);
        }
        let err = first_line(&out.stderr);
        if code == GH_EXIT_AUTH || err.contains("gh auth login") {
            return Err(TransportError::Connect(format!(
                "gh on {} is not logged in; run `gh auth login` there",
                self.host
            )));
        }
        Err(TransportError::Protocol(format!(
            "gh on {} failed (exit {code}): {err}",
            self.host
        )))
    }
}

// --- curl ---------------------------------------------------------------------

/// Placeholders [`CurlTransport::script`] fills, each with a
/// `shell::quote`d value or a number.
const P_HLEN: &str = "@@HLEN@@";
const P_METHOD: &str = "@@METHOD@@";
const P_DATA: &str = "@@DATA@@";
const P_TIME: &str = "@@TIME@@";
const P_CAP: &str = "@@CAP@@";
const P_URL: &str = "@@URL@@";

/// The script `via_host` runs. Read the notes on [`CurlTransport`] before
/// changing a line.
const CURL_SCRIPT: &str = r#"builtin unalias -a 2>/dev/null
builtin unset -f unset unalias builtin command set trap umask exit printf test [ curl rm mktemp cat head dd wc grep sed 2>/dev/null
set +x
umask 077
trap '' PIPE
unset SSLKEYLOGFILE CURL_HOME
printf '%s
' __fleet_begin__
if ! command -v curl >/dev/null 2>&1; then printf '%s
' __fleet_no_curl__; exit 0; fi
cv=$(curl -q --version 2>/dev/null | sed -n '1s/^curl \([0-9][0-9]*\)\.\([0-9][0-9]*\).*/\1 \2/p')
cmaj=${cv%% *}
cmin=${cv##* }
case "$cmaj" in ''|*[!0-9]*) cmaj=0 ;; esac
case "$cmin" in ''|*[!0-9]*) cmin=0 ;; esac
if [ "$cmaj" -lt 7 ] || { [ "$cmaj" -eq 7 ] && [ "$cmin" -lt 55 ]; }; then printf '%s
' __fleet_old_curl__; exit 0; fi
d=$(mktemp -d "${TMPDIR:-/tmp}/fleet-tracker.XXXXXX") || { printf '%s
' __fleet_no_tmp__; exit 0; }
trap 'rm -rf "$d"' EXIT HUP INT TERM
dd bs=1 count=@@HLEN@@ of="$d/h" 2>/dev/null
cat > "$d/b"
code=$(curl -q -sS --proto =https --proto-redir =https --max-redirs 0 --max-time @@TIME@@ --max-filesize @@CAP@@ -X @@METHOD@@ -H @"$d/h" @@DATA@@ -D "$d/rh" -o "$d/rb" -w '%{http_code}' @@URL@@ 2>"$d/err")
rc=$?
printf '__fleet_status__=%s
' "$code"
printf '__fleet_curl_exit__=%s
' "$rc"
if [ "$code" = 000 ] || [ "$rc" -ne 0 ]; then
  grep -a '^curl: (' "$d/err" 2>/dev/null | head -n 2
  exit 0
fi
printf '__fleet_head__=%s
' "$(wc -c < "$d/rh" | tr -d ' ')"
cat "$d/rh"
head -c $((@@CAP@@ + 1)) "$d/rb"
"#;

/// `curl` on a host (see the module docs). Where the credential is, and is
/// not:
///
/// - It is one of the request's headers, which fleet writes to **stdin**
///   ahead of the body; the script copies exactly that many bytes (`dd
///   bs=1`, which never reads past its count) into `$d/h` and the rest into
///   `$d/b`, both in a `mktemp -d` directory under `umask 077`, removed by
///   an EXIT trap whatever happens.
/// - `curl -q` (first, so `~/.curlrc` cannot add `--verbose` / `--trace`)
///   reads them with `-H @"$d/h"` / `--data-binary @"$d/b"`: the token is in
///   no argv (`ps` shows only the file names), no variable and no
///   environment.
/// - Nothing prints headers that were SENT; the script prints the answer's
///   status and curl's exit code, then either the headers and one byte past the
///   cap of its body (exit 0) or at most two `curl: (N) …` lines (any other
///   exit — a status is known as soon as the head arrived, so it never
///   vouches for the body on its own).
/// - https only, no redirect followed, bounded by `--max-time`, and the
///   host fence was checked before anything ran.
pub struct CurlTransport {
    ssh: Arc<dyn SshExec>,
    host: String,
    allow_host: super::https::HostPolicy,
    max_body: u64,
}

impl CurlTransport {
    pub fn new(
        ssh: Arc<dyn SshExec>,
        host: impl Into<String>,
        allow_host: super::https::HostPolicy,
    ) -> Self {
        CurlTransport {
            ssh,
            host: host.into(),
            allow_host,
            max_body: super::https::DEFAULT_MAX_BODY,
        }
    }

    /// The script for one request: every placeholder is a number, a method
    /// constant or a `shell::quote`d URL.
    pub fn script(
        method: Method,
        url: &str,
        header_len: usize,
        has_body: bool,
        timeout_secs: u64,
        cap: u64,
    ) -> String {
        CURL_SCRIPT
            .replace(P_HLEN, &header_len.to_string())
            .replace(P_METHOD, method.as_str())
            .replace(
                P_DATA,
                if has_body {
                    r#"--data-binary @"$d/b""#
                } else {
                    ""
                },
            )
            .replace(P_TIME, &timeout_secs.to_string())
            .replace(P_CAP, &cap.to_string())
            .replace(P_URL, &crate::shell::quote(url))
    }

    /// The bytes piped to the script: the header lines, then the body.
    fn stdin(req: &Request) -> Result<(Vec<u8>, usize), TransportError> {
        let mut head = String::new();
        for (k, v) in &req.headers {
            if k.is_empty()
                || k.contains(':')
                || k.bytes()
                    .chain(v.bytes())
                    .any(|b| b == b'\r' || b == b'\n' || b == 0)
            {
                return Err(TransportError::Refused(format!("malformed header {k:?}")));
            }
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push('\n');
        }
        // No `Expect: 100-continue` round trip for a larger body.
        head.push_str("Expect:\n");
        let n = head.len();
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(req.body.as_deref().unwrap_or_default());
        Ok((bytes, n))
    }
}

/// Split the script's output into a [`Response`]; `max_body` names the cap
/// in the [`TransportError::TooLarge`] a size-aborted answer becomes.
fn parse_curl(host: &str, raw: &[u8], max_body: u64) -> Result<Response, TransportError> {
    if raw.starts_with(b"__fleet_no_curl__") {
        return Err(TransportError::Connect(format!(
            "curl is not installed on {host}"
        )));
    }
    if raw.starts_with(b"__fleet_old_curl__") {
        return Err(TransportError::Connect(format!(
            "curl on {host} is older than 7.55 (no -H @file); upgrade it there"
        )));
    }
    if raw.starts_with(b"__fleet_no_tmp__") {
        return Err(TransportError::Connect(format!(
            "{host} has no writable temp directory for the request"
        )));
    }
    let status_line_end = super::http1::find(raw, b"\n")
        .ok_or_else(|| TransportError::Protocol(format!("no answer from curl on {host}")))?;
    let first = String::from_utf8_lossy(&raw[..status_line_end]);
    let code = first
        .strip_prefix("__fleet_status__=")
        .ok_or_else(|| TransportError::Protocol(format!("unexpected output from {host}")))?
        .trim()
        .to_string();
    // curl's exit code is the second line, always: `%{http_code}` reports
    // the status as soon as the head arrived, so a body cut by `--max-time`
    // (28), refused by `--max-filesize` (63) or lost to a recv error (56) /
    // partial transfer (18) still comes with the real status. Only exit 0
    // vouches for the body.
    let rest = &raw[status_line_end + 1..];
    let exit_line_end = super::http1::find(rest, b"\n")
        .ok_or_else(|| TransportError::Protocol(format!("a truncated answer from {host}")))?;
    let exit: i32 = String::from_utf8_lossy(&rest[..exit_line_end])
        .strip_prefix("__fleet_curl_exit__=")
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| TransportError::Protocol(format!("a truncated answer from {host}")))?;
    let rest = &rest[exit_line_end + 1..];
    if code == "000" || exit != 0 {
        let why: String = String::from_utf8_lossy(rest)
            .lines()
            .filter(|l| l.starts_with("curl: ("))
            .map(|l| crate::logging::redact(l).into_owned())
            .collect::<Vec<_>>()
            .join("; ")
            .chars()
            .take(300)
            .collect();
        return Err(match exit {
            28 => TransportError::Timeout,
            63 => TransportError::TooLarge(format!(
                "curl on {host} refused a body over {} MiB (status {code}): {why}",
                max_body / (1024 * 1024)
            )),
            _ if code == "000" => {
                TransportError::Connect(format!("curl on {host} (exit {exit}): {why}"))
            }
            _ => TransportError::Protocol(format!(
                "curl on {host} cut the answer short (exit {exit}, status {code}): {why}"
            )),
        });
    }
    let head_line_end = super::http1::find(rest, b"\n")
        .ok_or_else(|| TransportError::Protocol(format!("a truncated answer from {host}")))?;
    let head_len: usize = String::from_utf8_lossy(&rest[..head_line_end])
        .strip_prefix("__fleet_head__=")
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| TransportError::Protocol(format!("a truncated answer from {host}")))?;
    let rest = &rest[head_line_end + 1..];
    if rest.len() < head_len || head_len > HEAD_MAX {
        return Err(TransportError::Protocol(format!(
            "a truncated answer from {host}"
        )));
    }
    let (head, body) = rest.split_at(head_len);
    // The script prints one byte MORE than the cap, so "over the cap" is
    // distinguishable from "exactly the cap" — the same rule as
    // `conn::speak`. A chunked answer (no size for `--max-filesize` to
    // refuse up front, on a curl older than 8.4) is caught here.
    if body.len() as u64 > max_body {
        return Err(TransportError::TooLarge(format!(
            "curl on {host} answered more than {} MiB; refusing to buffer it \
             (silently truncating at the cap surfaced as unreadable JSON, \
             which names the wrong problem)",
            max_body / (1024 * 1024)
        )));
    }
    // A proxy's `200 Connection established` (or a `100 Continue`) comes
    // first: the answer is the LAST header block.
    let text = String::from_utf8_lossy(head);
    let last = text
        .rfind("HTTP/")
        .map(|i| &text[i..])
        .ok_or_else(|| TransportError::Protocol(format!("no status line from {host}")))?;
    let mut resp = parse_included(last.as_bytes())
        .ok_or_else(|| TransportError::Protocol(format!("no status line from {host}")))?;
    resp.body = body.to_vec();
    Ok(resp)
}

#[async_trait::async_trait]
impl HttpTransport for CurlTransport {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        let at = parse_target(&req.url)?;
        if !at.endpoint.is_tls() {
            return Err(TransportError::Refused(
                "plaintext http:// is never used for a tracker".into(),
            ));
        }
        if !(self.allow_host)(at.endpoint.host()) {
            return Err(TransportError::Refused(format!(
                "{} is not an allowed tracker host",
                at.endpoint.host()
            )));
        }
        let (stdin, header_len) = Self::stdin(&req)?;
        let secs = req.timeout.as_secs().max(1);
        let script = Self::script(
            req.method,
            &req.url,
            header_len,
            req.body.is_some(),
            secs,
            self.max_body,
        );
        let quoted = crate::shell::quote(&script);
        let args = ["bash", "-lc", quoted.as_str()];
        // Room for the body's one byte past the cap, the answer's headers,
        // the script's own lines, and a login profile's chatter before the
        // start marker; the body cap itself is `parse_curl`'s.
        let cap = (self.max_body as usize) + HEAD_MAX + 4096;
        let out = self
            .ssh
            .run_with_stdin(
                &self.host,
                &args,
                stdin,
                CONNECT_TIMEOUT,
                req.timeout + CONNECT_TIMEOUT,
                cap + 1,
            )
            .await
            .map_err(|e| ssh_error(&self.host, e))?;
        if out.status.code() == Some(255) {
            return Err(TransportError::Connect(format!(
                "ssh {}: {}",
                self.host,
                first_line(&out.stderr)
            )));
        }
        if out.stdout.len() > cap {
            return Err(TransportError::TooLarge(format!(
                "curl on {} answered more than {} MiB",
                self.host,
                self.max_body / (1024 * 1024)
            )));
        }
        parse_curl(&self.host, after_begin(&out.stdout), self.max_body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use serde_json::json;

    fn gh(f: &FakeSsh) -> GhCliTransport {
        GhCliTransport::new(Arc::new(f.clone()), "devbox")
    }

    #[tokio::test]
    async fn the_body_goes_on_stdin_and_the_script_quotes_its_one_value() {
        let f = FakeSsh::new();
        f.on(
            Match::contains("gh api"),
            Reply::ok("HTTP/2.0 200 OK\nContent-Type: application/json\nX-Ratelimit-Remaining: 4999\n\n{\"data\":{}}"),
        );
        let body = json!({"query": "{ viewer { login } }", "variables": {"q": "'; rm -rf / #"}});
        let r = gh(&f)
            .send(Request::post_json("https://api.github.com/graphql", &body))
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.header("x-ratelimit-remaining"), Some("4999"));
        assert_eq!(r.text(), "{\"data\":{}}");
        let call = &f.calls()[0];
        assert_eq!(call.host, "devbox");
        assert_eq!(call.stdin.as_deref(), Some(body.to_string().as_bytes()));
        let script = call.script().unwrap();
        assert!(
            script.contains(
                "gh api --include --hostname github.com --method POST '/graphql' --input -"
            ),
            "{script}"
        );
        assert!(
            !call.command().contains("rm -rf"),
            "the body is never in argv"
        );
    }

    #[tokio::test]
    async fn only_api_github_com_over_https_and_never_a_token() {
        let f = FakeSsh::new();
        for url in [
            "https://evil.example.com/graphql",
            "http://api.github.com/graphql",
            "https://api.github.com.evil.com/graphql",
            "https://169.254.169.254/latest/meta-data",
        ] {
            let e = gh(&f).send(Request::get(url)).await.unwrap_err();
            assert!(matches!(e, TransportError::Refused(_)), "{url}: {e}");
        }
        let e = gh(&f)
            .send(
                Request::get("https://api.github.com/user").header("Authorization", "Bearer ghp_x"),
            )
            .await
            .unwrap_err();
        assert!(matches!(e, TransportError::Refused(_)));
        assert!(f.calls().is_empty(), "refused before anything ran");
    }

    #[tokio::test]
    async fn a_missing_gh_a_logged_out_gh_and_a_down_host_are_unreachable_with_a_reason() {
        let f = FakeSsh::new();
        f.on(Match::Any, Reply::ok("__fleet_no_gh__\n"));
        let e = gh(&f)
            .send(Request::get("https://api.github.com/user"))
            .await
            .unwrap_err();
        assert!(
            e.is_unreachable() && e.to_string().contains("install the GitHub CLI"),
            "{e}"
        );

        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::fail(
                4,
                "To get started with GitHub CLI, please run:  gh auth login\n",
            ),
        );
        let e = gh(&f)
            .send(Request::get("https://api.github.com/user"))
            .await
            .unwrap_err();
        assert!(
            e.is_unreachable() && e.to_string().contains("gh auth login"),
            "{e}"
        );

        let f = FakeSsh::new();
        f.unreachable("devbox");
        let e = gh(&f)
            .send(Request::get("https://api.github.com/user"))
            .await
            .unwrap_err();
        assert!(e.is_unreachable(), "{e}");
    }

    /// `bash -lc` sources the login profile; a greeting it prints lands
    /// before `gh`'s output and must not hide the status line, the "no gh"
    /// marker included.
    #[tokio::test]
    async fn login_profile_noise_before_the_marker_is_ignored() {
        for noise in ["Welcome bob\n", "welcome"] {
            let f = FakeSsh::new();
            f.on(
                Match::Any,
                Reply::ok(&format!(
                    "{noise}__fleet_begin__\nHTTP/2.0 200 OK\nContent-Type: application/json\n\n{{\"data\":{{}}}}"
                )),
            );
            let r = gh(&f)
                .send(Request::get("https://api.github.com/user"))
                .await
                .unwrap_or_else(|e| panic!("{noise:?}: {e}"));
            assert_eq!((r.status, r.text().as_str()), (200, "{\"data\":{}}"));
            let script = f.calls()[0].script().unwrap();
            assert!(
                script.starts_with("printf '%s\\n' __fleet_begin__\n"),
                "the script prints the marker first: {script}"
            );
        }
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok("Welcome bob\n__fleet_begin__\n__fleet_no_gh__\n"),
        );
        let e = gh(&f)
            .send(Request::get("https://api.github.com/user"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("install the GitHub CLI"), "{e}");
    }

    #[tokio::test]
    async fn an_http_error_comes_back_as_a_response_and_output_is_capped() {
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::Exit {
                code: 1,
                stdout: b"HTTP/2.0 401 Unauthorized\r\nContent-Type: application/json\r\n\r\n{\"message\":\"Bad credentials\"}".to_vec(),
                stderr: b"gh: Bad credentials (HTTP 401)\n".to_vec(),
            },
        );
        let r = gh(&f)
            .send(Request::get("https://api.github.com/user"))
            .await
            .unwrap();
        assert_eq!(r.status, 401);
        assert!(r.text().contains("Bad credentials"));

        let f = FakeSsh::new();
        f.on(Match::Any, Reply::ok(&"x".repeat(HEAD_MAX + 4096)));
        let mut t = gh(&f);
        t.max_body = 1024;
        let e = t.send(Request::get("https://api.github.com/user")).await;
        assert!(matches!(e, Err(TransportError::TooLarge(_))), "{e:?}");
    }
}

#[cfg(test)]
mod curl_tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    const TOKEN: &str = "tok-via-host-not-real-0123456789";

    fn curl(f: &FakeSsh) -> CurlTransport {
        CurlTransport::new(
            Arc::new(f.clone()),
            "vpnbox",
            Arc::new(|h: &str| h == "jira.corp.example"),
        )
    }

    fn answer(status: &str, head: &str, body: &str) -> Reply {
        Reply::ok(&format!(
            "__fleet_begin__\n__fleet_status__={status}\n__fleet_curl_exit__=0\n__fleet_head__={}\n{head}{body}",
            head.len()
        ))
    }

    /// `bash -lc` sources the login profile; whatever it prints to stdout
    /// comes before the script's output and must not be where the status
    /// line is expected — with or without a trailing newline.
    #[tokio::test]
    async fn login_profile_noise_before_the_marker_is_ignored() {
        for noise in ["Welcome bob\nnvm: using node v20\n", "welcome"] {
            let f = FakeSsh::new();
            f.on(
                Match::Any,
                Reply::ok(&format!(
                    "{noise}__fleet_begin__\n__fleet_status__=200\n__fleet_curl_exit__=0\n__fleet_head__=15\nHTTP/2 200 \r\n\r\n{{\"ok\":true}}"
                )),
            );
            let r = curl(&f)
                .send(Request::get("https://jira.corp.example/x"))
                .await
                .unwrap_or_else(|e| panic!("{noise:?}: {e}"));
            assert_eq!((r.status, r.text().as_str()), (200, "{\"ok\":true}"));
        }
        // The marker precedes every outcome, the "no curl" one included.
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok("Welcome bob\n__fleet_begin__\n__fleet_no_curl__\n"),
        );
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("curl is not installed"), "{e}");
        assert!(
            CurlTransport::script(Method::Get, "https://jira.corp.example/x", 1, false, 1, 1)
                .contains("\n' __fleet_begin__\n"),
            "the script prints the marker"
        );
    }

    #[tokio::test]
    async fn the_token_rides_stdin_never_argv_and_the_answer_is_parsed() {
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            answer(
                "200",
                "HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200 \r\nretry-after: 7\r\ncontent-type: application/json\r\n\r\n",
                "{\"ok\":true}",
            ),
        );
        let req = Request::post_json(
            "https://jira.corp.example/rest/api/2/search?x=1",
            &serde_json::json!({"jql": "assignee = currentUser()"}),
        )
        .header("Authorization", format!("Bearer {TOKEN}"));
        let r = curl(&f).send(req).await.unwrap();
        assert_eq!(r.status, 200, "the last header block, not the proxy's");
        assert_eq!(r.header("retry-after"), Some("7"));
        assert_eq!(r.text(), "{\"ok\":true}");
        let call = &f.calls()[0];
        assert_eq!(call.host, "vpnbox");
        let argv = call.command();
        assert!(!argv.contains(TOKEN), "the token is never in argv");
        assert!(!argv.contains("currentUser"), "nor is the body");
        let stdin = call.stdin_str().unwrap();
        let (head, body) = stdin.split_once("Expect:\n").unwrap();
        assert!(head.contains(&format!("Authorization: Bearer {TOKEN}\n")));
        assert_eq!(body, "{\"jql\":\"assignee = currentUser()\"}");
        let script = call.script().unwrap();
        assert!(script.contains(&format!(
            "dd bs=1 count={} ",
            head.len() + "Expect:\n".len()
        )));
        assert!(script.contains("curl -q -sS --proto =https --proto-redir =https --max-redirs 0"));
        assert!(script.contains("-X POST -H @\"$d/h\" --data-binary @\"$d/b\""));
        assert!(script.contains("'https://jira.corp.example/rest/api/2/search?x=1'"));
        assert!(!script.contains(TOKEN));
    }

    /// Grep test: every placeholder is filled, and the only value spliced
    /// into the script is the URL, single-quoted.
    #[test]
    fn the_script_interpolates_nothing_unquoted() {
        let url = "https://jira.corp.example/x?q=a'b;$(reboot)";
        let s = CurlTransport::script(Method::Get, url, 42, false, 20, 1024);
        assert!(!s.contains("@@"), "a placeholder was left unfilled");
        assert!(s.contains(&crate::shell::quote(url)));
        assert!(!s.contains("--data-binary"), "no body, no data flag");
        // The raw URL appears only inside its quoting.
        let unquoted = s.replace(&crate::shell::quote(url), "");
        assert!(!unquoted.contains("$(reboot)"));
        for line in CURL_SCRIPT.lines().filter(|l| l.contains("@@")) {
            assert!(
                line.contains("curl -q") || line.starts_with("dd ") || line.starts_with("head -c"),
                "a placeholder outside the three lines that take one: {line}"
            );
        }
    }

    #[tokio::test]
    async fn the_fence_holds_before_anything_runs() {
        let f = FakeSsh::new();
        for url in [
            "http://jira.corp.example/rest/api/2/myself",
            "https://evil.example.com/x",
            "https://169.254.169.254/latest/meta-data",
            "https://jira.corp.example.evil.com/x",
        ] {
            let e = curl(&f).send(Request::get(url)).await.unwrap_err();
            assert!(matches!(e, TransportError::Refused(_)), "{url}: {e}");
        }
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x").header("X", "a\nInjected: 1"))
            .await
            .unwrap_err();
        assert!(matches!(e, TransportError::Refused(_)));
        assert!(f.calls().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_host_or_tracker_is_unreachable() {
        let f = FakeSsh::new();
        f.unreachable("vpnbox");
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(e.is_unreachable(), "{e}");

        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok("__fleet_status__=000\n__fleet_curl_exit__=6\ncurl: (6) Could not resolve host: jira.corp.example\n"),
        );
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(
            e.is_unreachable() && e.to_string().contains("Could not resolve"),
            "{e}"
        );

        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok(
                "__fleet_status__=000\n__fleet_curl_exit__=28\ncurl: (28) Operation timed out\n",
            ),
        );
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert_eq!(e, TransportError::Timeout);

        let f = FakeSsh::new();
        f.on(Match::Any, Reply::ok("__fleet_no_curl__\n"));
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(e.is_unreachable() && e.to_string().contains("curl is not installed"));
    }

    /// `%{http_code}` is known as soon as the head arrived; only exit 0
    /// vouches for the body. A status with a non-zero exit is the failure
    /// the exit names, never a 2xx with a cut body.
    #[tokio::test]
    async fn a_non_zero_curl_exit_is_never_a_response_even_with_a_status() {
        // 63: `--max-filesize` refused the body before writing it.
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok(
                "__fleet_status__=200\n__fleet_curl_exit__=63\ncurl: (63) Maximum file size exceeded\n",
            ),
        );
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, TransportError::TooLarge(m) if m.contains("Maximum file size")),
            "{e}"
        );

        // 28: `--max-time` fired mid-body; half a document is on disk.
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok(
                "__fleet_status__=200\n__fleet_curl_exit__=28\ncurl: (28) Operation timed out after 20000 milliseconds with 120000 bytes received\n",
            ),
        );
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert_eq!(e, TransportError::Timeout);

        // 18: a partial transfer (the peer closed early).
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            Reply::ok(
                "__fleet_status__=200\n__fleet_curl_exit__=18\ncurl: (18) transfer closed with 3000 bytes remaining to read\n",
            ),
        );
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, TransportError::Protocol(m) if m.contains("exit 18") && m.contains("bytes remaining")),
            "{e}"
        );

        // An answer that stops before the exit line is truncated, not a 200.
        let f = FakeSsh::new();
        f.on(Match::Any, Reply::ok("__fleet_status__=200\n"));
        let e = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(matches!(e, TransportError::Protocol(_)), "{e}");
    }

    /// The script prints one byte past the cap; a body over it is
    /// TooLarge, one exactly at it is a response — never a 2xx cut
    /// mid-token that the provider reports as unreadable JSON.
    #[tokio::test]
    async fn a_body_over_the_cap_is_too_large_and_one_at_the_cap_is_not() {
        let head = "HTTP/2 200 \r\ncontent-type: application/json\r\n\r\n";
        let f = FakeSsh::new();
        f.on(Match::Any, answer("200", head, &"x".repeat(1025)));
        let mut t = curl(&f);
        t.max_body = 1024;
        let e = t
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, TransportError::TooLarge(m) if m.contains("more than 0 MiB")),
            "{e}"
        );

        let f = FakeSsh::new();
        f.on(Match::Any, answer("200", head, &"x".repeat(1024)));
        let mut t = curl(&f);
        t.max_body = 1024;
        let r = t
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap();
        assert_eq!(r.body.len(), 1024);
        let script = CurlTransport::script(
            Method::Get,
            "https://jira.corp.example/x",
            1,
            false,
            1,
            1024,
        );
        assert!(
            script.contains("head -c $((1024 + 1)) \"$d/rb\""),
            "one byte past the cap: {script}"
        );
    }

    #[tokio::test]
    async fn an_http_error_is_a_response_for_the_provider_to_map() {
        let f = FakeSsh::new();
        f.on(
            Match::Any,
            answer(
                "429",
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\n\r\n",
                "",
            ),
        );
        let r = curl(&f)
            .send(Request::get("https://jira.corp.example/x"))
            .await
            .unwrap();
        assert_eq!((r.status, r.header("Retry-After")), (429, Some("30")));
    }
}

/// The real script, run locally against a fake `curl` that records its argv
/// and the header file it was handed.
#[cfg(test)]
mod curl_script_tests {
    use super::*;

    #[tokio::test]
    async fn the_script_splits_stdin_keeps_the_token_out_of_argv_and_cleans_up() {
        if std::process::Command::new("bash")
            .arg("-c")
            .arg("true")
            .status()
            .is_err()
        {
            eprintln!("skipping: no bash");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let tmp = dir.path().join("tmp");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        let log = dir.path().join("log");
        std::fs::create_dir_all(&log).unwrap();
        let fake = format!(
            r#"#!/bin/bash
if [ "$2" = "--version" ]; then echo "curl 8.5.0 (x86_64-pc-linux-gnu)"; exit 0; fi
printf '%s\n' "$@" > {log}/argv
hdr=""; body=""; dump=""; out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -H) hdr="${{2#@}}"; shift ;;
    --data-binary) body="${{2#@}}"; shift ;;
    -D) dump="$2"; shift ;;
    -o) out="$2"; shift ;;
  esac
  shift
done
cp "$hdr" {log}/headers
[ -n "$body" ] && cp "$body" {log}/body
printf 'HTTP/1.1 201 Created\r\nX-Seen: yes\r\n\r\n' > "$dump"
printf '{{"id":7}}' > "$out"
printf '201'
"#,
            log = log.display()
        );
        let curl = bin.join("curl");
        std::fs::write(&curl, fake).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();

        let token = "tok-stdin-only-0123456789abcdef";
        let req = Request::post_json(
            "https://jira.corp.example/rest/api/2/issue",
            &serde_json::json!({"a": 1}),
        )
        .header("Authorization", format!("Bearer {token}"));
        let (stdin, hlen) = CurlTransport::stdin(&req).unwrap();
        let script = CurlTransport::script(req.method, &req.url, hlen, true, 5, 4096);
        let mut child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("TMPDIR", &tmp)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use tokio::io::AsyncWriteExt;
            let mut pipe = child.stdin.take().unwrap();
            pipe.write_all(&stdin).await.unwrap();
        }
        let out = child.wait_with_output().await.unwrap();
        assert!(
            out.stdout.starts_with(b"__fleet_begin__\n"),
            "the script's first line is the start marker: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let resp = parse_curl("local", after_begin(&out.stdout), 4096)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        assert_eq!(resp.status, 201);
        assert_eq!(resp.header("x-seen"), Some("yes"));
        assert_eq!(resp.text(), "{\"id\":7}");
        let argv = std::fs::read_to_string(log.join("argv")).unwrap();
        assert!(!argv.contains(token), "argv: {argv}");
        assert!(
            argv.starts_with("-q\n"),
            "-q first, so ~/.curlrc cannot add --trace"
        );
        let headers = std::fs::read_to_string(log.join("headers")).unwrap();
        assert!(headers.contains(&format!("Authorization: Bearer {token}")));
        assert_eq!(
            std::fs::read_to_string(log.join("body")).unwrap(),
            "{\"a\":1}"
        );
        assert_eq!(
            std::fs::read_dir(&tmp).unwrap().count(),
            0,
            "the private temp directory is removed on exit"
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains(token));
    }
}
