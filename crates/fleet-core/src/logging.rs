//! Application logging: an hourly-rotated log file under `<app_data>/logs/`
//! (plus stderr in dev builds), with bearer-token redaction applied before any
//! line reaches a writer.
//!
//! Stack: `tracing` + `tracing-subscriber` (`EnvFilter`, so `RUST_LOG` works
//! as usual) + `tracing-appender` (hourly rotation, newest `MAX_LOG_FILES`
//! kept). `log` records from dependencies (tauri, tao, …) are bridged in by
//! `tracing-log`, so either macro family ends up in the same file.
//! `ReportLayer` copies every `ERROR` event into `report_ring()` for the hub
//! error channel (spec 2026-09-21).
//!
//! Why hourly: `tracing-appender` 0.2 has no size-based cap, so with daily
//! rotation a `RUST_LOG=debug` run could grow one day's file without bound.
//! Hourly rotation bounds a single file to one hour of output, and keeping
//! [`MAX_LOG_FILES`] files still covers the last three days.
//!
//! **Log with `tracing::{error,warn,info,debug}!`** (the `log::` macros also
//! work through the bridge). The print family (`eprintln!`, `eprint!`,
//! `println!`, `print!`, `dbg!`) is not allowed in production code:
//! `no_eprintln_tests.rs` fails the build on any, with no allowlist, so every
//! line reaches the redacting layers below. When [`init`] itself fails,
//! [`init_stderr_fallback`] installs a stderr-only subscriber so startup
//! errors still go through `tracing`.
//!
//! Redaction: every formatted line goes through [`redact`], which masks
//! `Bearer <token>`, `?token=` / `&token=` query values and bare 64-hex
//! strings (the shape of every token `mcp::generate_token` mints). It is a
//! safety net, not a licence to log secrets: never pass a token to a log
//! macro on purpose.

use regex::Regex;
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tracing_subscriber::fmt::MakeWriter;

/// Log file names are `<prefix>.<YYYY-MM-DD-HH>.<suffix>`, e.g.
/// `claude-fleet.2026-09-11-14.log` (UTC hour). Builds before hourly rotation
/// wrote `claude-fleet.<YYYY-MM-DD>.log`; those are still recognised, ordered
/// before the same day's hourly files, and pruned by the appender as the
/// oldest files.
pub const LOG_FILE_PREFIX: &str = "claude-fleet";
pub const LOG_FILE_SUFFIX: &str = "log";
/// The appender's rotation period. See the module docs for why it is hourly.
const ROTATION: tracing_appender::rolling::Rotation = tracing_appender::rolling::Rotation::HOURLY;
/// Rotated files kept on disk (the newest, i.e. the current hour's, included):
/// three days of hourly files.
pub const MAX_LOG_FILES: usize = 72;
/// Filter used when `RUST_LOG` is unset or unparsable: `info` for this app's
/// crates, `warn` for every dependency.
pub const DEFAULT_FILTER: &str =
    "warn,claude_fleet_lib=info,claude_fleet=info,fleet_core=info,fleet_hub=info";
/// Set to `1` to also log to stderr in a release build (debug builds always do).
pub const STDERR_ENV: &str = "CLAUDE_FLEET_LOG_STDERR";

/// The directory log files live in, given the app data directory.
pub fn log_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Whether `name` is one of our log files (`claude-fleet.<date>.log`).
fn is_log_file_name(name: &str) -> bool {
    name.starts_with(&format!("{LOG_FILE_PREFIX}."))
        && name.ends_with(&format!(".{LOG_FILE_SUFFIX}"))
        && name.len() > LOG_FILE_PREFIX.len() + LOG_FILE_SUFFIX.len() + 2
}

