//! The CLI's only stdout/stderr writer. Everything else logs through
//! `tracing`; the `no_eprintln` guard allowlists this file by name.

use std::io::Write;

pub fn line(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{s}");
}

pub fn error(s: &str) {
    let mut e = std::io::stderr().lock();
    let _ = writeln!(e, "fleet-hub: {s}");
}
