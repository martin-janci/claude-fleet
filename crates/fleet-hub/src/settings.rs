//! `fleet-hub settings …` — the hub operator's review of settings proposals
//! (declarative pages P5, design D-P4). An agent proposes a change over the
//! control API (`set_setting { propose: true }`); the operator applies or
//! rejects it here, and reads a setting's history.
//!
//! Reads and writes `state.db` directly, as the person at the hub's
//! console: an applied proposal is recorded with actor `person` (the master
//! token over `/mcp` would record `agent (control API)`). Most keys are read
//! on use, so the running hub sees the new value at once. What
//! `service::settings::set` does in-process besides the write — the
//! `settings` frame for paired devices, `/events`' `context_full`
//! threshold, the update refresh's wake-up — happens in THIS process,
//! on a silent bus; the running hub catches up within [`WATCH_EVERY`]
//! through [`spawn_watch`], which follows the audit trail and the few
//! values the hub caches. A key marked "applies after a restart" still
//! needs one.

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::settings_review;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Subcommand, Debug)]
pub enum SettingsCmd {
    /// List the proposals waiting for review: key, now → proposed, who, why.
    Proposals {
        /// Print the answer as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// Apply proposals by id (from `settings proposals`).
    Apply {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// Reject proposals by id.
    Reject {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// A setting's writes, newest first: when, who, before → after.
    History {
        key: String,
        /// How many (1-100). [default: 20]
        #[arg(long)]
        limit: Option<i64>,
        #[arg(long)]
        json: bool,
    },
}

fn open(opts: &HubOptions, env: &HashMap<String, String>) -> Result<Store, String> {
    let path = serve::existing_db(&config::resolve_data_dir(opts, env))?;
    Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
        .map_err(|e| format!("open {}: {e}", path.display()))
}

fn shown(v: &str) -> &str {
    if v.is_empty() {
        "(empty)"
    } else {
        v
    }
}

pub fn run(
    cmd: SettingsCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let s = open(opts, env)?;
    match cmd {
        SettingsCmd::Proposals { json } => {
            let rows = settings_review::pending(&s).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
            } else if rows.is_empty() {
                out::line("No settings proposals are waiting for review.");
            } else {
                for p in &rows {
                    let who = p.row.source_detail.as_deref().unwrap_or(&p.row.source);
                    out::line(&format!(
                        "#{}  {}: {} → {}  ({who})",
                        p.row.id,
                        p.row.key,
                        shown(&p.current),
                        shown(&p.row.value)
                    ));
                    if let Some(why) = &p.row.why {
                        out::line(&format!("      why: {why}"));
                    }
                }
                out::line("Apply with `fleet-hub settings apply <id>…`, or `reject <id>…`.");
            }
            Ok(ExitCode::SUCCESS)
        }
        SettingsCmd::Apply { ids } => report(settings_review::decide(&s, &ids, &[])),
        SettingsCmd::Reject { ids } => report(settings_review::decide(&s, &[], &ids)),
        SettingsCmd::History { key, limit, json } => {
            let rows = settings_review::history(&s, &key, limit).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
            } else if rows.is_empty() {
                out::line(&format!(
                    "{key} has not been changed since history was kept."
                ));
            } else {
                for h in &rows {
                    let who = match &h.actor_detail {
                        Some(d) => format!("{} ({d})", h.actor),
                        None => h.actor.clone(),
                    };
                    let proposal = h
                        .proposal_id
                        .map(|id| format!(", proposal #{id}"))
                        .unwrap_or_default();
                    out::line(&format!(
                        "{}  {} → {}  by {who}{proposal}",
                        h.at,
                        h.before.as_deref().map_or("(default)", shown),
                        shown(&h.after)
                    ));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn report(
    r: Result<settings_review::Decided, fleet_core::ipc_error::IpcError>,
) -> Result<ExitCode, String> {
    let d = r.map_err(|e| e.message)?;
    for id in &d.applied {
        out::line(&format!("applied #{id}"));
    }
    for id in &d.rejected {
        out::line(&format!("rejected #{id}"));
    }
    for f in &d.failed {
        out::error(&format!("#{}: {}", f.id, f.error));
    }
    Ok(if d.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// How often the running hub looks for settings another process wrote.
pub(crate) const WATCH_EVERY: Duration = Duration::from_secs(5);

/// What the settings watch last saw: the values the running hub keeps
/// outside the store, and the newest audit row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Seen {
    /// `health.context_red_pct` as `/events` uses it.
    pub red_pct: f64,
    /// `update.track` and `update.check_interval_secs`, as stored.
    pub update: (Option<String>, Option<String>),
    /// The newest `setting_audit` id.
    pub audit_id: i64,
}

/// What the running hub has to do after a look.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Effects {
    /// A new `context_full` threshold for `/events`.
    pub red_pct: Option<f64>,
    /// The update channel's track or interval changed: wake its refresh.
    pub wake_update: bool,
    /// Registered keys written since the last look: a `settings` frame each.
    pub keys: Vec<String>,
}

/// Read what the watch compares. `audit_after: None` starts the watch (no
/// keys); `Some(id)` also returns the keys written after audit row `id`.
pub(crate) fn look(s: &Store, audit_after: Option<i64>) -> Result<(Seen, Vec<String>), String> {
    use fleet_core::service::settings::{UPDATE_CHECK_INTERVAL_SECS, UPDATE_TRACK};
    let (audit_id, keys) = s
        .setting_audit_since(audit_after)
        .map_err(|e| format!("read the settings audit: {e}"))?;
    let get = |k: &str| s.get_setting(k).map_err(|e| format!("read {k}: {e}"));
    Ok((
        Seen {
            red_pct: fleet_core::service::health::context_red_pct(s),
            update: (get(UPDATE_TRACK)?, get(UPDATE_CHECK_INTERVAL_SECS)?),
            audit_id,
        },
        keys,
    ))
}

/// What changed between two looks. Pure: the watch's whole decision.
///
/// A write this process made through `service::settings::set` already did
/// all of this; doing it again is harmless (the same threshold, one more
/// channel refresh, a second `settings` frame a device re-reads on).
pub(crate) fn effects(prev: &Seen, now: &Seen, keys: Vec<String>) -> Effects {
    Effects {
        red_pct: (prev.red_pct.to_bits() != now.red_pct.to_bits()).then_some(now.red_pct),
        wake_update: prev.update != now.update,
        keys,
    }
}

/// Follow settings written behind the running hub's back — `fleet-hub
/// settings apply`, which writes `state.db` from its own process — and do
/// what `service::settings::set` would have done in the hub: stamp the new
/// `context_full` threshold on `bus`, wake the update refresh, and emit a
/// `settings` frame per key. Stopped with the ticks.
pub(crate) fn spawn_watch(
    store: Arc<Mutex<Store>>,
    bus: Arc<fleet_core::events::BroadcastEventBus>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut seen = match store.lock() {
            Ok(s) => match look(&s, None) {
                Ok((seen, _)) => seen,
                Err(e) => {
                    tracing::warn!(error = %e, "settings watch not started");
                    return;
                }
            },
            Err(_) => return,
        };
        let mut warned = false;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(WATCH_EVERY) => {}
            }
            // The guard is dropped before the next `.await`.
            let Ok(s) = store.lock() else { break };
            let (now, keys) = match look(&s, Some(seen.audit_id)) {
                Ok(v) => v,
                Err(e) => {
                    if !warned {
                        tracing::warn!(error = %e, "settings watch: cannot read settings");
                        warned = true;
                    }
                    continue;
                }
            };
            let fx = effects(&seen, &now, keys);
            if let Some(pct) = fx.red_pct {
                bus.set_context_red_pct(pct);
            }
            if fx.wake_update {
                fleet_core::service::update::settings_changed(
                    fleet_core::service::settings::UPDATE_TRACK,
                );
            }
            for key in &fx.keys {
                s.emit_settings_changed(key);
            }
            drop(s);
            seen = now;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: SettingsCmd,
    }

    #[test]
    fn apply_and_reject_need_an_id() {
        assert!(T::try_parse_from(["t", "apply"]).is_err());
        let t = T::try_parse_from(["t", "reject", "3", "4"]).unwrap();
        assert!(matches!(t.cmd, SettingsCmd::Reject { ids } if ids == [3, 4]));
    }

    #[test]
    fn apply_writes_the_value_as_a_person_through_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        let path = dir.path().join("state.db");
        let id = {
            let s =
                Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            settings_review::propose(
                &s,
                "work.recent_days",
                "3",
                None,
                fleet_core::service::settings::Actor::Agent("control API"),
            )
            .unwrap()
            .id
        };
        assert_eq!(
            run(SettingsCmd::Apply { ids: vec![id] }, &opts, &env).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            run(SettingsCmd::Apply { ids: vec![id] }, &opts, &env).unwrap(),
            ExitCode::FAILURE,
            "already applied"
        );
        let s = open(&opts, &env).unwrap();
        let h = settings_review::history(&s, "work.recent_days", None).unwrap();
        assert_eq!(
            (h[0].actor.as_str(), h[0].proposal_id),
            ("person", Some(id))
        );
    }

    #[test]
    fn the_hub_notices_a_setting_another_process_wrote() {
        use fleet_core::service::settings::{self as reg, Actor};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        // The running hub's connection, and the CLI's, on the same file.
        let hub = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
        let (seen, keys) = look(&hub, None).unwrap();
        assert!(keys.is_empty());
        assert_eq!(
            effects(&seen, &look(&hub, Some(seen.audit_id)).unwrap().0, vec![]),
            Effects::default()
        );

        let cli = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
        reg::set_by(&cli, reg::HEALTH_CONTEXT_RED_PCT, "70", Actor::Person, None).unwrap();
        let (now, keys) = look(&hub, Some(seen.audit_id)).unwrap();
        let fx = effects(&seen, &now, keys);
        assert_eq!(fx.red_pct, Some(70.0));
        assert!(!fx.wake_update);
        assert_eq!(fx.keys, [reg::HEALTH_CONTEXT_RED_PCT]);

        reg::set_by(
            &cli,
            reg::UPDATE_CHECK_INTERVAL_SECS,
            "7200",
            Actor::Person,
            None,
        )
        .unwrap();
        let (later, keys) = look(&hub, Some(now.audit_id)).unwrap();
        let fx = effects(&now, &later, keys);
        assert_eq!(fx.red_pct, None, "unchanged since the last look");
        assert!(fx.wake_update);
        assert_eq!(fx.keys, [reg::UPDATE_CHECK_INTERVAL_SECS]);

        // Nothing new: nothing to do.
        let (same, keys) = look(&hub, Some(later.audit_id)).unwrap();
        assert_eq!(effects(&later, &same, keys), Effects::default());
    }
}
