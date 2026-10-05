//! `fleet-hub session …` — the operator's side of the claim path
//! (multi-user M1, T12).
//!
//! **Written straight to `state.db`, not through a tool**, and for a reason
//! that is not convenience: `session_claim` is `guard::Access::HostToken`, so
//! the master token cannot reach it at all — the proof it rests on is a tmux
//! pane on a host, which the hub has none of. The operator's authority here
//! is shell access to the hub machine, which spec §4.5 already puts outside
//! what fleet promises, so the honest shape is the one `fleet-hub client
//! grant assets` and `client bind-person` already use: open the store and
//! write. The auth-epoch / row-version machinery means a running hub honours
//! the change from its next pass, so no running hub is needed.
//!
//! **`unclaimed` is also the only way a human sees those rows at all** on a
//! hub with more than one person: the API serves an out-of-scope caller a
//! per-host COUNT and nothing more (spec §4.3), and
//! `HostRow.unclaimed_sessions` is withheld the moment a second person
//! exists. Hence `--host`, which lists one host's unclaimed rows: without it
//! `claim <id>` names an id nothing on the machine will tell the operator.
//! Both halves are the same concession, and neither is reachable over the
//! control API.

use crate::config::HubOptions;
use crate::out;
use clap::Subcommand;
use fleet_core::store::VISIBILITY_UNCLAIMED;
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// How many sessions nobody owns, per host. With --host, list that host's.
    ///
    /// An `unclaimed` session is one a reconcile pass found rather than one
    /// fleet started: nobody can speak for it, and no API caller is told
    /// anything about it beyond these counts.
    Unclaimed {
        /// List this host's unclaimed sessions (id, tmux name, status)
        /// instead of printing the counts.
        #[arg(long)]
        host: Option<String>,
    },
    /// Give an unclaimed session to a person: it becomes theirs and private.
    ///
    /// By ROW ID, never by tmux name: a name is reused by the next session
    /// started on that host, and a claim written against a reused name would
    /// hand over the wrong session. Refused when the session already belongs
    /// to someone — a claim never transfers ownership.
    Claim {
        /// The session's fleet row id (see `session unclaimed --host`).
        id: i64,
        /// Whose it becomes, by person name (`fleet-hub client list`).
        #[arg(long)]
        person: String,
    },
}

pub fn run(
    cmd: SessionCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    match cmd {
        SessionCmd::Unclaimed { host: None } => {
            let counts = store.unclaimed_counts_by_host().map_err(|e| e.message)?;
            if counts.is_empty() {
                out::line("no unclaimed sessions on any host");
            } else {
                for (host, n) in &counts {
                    out::line(&format!("{host}: {n}"));
                }
                out::line(
                    "claim one with: fleet-hub session unclaimed --host <alias>, then \
                     fleet-hub session claim <id> --person <name>",
                );
            }
        }
        SessionCmd::Unclaimed { host: Some(alias) } => {
            let rows: Vec<_> = store
                .list_all_sessions()
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|r| {
                    r.host_alias == alias && r.visibility == VISIBILITY_UNCLAIMED
                        // Excluded exactly as the counts exclude them: a ghost
                        // is a row the next reconcile pass deletes, so an id
                        // printed here would be an id to chase.
                        && r.status != "ghost"
                })
                .collect();
            if rows.is_empty() {
                out::line(&format!("no unclaimed sessions on {alias}"));
            } else {
                for r in &rows {
                    out::line(&format!(
                        "{}\t{}\t{}{}",
                        r.id,
                        r.tmux_name,
                        r.status,
                        if r.lost_at.is_some() { "\tlost" } else { "" }
                    ));
                }
            }
        }
        SessionCmd::Claim { id, person } => {
            // The person must already exist. Unlike `client bind-person` —
            // where naming a colleague IS handing them a laptop — a claim
            // names an EXISTING person's session, and creating a `people` row
            // from a typo would attribute a session to a person nobody can
            // reach and that only `person disable` could undo.
            let to = fleet_core::service::sessions::person_named(&store, &person)
                .map_err(|e| e.message)?;
            let row = fleet_core::service::sessions::write_claim(&store, id, to, "master")
                .map_err(|e| e.message)?;
            if row.owner_person_id == Some(to) {
                out::line(&format!(
                    "session {id} ({}/{}) now belongs to {person} and is {}",
                    row.host_alias, row.tmux_name, row.visibility
                ));
            } else {
                // `claim_if_unclaimed` raises `E_FORBIDDEN` for a row owned by
                // somebody else, so this is unreachable today; printing the
                // state rather than claiming success is what keeps it that way
                // if the store's answer ever widens.
                return Err(format!(
                    "session {id} was not claimed for {person}: it is owned by {:?}",
                    row.owner_person_id
                ));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: SessionCmd,
    }

    #[test]
    fn the_subcommands_parse() {
        let t = T::try_parse_from(["t", "unclaimed"]).unwrap();
        assert!(matches!(t.cmd, SessionCmd::Unclaimed { host: None }));
        let t = T::try_parse_from(["t", "unclaimed", "--host", "mefistos"]).unwrap();
        assert!(matches!(t.cmd, SessionCmd::Unclaimed { host: Some(h) } if h == "mefistos"));
        let t = T::try_parse_from(["t", "claim", "17", "--person", "ada"]).unwrap();
        assert!(matches!(t.cmd, SessionCmd::Claim { id: 17, person } if person == "ada"));
    }

    /// The id is a number, so a tmux name cannot be passed by accident — the
    /// one thing a claim must never be addressed by (a name is reused by the
    /// next session on that host).
    #[test]
    fn a_claim_refuses_a_name_where_the_id_goes() {
        assert!(T::try_parse_from(["t", "claim", "dev-foo", "--person", "ada"]).is_err());
    }

    /// `--person` is required: a claim with nobody to attribute the session to
    /// is not a smaller claim, it is a different operation that does not exist.
    #[test]
    fn a_claim_needs_a_person() {
        assert!(T::try_parse_from(["t", "claim", "17"]).is_err());
    }
}
