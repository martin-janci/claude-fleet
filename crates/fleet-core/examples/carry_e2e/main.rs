//! Cross-host check of the move carry engine; the harness itself is in
//! `unix.rs`, whose module doc says what it runs and how.
//!
//!   cargo run -p fleet-core --example carry_e2e -- <ssh-host>
//!
//! Unix only: the source side runs the generated scripts under a local
//! `bash -lc` and sets POSIX modes on its fixtures.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
fn main() {
    unix::main()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("carry_e2e runs the source side under a local bash: Unix only");
    std::process::exit(2);
}
