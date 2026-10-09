//! The Docker Engine API, the handful of calls the updater makes (design
//! §8.1), behind a trait so the update loop is tested against a simulated
//! daemon (`engine::tests`).
//!
//! Inspect results stay `serde_json::Value`: the updater reads a few fields
//! and passes the rest back to `containers/create` untouched (`spec.rs`), so
//! a field this code does not know is carried over rather than dropped.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::http::{demux, exchange, Response};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerError(pub String);

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DockerError {}

impl From<String> for DockerError {
    fn from(s: String) -> Self {
        DockerError(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait Docker: Send + Sync {
    /// `GET /containers/{name}/json`; `None` for a 404.
    async fn inspect_container(&self, name: &str) -> Result<Option<Value>, DockerError>;
    /// `GET /images/{ref}/json`; `None` for a 404.
    async fn inspect_image(&self, reference: &str) -> Result<Option<Value>, DockerError>;
    /// `GET /containers/json?all=1&filters={"label":[…]}`.
    async fn list_containers(&self, labels: &[String]) -> Result<Vec<Value>, DockerError>;
    /// `POST /images/create?fromImage=<image>&tag=<digest>`, to the end of the stream.
    async fn pull(&self, image: &str, digest: &str) -> Result<(), DockerError>;
    /// `POST /containers/create?name=…`; the new container's id.
    async fn create(&self, name: &str, body: &Value) -> Result<String, DockerError>;
    async fn start(&self, id: &str) -> Result<(), DockerError>;
    /// SIGTERM, then SIGKILL after `timeout_secs`. Stopping a stopped container is fine.
    async fn stop(&self, id: &str, timeout_secs: u64) -> Result<(), DockerError>;
    async fn remove(&self, id: &str) -> Result<(), DockerError>;
    /// Run `cmd` in a running container and wait for it.
    async fn exec(&self, id: &str, cmd: &[String]) -> Result<ExecOutput, DockerError>;
    /// The last `tail` lines of stdout and stderr, interleaved.
    async fn logs(&self, id: &str, tail: u32) -> Result<String, DockerError>;
}

/// The Engine API over its unix socket.
pub struct EngineDocker {
    socket: PathBuf,
    timeout: Duration,
    pull_timeout: Duration,
}

const SMALL: u64 = 8 << 20;
/// Pull progress is a JSON line per layer update; generous.
const PULL_MAX: u64 = 256 << 20;

impl EngineDocker {
    /// `DOCKER_HOST` (`unix://` only), else `/var/run/docker.sock`.
    pub fn from_env() -> Result<EngineDocker, String> {
        let socket = match std::env::var("DOCKER_HOST") {
            Ok(h) if !h.trim().is_empty() => PathBuf::from(
                h.strip_prefix("unix://")
                    .ok_or_else(|| format!("DOCKER_HOST={h}: only unix:// sockets"))?,
            ),
            _ => PathBuf::from("/var/run/docker.sock"),
        };
        Ok(EngineDocker {
            socket,
            timeout: Duration::from_secs(120),
            pull_timeout: Duration::from_secs(1800),
        })
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        max: u64,
        timeout: Duration,
    ) -> Result<Response, DockerError> {
        let bytes = body.map(|b| serde_json::to_vec(b).expect("a Value serialises"));
        let fut = async {
            let stream = tokio::net::UnixStream::connect(&self.socket)
                .await
                .map_err(|e| format!("docker socket {}: {e}", self.socket.display()))?;
            let headers: &[(&str, &str)] = if bytes.is_some() {
                &[("Content-Type", "application/json")]
            } else {
                &[]
            };
            exchange(
                stream,
                method,
                "docker",
                path,
                headers,
                bytes.as_deref(),
                max,
            )
            .await
            .map_err(|e| format!("docker {method} {path}: {e}"))
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| DockerError(format!("docker {method} {path}: timed out")))?
            .map_err(DockerError)
    }

    async fn json(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, DockerError> {
        let r = self.call(method, path, body, SMALL, self.timeout).await?;
        expect_ok(method, path, &r)?;
        if r.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&r.body)
            .map_err(|e| DockerError(format!("docker {method} {path}: {e}")))
    }
}

fn expect_ok(method: &str, path: &str, r: &Response) -> Result<(), DockerError> {
    if r.ok() {
        return Ok(());
    }
    let msg = serde_json::from_slice::<Value>(&r.body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| r.text());
    Err(DockerError(format!(
        "docker {method} {path}: HTTP {}: {}",
        r.status,
        msg.trim()
    )))
}

/// Percent-encode a query value.
pub fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A path segment: a container name, id or image reference. Docker accepts
/// `repo@sha256:…` and `repo:tag` verbatim in the path; only `/` in the
/// repository needs no escaping either. Anything else is refused rather than
/// escaped, so a crafted name cannot reach another endpoint.
fn seg(s: &str) -> Result<&str, DockerError> {
    if !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@' | b'/')
        })
        && !s.contains("..")
    {
        Ok(s)
    } else {
        Err(DockerError(format!(
            "refusing the docker object name {s:?}"
        )))
    }
}

#[async_trait]
impl Docker for EngineDocker {
    async fn inspect_container(&self, name: &str) -> Result<Option<Value>, DockerError> {
        let path = format!("/containers/{}/json", seg(name)?);
        let r = self.call("GET", &path, None, SMALL, self.timeout).await?;
        if r.status == 404 {
            return Ok(None);
        }
        expect_ok("GET", &path, &r)?;
        serde_json::from_slice(&r.body)
            .map(Some)
            .map_err(|e| DockerError(format!("GET {path}: {e}")))
    }

