//! Build identity (update-channel design U11): the commit and CI build this
//! binary came from, so `fleet-hub healthcheck --ready` can prove the updater
//! started the build it asked for, not merely one with the same version.
//!
//! `FLEET_GIT_SHA` / `FLEET_BUILD_ID` win when set (CI, and the Docker build,
//! whose context has no `.git`); otherwise `git rev-parse HEAD` in a checkout;
//! otherwise `unknown` / `local`. Only paths that exist are watched: a missing
//! `rerun-if-changed` path would re-run this script, and relink the hub, on
//! every build.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FLEET_GIT_SHA");
    println!("cargo:rerun-if-env-changed=FLEET_BUILD_ID");

    let sha = std::env::var("FLEET_GIT_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(git_head)
        .unwrap_or_else(|| "unknown".into());
    let build_id = std::env::var("FLEET_BUILD_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "local".into());
    println!("cargo:rustc-env=FLEET_GIT_SHA={}", sha.trim());
    println!("cargo:rustc-env=FLEET_BUILD_ID={}", build_id.trim());
}

fn git_head() -> Option<String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let git_dir = Path::new(&manifest).join("../../.git");
    let head = git_dir.join("HEAD");
    if !head.is_file() {
        return None;
    }
    println!("cargo:rerun-if-changed={}", head.display());
    // HEAD names a ref; the ref file is what moves on a commit.
    if let Ok(text) = std::fs::read_to_string(&head) {
        if let Some(r) = text.trim().strip_prefix("ref: ") {
            let ref_file = git_dir.join(r);
            if ref_file.is_file() {
                println!("cargo:rerun-if-changed={}", ref_file.display());
            }
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&manifest)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?;
    let sha = sha.trim();
    (sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sha.to_string())
}
