//! `fleet-hub update …` — the hub's own updates, from the command line
//! (update-channel design §7, slice S3).
//!
//! `check` is Git mode: it reads the published release channel directly,
//! verifies it against the compiled-in release key, and decides for THIS
//! build under the hub's own `update.*` settings and pin. It needs no
//! running hub and installs nothing; installing is `fleet-updater`'s (S6).

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::update::{self as upd, GitCheck, HttpsFetch};
use fleet_core::store::{now_unix, Store};
use fleet_update::channel::CheckOutcome;
use fleet_update::wire::Installed;
use fleet_update::{Artifact, Component, Track, Version};
use serde::Serialize;
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum UpdateCmd {
    /// What this hub build should run, read straight from the published
    /// release channel and verified against the release key. Uses the
    /// hub's `update.*` settings and pin when its database exists; needs no
    /// running hub and changes nothing.
    ///
    /// Exits 0 with any answer (up to date, an update, a hold); 1 when there
    /// is none: no channel published, unverified, or unreachable. `--json`
    /// prints the whole decision.
    Check {
        /// The track to read instead of `update.track`: stable, beta or
        /// nightly.
        #[arg(long)]
        track: Option<String>,
        /// Print the decision as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
}

/// `--track`, with the flag named in the error.
fn parse_track(s: &str) -> Result<Track, String> {
    s.parse().map_err(|e| format!("--track: {e}"))
}

/// A closed-set value as it is spelled on the wire.
fn snake(v: &impl Serialize) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

pub async fn run(
    cmd: UpdateCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    match cmd {
        UpdateCmd::Check { track, json } => {
            let track = track.as_deref().map(parse_track).transpose()?;
            // The hub's settings when it has a database; a hub that was never
            // initialised checks under the defaults.
            let store = serve::existing_db(&config::resolve_data_dir(opts, env))
                .ok()
                .map(|p| {
                    Store::open_read_only(&p)
                        .map_err(|e| format!("open {} read-only: {e}", p.display()))
                })
                .transpose()?;
            let setup = GitCheck::from_store(store.as_ref(), Component::Hub, "hub:self", track)
                .map_err(|e| e.message)?;
            drop(store);
            let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
            let installed = Installed {
                version,
                commit: Some(crate::ready::COMMIT.to_string()),
                build_id: Some(crate::ready::BUILD_ID.to_string()),
                digest: None,
            };
            let req = upd::hub_self_request(installed);
            let fetch = HttpsFetch::new(Some(&setup.base_url));
            let outcome = upd::git_check(fetch, setup, upd::trusted_keys(), &req, now_unix())
                .await
                .map_err(|e| format!("{}: {}", e.code, e.message))?;
            if json {
                out::line(
                    &serde_json::to_string_pretty(&outcome.decision).map_err(|e| e.to_string())?,
                );
            } else {
                let p = &req.platform;
                let platform = format!("{}, {}", p.os_arch(), p.variant);
                for l in render(&platform, &outcome) {
                    out::line(&l);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Where the target comes from, in one line.
fn artifact_line(a: &Artifact, url: Option<&str>) -> String {
    match a {
        Artifact::Oci { image, digest, .. } => format!("image  {image}@{digest}"),
        _ => match url {
            Some(u) => format!("file   {u}"),
            None => "file   (no download URL)".into(),
        },
    }
}

/// The decision, for a person.
fn render(platform: &str, o: &CheckOutcome) -> Vec<String> {
    let d = &o.decision;
    let mut lines = vec![format!(
        "fleet-hub {} ({platform}) on {}: {}",
        d.installed,
        d.track.as_str(),
        snake(&d.status)
    )];
    lines.push(format!(
        "  why    {} — {}",
        snake(&d.reason.code),
        d.reason.text
    ));
    if let Some(t) = &d.target {
        let mandatory = if t.mandatory {
            match &t.deadline {
                Some(dl) => format!(", mandatory by {dl}"),
                None => ", mandatory".into(),
            }
        } else {
            String::new()
        };
        lines.push(format!("  target {}{mandatory}", t.version));
        lines.push(format!(
            "  {}",
            artifact_line(&t.artifact, t.url.as_deref())
        ));
        if let Some(v) = &o.verified {
            lines.push(format!(
                "  signed release manifest and channel #{} verified against the release key",
                v.channel_sequence
            ));
        }
    }
    lines.push(format!("  mode   {} (update.hub.mode)", snake(&d.mode)));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_parse() {
        assert_eq!(parse_track("beta"), Ok(Track::Beta));
        assert!(parse_track("edge").is_err());
    }
}
