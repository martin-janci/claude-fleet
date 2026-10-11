//! `fleet-hub person …` — the operator's side of the `people` table
//! (multi-user M1, migration 098).
//!
//! **Written straight to `state.db`, not through a tool**, for
//! `session.rs`'s reason and one more. There is no `people` tool at all:
//! M1 gives no caller admin authority over another person (spec §4.1), so
//! who this hub's people ARE is not a question the control API answers to
//! anybody. The operator's authority here is shell access to the hub
//! machine, which spec §4.5 already puts outside what fleet promises, and
//! it is the same authority `fleet-hub client bind-person` and `session
//! claim` already use: open the store and write. The auth-epoch and
//! grant-generation machinery means a running hub honours the change from
//! its next request, so no running hub is needed.
//!
//! **`list` is the only thing that maps an id to a human.** `client list`
//! prints a device's person as a bare `person 3` — the column is an id
//! because a device row carries an id — so without this subcommand an
//! operator reading it has nothing to look the number up in.
//!
//! **What is NOT here.** No `add`: a person is created by naming them
//! (`pair --person`, `client bind-person`), so adding one without a device
//! would only make a row nobody can reach. No `enable`: there is no
//! `Store::enable_person`, deliberately — see [`PersonCmd::Disable`]. And
//! nothing in this module reads a session: `list` prints PEOPLE, never
//! their sessions, a count of them, or anything derived from a session
//! row. An operator may take a departed person's reach away; privacy has
//! no admin override, and this file is one of the places that has to keep
//! being true.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{fmt_time, table};
use clap::Subcommand;
use fleet_core::store::{PersonRow, Store};
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum PersonCmd {
    /// Every person this hub knows, disabled ones included, with their ids.
    ///
    /// The id is the point: `client list` names a device's person as
    /// `person 3`, and this is what says who 3 is. Disabled people are in
    /// the list on purpose — a grant and a session still point at them, and
    /// a row the listing hid would be a row the operator cannot act on.
    List {
        /// Print the rows as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Rename a live person, and/or set the name shown in their place.
    ///
    /// Refused when another LIVE person already holds the new name; a name
    /// only a DISABLED person holds is free to take, so a departed
    /// colleague never blocks a new one (`idx_people_live_name`).
    ///
    /// A rename moves nothing but the text. Every grant is addressed to the
    /// person's ID, so renaming cannot hand somebody else's share to a
    /// different human, and the personal-owner flag does not travel with
    /// the name either.
    Rename {
        /// The live person to change, by name (`person list`).
        person: String,
        /// Their new name (1-64 characters, one line).
        #[arg(long)]
        to: Option<String>,
        /// Shown in place of their name; pass "" to clear it.
        #[arg(long)]
        display_name: Option<String>,
    },
    /// End a person's reach: they have left, or you are taking it away.
    ///
    /// Both halves in one transaction — every live DEVICE of theirs is
    /// revoked, and every live grant TO them (every share anybody made
    /// them) with it, so a token minted for them later cannot restore
    /// access you believed you had removed.
    ///
    /// What it does NOT do: their own sessions stay theirs and stay
    /// private, unreadable by anyone else — nothing re-attributes a
    /// departed person's work; the grants THEY made stand, because a grant
    /// belongs to the session's owner and this is no authority over
    /// somebody else's share; and this hub's own owner cannot be disabled
    /// at all, since every access gate keys on that row. **There is no
    /// re-enable in this release.**
    ///
    /// Refused without `--force`, which is where the count of devices and
    /// grants about to go is printed.
    Disable {
        /// The live person to disable, by name (`person list`).
        person: String,
        /// Do it. Without this the command prints what it would revoke and
        /// changes nothing.
        #[arg(long)]
        force: bool,
    },
    /// Link a single-sign-on account (Keycloak, or any OpenID Connect
    /// provider) to a live person, so signing in at `/auth/oidc/start`
    /// pairs a device as THEM.
    ///
    /// This is how an account becomes an EXISTING person — this hub's own
    /// owner above all: a sign-in never links itself to a person by name,
    /// since anybody who can set their username at the provider could then
    /// become a colleague. The subject is the account's `sub` claim; the
    /// refusal page a sign-in lands on prints this command with it filled
    /// in. An account is one person: linking it to a second is refused
    /// until `unlink-sso`.
    LinkSso {
        /// The live person, by name (`person list`).
        person: String,
        /// The account's `sub` claim (a UUID on Keycloak).
        #[arg(long)]
        subject: String,
        /// The provider's issuer URL. Defaults to FLEET_HUB_OIDC_ISSUER.
        #[arg(long)]
        issuer: Option<String>,
    },
    /// Remove a single-sign-on link. The person and their devices are
    /// untouched; a device already paired keeps working until `client
    /// revoke`.
    UnlinkSso {
        /// The account's `sub` claim.
        #[arg(long)]
        subject: String,
        /// The provider's issuer URL. Defaults to FLEET_HUB_OIDC_ISSUER.
        #[arg(long)]
        issuer: Option<String>,
    },
}

/// `--issuer`, else the hub's configured one.
fn sso_issuer(flag: Option<String>, env: &HashMap<String, String>) -> Result<String, String> {
    flag.or_else(|| env.get(fleet_core::mcp::oidc::ENV_ISSUER).cloned())
        .map(|i| fleet_core::store::normalize_issuer(&i))
        .filter(|i| !i.is_empty())
        .ok_or_else(|| {
            format!(
                "no issuer: pass --issuer <url> or set {}",
                fleet_core::mcp::oidc::ENV_ISSUER
            )
        })
}

/// The live person holding `name`.
///
/// Live only, because [`Store::get_person_by_name`] is live only and that is
/// the row `idx_people_live_name` guarantees is unique — a name a departed
/// colleague and a new one have both held resolves to the new one, which is
/// the right half of that ambiguity for both commands here. Hence the
/// pointer to `list` in the refusal: a disabled person is addressable
/// nowhere, and the operator should be told that rather than left to read
/// "no such person".
fn live_person(store: &Store, name: &str) -> Result<PersonRow, String> {
    let name = fleet_core::store::validate_person_name(name).map_err(|e| e.message)?;
    store
        .get_person_by_name(&name)
        .map_err(|e| e.message)?
        .ok_or_else(|| {
            format!(
                "no live person named '{name}'; `fleet-hub person list` names every person \
                 this hub knows, disabled ones included"
            )
        })
}

/// How many live devices belong to `person`.
fn live_devices(store: &Store, person: i64) -> Result<usize, String> {
    Ok(store
        .active_client_tokens()
        .map_err(|e| e.message)?
        .into_iter()
        .filter(|c| c.person_id == Some(person))
        .count())
}

/// How many live grants reach `person` — shares other people made them.
fn live_grants(store: &Store, person: i64) -> Result<usize, String> {
    Ok(store
        .grants_for_person(person)
        .map_err(|e| e.message)?
        .len())
}

pub fn run(
    cmd: PersonCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    match cmd {
        PersonCmd::List { json } => {
            let people = store.list_people().map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&people).map_err(|e| e.to_string())?);
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<Vec<String>> = people
                .iter()
                .map(|p| {
                    vec![
                        p.id.to_string(),
                        p.name.clone(),
                        p.display_name.clone().unwrap_or_else(|| "-".to_string()),
                        if p.is_personal_owner { "owner" } else { "-" }.to_string(),
                        fmt_time(Some(p.created_at)),
                        fmt_time(p.disabled_at),
                    ]
                })
                .collect();
            out::line(&table(
                &["ID", "NAME", "DISPLAY", "OWNER", "CREATED", "DISABLED"],
                &rows,
            ));
            out::line(
                "OWNER marks this hub's own owner, who cannot be disabled. A time under \
                 DISABLED means that person's devices and the shares made to them were \
                 revoked; their own sessions are still theirs.",
            );
        }
        PersonCmd::Rename {
            person,
            to,
            display_name,
        } => {
            if to.is_none() && display_name.is_none() {
                return Err(
                    "nothing to change: pass --to <new name> and/or --display-name <text> \
                     (\"\" clears it)"
                        .to_string(),
                );
            }
            let row = live_person(&store, &person)?;
            // `rename_person` owns both rules — the name's shape and the
            // live-name collision — and refuses with its own `E_EXISTS`
            // sentence rather than letting `idx_people_live_name` surface as
            // a constraint message. Nothing is re-checked here.
            let after = store
                .rename_person(row.id, to.as_deref(), display_name.as_deref())
                .map_err(|e| e.message)?;
            out::line(&format!(
                "person {} is now '{}'{}; every grant and session of theirs is addressed to \
                 the id, so nothing else moved",
                after.id,
                after.name,
                match &after.display_name {
                    Some(d) => format!(" (shown as '{d}')"),
                    None => String::new(),
                }
            ));
        }
        PersonCmd::Disable { person, force } => {
            let row = live_person(&store, &person)?;
            // The store owns the rule that this hub's own owner cannot be
            // disabled, and it checks it before it writes anything — so let
            // the refusal be its own words instead of restating the reason
            // here, where the two could drift. The `unwrap_or_else` is the
            // fail-closed half: if `disable_person` ever stopped refusing,
            // this command still does.
            if row.is_personal_owner {
                return Err(store
                    .disable_person(row.id)
                    .err()
                    .map(|e| e.message)
                    .unwrap_or_else(|| {
                        format!(
                            "person {} ('{}') is this hub's personal owner and must not be \
                             disabled: every access gate keys on that row",
                            row.id, row.name
                        )
                    }));
            }
            let devices = live_devices(&store, row.id)?;
            let grants = live_grants(&store, row.id)?;
            if !force {
                return Err(format!(
                    "disabling {} (person {}) would revoke {devices} device(s) of theirs and \
                     {grants} grant(s) made TO them, in one transaction. Their own sessions \
                     stay private and stay theirs, and the shares they MADE stand. There is \
                     no re-enable in this release. Pass --force to do it.",
                    row.name, row.id
                ));
            }
            store.disable_person(row.id).map_err(|e| e.message)?;
            // Measured, not assumed: the two halves are counted again after
            // the write, so the line below is evidence that both happened
            // rather than a restatement of what was meant to happen.
            let devices_left = live_devices(&store, row.id)?;
            let grants_left = live_grants(&store, row.id)?;
            out::line(&format!(
                "disabled {} (person {}): {} device(s) revoked, {} grant(s) to them revoked",
                row.name,
                row.id,
                devices - devices_left,
                grants - grants_left
            ));
            out::line(
                "Their own sessions are untouched: still private, still theirs, readable by \
                 nobody else. The shares they made stand — those belong to each session's \
                 owner. There is no re-enable in this release.",
            );
        }
        PersonCmd::LinkSso {
            person,
            subject,
            issuer,
        } => {
            let issuer = sso_issuer(issuer, env)?;
            let row = live_person(&store, &person)?;
            store
                .link_identity(&issuer, &subject, row.id)
                .map_err(|e| e.message)?;
            out::line(&format!(
                "linked {issuer} account {} to {} (person {}): signing in with it pairs a                  device as them",
                subject.trim(),
                row.name,
                row.id
            ));
        }
        PersonCmd::UnlinkSso { subject, issuer } => {
            let issuer = sso_issuer(issuer, env)?;
            if !store
                .unlink_identity(&issuer, &subject)
                .map_err(|e| e.message)?
            {
                return Err(format!(
                    "no {issuer} account {} is linked to anybody",
                    subject.trim()
                ));
            }
            out::line(&format!(
                "unlinked {issuer} account {}; devices already paired with it stay paired                  until `fleet-hub client revoke`",
                subject.trim()
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use fleet_core::store::{GrantRecipient, GRANT_WATCH};

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: PersonCmd,
    }

    #[test]
    fn the_subcommands_parse() {
        let t = T::try_parse_from(["t", "list"]).unwrap();
        assert!(matches!(t.cmd, PersonCmd::List { json: false }));
        let t = T::try_parse_from(["t", "list", "--json"]).unwrap();
        assert!(matches!(t.cmd, PersonCmd::List { json: true }));
        let t = T::try_parse_from(["t", "rename", "ada", "--to", "ada.l"]).unwrap();
        assert!(
            matches!(t.cmd, PersonCmd::Rename { person, to, display_name: None }
                if person == "ada" && to.as_deref() == Some("ada.l"))
        );
        let t = T::try_parse_from(["t", "disable", "ada", "--force"]).unwrap();
        assert!(matches!(t.cmd, PersonCmd::Disable { person, force: true } if person == "ada"));
    }

    /// `--force` is a flag the operator types, never a default: a parse
    /// without it must come back `force: false` so the confirmation gate in
    /// `run` is reachable at all.
    #[test]
    fn disable_does_not_default_to_force() {
        let t = T::try_parse_from(["t", "disable", "ada"]).unwrap();
        assert!(matches!(t.cmd, PersonCmd::Disable { force: false, .. }));
    }

    // --- behaviour, against a real store -------------------------------------

    struct Hub {
        _dir: tempfile::TempDir,
        opts: HubOptions,
        env: HashMap<String, String>,
    }

    impl Hub {
        /// A data dir with a migrated `state.db`, as `fleet-hub init` leaves
        /// it: one person, this hub's owner.
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let opts = HubOptions {
                data_dir: Some(dir.path().to_path_buf()),
                ..HubOptions::default()
            };
            let env = HashMap::new();
            // Creates the database; `run` then refuses nothing.
            drop(crate::serve::open_store(&opts, &env).expect("store"));
            Hub {
                _dir: dir,
                opts,
                env,
            }
        }

        fn store(&self) -> Store {
            crate::serve::open_store(&self.opts, &self.env).expect("store")
        }

        fn run(&self, cmd: PersonCmd) -> Result<ExitCode, String> {
            run(cmd, &self.opts, &self.env)
        }

        fn person(&self, name: &str) -> i64 {
            self.store().create_person(name, None).expect("person").id
        }

        fn owner(&self) -> PersonRow {
            let s = self.store();
            let id = s.personal_owner_id().expect("owner").expect("one owner");
            s.get_person(id).expect("row").expect("row")
        }

        /// A live device bound to `person`.
        fn device(&self, name: &str, person: i64) {
            let s = self.store();
            s.insert_client_token(name, &format!("{name}-sha256"), "full")
                .expect("token");
            s.set_client_person(name, Some(person)).expect("bind");
        }

        /// A session owned by `owner`, shared with `to` at `watch`.
        fn shared_session(&self, name: &str, owner: i64, to: i64) -> i64 {
            let s = self.store();
            s.insert_host("h", None).ok();
            let id = s
                .upsert_bg_session("h", name, None, &format!("{name}-cid"), None, 1, "bg", 1)
                .expect("session");
            fleet_core::service::sessions::write_claim(&s, id, owner, "test").expect("claim");
            s.grant_session(id, GrantRecipient::Person(to), GRANT_WATCH, owner)
                .expect("grant");
            id
        }
    }

    #[test]
    fn link_sso_links_a_live_person_and_unlink_sso_frees_the_account() {
        let hub = Hub::new();
        let ada = hub.person("ada");
        let iss = "https://sso.example.com/realms/acme";
        assert!(
            hub.run(PersonCmd::LinkSso {
                person: "ada".into(),
                subject: "sub-1".into(),
                issuer: None,
            })
            .unwrap_err()
            .contains("FLEET_HUB_OIDC_ISSUER"),
            "no issuer anywhere is refused"
        );
        hub.run(PersonCmd::LinkSso {
            person: "ada".into(),
            subject: "sub-1".into(),
            issuer: Some(format!("{iss}/")),
        })
        .expect("link");
        assert_eq!(
            hub.store()
                .get_identity(iss, "sub-1")
                .expect("read")
                .expect("linked")
                .person_id,
            ada
        );
        hub.run(PersonCmd::UnlinkSso {
            subject: "sub-1".into(),
            issuer: Some(iss.into()),
        })
        .expect("unlink");
        assert!(hub
            .run(PersonCmd::UnlinkSso {
                subject: "sub-1".into(),
                issuer: Some(iss.into()),
            })
            .is_err());
    }

    /// The listing is how an operator reading `person 3` finds out who 3 is,
    /// and a DISABLED person has to be in it: a grant and a session still
    /// point at them, and a row the listing hid would be one the operator
    /// cannot act on. Remove the disabled rows (or the DISABLED column) and
    /// this fails.
    #[test]
    fn list_names_every_person_and_marks_a_disabled_one() {
        let hub = Hub::new();
        let ada = hub.person("ada");
        let bob = hub.person("bob");
        hub.store().disable_person(bob).expect("disable");

        // The table, as the operator reads it.
        let people = hub.store().list_people().expect("people");
        let by_id = |id: i64| people.iter().find(|p| p.id == id).expect("row").clone();
        assert_eq!(by_id(ada).name, "ada");
        assert!(by_id(ada).disabled_at.is_none(), "ada is live");
        assert!(
            by_id(bob).disabled_at.is_some(),
            "a disabled person is still listed, and carries the stamp the DISABLED column shows"
        );
        assert!(
            hub.owner().is_personal_owner,
            "the OWNER column has a row to mark"
        );
        assert_eq!(
            hub.run(PersonCmd::List { json: false }),
            Ok(ExitCode::SUCCESS)
        );
        assert_eq!(
            hub.run(PersonCmd::List { json: true }),
            Ok(ExitCode::SUCCESS)
        );
    }

    /// Disabling this hub's own owner is refused — with `--force`, which is
    /// the only way the gate could be got past. Drop the `is_personal_owner`
    /// arm in `run` and the store still refuses, which is the point of
    /// asserting on the message rather than on the exit code.
    #[test]
    fn disable_refuses_the_personal_owner() {
        let hub = Hub::new();
        let owner = hub.owner();
        let e = hub
            .run(PersonCmd::Disable {
                person: owner.name.clone(),
                force: true,
            })
            .expect_err("the owner is not disable-able");
        assert!(e.contains("personal owner"), "{e}");
        assert!(hub.owner().disabled_at.is_none(), "and nothing was written");
    }

    /// Both halves of "their reach is gone", and neither half more: the
    /// devices and the grants TO them go, the grant they MADE stays, and
    /// their own session stays theirs. Remove
    /// `revoke_live_grants_to_person` from `disable_person` and the second
    /// assertion fails; widen it to the grants they made and the third does.
    #[test]
    fn disable_revokes_their_devices_and_the_grants_to_them_but_not_the_ones_they_made() {
        let hub = Hub::new();
        let ada = hub.person("ada");
        let bob = hub.person("bob");
        hub.device("ada-laptop", ada);
        hub.device("ada-phone", ada);
        // bob shares his session with ada: a grant TO ada, which goes.
        let bobs = hub.shared_session("bobs-work", bob, ada);
        // ada shares hers with bob: a grant ada MADE, which stands.
        let adas = hub.shared_session("adas-work", ada, bob);

        assert_eq!(live_devices(&hub.store(), ada), Ok(2));
        assert_eq!(live_grants(&hub.store(), ada), Ok(1));

        // Unforced, it is a dry run: it says what would go and writes nothing.
        let preview = hub
            .run(PersonCmd::Disable {
                person: "ada".into(),
                force: false,
            })
            .expect_err("refused without --force");
        assert!(preview.contains("2 device(s)"), "{preview}");
        assert!(preview.contains("1 grant(s)"), "{preview}");
        assert_eq!(
            live_devices(&hub.store(), ada),
            Ok(2),
            "nothing was written"
        );

        hub.run(PersonCmd::Disable {
            person: "ada".into(),
            force: true,
        })
        .expect("disabled");

        let s = hub.store();
        assert_eq!(
            live_devices(&s, ada),
            Ok(0),
            "every device of theirs revoked"
        );
        assert_eq!(live_grants(&s, ada), Ok(0), "every grant TO them revoked");
        assert_eq!(
            live_grants(&s, bob),
            Ok(1),
            "the grant ada MADE belongs to no-one but the session's owner and stands"
        );
        assert!(s.grants_for_person(bob).expect("bob").contains_key(&adas));
        // Her own session is still hers, and still private.
        let row = s.get_session_by_id(adas).expect("row").expect("row");
        assert_eq!(row.owner_person_id, Some(ada));
        assert_eq!(row.visibility, "private");
        // And bob's is untouched too — disabling a recipient is not an
        // authority over the session she was shown.
        assert_eq!(
            s.get_session_by_id(bobs)
                .expect("row")
                .expect("row")
                .owner_person_id,
            Some(bob)
        );
    }

    /// `idx_people_live_name`, surfaced: a LIVE person's name is taken, a
    /// DISABLED person's is free. Drop the collision check in
    /// `rename_person` and the first half fails; widen it to disabled rows
    /// and the second does.
    #[test]
    fn rename_refuses_a_live_name_and_accepts_a_disabled_ones() {
        let hub = Hub::new();
        let ada = hub.person("ada");
        let gone = hub.person("eve");
        hub.store().disable_person(gone).expect("disable");

        let e = hub
            .run(PersonCmd::Rename {
                person: "ada".into(),
                to: Some("owner".into()),
                display_name: None,
            })
            .expect_err("a live person holds 'owner'");
        assert!(e.contains("already exists"), "{e}");
        assert!(!e.contains("UNIQUE"), "not a raw constraint message: {e}");
        assert_eq!(
            hub.store().get_person(ada).unwrap().unwrap().name,
            "ada",
            "and nothing was written"
        );

        hub.run(PersonCmd::Rename {
            person: "ada".into(),
            to: Some("eve".into()),
            display_name: None,
        })
        .expect("a departed colleague's name is free to take");
        assert_eq!(hub.store().get_person(ada).unwrap().unwrap().name, "eve");
        assert_eq!(
            hub.store().get_person(gone).unwrap().unwrap().name,
            "eve",
            "the disabled row keeps its name for the audit trail"
        );
    }

    /// The positive control, and invariant 2 of the brief: a rename changes
    /// the text and nothing else. The grant still points at the same person
    /// id, at the same level, on the same session — a rename is not a way to
    /// re-home somebody's share onto another human.
    #[test]
    fn a_rename_moves_no_grant_and_no_flag() {
        let hub = Hub::new();
        let ada = hub.person("ada");
        let bob = hub.person("bob");
        let session = hub.shared_session("work", bob, ada);
        let before = hub.store().grants_for_person(ada).expect("grants");

        hub.run(PersonCmd::Rename {
            person: "ada".into(),
            to: Some("ada.lovelace".into()),
            display_name: Some("Ada L.".into()),
        })
        .expect("renamed");

        let s = hub.store();
        let row = s.get_person(ada).unwrap().unwrap();
        assert_eq!(row.name, "ada.lovelace");
        assert_eq!(row.display_name.as_deref(), Some("Ada L."));
        assert!(!row.is_personal_owner, "the owner flag does not travel");
        assert_eq!(
            s.grants_for_person(ada).expect("grants"),
            before,
            "the grant is addressed to the id: the same session at the same level"
        );
        assert_eq!(
            s.grants_for_person(bob).expect("grants").len(),
            0,
            "and it did not reach anybody new"
        );
        assert!(before.contains_key(&session));
        // Nobody else was touched.
        assert_eq!(s.get_person(bob).unwrap().unwrap().name, "bob");
        assert!(hub.run(PersonCmd::List { json: false }).is_ok());
    }

    /// A rename with nothing to rename is a typo, not a smaller rename.
    #[test]
    fn rename_needs_something_to_change() {
        let hub = Hub::new();
        hub.person("ada");
        let e = hub
            .run(PersonCmd::Rename {
                person: "ada".into(),
                to: None,
                display_name: None,
            })
            .expect_err("nothing to change");
        assert!(e.contains("--to"), "{e}");
    }

    /// A disabled person is addressable nowhere, and the refusal says where
    /// to look rather than leaving the operator with "no such person".
    #[test]
    fn a_name_nobody_live_holds_is_refused_with_a_pointer_to_list() {
        let hub = Hub::new();
        let e = hub
            .run(PersonCmd::Disable {
                person: "nobody".into(),
                force: true,
            })
            .expect_err("no such live person");
        assert!(e.contains("person list"), "{e}");
    }
}
