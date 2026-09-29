//! What each caller costs this hub, counted per caller label.
//!
//! # Why
//!
//! A hub serves a desktop, some agents and — since paired-client access — a
//! phone, and it could not answer the simplest operational question about any
//! of them: how much are they asking for? There is no `/metrics`, no counter,
//! nothing but `tracing` lines in a rotating file. So "the phone app is
//! hammering the hub" and "the phone app is idle" look identical from the
//! outside, and every byte figure about a client is a measurement somebody
//! took by hand with `curl`, once.
//!
//! # What is counted, and what deliberately is not
//!
//! The dimension is [`Caller::label`] — the same key the rate limiter and the
//! stream cap already use, so a number here lines up with a refusal there.
//! Per caller: tool calls, the ones that came back an error, and the streams
//! it holds open.
//!
//! **No session id, no prompt, no project path, no host.** A metrics endpoint
//! is scraped on a timer and kept for months; a series labelled with a
//! session id is an activity log of the operator's work with a retention
//! policy nobody chose. The caller label is the coarsest thing that still
//! answers the question.
//!
//! Tool *names* are not a dimension either: 81 tools times N callers is a
//! cardinality that costs more to store than the answer is worth, and the
//! question this exists for is "which client is expensive", not "which tool".

use std::collections::BTreeMap;
use std::sync::Mutex;

/// One caller's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallerCounters {
    pub calls: u64,
    pub errors: u64,
}

/// Per-caller counters for the control API.
///
/// A `Mutex<BTreeMap>` rather than atomics per caller: the write is once per
/// tool call, which is already doing SQLite work under a global lock, and the
/// ordered map makes the exposition stable between scrapes instead of
/// reshuffling with the hash seed.
#[derive(Debug, Default)]
pub struct Metrics {
    by_caller: Mutex<BTreeMap<String, CallerCounters>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one tool call by `label`, and whether it failed.
    pub fn record_call(&self, label: &str, failed: bool) {
        let Ok(mut map) = self.by_caller.lock() else {
            // A poisoned counter must never take a tool call down with it.
            return;
        };
        let e = map.entry(label.to_string()).or_default();
        e.calls += 1;
        if failed {
            e.errors += 1;
        }
    }

    /// A snapshot, for the exposition.
    pub fn snapshot(&self) -> BTreeMap<String, CallerCounters> {
        self.by_caller.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// The Prometheus text exposition, given the streams each caller holds
    /// and the process gauges.
    ///
    /// Written by hand rather than through a client library: a few series and
    /// no histograms do not justify a dependency, and the format is four
    /// lines of rules.
    pub fn expose(&self, streams: &BTreeMap<String, usize>, hub: &HubGauges) -> String {
        let calls = self.snapshot();
        let mut out = String::new();
        out.push_str("# HELP fleet_tool_calls_total Control-API tool calls, by caller.\n");
        out.push_str("# TYPE fleet_tool_calls_total counter\n");
        for (label, c) in &calls {
            out.push_str(&format!(
                "fleet_tool_calls_total{{caller=\"{}\"}} {}\n",
                escape(label),
                c.calls
            ));
        }
        out.push_str(
            "# HELP fleet_tool_errors_total Tool calls that answered an error, by caller.\n",
        );
        out.push_str("# TYPE fleet_tool_errors_total counter\n");
        for (label, c) in &calls {
            out.push_str(&format!(
                "fleet_tool_errors_total{{caller=\"{}\"}} {}\n",
                escape(label),
                c.errors
            ));
        }
        out.push_str("# HELP fleet_event_streams_open Open /events streams, by caller.\n");
        out.push_str("# TYPE fleet_event_streams_open gauge\n");
        for (label, n) in streams {
            out.push_str(&format!(
                "fleet_event_streams_open{{caller=\"{}\"}} {}\n",
                escape(label),
                n
            ));
        }
        out.push_str("# HELP fleet_reconcile_duration_ms Wall time of the last reconcile pass.\n");
        out.push_str("# TYPE fleet_reconcile_duration_ms gauge\n");
        if let Some(ms) = hub.reconcile_duration_ms {
            out.push_str(&format!("fleet_reconcile_duration_ms {ms}\n"));
        }
        out.push_str(
            "# HELP fleet_reconcile_failures_total Reconcile passes that failed since start.\n",
        );
        out.push_str("# TYPE fleet_reconcile_failures_total counter\n");
        out.push_str(&format!(
            "fleet_reconcile_failures_total {}\n",
            hub.reconcile_failures_total
        ));
        out.push_str(
            "# HELP fleet_sessions Fleet sessions by claude_status (external rows excluded).\n",
        );
        out.push_str("# TYPE fleet_sessions gauge\n");
        for (status, n) in &hub.sessions_by_status {
            out.push_str(&format!(
                "fleet_sessions{{status=\"{}\"}} {n}\n",
                escape(status)
            ));
        }
        out.push_str("# HELP fleet_hosts_reachable Hosts whose last probe succeeded.\n");
        out.push_str("# TYPE fleet_hosts_reachable gauge\n");
        out.push_str(&format!("fleet_hosts_reachable {}\n", hub.hosts_reachable));
        out
    }
}

/// Process-level gauges beside the per-caller counters (perf-logs §5:
/// `/metrics` could not answer "is reconcile healthy"). Read from the
/// cached rows and the tick stats — no network, one short lock.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HubGauges {
    pub reconcile_duration_ms: Option<i64>,
    pub reconcile_failures_total: u64,
    pub sessions_by_status: BTreeMap<String, u32>,
    pub hosts_reachable: u32,
}