/// Sort key for a log file name: its date stamp, with a legacy daily stamp
/// (`YYYY-MM-DD`) keyed as `YYYY-MM-DD-` so it orders strictly before every
/// hourly file of that day (`YYYY-MM-DD-HH`), hour `00` included: a prefix
/// sorts before its extensions, so the new hour-00 file never ties with it
/// and stays the current file. Hourly stamps then sort chronologically as
/// strings, so no mtime lookups are needed.
fn log_sort_key(name: &str) -> String {
    let stamp = name
        .strip_prefix(&format!("{LOG_FILE_PREFIX}."))
        .and_then(|s| s.strip_suffix(&format!(".{LOG_FILE_SUFFIX}")))
        .unwrap_or(name);
    if stamp.len() == "YYYY-MM-DD".len() {
        format!("{stamp}-")
    } else {
        stamp.to_string()
    }
}

/// Our log files in `dir`, oldest first (see [`log_sort_key`]).
pub fn log_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_log_file_name)
        })
        .collect();
    files.sort_by_cached_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(log_sort_key)
            .unwrap_or_default()
    });
    files
}

/// The file currently being written (the newest), if any exists yet.
pub fn current_log_file(dir: &Path) -> Option<PathBuf> {
    log_files(dir).pop()
}

/// Bytes read from the end of each file by [`tail_lines`]: plenty for a few
/// hundred lines, and bounded so a runaway log can't balloon a diagnostics
/// bundle.
const TAIL_READ_BYTES: u64 = 256 * 1024;

fn read_tail(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL_READ_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        // Drop the (probably partial) first line of a mid-file read.
        if let Some(nl) = text.find('\n') {
            text.drain(..=nl);
        }
    }
    Ok(text)
}

/// The last `n` lines across the log files in `dir`, oldest first. Walks
/// back into earlier files when the current one is short. Unreadable files
/// are skipped. Lines are returned as written (already redacted by the
/// writer); callers that expose them should still run [`redact`].
pub fn tail_lines(dir: &Path, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in log_files(dir).iter().rev() {
        if out.len() >= n {
            break;
        }
        let Ok(text) = read_tail(path) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let need = n - out.len();
        let take_from = lines.len().saturating_sub(need);
        // Prepend this (older) file's tail.
        let mut chunk: Vec<String> = lines[take_from..].iter().map(|l| l.to_string()).collect();
        chunk.append(&mut out);
        out = chunk;
    }
    out
}

/// `Bearer` plus a candidate value (group 3). Whether the value is masked is
/// decided by [`is_token_shaped`], so prose such as "Bearer authentication"
/// survives.
static BEARER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer)(\s+)([A-Za-z0-9\-._~+/]{8,}=*)").expect("bearer regex")
});
static QUERY_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)([?&](?:access_)?token=)[^&\s"'#]+"#).expect("query token regex")
});
/// Runs of 64 or more hex digits. The regex crate has no lookaround, so the
/// rule "exactly 64 hex digits, not part of a longer hex run" (what
/// `(?<![0-9A-Fa-f])[0-9A-Fa-f]{64}(?![0-9A-Fa-f])` would say) is finished
/// in [`redact`]: a leftmost-greedy match of this pattern is always a maximal
/// run, so a match of length exactly 64 has a non-hex character (or the
/// text edge) on both sides. Unlike a `\b` rule this masks a token glued to
/// letters or `_` (`tok_<64 hex>`), and still leaves longer hex runs (a
/// SHA-512 digest) alone.
static HEX_RUN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9A-Fa-f]{64,}").expect("hex run regex"));

/// Placeholder that replaces masked material.
pub const REDACTED: &str = "[REDACTED]";

/// Length from which a `Bearer` value without any digit is still treated as
/// a token. Real tokens (64-hex, JWTs, OAuth access tokens) contain digits
/// or are long; English words after "bearer" are neither.
const BEARER_NO_DIGIT_MIN_LEN: usize = 24;

/// Whether a value following `Bearer` looks like a credential rather than
/// the next word of a sentence: it contains a digit, or it is at least
/// [`BEARER_NO_DIGIT_MIN_LEN`] characters long (padding excluded).
fn is_token_shaped(value: &str) -> bool {
    value.bytes().any(|b| b.is_ascii_digit())
        || value.trim_end_matches('=').len() >= BEARER_NO_DIGIT_MIN_LEN
}