    async fn inspect_image(&self, reference: &str) -> Result<Option<Value>, DockerError> {
        let path = format!("/images/{}/json", seg(reference)?);
        let r = self.call("GET", &path, None, SMALL, self.timeout).await?;
        if r.status == 404 {
            return Ok(None);
        }
        expect_ok("GET", &path, &r)?;
        serde_json::from_slice(&r.body)
            .map(Some)
            .map_err(|e| DockerError(format!("GET {path}: {e}")))
    }

    async fn list_containers(&self, labels: &[String]) -> Result<Vec<Value>, DockerError> {
        let filters = serde_json::json!({ "label": labels }).to_string();
        let v = self
            .json(
                "GET",
                &format!("/containers/json?all=1&filters={}", q(&filters)),
                None,
            )
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    async fn pull(&self, image: &str, digest: &str) -> Result<(), DockerError> {
        let path = format!(
            "/images/create?fromImage={}&tag={}",
            q(seg(image)?),
            q(seg(digest)?)
        );
        let r = self
            .call("POST", &path, None, PULL_MAX, self.pull_timeout)
            .await?;
        expect_ok("POST", &path, &r)?;
        // A 200 can still carry the failure as the stream's last line.
        for line in r.text().lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if let Some(e) = v.get("error").and_then(Value::as_str) {
                    return Err(DockerError(format!("pull {image}@{digest}: {e}")));
                }
            }
        }
        Ok(())
    }

