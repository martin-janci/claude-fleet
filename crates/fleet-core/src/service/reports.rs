//! Ingest for error reports: clamp, redact, rate-limit, store, log.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

use crate::ipc_error::{codes, lock, IpcError};
use crate::logging;
use crate::service::settings;
use crate::store::Store;
use fleet_proto::report::{Report, ReportBatch, FRAME_BATCH_MAX, HTTP_BATCH_MAX, RATE_PER_MINUTE};
use std::collections::HashMap;
use std::sync::Mutex;

/// Fixed one-minute windows per origin. Pruned of stale windows on every
/// call, so it never holds more entries than there are live origins.
#[derive(Debug, Default)]
pub struct RateWindows {
    inner: Mutex<HashMap<String, (i64, u32)>>,
}

impl RateWindows {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `origin` may store `n` more reports at `now`. Counts them if so.
    ///
    /// Callers pass at least 1 even for an empty batch — see [`ingest`].
    pub fn admit(&self, origin: &str, n: u32, now: i64) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|_, (start, _)| now - *start < 60);
        let (start, count) = g.entry(origin.to_string()).or_insert((now, 0));
        if now - *start >= 60 {
            *start = now;
            *count = 0;
        }
        if count.saturating_add(n) > RATE_PER_MINUTE {
            return false;
        }
        *count += n;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingested {
    pub stored: usize,
    pub dropped_by_sender: u32,
}

fn validate(batch: &ReportBatch, batch_max: usize) -> Result<(), IpcError> {
    if batch.reports.len() > batch_max {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("a report batch carries at most {batch_max} reports"),
        ));
    }
    if let Some(bad) = batch
        .reports
        .iter()
        .find(|r| r.level != "error" && r.level != "warn")
    {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "report level must be error or warn, not {:?}",
                logging::redact(&bad.level)
            ),
        ));
    }
    Ok(())
}

/// Clamp and redact one report in place.
fn sanitise(r: &mut Report) {
    r.clamp();
    r.message = logging::redact(&r.message).into_owned();
    r.component = logging::redact(&r.component).into_owned();
    if let Some(c) = &r.code {
        r.code = Some(logging::redact(c).into_owned());
    }
    if let Some(ctx) = &r.context {
        if let Ok(text) = serde_json::to_string(ctx) {
            let red = logging::redact(&text);
            if red != text {
                // A redaction that lands inside a JSON string is still valid
                // JSON, so this normally re-parses. If it ever does not, the
                // context is gone — say so, rather than hand the reader a row
                // that looks like it never had one.
                r.context = serde_json::from_str(&red).ok();
                r.truncated |= r.context.is_none();
            }
        }
    }
}

/// Store a batch under `origin`: validate, clamp, redact, rate-limit, insert,
/// prune to `reports.max_rows`, and log one warn line per report.
pub fn ingest(
    store: &Mutex<Store>,
    windows: &RateWindows,
    origin: &str,
    mut batch: ReportBatch,
    batch_max: usize,
    now: i64,
) -> Result<Ingested, IpcError> {
    validate(&batch, batch_max)?;
    // An empty batch counts as one report. `{"reports":[],"dropped":1}` still
    // costs the hub a log line, so a sender that only ever reported drops
    // would otherwise flood the log at any rate it liked.
    if !windows.admit(origin, (batch.reports.len() as u32).max(1), now) {
        return Err(IpcError::new(
            codes::E_RATE_LIMITED,
            format!("{origin} may store {RATE_PER_MINUTE} reports per minute"),
        ));
    }
    for r in &mut batch.reports {
        sanitise(r);
    }
    let stored = {
        let s = lock(store)?;
        let n = s.insert_reports(origin, &batch.reports, now)?;
        let max_rows = settings::get_string(&s, settings::REPORTS_MAX_ROWS)
            .parse()
            .unwrap_or(5000);
        let _ = s.prune_reports_to(max_rows);
        n
    };
    for r in &batch.reports {
        tracing::warn!(
            target: "fleet_core::report",
            origin, level = %r.level, component = %r.component, code = ?r.code,
            "{}", r.message
        );
    }
    if batch.dropped > 0 {
        tracing::warn!(target: "fleet_core::report", origin, dropped = batch.dropped, "reports dropped by the sender");
    }
    Ok(Ingested {
        stored,
        dropped_by_sender: batch.dropped,
    })
}

/// The hub's own errors: drain the process ring into the table, exempt from
/// the rate limit. Returns how many were stored.
pub fn drain_own_ring(store: &Mutex<Store>, now: i64) -> usize {
    let mut batch = logging::report_ring().drain(HTTP_BATCH_MAX);
    if batch.reports.is_empty() {
        return 0;
    }
    for r in &mut batch.reports {
        sanitise(r);
    }
    let Ok(s) = lock(store) else { return 0 };
    let n = s.insert_reports("hub", &batch.reports, now).unwrap_or(0);
    let max_rows = settings::get_string(&s, settings::REPORTS_MAX_ROWS)
        .parse()
        .unwrap_or(5000);
    let _ = s.prune_reports_to(max_rows);
    n
}

/// Delete rows older than `reports.max_age_secs`; `0` means never.
pub fn sweep_by_age(store: &Mutex<Store>, now: i64) -> usize {
    let Ok(s) = lock(store) else { return 0 };
    let max_age = settings::get_secs(&s, settings::REPORTS_MAX_AGE_SECS);
    if max_age == 0 {
        return 0;
    }
    s.sweep_reports_older_than(now - max_age as i64)
        .unwrap_or(0)
}