impl HubGauges {
    /// `pool` is the hub's read pool: the counts go through it when there is
    /// one, so a scrape never waits behind (or holds up) the writer. The two
    /// counts are SQL aggregates with `summarize`'s filters — no row decoded.
    pub fn read(
        store: &Mutex<crate::store::Store>,
        pool: Option<&crate::store::ReadPool>,
        stats: &crate::service::tick::TickStats,
    ) -> Self {
        let r = stats.reconcile();
        let (sessions_by_status, hosts_reachable) = match crate::store::read_via(pool, store).lock()
        {
            Ok(s) => (
                s.count_sessions_by_claude_status().unwrap_or_default(),
                s.count_reachable_hosts().unwrap_or_default(),
            ),
            Err(_) => (BTreeMap::new(), 0),
        };
        Self {
            reconcile_duration_ms: r.last_duration_ms,
            reconcile_failures_total: r.failures_total,
            sessions_by_status,
            hosts_reachable,
        }
    }
}

/// Escape a label value for the text exposition.
///
/// A caller label carries a paired client's name, which is the one part of it
/// this fleet did not author. A quote or a newline there would forge a second
/// series — the same class of injection `guard::scrub_line` exists for on the
/// audit trail, in the syntax this format uses.
fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// State for `GET /metrics`: the counters, the stream limiter the gauge
/// reads, and — for [`HubGauges`] — the store and the tick stats.
#[derive(Clone)]
pub struct MetricsState {
    pub metrics: std::sync::Arc<Metrics>,
    pub streams: std::sync::Arc<super::guard::LongPollLimiter>,
    pub store: std::sync::Arc<Mutex<crate::store::Store>>,
    /// The hub's read-only connections; `None` reads through `store`.
    pub read_pool: Option<std::sync::Arc<crate::store::ReadPool>>,
    pub stats: std::sync::Arc<crate::service::tick::TickStats>,
}

