//! The embedding application's version.
//!
//! `env!("CARGO_PKG_VERSION")` inside fleet-core is fleet-core's own crate
//! version — a deliberate, unpublishable `0.1.0` (see
//! `crates/fleet-core/Cargo.toml`) — not the release version of the desktop app
//! or of `fleet-hub`. So there is deliberately **no fallback**: each embedder
//! calls [`set`] with its own `CARGO_PKG_VERSION` as the first statement of
//! `main`/`run`, and everything that reports a version (`fleet_health`, the
//! diagnostics bundle, the usage User-Agent, the agent/hub handshake) reads
//! [`get`].
//!
//! Reading before [`set`] is a startup-order bug in the embedder, not a runtime
//! condition, so [`get`] panics instead of inventing a version: the old
//! `unwrap_or(env!("CARGO_PKG_VERSION"))` answered `0.1.0`, which is wrong yet
//! looks perfectly plausible in a health payload, a diagnostics bundle and a
//! User-Agent string (F-C22). A caller that genuinely must tolerate "not set
//! yet" uses [`try_get`] and decides for itself. The one exception is
//! fleet-core's own unit tests, which have no embedder at all and record an
//! unmistakable sentinel instead — see `unset`.

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// Record the embedding application's version. The first call wins; later calls
/// are ignored, so nothing can change it mid-process.
///
/// Mandatory: every binary that links fleet-core must call this before anything
/// reads [`get`].
pub fn set(version: &'static str) {
    let _ = VERSION.set(version);
}

/// The embedder's version.
///
/// # Panics
///
/// If [`set`] was never called. Both binaries in this workspace call it as
/// their first statement (`src-tauri/src/lib.rs`, `crates/fleet-hub/src/main.rs`);
/// a test in another crate that reaches a reporting path must do the same
/// (`claude_fleet_lib::declare_app_version()`). fleet-core's own unit tests are
/// the single exception — see `unset` below.
pub fn get() -> &'static str {
    match try_get() {
        Some(v) => v,
        None => unset(),
    }
}

/// No embedder declared a version. In every real build that is a bug in the
/// binary's startup order, and the only honest answer is to stop: reporting
/// fleet-core's 0.1.0 is what this module exists to prevent.
#[cfg(not(test))]
fn unset() -> &'static str {
    panic!(
        "fleet_core::app_version::get() called before set(): the embedding binary must call \
         fleet_core::app_version::set(env!(\"CARGO_PKG_VERSION\")) before anything reports a \
         version (fleet-core's own 0.1.0 is not an app version)"
    )
}

/// fleet-core's own test binary has no embedder — there is no `main` to call
/// [`set`] — and dozens of its tests exercise the reporting paths. `cfg(test)`
/// is set only while compiling *this* crate's unit tests, so this branch exists
/// in no real build and in no other crate's tests either: fleet-hub, the
/// desktop app and both of their test binaries get the panic above. The value
/// is deliberately not a plausible release number.
#[cfg(test)]
fn unset() -> &'static str {
    set_for_tests();
    TEST_VERSION
}

/// The embedder's version, or `None` when [`set`] has not been called yet — for
/// a caller that must not panic. There is no default: `None` means "this process
/// never declared a version", never `0.1.0`.
pub fn try_get() -> Option<&'static str> {
    VERSION.get().copied()
}

/// The version fleet-core's own tests record: they have no embedder, and [`get`]
/// no longer invents one. Deliberately not a plausible release number — if this
/// string ever turns up outside a test run, that is the bug.
#[cfg(test)]
pub(crate) const TEST_VERSION: &str = "0.0.0-fleet-core-test";

/// Declare a version for fleet-core's own test binary. Idempotent and safe to
/// call from any test in any order: [`set`] is first-call-wins, so tests that
/// compare a payload's version against [`get`] agree with each other however
/// the harness interleaves them.
#[cfg(test)]
pub(crate) fn set_for_tests() {
    set(TEST_VERSION);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_reports_the_version_that_was_set() {
        set_for_tests();
        let v = get();
        assert!(!v.is_empty());
        assert_eq!(Some(v), try_get());
    }

    #[test]
    fn set_is_first_call_wins() {
        set_for_tests();
        let first = get();
        set("9.8.7");
        assert_eq!(get(), first, "a later set() must not change the version");
    }

    #[test]
    fn there_is_no_crate_version_fallback() {
        set_for_tests();
        assert_ne!(
            get(),
            env!("CARGO_PKG_VERSION"),
            "fleet-core's own crate version must never be reported as the app version"
        );
    }
}
