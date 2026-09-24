//! Tracker transports that run on a fleet host over SSH (work graph M6).
//!
//! * [`GhCliTransport`] (`via_cli:<alias>`, M6.1): `gh api` on the host. The
//!   request body goes to `gh` on **stdin** (`--input -`), and `gh` adds the
//!   host's own GitHub login: fleet never reads, stores or sends a GitHub
//!   token, and a request that carries an `Authorization` header is refused.
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
            "command -v gh >/dev/null 2>&1 || {{ printf '%s\\n' {NO_GH}; exit 0; }}\n\
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
        if out.stdout.starts_with(NO_GH.as_bytes()) {
            return Err(TransportError::Connect(format!(
                "gh is not installed on {}; install the GitHub CLI there and run `gh auth login`",
                self.host
            )));
        }
        if let Some(resp) = parse_included(&out.stdout) {
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
