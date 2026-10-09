//! Presence (redesign step 11.7b): who is looking at a session right now.
//!
//! A client that has a session open calls `session_presence` every
//! [`HEARTBEAT_SECS`] and once more with `leaving` when it closes it. The hub
//! keeps the answers in memory, nowhere else: presence is a fact about the
//! next minute, so a restart that forgets it loses nothing, and there is no
//! table, no migration and no event to replay.
//!
//! **Who learns it.** The tool is gated at `Reach::Watch`, so only a caller
//! who may read the session row may report being on it at all. A caller who
//! cannot see the row gets `E_NOTFOUND`, as for every session tool, and so
//! learns nothing, not even that somebody is watching. Among those who may,
//! the owner sees everyone looking; anyone else sees the owner and
//! themselves — never another grantee, the rule `session_access` keeps
//! (who else holds a grant is not what a share promised the grantee).
//!
//! **Expiry is the only cleanup that has to work.** A window that crashes
//! never says `leaving`; its entry is dropped [`PRESENCE_TTL_SECS`] after its
//! last heartbeat, at the next read or write of the board. A revoked grantee
//! cannot report again (the gate refuses them), so their entry lapses the
//! same way.

use crate::ipc_error::IpcError;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

/// How often a client with a session open reports it.
pub const HEARTBEAT_SECS: i64 = 20;

/// How long a report counts: two missed heartbeats and a little slack.
pub const PRESENCE_TTL_SECS: i64 = 2 * HEARTBEAT_SECS + 5;

/// The most viewers one session keeps. A bound, not a product limit: the
/// board lives in memory and a misbehaving client cycling device names must
/// not grow it without end. The oldest report is dropped first.
pub const MAX_VIEWERS_PER_SESSION: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    person_id: i64,
    /// The paired device's name; `None` for the master token.
    device: Option<String>,
    /// When this viewer started looking, kept across heartbeats.
    since: i64,
    seen_at: i64,
}

/// The hub's in-memory record of who has which session open.
#[derive(Debug, Default)]
pub struct PresenceBoard {
    sessions: Mutex<HashMap<i64, Vec<Seen>>>,
}

/// One live report, as [`PresenceBoard::viewers`] hands it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub person_id: i64,
    pub device: Option<String>,
    pub since: i64,
}

