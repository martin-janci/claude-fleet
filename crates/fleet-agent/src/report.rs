//! The agent's copy of the error-report layer: `fleet-agent` depends on
//! `fleet-proto` only, so the ~40 lines of `fleet_core::logging::ReportLayer`
//! live here too. The record, the ring and the caps are `fleet-proto`'s.

use fleet_proto::report::{Report, ReportRing};
use std::sync::LazyLock;

/// The process-wide queue of error-level events, fed by [`ReportLayer`] and
/// drained on each heartbeat by `conn::serve`. Bounded at `RING_CAP`; when
/// nothing drains it (an install with `report_errors: false`) it just wraps.
pub fn ring() -> &'static ReportRing {
    static RING: LazyLock<ReportRing> = LazyLock::new(ReportRing::new);
    &RING
}

/// Flatten one event's fields into a [`Report`]: `message` is the message,
/// `code` is the code, everything else is appended as ` key=value`.
#[derive(Default)]
struct Visitor {
    message: String,
    code: Option<String>,
    rest: String,
}

impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "code" => self.code = Some(format!("{value:?}").trim_matches('"').to_string()),
            name => {
                let _ = write!(self.rest, " {name}={value:?}");
            }
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        match field.name() {
            "message" => self.message = value.to_string(),
            "code" => self.code = Some(value.to_string()),
            name => {
                let _ = write!(self.rest, " {name}={value}");
            }
        }
    }
}

/// An `ERROR` event as a clamped report, or `None` for any other level.
pub fn report_from_event(event: &tracing::Event<'_>) -> Option<Report> {
    if *event.metadata().level() != tracing::Level::ERROR {
        return None;
    }
    let mut v = Visitor::default();
    event.record(&mut v);
    let mut r = Report::error(
        event.metadata().target(),
        &format!("{}{}", v.message, v.rest),
    );
    r.code = v.code;
    r.clamp();
    Some(r)
}

/// Pushes every `ERROR` event into [`ring`]. Never logs from inside: that
/// re-enters the subscriber.
pub struct ReportLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ReportLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(r) = report_from_event(event) {
            ring().push(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_event_lands_in_the_ring_with_its_code() {
        use tracing_subscriber::layer::SubscriberExt as _;
        // The ring is process-global and this crate's tests run in parallel,
        // so a length assertion would be racy; instead push a unique message
        // and find *this* report after draining everything (see
        // `fleet_core::logging`'s version of this test).
        let unique = format!(
            "refused-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let before = ring().len();
        let sub = tracing_subscriber::registry().with(ReportLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(target: "fleet_agent::conn", code = "E_DIAL", "dial failed: {}", unique);
            tracing::info!("not captured");
        });
        assert!(ring().len() > before);
        let b = ring().drain(usize::MAX);
        let r = b
            .reports
            .iter()
            .find(|r| r.message.starts_with(&format!("dial failed: {unique}")))
            .unwrap_or_else(|| panic!("no report with our unique message in {:?}", b.reports));
        assert_eq!(r.component, "fleet_agent::conn");
        assert_eq!(r.code.as_deref(), Some("E_DIAL"));
        assert!(r.message.starts_with(&format!("dial failed: {unique}")));
    }
}
