//! Client logs on request: the desktop's and the phone's own log, sent to
//! the hub only when the self debug agent asks for it (`fleet-diagnostics`
//! `client_log`). Spec: thread "Self debug agent", 2026-10-17.
//!
//! One route, `POST /log` (`mcp::client_log_route`), behind `authorize`:
//! - a POLL (`{}`) answers `200 {"want": {"id", "minutes"}}` while a request
//!   is open that this caller has not answered, else `204`;
//! - an UPLOAD (`{"id", "lines", "app"?}`) keeps the lines for that caller.
//!
//! A client polls every [`POLL_EVERY`]; an agent's `client_log` call opens a
//! request when no fresh upload answers it, and the next call reads what
//! came in. Everything lives in this process's memory: nothing is written
//! to the store, and the uploads expire after [`UPLOAD_TTL_SECS`]. A client
//! redacts before it sends ([`crate::logging::redact`]); the diagnostics
//! server redacts again with every secret the store knows.

use crate::http_client::HubTransport;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How often a client asks the hub whether its log is wanted.
pub const POLL_EVERY: Duration = Duration::from_secs(30);
/// A request stays open this long.
const WANT_TTL_SECS: i64 = 600;
/// An upload answers `client_log` for this long, then is dropped.
const UPLOAD_TTL_SECS: i64 = 3600;
/// Most lines one upload keeps, and most characters per line.
pub const UPLOAD_MAX_LINES: usize = 2000;
const LINE_MAX_CHARS: usize = 2000;
/// Most clients whose uploads are kept at once.
const MAX_CLIENTS: usize = 16;
/// The widest window a request asks for: a day of a client's log.
pub const MAX_MINUTES: i64 = 1440;

#[derive(Debug, Clone)]
struct Want {
    id: String,
    minutes: i64,
    /// Only callers whose label contains this answer it.
    client: Option<String>,
    asked_at: i64,
    answered: HashSet<String>,
}

/// One client's log, as it sent it.
#[derive(Debug, Clone)]
pub struct Upload {
    pub at: i64,
    pub app: Option<String>,
    pub minutes: i64,
    pub lines: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ClientLogs {
    want: Option<Want>,
    uploads: HashMap<String, Upload>,
}

/// The hub process's client logs.
pub fn hub_logs() -> &'static Mutex<ClientLogs> {
    static LOGS: OnceLock<Mutex<ClientLogs>> = OnceLock::new();
    LOGS.get_or_init(|| Mutex::new(ClientLogs::default()))
}

/// What a client sent to `POST /log`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogPost {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub lines: Option<Vec<String>>,
    #[serde(default)]
    pub app: Option<String>,
}

impl ClientLogs {
    fn expire(&mut self, now: i64) {
        if self
            .want
            .as_ref()
            .is_some_and(|w| now - w.asked_at > WANT_TTL_SECS)
        {
            self.want = None;
        }
        self.uploads.retain(|_, u| now - u.at <= UPLOAD_TTL_SECS);
    }

    /// `POST /log` from `origin`: the request it should answer, if any.
    /// An upload for anything but the open request is dropped.
    pub fn post(&mut self, origin: &str, body: LogPost, now: i64) -> Option<Value> {
        self.expire(now);
        let Some(lines) = body.lines else {
            let w = self.want.as_ref()?;
            if w.answered.contains(origin)
                || w.client.as_deref().is_some_and(|c| !matches(origin, c))
            {
                return None;
            }
            return Some(json!({ "want": { "id": w.id, "minutes": w.minutes } }));
        };
        let w = self.want.as_mut()?;
        if body.id.as_deref() != Some(w.id.as_str()) {
            return None;
        }
        w.answered.insert(origin.to_string());
        let minutes = w.minutes;
        let keep = lines.len().saturating_sub(UPLOAD_MAX_LINES);
        let lines = lines
            .into_iter()
            .skip(keep)
            .map(|l| l.chars().take(LINE_MAX_CHARS).collect())
            .collect();
        if !self.uploads.contains_key(origin) && self.uploads.len() >= MAX_CLIENTS {
            if let Some(oldest) = self
                .uploads
                .iter()
                .min_by_key(|(_, u)| u.at)
                .map(|(k, _)| k.clone())
            {
                self.uploads.remove(&oldest);
            }
        }
        self.uploads.insert(
            origin.to_string(),
            Upload {
                at: now,
                app: body.app.map(|a| a.chars().take(80).collect()),
                minutes,
                lines,
            },
        );
        None
    }