impl PresenceBoard {
    pub fn new() -> Self {
        Self::default()
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Vec<Seen>>> {
        // Nothing on the board is worth refusing a call over: a poisoned
        // lock still holds a usable map.
        self.sessions.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn prune(map: &mut HashMap<i64, Vec<Seen>>, now: i64) {
        map.retain(|_, seen| {
            seen.retain(|s| now - s.seen_at <= PRESENCE_TTL_SECS);
            !seen.is_empty()
        });
    }

    /// `person` on `device` has `session` open at `now`.
    pub fn here(&self, session: i64, person: i64, device: Option<&str>, now: i64) {
        let mut map = self.guard();
        Self::prune(&mut map, now);
        let seen = map.entry(session).or_default();
        if let Some(s) = seen
            .iter_mut()
            .find(|s| s.person_id == person && s.device.as_deref() == device)
        {
            s.seen_at = now;
            return;
        }
        if seen.len() >= MAX_VIEWERS_PER_SESSION {
            if let Some(oldest) = seen
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.seen_at)
                .map(|(i, _)| i)
            {
                seen.remove(oldest);
            }
        }
        seen.push(Seen {
            person_id: person,
            device: device.map(str::to_owned),
            since: now,
            seen_at: now,
        });
    }

    /// `person` on `device` closed `session`.
    pub fn leave(&self, session: i64, person: i64, device: Option<&str>) {
        let mut map = self.guard();
        if let Some(seen) = map.get_mut(&session) {
            seen.retain(|s| !(s.person_id == person && s.device.as_deref() == device));
            if seen.is_empty() {
                map.remove(&session);
            }
        }
    }

    /// Who has `session` open at `now`, earliest first.
    pub fn viewers(&self, session: i64, now: i64) -> Vec<Presence> {
        let mut map = self.guard();
        Self::prune(&mut map, now);
        let mut out: Vec<Presence> = map
            .get(&session)
            .map(|seen| {
                seen.iter()
                    .map(|s| Presence {
                        person_id: s.person_id,
                        device: s.device.clone(),
                        since: s.since,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|p| (p.since, p.person_id));
        out
    }

    /// Every person looking at something at `now`, with the sessions they
    /// have open. The caller decides which of those sessions it may name.
    pub fn watching(&self, now: i64) -> BTreeMap<i64, BTreeSet<i64>> {
        let mut map = self.guard();
        Self::prune(&mut map, now);
        let mut out: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
        for (session, seen) in map.iter() {
            for s in seen {
                out.entry(s.person_id).or_default().insert(*session);
            }
        }
        out
    }
}

/// `session_presence`: report that this client has a session open (or has
/// closed it) and read who else has.
#[derive(Debug, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "SessionPresenceParams")]
pub struct SessionPresenceArgs {
    /// The session on screen.
    pub session_id: i64,
    /// True when the session was just closed: drop this device's report.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub leaving: bool,
}

/// One person looking at the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    pub person_id: i64,
    /// The person's display name, else their name.
    pub name: String,
    /// The paired device they are looking from; absent for the hub's own
    /// token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Since when (unix seconds) this device has had it open.
    pub since: i64,
    /// This is the caller's own report.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub you: bool,
}

/// What `session_presence` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceView {
    pub session_id: i64,
    /// Everyone with the session open, the caller included, earliest first.
    /// A person on two devices is listed once per device.
    pub viewers: Vec<Viewer>,
    /// How often to report again, in seconds.
    pub heartbeat_secs: i64,
}

/// Record the caller's report and answer the session's viewers. The caller
/// has already passed the session's `Reach::Watch` gate; `person` is the
/// person that caller proves (`None` reports nothing and only reads), and
/// `owner` the session's owner: a caller who is not the owner is answered
/// only the owner and themselves.
pub fn session_presence(
    store: &Store,
    board: &PresenceBoard,
    owner: Option<i64>,
    person: Option<i64>,
    device: Option<&str>,
    args: &SessionPresenceArgs,
    now: i64,
) -> Result<PresenceView, IpcError> {
    if let Some(p) = person {
        if args.leaving {
            board.leave(args.session_id, p, device);
        } else {
            board.here(args.session_id, p, device, now);
        }
    }
    let mut names: HashMap<i64, Option<String>> = HashMap::new();
    let mut viewers = Vec::new();
    let is_owner = person.is_some() && person == owner;
    for v in board.viewers(args.session_id, now) {
        if !is_owner && Some(v.person_id) != owner && Some(v.person_id) != person {
            continue;
        }
        let name = match names.get(&v.person_id) {
            Some(n) => n.clone(),
            None => {
                let n = store
                    .get_person(v.person_id)?
                    .filter(|p| p.disabled_at.is_none())
                    .map(|p| p.display_name.unwrap_or(p.name));
                names.insert(v.person_id, n.clone());
                n
            }
        };
        // A person disabled since they reported is nobody to show.
        let Some(name) = name else { continue };
        viewers.push(Viewer {
            you: Some(v.person_id) == person && v.device.as_deref() == device,
            person_id: v.person_id,
            name,
            device: v.device,
            since: v.since,
        });
    }
    Ok(PresenceView {
        session_id: args.session_id,
        viewers,
        heartbeat_secs: HEARTBEAT_SECS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_lasts_until_its_ttl_and_a_heartbeat_keeps_since() {
        let b = PresenceBoard::new();
        b.here(7, 1, Some("mac"), 100);
        b.here(7, 1, Some("mac"), 120);
        let v = b.viewers(7, 120 + PRESENCE_TTL_SECS);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].since, 100, "a heartbeat must not restart the clock");
        assert!(b.viewers(7, 121 + PRESENCE_TTL_SECS).is_empty());
    }

    #[test]
    fn leaving_drops_only_that_device() {
        let b = PresenceBoard::new();
        b.here(7, 1, Some("mac"), 100);
        b.here(7, 1, Some("phone"), 101);
        b.here(7, 2, None, 102);
        b.leave(7, 1, Some("mac"));
        let v = b.viewers(7, 103);
        assert_eq!(
            v.iter()
                .map(|p| (p.person_id, p.device.clone()))
                .collect::<Vec<_>>(),
            vec![(1, Some("phone".into())), (2, None)]
        );
    }

    #[test]
    fn sessions_do_not_share_viewers() {
        let b = PresenceBoard::new();
        b.here(7, 1, None, 100);
        assert!(b.viewers(8, 100).is_empty());
    }

    #[test]
    fn one_session_keeps_a_bounded_number_of_viewers() {
        let b = PresenceBoard::new();
        for i in 0..(MAX_VIEWERS_PER_SESSION as i64 + 5) {
            b.here(7, 1, Some(&format!("d{i}")), 100 + i);
        }
        let v = b.viewers(7, 100 + MAX_VIEWERS_PER_SESSION as i64 + 5);
        assert_eq!(v.len(), MAX_VIEWERS_PER_SESSION);
        assert_eq!(v[0].device.as_deref(), Some("d5"), "the oldest went first");
    }

    #[test]
    fn watching_groups_live_reports_by_person() {
        let b = PresenceBoard::new();
        b.here(7, 1, None, 100);
        b.here(8, 1, None, 100);
        b.here(8, 2, None, 10);
        let w = b.watching(100);
        assert_eq!(w.get(&1).map(|s| s.len()), Some(2));
        assert!(!w.contains_key(&2), "an expired report is not watching");
    }

    #[test]
    fn the_view_names_people_marks_the_caller_and_skips_the_disabled() {
        let s = Store::open_in_memory().unwrap();
        let ana = s.create_person("ana", None).unwrap().id;
        let bo = s.create_person("bo", None).unwrap().id;
        let b = PresenceBoard::new();
        b.here(7, bo, Some("bo-phone"), 90);
        let args = SessionPresenceArgs {
            session_id: 7,
            leaving: false,
        };
        let v =
            session_presence(&s, &b, Some(ana), Some(ana), Some("ana-mac"), &args, 100).unwrap();
        assert_eq!(v.heartbeat_secs, HEARTBEAT_SECS);
        assert_eq!(
            v.viewers
                .iter()
                .map(|v| (v.name.as_str(), v.you))
                .collect::<Vec<_>>(),
            vec![("bo", false), ("ana", true)]
        );
        s.disable_person(bo).unwrap();
        let v =
            session_presence(&s, &b, Some(ana), Some(ana), Some("ana-mac"), &args, 101).unwrap();
        assert_eq!(v.viewers.len(), 1);
        let gone = SessionPresenceArgs {
            session_id: 7,
            leaving: true,
        };
        let v =
            session_presence(&s, &b, Some(ana), Some(ana), Some("ana-mac"), &gone, 102).unwrap();
        assert!(v.viewers.is_empty());
    }

    #[test]
    fn a_caller_with_no_person_reads_without_reporting() {
        let s = Store::open_in_memory().unwrap();
        let b = PresenceBoard::new();
        let args = SessionPresenceArgs {
            session_id: 7,
            leaving: false,
        };
        let v = session_presence(&s, &b, Some(1), None, None, &args, 100).unwrap();
        assert!(v.viewers.is_empty());
        assert!(b.viewers(7, 100).is_empty());
    }

    #[test]
    fn a_grantee_sees_the_owner_and_themselves_but_no_other_grantee() {
        let s = Store::open_in_memory().unwrap();
        let owner = s.create_person("ana", None).unwrap().id;
        let bo = s.create_person("bo", None).unwrap().id;
        let cy = s.create_person("cy", None).unwrap().id;
        let b = PresenceBoard::new();
        b.here(7, owner, Some("mac"), 90);
        b.here(7, cy, Some("cy-phone"), 91);
        let args = SessionPresenceArgs {
            session_id: 7,
            leaving: false,
        };
        let names = |v: PresenceView| v.viewers.into_iter().map(|v| v.name).collect::<Vec<_>>();
        let v =
            session_presence(&s, &b, Some(owner), Some(bo), Some("bo-mac"), &args, 100).unwrap();
        assert_eq!(names(v), vec!["ana", "bo"]);
        let v =
            session_presence(&s, &b, Some(owner), Some(owner), Some("mac"), &args, 101).unwrap();
        assert_eq!(names(v), vec!["ana", "cy", "bo"]);
    }
}