/// Replace each match of `re` for which `mask` returns `Some`, leaving the
/// others as they are. Borrowed when nothing was replaced.
fn mask_matches<'a>(
    re: &Regex,
    input: &'a str,
    mask: impl Fn(&regex::Captures<'_>) -> Option<String>,
) -> Cow<'a, str> {
    let mut out = String::new();
    let mut last = 0;
    let mut changed = false;
    for caps in re.captures_iter(input) {
        let Some(replacement) = mask(&caps) else {
            continue;
        };
        let m = caps.get(0).expect("group 0 always participates");
        out.push_str(&input[last..m.start()]);
        out.push_str(&replacement);
        last = m.end();
        changed = true;
    }
    if !changed {
        return Cow::Borrowed(input);
    }
    out.push_str(&input[last..]);
    Cow::Owned(out)
}

/// Mask anything that looks like a bearer token: a token-shaped value after
/// `Bearer` (see [`is_token_shaped`]; "the bearer of" and "Bearer
/// authentication" survive), `?token=` / `&token=` query values, and runs
/// of exactly 64 hex digits (see [`HEX_RUN_RE`]). Text with nothing to mask
/// is returned borrowed and unchanged.
pub fn redact(input: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(input);
    if let Cow::Owned(s) = mask_matches(&BEARER_RE, &out, |c| {
        is_token_shaped(&c[3]).then(|| format!("{}{}{REDACTED}", &c[1], &c[2]))
    }) {
        out = Cow::Owned(s);
    }
    if let Cow::Owned(s) = QUERY_TOKEN_RE.replace_all(&out, format!("${{1}}{REDACTED}")) {
        out = Cow::Owned(s);
    }
    if let Cow::Owned(s) = mask_matches(&HEX_RUN_RE, &out, |c| {
        (c[0].len() == 64).then(|| REDACTED.to_string())
    }) {
        out = Cow::Owned(s);
    }
    out
}

/// Shortest literal secret [`redact_secrets`] will mask. Anything shorter is
/// ignored: blanking every occurrence of a 3-letter string would shred
/// ordinary text, and no real token is that short.
pub const MIN_SECRET_LEN: usize = 8;

/// [`redact`], plus literal masking of the given known secrets (e.g. the
/// master and per-host tokens read from the store) wherever they appear —
/// belt and braces for token shapes the patterns don't recognise.
pub fn redact_secrets(input: &str, secrets: &[String]) -> String {
    let mut out = redact(input).into_owned();
    for s in secrets {
        if s.len() >= MIN_SECRET_LEN && out.contains(s.as_str()) {
            out = out.replace(s.as_str(), REDACTED);
        }
    }
    out
}

/// The process-wide queue of error-level events, fed by [`ReportLayer`] and
/// drained by whoever reports to a hub (the desktop's flusher, the hub's own
/// tick). Bounded at `RING_CAP`; in a process nothing drains it just wraps.
pub fn report_ring() -> &'static fleet_proto::report::ReportRing {
    static RING: LazyLock<fleet_proto::report::ReportRing> =
        LazyLock::new(fleet_proto::report::ReportRing::new);
    &RING
}

/// Flatten one event's fields into a [`Report`](fleet_proto::report::Report):
/// `message` is the message, `code` is the code, everything else is
/// appended as ` key=value`.
#[derive(Default)]
struct ReportVisitor {
    message: String,
    code: Option<String>,
    rest: String,
}

impl tracing::field::Visit for ReportVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "code" => self.code = Some(format!("{value:?}").trim_matches('"').to_string()),
            name => {
                use std::fmt::Write as _;
                let _ = write!(self.rest, " {name}={value:?}");
            }
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "code" => self.code = Some(value.to_string()),
            name => {
                use std::fmt::Write as _;
                let _ = write!(self.rest, " {name}={value}");
            }
        }
    }
}

/// An `ERROR` event as a clamped report, or `None` for any other level.
pub fn report_from_event(event: &tracing::Event<'_>) -> Option<fleet_proto::report::Report> {
    if *event.metadata().level() != tracing::Level::ERROR {
        return None;
    }
    let mut v = ReportVisitor::default();
    event.record(&mut v);
    let mut r = fleet_proto::report::Report::error(
        event.metadata().target(),
        &format!("{}{}", v.message, v.rest),
    );
    r.code = v.code;
    r.clamp();
    Some(r)
}