    async fn create(&self, name: &str, body: &Value) -> Result<String, DockerError> {
        let v = self
            .json(
                "POST",
                &format!("/containers/create?name={}", q(seg(name)?)),
                Some(body),
            )
            .await?;
        v.get("Id")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| DockerError("containers/create answered without an Id".into()))
    }

    async fn start(&self, id: &str) -> Result<(), DockerError> {
        let path = format!("/containers/{}/start", seg(id)?);
        let r = self.call("POST", &path, None, SMALL, self.timeout).await?;
        // 304: already started.
        if r.status == 304 {
            return Ok(());
        }
        expect_ok("POST", &path, &r)
    }

    async fn stop(&self, id: &str, timeout_secs: u64) -> Result<(), DockerError> {
        let path = format!("/containers/{}/stop?t={timeout_secs}", seg(id)?);
        let r = self
            .call(
                "POST",
                &path,
                None,
                SMALL,
                self.timeout + Duration::from_secs(timeout_secs),
            )
            .await?;
        if r.status == 304 {
            return Ok(());
        }
        expect_ok("POST", &path, &r)
    }

    async fn remove(&self, id: &str) -> Result<(), DockerError> {
        let path = format!("/containers/{}?force=1", seg(id)?);
        let r = self
            .call("DELETE", &path, None, SMALL, self.timeout)
            .await?;
        if r.status == 404 {
            return Ok(());
        }
        expect_ok("DELETE", &path, &r)
    }

    async fn exec(&self, id: &str, cmd: &[String]) -> Result<ExecOutput, DockerError> {
        let created = self
            .json(
                "POST",
                &format!("/containers/{}/exec", seg(id)?),
                Some(&serde_json::json!({
                    "Cmd": cmd, "AttachStdout": true, "AttachStderr": true, "Tty": false
                })),
            )
            .await?;
        let exec_id = created
            .get("Id")
            .and_then(Value::as_str)
            .ok_or_else(|| DockerError("exec create answered without an Id".into()))?
            .to_string();
        let path = format!("/exec/{}/start", seg(&exec_id)?);
        let r = self
            .call(
                "POST",
                &path,
                Some(&serde_json::json!({ "Detach": false, "Tty": false })),
                SMALL,
                self.timeout,
            )
            .await?;
        expect_ok("POST", &path, &r)?;
        let (out, err) = demux(&r.body);
        let info = self
            .json("GET", &format!("/exec/{}/json", seg(&exec_id)?), None)
            .await?;
        Ok(ExecOutput {
            exit_code: info.get("ExitCode").and_then(Value::as_i64).unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    }

    async fn logs(&self, id: &str, tail: u32) -> Result<String, DockerError> {
        let path = format!(
            "/containers/{}/logs?stdout=1&stderr=1&timestamps=1&tail={tail}",
            seg(id)?
        );
        let r = self.call("GET", &path, None, SMALL, self.timeout).await?;
        expect_ok("GET", &path, &r)?;
        let (mut out, err) = demux(&r.body);
        out.extend_from_slice(&err);
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

// ── reading inspect output ──

pub fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(v, |v, k| v.get(*k))?.as_str()
}

/// The container's image id (`sha256:…`), the field compose and the gates
/// compare.
pub fn container_image_id(c: &Value) -> Option<&str> {
    str_at(c, &["Image"])
}

pub fn container_id(c: &Value) -> Option<&str> {
    str_at(c, &["Id"])
}

pub fn is_running(c: &Value) -> bool {
    c.pointer("/State/Running").and_then(Value::as_bool) == Some(true)
}

pub fn restart_count(c: &Value) -> i64 {
    c.get("RestartCount").and_then(Value::as_i64).unwrap_or(0)
}

/// `healthy`, `unhealthy`, `starting`; `None` for an image without a HEALTHCHECK.
pub fn health(c: &Value) -> Option<&str> {
    str_at(c, &["State", "Health", "Status"])
}

/// Whether the image's `RepoDigests` records `image@digest`.
pub fn has_repo_digest(img: &Value, image: &str, digest: &str) -> bool {
    let want = format!("{image}@{digest}");
    img.get("RepoDigests")
        .and_then(Value::as_array)
        .is_some_and(|ds| ds.iter().any(|d| d.as_str() == Some(want.as_str())))
}

/// The first `RepoDigests` entry's digest, for reporting what runs.
pub fn first_repo_digest(img: &Value) -> Option<String> {
    img.get("RepoDigests")?
        .as_array()?
        .first()?
        .as_str()?
        .split_once('@')
        .map(|(_, d)| d.to_string())
}

/// An environment variable of a container (`Config.Env`).
pub fn container_env<'a>(c: &'a Value, key: &str) -> Option<&'a str> {
    c.pointer("/Config/Env")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_values_are_escaped_and_names_checked() {
        assert_eq!(q("sha256:ab"), "sha256%3Aab");
        assert_eq!(
            q(r#"{"label":["a=b"]}"#),
            "%7B%22label%22%3A%5B%22a%3Db%22%5D%7D"
        );
        assert!(seg("ghcr.io/martin-janci/fleet-hub@sha256:abc").is_ok());
        assert!(seg("../../exec").is_err());
        assert!(seg("x?force=1").is_err());
        assert!(seg("").is_err());
    }

    #[test]
    fn reads_inspect_fields() {
        let c = json!({
            "Id": "abc", "Image": "sha256:img", "RestartCount": 2,
            "State": {"Running": true, "Health": {"Status": "healthy"}},
            "Config": {"Env": ["A=1", "FLEET_HUB_DATA_DIR=/data", "AB=2"]}
        });
        assert_eq!(container_image_id(&c), Some("sha256:img"));
        assert!(is_running(&c));
        assert_eq!(restart_count(&c), 2);
        assert_eq!(health(&c), Some("healthy"));
        assert_eq!(container_env(&c, "FLEET_HUB_DATA_DIR"), Some("/data"));
        assert_eq!(container_env(&c, "A"), Some("1"));
        let img = json!({"RepoDigests": ["ghcr.io/x/hub@sha256:d1"]});
        assert!(has_repo_digest(&img, "ghcr.io/x/hub", "sha256:d1"));
        assert!(!has_repo_digest(&img, "ghcr.io/x/hub", "sha256:d2"));
        assert_eq!(first_repo_digest(&img).as_deref(), Some("sha256:d1"));
    }
}