    /// The uploads that answer a call for `client` over `minutes`: sent in
    /// the last [`WANT_TTL_SECS`] and covering at least that window.
    pub fn fresh(&mut self, client: Option<&str>, minutes: i64, now: i64) -> Vec<(String, Upload)> {
        self.expire(now);
        let mut got: Vec<(String, Upload)> = self
            .uploads
            .iter()
            .filter(|(o, u)| {
                now - u.at <= WANT_TTL_SECS
                    && u.minutes >= minutes
                    && client.is_none_or(|c| matches(o, c))
            })
            .map(|(o, u)| (o.clone(), u.clone()))
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        got
    }

    /// Open a request for `client`'s log over `minutes`, unless one that
    /// covers it is already open. Answers its id.
    pub fn ask(&mut self, client: Option<&str>, minutes: i64, now: i64) -> String {
        self.expire(now);
        if let Some(w) = &self.want {
            if w.minutes >= minutes && (w.client.is_none() || w.client.as_deref() == client) {
                return w.id.clone();
            }
        }
        let id = crate::mcp::generate_token()[..12].to_string();
        self.want = Some(Want {
            id: id.clone(),
            minutes,
            client: client.map(str::to_string),
            asked_at: now,
            answered: HashSet::new(),
        });
        id
    }

    /// Who sent a log this hour, for an answer that has none to show.
    pub fn seen(&mut self, now: i64) -> Vec<Value> {
        self.expire(now);
        let mut v: Vec<Value> = self
            .uploads
            .iter()
            .map(|(o, u)| json!({ "client": o, "at": u.at, "minutes": u.minutes }))
            .collect();
        v.sort_by_key(|x| x["client"].as_str().map(str::to_string));
        v
    }
}

fn matches(origin: &str, client: &str) -> bool {
    origin.to_lowercase().contains(&client.to_lowercase())
}

// ── the client side ──

/// The lines of this process's own log over the last `minutes`, redacted,
/// oldest first: what a desktop uploads. Empty when it logs to no file.
pub fn own_lines(dir: Option<&Path>, minutes: i64, now: i64) -> Vec<String> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    let args = json!({ "minutes": minutes.clamp(1, MAX_MINUTES), "limit": 1000 });
    crate::service::fleet_agents::diagnostics::hub_log(dir, &args, now)
        .ok()
        .and_then(|v| {
            v["lines"].as_array().map(|ls| {
                ls.iter()
                    .filter_map(Value::as_str)
                    .map(|l| crate::logging::redact(l).into_owned())
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// What one poll came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Nothing wanted.
    Idle,
    /// Asked for, and sent this many lines.
    Sent(usize),
    /// The hub has no `/log` (older) or refuses this client: stop.
    Stop,
    /// No answer: try again on the next tick.
    Retry,
}

/// The desktop's side of `/log`.
pub struct LogPoller {
    pub url: String,
    pub token: String,
    pub app: String,
    pub transport: Arc<dyn HubTransport>,
    /// Where this process logs (`logging::active_log_dir`).
    pub dir: Option<std::path::PathBuf>,
}

impl LogPoller {
    pub async fn poll_once(&self, now: i64) -> Poll {
        let asked = match self
            .transport
            .post_json(&self.url, &self.token, "{}".to_string())
            .await
        {
            Ok(r) if r.status == 204 => return Poll::Idle,
            Ok(r) if r.status == 200 => r.body,
            Ok(r) if matches!(r.status, 401 | 403 | 404) => return Poll::Stop,
            Ok(_) | Err(_) => return Poll::Retry,
        };
        let want: Value = serde_json::from_str(&asked).unwrap_or(Value::Null);
        let (Some(id), Some(minutes)) = (
            want["want"]["id"].as_str(),
            want["want"]["minutes"].as_i64(),
        ) else {
            return Poll::Retry;
        };
        let lines = own_lines(self.dir.as_deref(), minutes, now);
        let n = lines.len();
        let body = json!({ "id": id, "lines": lines, "app": self.app }).to_string();
        match self.transport.post_json(&self.url, &self.token, body).await {
            Ok(r) if r.status / 100 == 2 => Poll::Sent(n),
            _ => Poll::Retry,
        }
    }

    /// Every [`POLL_EVERY`] until `shutdown`, or until the hub says stop.
    pub async fn run(self, shutdown: tokio_util::sync::CancellationToken) {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(POLL_EVERY) => {}
            }
            match self.poll_once(crate::store::now_unix()).await {
                Poll::Stop => {
                    tracing::info!("[log] the hub takes no client log from this app; not polling");
                    return;
                }
                Poll::Sent(n) => tracing::info!(lines = n, "[log] sent this app's log to the hub"),
                Poll::Idle | Poll::Retry => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