/// Dependency `ERROR` events that say nothing this app can act on, as
/// `(target, message prefix)` pairs.
///
/// `rmcp` logs at ERROR when an MCP client hangs up before it reads the
/// response it asked for: `fail to response message error=channel closed`. A
/// control-API client closing its pipe is ordinary. Those lines were the
/// *only* ERRORs in five days of desktop logs, so `grep ERROR` found nothing
/// but them, and [`ReportLayer`] shipped each one to the hub as an error
/// report — a dependency's shrug filed as this app's fault.
///
/// Dropped here rather than in [`DEFAULT_FILTER`], which can only silence a
/// target wholesale and would take rmcp's real errors with it. Keep the
/// message prefixes narrow for the same reason.
const DEPENDENCY_NOISE: &[(&str, &str)] = &[("rmcp", "fail to response message")];

/// Whether `target` is `t` or a module under it.
fn target_matches(target: &str, t: &str) -> bool {
    target
        .strip_prefix(t)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// Whether a `(target, message)` pair matches [`DEPENDENCY_NOISE`]. The
/// level is the caller's to check.
fn is_dependency_noise(target: &str, message: &str) -> bool {
    DEPENDENCY_NOISE
        .iter()
        .any(|(t, m)| target_matches(target, t) && message.starts_with(m))
}

/// [`is_dependency_noise`] for a live event: `ERROR` only, and the target is
/// checked before the message so no non-matching event pays for a visit.
pub fn event_is_dependency_noise(event: &tracing::Event<'_>) -> bool {
    if *event.metadata().level() != tracing::Level::ERROR {
        return false;
    }
    let target = event.metadata().target();
    if !DEPENDENCY_NOISE
        .iter()
        .any(|(t, _)| target_matches(target, t))
    {
        return false;
    }
    let mut v = ReportVisitor::default();
    event.record(&mut v);
    is_dependency_noise(target, &v.message)
}

/// Drops [`event_is_dependency_noise`] events for every layer in the stack:
/// `Layered::event_enabled` ANDs its layers, so one `false` here keeps the
/// line out of the file, out of stderr and out of [`report_ring`] alike.
pub struct NoiseFilter;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for NoiseFilter {
    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        !event_is_dependency_noise(event)
    }
}

/// Pushes every `ERROR` event into [`report_ring`]. Never logs: a layer that
/// logs re-enters the subscriber.
pub struct ReportLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ReportLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(r) = report_from_event(event) {
            report_ring().push(r);
        }
    }
}

/// An `io::Write` adapter that redacts each buffer before forwarding it.
/// `tracing-subscriber`'s fmt layer formats a whole event into one buffer and
/// writes it with a single `write_all`, so every call sees complete lines.
pub struct RedactingWriter<W: Write>(W);

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0.write_all(redact(&text).as_bytes())?;
        // Report the caller's length: the redacted text may differ in size,
        // but all of the caller's bytes were consumed.
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// `MakeWriter` wrapper producing [`RedactingWriter`]s.
pub struct RedactingMakeWriter<M>(pub M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingMakeWriter<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter(self.0.make_writer())
    }
}

/// The rotating file appender [`init`] installs: [`ROTATION`] rotation,
/// newest [`MAX_LOG_FILES`] kept.
fn build_appender(dir: &Path) -> Result<tracing_appender::rolling::RollingFileAppender, String> {
    tracing_appender::rolling::Builder::new()
        .rotation(ROTATION)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
        .map_err(|e| format!("open log file in {}: {e}", dir.display()))
}

/// The `EnvFilter` in effect: `RUST_LOG` when set and valid, else
/// [`DEFAULT_FILTER`]. Returns whether `RUST_LOG` was used.
fn env_filter() -> (tracing_subscriber::EnvFilter, bool) {
    match tracing_subscriber::EnvFilter::try_from_default_env() {
        Ok(f) => (f, true),
        Err(_) => (tracing_subscriber::EnvFilter::new(DEFAULT_FILTER), false),
    }
}

