//! The embedding application's version.
//!
//! `env!("CARGO_PKG_VERSION")` inside fleet-core is fleet-core's own crate
//! version, not the release version of the desktop app or `fleet-hub`. Each
//! embedder calls [`set`] with its own `CARGO_PKG_VERSION` first thing, and
//! everything that reports a version (`fleet_health`, the diagnostics bundle,
//! the usage User-Agent) reads [`get`].

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// Record the embedding application's version. The first call wins.
pub fn set(version: &'static str) {
    let _ = VERSION.set(version);
}

/// The embedder's version, or fleet-core's own when [`set`] was never called.
pub fn get() -> &'static str {
    resolve(VERSION.get().copied())
}

fn resolve(embedder: Option<&'static str>) -> &'static str {
    embedder.unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_the_embedder_version_and_falls_back_to_the_crate() {
        assert_eq!(resolve(Some("9.8.7")), "9.8.7");
        assert_eq!(resolve(None), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn get_is_either_the_embedder_version_or_the_crate_version() {
        // No test calls `set`; whichever it is, `get` is non-empty.
        assert!(!get().is_empty());
    }
}