// Keep the frame cap name in scope for the tests and for `ws.rs`.
pub const AGENT_BATCH_MAX: usize = FRAME_BATCH_MAX;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ReportFilter;
    use fleet_proto::report::{
        Report, FRAME_BATCH_MAX, HTTP_BATCH_MAX, MESSAGE_MAX, RATE_PER_MINUTE,
    };

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    fn batch(msgs: &[&str]) -> ReportBatch {
        ReportBatch {
            reports: msgs
                .iter()
                .map(|m| Report::error("fleet_core::ssh", m))
                .collect(),
            dropped: 0,
        }
    }

    #[test]
    fn stores_clamped_and_redacted_rows() {
        let s = store();
        let w = RateWindows::new();
        let mut b = batch(&["ssh failed: Authorization: Bearer abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"]);
        b.reports[0].message.push_str(&"x".repeat(MESSAGE_MAX));
        let got = ingest(&s, &w, "client:desk", b, HTTP_BATCH_MAX, 1000).unwrap();
        assert_eq!(got.stored, 1);
        let rows = s
            .lock()
            .unwrap()
            .list_reports(&ReportFilter::default())
            .unwrap();
        assert!(
            !rows[0].message.contains("abcdef0123456789abcdef"),
            "token redacted"
        );
        assert!(rows[0].truncated);
        assert_eq!(rows[0].received_at, 1000);
        assert_eq!(rows[0].origin, "client:desk");
    }

    #[test]
    fn refuses_a_bad_level_and_an_oversize_batch() {
        let s = store();
        let w = RateWindows::new();
        let mut b = batch(&["m"]);
        b.reports[0].level = "debug".into();
        let e = ingest(&s, &w, "hub", b, HTTP_BATCH_MAX, 1).unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
        let too_many = batch(&["m"; FRAME_BATCH_MAX + 1]);
        let e = ingest(&s, &w, "hub", too_many, FRAME_BATCH_MAX, 1).unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
    }

    #[test]
    fn the_row_cap_is_applied_on_insert() {
        let s = store();
        s.lock()
            .unwrap()
            .set_setting(settings::REPORTS_MAX_ROWS, "100")
            .unwrap();
        let w = RateWindows::new();
        // Three origins so the rate limit never bites.
        for (i, o) in ["a", "b", "c"].iter().enumerate() {
            let b = batch(&["m"; 50]);
            ingest(&s, &w, o, b, HTTP_BATCH_MAX, 10 + i as i64).unwrap();
        }
        let rows = s
            .lock()
            .unwrap()
            .list_reports(&ReportFilter {
                limit: 1000,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 100);
    }

    #[test]
    fn the_rate_limit_refuses_the_batch_that_crosses_the_minute() {
        let s = store();
        let w = RateWindows::new();
        ingest(&s, &w, "host:box", batch(&["m"; 50]), HTTP_BATCH_MAX, 0).unwrap();
        ingest(&s, &w, "host:box", batch(&["m"; 10]), HTTP_BATCH_MAX, 30).unwrap();
        let e = ingest(&s, &w, "host:box", batch(&["m"]), HTTP_BATCH_MAX, 31).unwrap_err();
        assert_eq!(e.code, codes::E_RATE_LIMITED);
        assert_eq!(
            s.lock()
                .unwrap()
                .list_reports(&ReportFilter {
                    limit: 1000,
                    ..Default::default()
                })
                .unwrap()
                .len(),
            60
        );
        // Another origin is unaffected; the next minute admits again.
        ingest(&s, &w, "host:other", batch(&["m"]), HTTP_BATCH_MAX, 31).unwrap();
        ingest(&s, &w, "host:box", batch(&["m"]), HTTP_BATCH_MAX, 61).unwrap();
    }

    /// A batch with no reports is not free: it costs a log line, so it is
    /// rate-limited like a one-report batch.
    #[test]
    fn an_empty_dropped_only_batch_is_rate_limited_too() {
        let s = store();
        let w = RateWindows::new();
        let only_dropped = || ReportBatch {
            reports: Vec::new(),
            dropped: 1,
        };
        for i in 0..RATE_PER_MINUTE {
            ingest(
                &s,
                &w,
                "host:box",
                only_dropped(),
                HTTP_BATCH_MAX,
                i as i64 % 60,
            )
            .unwrap();
        }
        let e = ingest(&s, &w, "host:box", only_dropped(), HTTP_BATCH_MAX, 59).unwrap_err();
        assert_eq!(e.code, codes::E_RATE_LIMITED);
        // The next minute admits again.
        ingest(&s, &w, "host:box", only_dropped(), HTTP_BATCH_MAX, 60).unwrap();
    }

    #[test]
    fn sweep_by_age_honours_zero_as_never() {
        let s = store();
        let w = RateWindows::new();
        ingest(&s, &w, "hub", batch(&["old"]), HTTP_BATCH_MAX, 0).unwrap();
        s.lock()
            .unwrap()
            .set_setting(settings::REPORTS_MAX_AGE_SECS, "0")
            .unwrap();
        assert_eq!(sweep_by_age(&s, 10_000_000), 0);
        s.lock()
            .unwrap()
            .set_setting(settings::REPORTS_MAX_AGE_SECS, "100")
            .unwrap();
        assert_eq!(sweep_by_age(&s, 10_000_000), 1);
    }

    #[test]
    fn drain_own_ring_stores_under_origin_hub() {
        let s = store();
        logging::report_ring().push(Report::error("fleet_core::tick", "boom"));
        assert!(drain_own_ring(&s, 5) >= 1);
        let rows = s
            .lock()
            .unwrap()
            .list_reports(&ReportFilter {
                origin: Some("hub".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(rows.iter().any(|r| r.message == "boom"));
    }
}