/// Install the global subscriber: the rotating file under
/// `log_dir_in(data_dir)` and, in debug builds or with
/// `CLAUDE_FLEET_LOG_STDERR=1`, stderr. Returns the log directory.
///
/// Call once, as early as possible. A second call (or a failure to create
/// the directory / appender) returns `Err` and leaves any existing
/// subscriber in place; the app keeps running without a file log.
pub fn init(data_dir: &Path) -> Result<PathBuf, String> {
    init_in(&log_dir_in(data_dir))
}

/// [`init`] with an explicit log directory (the `fleet-hub --log-dir` case).
pub fn init_in(log_dir: &Path) -> Result<PathBuf, String> {
    init_in_with(log_dir, false)
}

/// [`init_in`], plus `force_stderr`: always add the (redacting) stderr layer,
/// whatever the build profile and [`STDERR_ENV`]. `fleet-hub serve` passes
/// `true` so docker / journald see its log; the desktop never does.
pub fn init_in_with(log_dir: &Path, force_stderr: bool) -> Result<PathBuf, String> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let dir = log_dir.to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create log dir {}: {e}", dir.display()))?;
    let appender = build_appender(&dir)?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(RedactingMakeWriter(appender))
        .with_target(true);
    let want_stderr = want_stderr_layer(
        force_stderr,
        cfg!(debug_assertions),
        std::env::var(STDERR_ENV).ok().as_deref(),
    );
    let stderr_layer = want_stderr.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(RedactingMakeWriter(std::io::stderr))
            .with_target(true)
    });
    let (filter, from_env) = env_filter();

    tracing_subscriber::registry()
        .with(filter)
        .with(NoiseFilter)
        .with(file_layer)
        .with(stderr_layer)
        .with(ReportLayer)
        .try_init()
        .map_err(|e| format!("install log subscriber: {e}"))?;

    if !from_env && std::env::var_os("RUST_LOG").is_some() {
        tracing::warn!("RUST_LOG is set but invalid; using the default filter {DEFAULT_FILTER:?}");
    }
    Ok(dir)
}

