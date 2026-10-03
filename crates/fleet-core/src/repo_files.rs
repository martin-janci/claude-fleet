//! Files from outside this crate that tests hold the code to: the
//! frontend's mirrors (`src/lib/*.ts`), the desktop's handler list and
//! commands (`src-tauri/src/…`), the user guides (`docs/*.md`).
//!
//! Read when the test runs, never with `include_str!`: a compiled-in copy
//! makes the file an input of fleet-core's test target, so a one-line edit to
//! any of them recompiled that whole target (~26 s) before a single test
//! could run (RUST-BUILD-PERFORMANCE-AUDIT.md, Appendix E). A file of this
//! crate may still be `include_str!`ed: editing it recompiles the crate
//! anyway.

/// The file at `rel`, a path from the repository root.
pub(crate) fn read(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}