/// `GET /metrics` — the Prometheus text exposition.
///
/// Master only. A per-host token and a paired phone are both callers this
/// endpoint reports ON, and letting one read the others' figures would make
/// a read-only device a traffic monitor for the operator's own work. The
/// refusal is 403 with a sentence, not 404: the route exists and the token
/// is the problem, and saying otherwise sends the operator hunting a typo.
pub async fn handle_metrics(
    axum::extract::State(state): axum::extract::State<MetricsState>,
    axum::extract::Extension(caller): axum::extract::Extension<super::auth::Caller>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !caller.is_master() {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "metrics are master-token only\n",
        )
            .into_response();
    }
    let streams = state.streams.active_by_key();
    // A blocking SQLite read: off the async worker.
    let hub = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            HubGauges::read(&state.store, state.read_pool.as_deref(), &state.stats)
        })
        .await
        .unwrap_or_default()
    };
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.expose(&streams, &hub),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_calls_and_errors_per_caller() {
        let m = Metrics::new();
        m.record_call("client:phone", false);
        m.record_call("client:phone", true);
        m.record_call("master", false);

        let snap = m.snapshot();
        assert_eq!(
            snap["client:phone"],
            CallerCounters {
                calls: 2,
                errors: 1
            }
        );
        assert_eq!(
            snap["master"],
            CallerCounters {
                calls: 1,
                errors: 0
            }
        );
    }

    #[test]
    fn the_exposition_carries_every_series_and_the_stream_gauge() {
        let m = Metrics::new();
        m.record_call("client:phone", true);
        let streams = BTreeMap::from([("client:phone".to_string(), 2usize)]);

        let text = m.expose(&streams, &HubGauges::default());
        assert!(text.contains("fleet_tool_calls_total{caller=\"client:phone\"} 1"));
        assert!(text.contains("fleet_tool_errors_total{caller=\"client:phone\"} 1"));
        assert!(text.contains("fleet_event_streams_open{caller=\"client:phone\"} 2"));
        // Prometheus requires a TYPE line per metric family: three per-caller
        // series plus the four process gauges.
        assert_eq!(text.matches("# TYPE ").count(), 7);
    }

    /// A client's name is the one part of a caller label this fleet did not
    /// author. Unescaped, a quote there closes the label and everything after
    /// it is read as another series.
    #[test]
    fn a_hostile_client_name_cannot_forge_a_series() {
        let m = Metrics::new();
        m.record_call(
            "client:evil\" } 99\nfleet_tool_calls_total{caller=\"fake",
            false,
        );

        let text = m.expose(&BTreeMap::new(), &HubGauges::default());
        // Counted by LINE, not by substring: the injected text is still
        // present — inside the quoted, escaped label value, which is exactly
        // where it is harmless. What must not exist is a second series line.
        let series = text
            .lines()
            .filter(|l| l.starts_with("fleet_tool_calls_total{"))
            .count();
        assert_eq!(series, 1, "one series line, not two:\n{text}");
        assert!(
            !text.contains("\n} 99"),
            "the newline in the name must not have started a line:\n{text}"
        );
        assert!(text.contains("\\n"), "it is escaped, not stripped:\n{text}");
    }

    /// Two scrapes of an unchanged hub must be the same bytes, or every diff
    /// of a scrape is noise.
    #[test]
    fn the_exposition_is_stable_between_scrapes() {
        let m = Metrics::new();
        for name in ["client:phone", "master", "host:alpha", "client:tablet"] {
            m.record_call(name, false);
        }
        let none = HubGauges::default();
        assert_eq!(
            m.expose(&BTreeMap::new(), &none),
            m.expose(&BTreeMap::new(), &none)
        );
    }

    /// With a read pool, a scrape reads through it: holding the WRITER's
    /// lock (a reconcile pass mid-transaction) does not stall `/metrics`.
    #[test]
    fn hub_gauges_read_through_the_pool_without_the_writer() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store =
            crate::store::Store::open_with_bus(&path, Arc::new(crate::events::NoopEventBus))
                .unwrap();
        store.upsert_host("box").unwrap();
        store
            .upsert_session("s", "box", None, None, 0, 0, "running", None)
            .unwrap();
        let writer = Arc::new(Mutex::new(store));
        let pool = Arc::new(
            crate::store::ReadPool::open(&path, crate::store::READ_POOL_SIZE)
                .unwrap()
                .expect("a WAL file opens a pool"),
        );
        let stats = crate::service::tick::tick_stats();

        let _held = writer.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let writer = Arc::clone(&writer);
            let pool = Arc::clone(&pool);
            std::thread::spawn(move || {
                let _ = tx.send(HubGauges::read(&writer, Some(&*pool), &stats));
            });
        }
        let hub = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the pooled read must not wait on the writer's lock");
        assert_eq!(hub.hosts_reachable, 1);
        assert_eq!(hub.sessions_by_status.get("unknown"), Some(&1));
    }

    #[test]
    fn expose_adds_the_hub_gauges() {
        let m = Metrics::new();
        let mut by_status = BTreeMap::new();
        by_status.insert("working".to_string(), 12u32);
        by_status.insert("idle".to_string(), 3u32);
        let hub = HubGauges {
            reconcile_duration_ms: Some(812),
            reconcile_failures_total: 2,
            sessions_by_status: by_status,
            hosts_reachable: 5,
        };
        let out = m.expose(&BTreeMap::new(), &hub);
        for line in [
            "# TYPE fleet_reconcile_duration_ms gauge",
            "fleet_reconcile_duration_ms 812",
            "# TYPE fleet_reconcile_failures_total counter",
            "fleet_reconcile_failures_total 2",
            "# TYPE fleet_sessions gauge",
            "fleet_sessions{status=\"idle\"} 3",
            "fleet_sessions{status=\"working\"} 12",
            "# TYPE fleet_hosts_reachable gauge",
            "fleet_hosts_reachable 5",
        ] {
            assert!(out.contains(line), "missing {line:?} in:\n{out}");
        }
        let none = m.expose(&BTreeMap::new(), &HubGauges::default());
        assert!(
            !none
                .lines()
                .any(|l| l.starts_with("fleet_reconcile_duration_ms")),
            "no pass yet: HELP/TYPE only, no sample:\n{none}"
        );
        assert!(none.contains("# TYPE fleet_reconcile_duration_ms gauge"));
    }
}