/// Whether [`init_in_with`] adds the stderr layer: forced by the caller,
/// always in a debug build, else when [`STDERR_ENV`] is `1` / `true`.
fn want_stderr_layer(force: bool, debug_build: bool, env_value: Option<&str>) -> bool {
    force || debug_build || env_value.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Fallback when [`init`] failed: a stderr-only subscriber with the same
/// redaction and filter, so startup errors (why file logging is unavailable
/// above all) still go through `tracing` instead of being dropped. A no-op
/// when a subscriber is already installed.
pub fn init_stderr_fallback() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let (filter, _) = env_filter();
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(NoiseFilter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(RedactingMakeWriter(std::io::stderr))
                .with_target(true),
        )
        .with(ReportLayer)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_event_becomes_a_report_and_a_warn_does_not() {
        use tracing_subscriber::layer::SubscriberExt as _;
        // The ring is process-global and fleet-core's tests run in parallel
        // threads, so a length assertion would be racy; instead push a
        // unique message and find *this* report after draining everything.
        let unique = format!(
            "refused-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let before = report_ring().len();
        let sub = tracing_subscriber::registry().with(ReportLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(target: "fleet_core::ssh", code = "E_SSH", host = "box", "ssh failed: {}", unique);
            tracing::warn!(target: "fleet_core::ssh", "ignored");
        });
        assert!(report_ring().len() > before);
        let b = report_ring().drain(usize::MAX);
        let r = b
            .reports
            .iter()
            .find(|r| r.message.starts_with(&format!("ssh failed: {unique}")))
            .unwrap_or_else(|| panic!("no report with our unique message in {:?}", b.reports));
        assert_eq!(r.component, "fleet_core::ssh");
        assert_eq!(r.code.as_deref(), Some("E_SSH"));
        assert!(
            r.message.starts_with(&format!("ssh failed: {unique}")),
            "{}",
            r.message
        );
        assert!(r.message.contains("host=box"), "{}", r.message);
        assert_eq!(r.level, "error");
    }

    #[test]
    fn rmcp_hang_up_errors_are_noise_and_real_ones_are_not() {
        // The line that filled the ERROR budget, and its whole module tree.
        assert!(is_dependency_noise(
            "rmcp::service",
            "fail to response message error=channel closed"
        ));
        assert!(is_dependency_noise("rmcp", "fail to response message"));
        // A different rmcp error still reaches the log and the hub.
        assert!(!is_dependency_noise("rmcp::service", "response error id=9"));
        // Our own targets are never noise, whatever they say.
        assert!(!is_dependency_noise(
            "fleet_core::mcp",
            "fail to response message error=channel closed"
        ));
        // A target that merely starts with the same letters is not that target.
        assert!(!is_dependency_noise(
            "rmcpx::service",
            "fail to response message"
        ));
    }

    #[test]
    fn a_noise_error_never_reaches_the_layers_below_the_filter() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt as _;

        /// Records what `on_event` is actually handed. A local ring, so this
        /// test does not race fleet-core's other threads over the global one.
        struct Capture(Arc<Mutex<Vec<String>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let mut v = ReportVisitor::default();
                event.record(&mut v);
                self.0.lock().unwrap().push(v.message);
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sub = tracing_subscriber::registry()
            .with(NoiseFilter)
            .with(Capture(Arc::clone(&seen)));
        tracing::subscriber::with_default(sub, || {
            // Dropped: rmcp's client-hung-up shrug.
            tracing::error!(target: "rmcp::service", "fail to response message error=channel closed");
            // Kept: an rmcp error that is not on the list.
            tracing::error!(target: "rmcp::service", "response error id=9");
            // Kept: the same words at a level the filter does not consider.
            tracing::warn!(target: "rmcp::service", "fail to response message error=channel closed");
        });

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].starts_with("response error id=9"), "{seen:?}");
        assert!(seen[1].starts_with("fail to response message"), "{seen:?}");
    }

    #[test]
    fn stderr_layer_is_forced_or_debug_or_env_opt_in() {
        // The desktop path (`force = false`) keeps its old rule.
        assert!(!want_stderr_layer(false, false, None));
        assert!(!want_stderr_layer(false, false, Some("0")));
        assert!(want_stderr_layer(false, false, Some("1")));
        assert!(want_stderr_layer(false, false, Some("TRUE")));
        assert!(want_stderr_layer(false, true, None));
        // `fleet-hub serve` forces it in a release build with no env.
        assert!(want_stderr_layer(true, false, None));
        assert!(want_stderr_layer(true, false, Some("0")));
    }

    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn redacts_bearer_tokens() {
        assert_eq!(
            redact("Authorization: Bearer abc.DEF-123_~+/xyz=="),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(
            redact("header bearer\tsecret-value-42"),
            "header bearer\t[REDACTED]"
        );
        // JSON-embedded header value.
        assert_eq!(
            redact(r#"{"Authorization": "Bearer tok-12345"}"#),
            r#"{"Authorization": "Bearer [REDACTED]"}"#
        );
    }

    #[test]
    fn bearer_masks_only_token_shaped_values() {
        // Prose: the word after "bearer" is not a credential.
        for s in [
            "Bearer authentication failed",
            "server requires bearer authorization.",
            "uses Bearer tokens, not cookies",
            "the bearer of bad news",
        ] {
            let r = redact(s);
            assert!(matches!(r, Cow::Borrowed(_)), "{s:?} should be untouched");
        }
        // A digit anywhere makes it a token.
        assert_eq!(redact("Bearer abcdefgh1"), "Bearer [REDACTED]");
        assert_eq!(redact(&format!("Bearer {HEX}")), "Bearer [REDACTED]");
        // JWT shape, lowercase scheme.
        assert_eq!(
            redact("authorization: bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig_Abc-123"),
            "authorization: bearer [REDACTED]"
        );
        // No digit but 24+ characters (a letters-only token)...
        assert_eq!(
            redact("Bearer abcdefghijklmnopqrstuvwx"),
            "Bearer [REDACTED]"
        );
        // ...where `=` padding does not count toward the length.
        assert_eq!(
            redact("Bearer abcdefghijklmnopqrstuvw="),
            "Bearer abcdefghijklmnopqrstuvw="
        );
        // Prose and a token in one line: only the token goes.
        assert_eq!(
            redact("Bearer authentication with Bearer tok-98765"),
            "Bearer authentication with Bearer [REDACTED]"
        );
    }

    #[test]
    fn redacts_query_tokens() {
        assert_eq!(
            redact("POST http://127.0.0.1:4180/hook?token=s3cret&x=1"),
            "POST http://127.0.0.1:4180/hook?token=[REDACTED]&x=1"
        );
        assert_eq!(redact("/mcp?a=1&token=zzz"), "/mcp?a=1&token=[REDACTED]");
        assert_eq!(
            redact("/x?access_token=zzz done"),
            "/x?access_token=[REDACTED] done"
        );
    }

    #[test]
    fn redacts_bare_64_hex() {
        assert_eq!(redact(&format!("token {HEX} end")), "token [REDACTED] end");
        assert_eq!(redact(&HEX.to_uppercase()), "[REDACTED]");
        // 40-hex (a git sha1) and 65-hex runs are left alone.
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(redact(sha1), sha1);
        let long = format!("{HEX}0");
        assert_eq!(redact(&long), long);
    }

    #[test]
    fn hex64_uses_hex_boundaries_not_word_boundaries() {
        // Glued to non-hex word characters: a `\b` rule missed these.
        assert_eq!(redact(&format!("tok_{HEX}")), "tok_[REDACTED]");
        assert_eq!(redact(&format!("zz{HEX}zz")), "zz[REDACTED]zz");
        assert_eq!(redact(&format!("x-{HEX}.log")), "x-[REDACTED].log");
        // Part of a longer hex run on either side: not a 64-hex token.
        for s in [format!("a{HEX}"), format!("{HEX}f"), format!("{HEX}{HEX}")] {
            let r = redact(&s);
            assert!(matches!(r, Cow::Borrowed(_)), "{s:?} should be untouched");
        }
        // Two tokens split by a non-hex character are both masked.
        assert_eq!(redact(&format!("{HEX}:{HEX}")), "[REDACTED]:[REDACTED]");
        // 63 hex digits are too short.
        assert_eq!(redact(&HEX[..63]), &HEX[..63]);
    }

    #[test]
    fn ordinary_text_is_untouched_and_borrowed() {
        for s in [
            "[reconcile-tick] enabled every 20s",
            "ssh mefistos: command exceeded 30s wall clock",
            "token mode must be 'full' or 'readonly'",
            "the bearer of bad news",
            "",
        ] {
            let r = redact(s);
            assert!(matches!(r, Cow::Borrowed(_)), "{s:?} should not allocate");
            assert_eq!(r, s);
        }
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = redact(&format!("Bearer xxxxxxxx42 ?token=y {HEX}")).into_owned();
        assert!(!once.contains("xxxxxxxx42"), "{once}");
        assert_eq!(redact(&once), once);
    }

    #[test]
    fn redact_secrets_masks_known_literals_but_not_short_ones() {
        let secrets = vec!["host-token-XYZ".to_string(), "abc".to_string()];
        let out = redact_secrets("got host-token-XYZ from abc", &secrets);
        assert_eq!(out, "got [REDACTED] from abc");
    }

    #[test]
    fn redacting_writer_masks_before_the_inner_writer() {
        let mut inner = Vec::new();
        {
            let mut w = RedactingWriter(&mut inner);
            w.write_all(format!("INFO hook Bearer {HEX}\n").as_bytes())
                .unwrap();
        }
        assert_eq!(
            String::from_utf8(inner).unwrap(),
            "INFO hook Bearer [REDACTED]\n"
        );
    }

    #[test]
    fn log_dir_resolves_under_the_data_dir() {
        let data = Path::new("/tmp/claude-fleet-data");
        assert_eq!(
            log_dir_in(data),
            PathBuf::from("/tmp/claude-fleet-data/logs")
        );
    }

    #[test]
    fn log_files_filters_and_orders_by_date() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for name in [
            "claude-fleet.2026-09-10.log",
            "claude-fleet.2026-09-11.log",
            "claude-fleet.2026-09-09.log",
            "other.2026-09-11.log",
            "claude-fleet.log.bak",
            "state.db",
        ] {
            std::fs::write(d.join(name), "x\n").unwrap();
        }
        let names: Vec<String> = log_files(d)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "claude-fleet.2026-09-09.log",
                "claude-fleet.2026-09-10.log",
                "claude-fleet.2026-09-11.log"
            ]
        );
        assert_eq!(
            current_log_file(d).unwrap().file_name().unwrap(),
            "claude-fleet.2026-09-11.log"
        );
        assert!(current_log_file(&d.join("missing")).is_none());
    }

    #[test]
    fn log_files_orders_legacy_daily_before_same_day_hourly() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for name in [
            "claude-fleet.2026-09-11-14.log",
            "claude-fleet.2026-09-11.log",
            "claude-fleet.2026-09-11-03.log",
            "claude-fleet.2026-09-10.log",
            "claude-fleet.2026-09-12-00.log",
        ] {
            std::fs::write(d.join(name), "x\n").unwrap();
        }
        let names: Vec<String> = log_files(d)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "claude-fleet.2026-09-10.log",
                "claude-fleet.2026-09-11.log",
                "claude-fleet.2026-09-11-03.log",
                "claude-fleet.2026-09-11-14.log",
                "claude-fleet.2026-09-12-00.log",
            ]
        );
    }

    #[test]
    fn legacy_daily_file_sorts_strictly_before_that_days_hour_00() {
        let legacy = log_sort_key("claude-fleet.2026-09-11.log");
        let hour00 = log_sort_key("claude-fleet.2026-09-11-00.log");
        let prev_day = log_sort_key("claude-fleet.2026-09-10-23.log");
        assert!(legacy < hour00, "{legacy:?} must sort before {hour00:?}");
        assert!(
            prev_day < legacy,
            "{prev_day:?} must sort before {legacy:?}"
        );

        // Upgrade day at 00 UTC: the new hour-00 file is the current one, so
        // the diagnostics tail reads it last whatever the directory order.
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("claude-fleet.2026-09-11-00.log"), "new\n").unwrap();
        std::fs::write(d.join("claude-fleet.2026-09-11.log"), "old\n").unwrap();
        assert_eq!(
            current_log_file(d).unwrap().file_name().unwrap(),
            "claude-fleet.2026-09-11-00.log"
        );
        assert_eq!(tail_lines(d, 1), ["new"]);
        assert_eq!(tail_lines(d, 2), ["old", "new"]);
    }

    #[test]
    fn appender_writes_hourly_stamped_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = build_appender(tmp.path()).unwrap();
        a.write_all(b"hello\n").unwrap();
        a.flush().unwrap();
        let files = log_files(tmp.path());
        assert_eq!(files.len(), 1, "{files:?}");
        let name = files[0].file_name().unwrap().to_str().unwrap().to_string();
        let stamp = name
            .strip_prefix("claude-fleet.")
            .and_then(|s| s.strip_suffix(".log"))
            .unwrap();
        assert_eq!(stamp.len(), "YYYY-MM-DD-HH".len(), "{name}");
        assert_eq!(tail_lines(tmp.path(), 5), ["hello"]);
    }

    #[test]
    fn tail_lines_spans_files_oldest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("claude-fleet.2026-09-10.log"), "a1\na2\na3\n").unwrap();
        std::fs::write(d.join("claude-fleet.2026-09-11.log"), "b1\nb2\n").unwrap();
        assert_eq!(tail_lines(d, 3), ["a3", "b1", "b2"]);
        assert_eq!(tail_lines(d, 1), ["b2"]);
        assert_eq!(tail_lines(d, 50), ["a1", "a2", "a3", "b1", "b2"]);
        assert!(tail_lines(&d.join("missing"), 5).is_empty());
    }
}
