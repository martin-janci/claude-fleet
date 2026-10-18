//! Which threads Control still follows (Control review F10).
//!
//! Before this, a session Control started or briefed was followed forever:
//! the operator feed's subscription was every receipt ever written, so a
//! session briefed a week ago still woke Control with each finished turn.
//! Now a thread is followed only while it matters:
//!
//! * a **session** stops being followed when it is killed or removed, done
//!   (Claude reported it completed), its pull request merged after Control
//!   last briefed it, or nothing happened on it for
//!   `control.follow_quiet_hours` (default 24) — no turn, no new brief,
//!   not working now. A new brief (a receipt newer than the merge, a turn)
//!   follows it again;
//! * a **mission** is followed while it is not finished (`completed`,
//!   `failed`, `cancelled`);
//! * either is not followed while **muted** (`control_mutes`, the Control
//!   threads column's bell or `control_journal { action: mute }`), until it
//!   is unmuted.
//!
//! The decision is [`follow_state`], PURE; [`follows`] reads the facts and
//! answers every candidate with its state, which the operator feed filters
//! by, `control_journal` lists (the MCP-visible flag) and the desktop draws.

use crate::service::settings;
use crate::store::{SessionRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Whether Control follows a thread, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowState {
    Following,
    /// Muted by the person (or Control).
    Muted,
    /// Killed, removed, or lost for good.
    Killed,
    /// Its pull request merged after Control last briefed it.
    Merged,
    /// Claude reported it completed; a finished mission.
    Done,
    /// Nothing happened on it for `control.follow_quiet_hours`.
    Quiet,
}

impl FollowState {
    pub fn following(self) -> bool {
        self == FollowState::Following
    }
}

/// What [`follow_state`] decides a session on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionFacts {
    pub muted: bool,
    /// Its row is gone, or it is a ghost killed or missing.
    pub killed: bool,
    /// A `pr_merged` event at or after Control's last brief.
    pub merged: bool,
    /// Claude reported the session completed.
    pub done: bool,
    /// A turn is running now.
    pub working: bool,
    /// Its last sign of life: the newest of its last turn, Control's last
    /// brief and its creation, unix seconds.
    pub last_seen: i64,
}

/// PURE: whether Control follows a session, given its facts. Muted
/// outranks the rest (the person's word), then killed, merged, done, and
/// quiet last: a session working now is never quiet.
pub fn follow_state(f: &SessionFacts, now: i64, quiet_secs: i64) -> FollowState {
    if f.muted {
        FollowState::Muted
    } else if f.killed {
        FollowState::Killed
    } else if f.merged {
        FollowState::Merged
    } else if f.done {
        FollowState::Done
    } else if !f.working && now - f.last_seen >= quiet_secs {
        FollowState::Quiet
    } else {
        FollowState::Following
    }
}

/// PURE: whether Control follows a mission in `state`.
pub fn mission_follow_state(state: &str, muted: bool) -> FollowState {
    if muted {
        FollowState::Muted
    } else if matches!(state, "completed" | "failed" | "cancelled") {
        FollowState::Done
    } else {
        FollowState::Following
    }
}

/// PURE: a session row's facts (all but `muted` and `merged`, which are
/// other tables'). `brief_at` is Control's last receipt naming it (`0`
/// for none).
pub fn session_facts(row: &SessionRow, brief_at: i64) -> SessionFacts {
    let killed = row.status == "ghost"
        && matches!(
            row.lost_reason.as_deref(),
            Some("killed" | "missing") | None
        );
    SessionFacts {
        muted: false,
        killed,
        merged: false,
        done: row.claude_status.as_deref() == Some("completed"),
        working: row.status == "running" && row.claude_status.as_deref() == Some("working"),
        last_seen: row
            .last_turn_at
            .unwrap_or(0)
            .max(brief_at)
            .max(row.created_at),
    }
}

/// One thread Control started or was handed, and whether it is followed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Follow {
    /// `session` | `mission`.
    pub kind: String,
    pub id: i64,
    pub state: FollowState,
    /// A session's tmux name, a mission's name.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Muted (also when another reason would stop it anyway).
    pub muted: bool,
}

/// `control.follow_quiet_hours`, in seconds.
pub fn quiet_secs(s: &Store) -> i64 {
    settings::get_string(s, settings::CONTROL_FOLLOW_QUIET_HOURS)
        .parse::<i64>()
        .unwrap_or(24)
        .max(1)
        * 3600
}

/// Every session and mission Control started or was handed, with whether
/// it is followed now. `exclude` is Control's own row.
pub fn follows(s: &Store, exclude: Option<i64>, now: i64) -> Vec<Follow> {
    let quiet = quiet_secs(s);
    let mutes: BTreeSet<(String, i64)> =
        s.control_mutes().unwrap_or_default().into_iter().collect();
    let muted = |kind: &str, id: i64| mutes.contains(&(kind.to_string(), id));
    let mut out = Vec::new();
    for (id, brief_at) in s.control_follow_candidates(exclude).unwrap_or_default() {
        let Ok(Some(row)) = s.get_session_by_id(id) else {
            continue;
        };
        let mut f = session_facts(&row, brief_at);
        f.muted = muted("session", id);
        f.merged = s
            .session_event_at_or_after(
                id,
                crate::store::PR_EVENT_MERGED,
                brief_at.max(row.created_at),
            )
            .unwrap_or(false);
        out.push(Follow {
            kind: "session".into(),
            id,
            state: follow_state(&f, now, quiet),
            name: row.tmux_name.clone(),
            host_alias: Some(row.host_alias.clone()),
            muted: f.muted,
        });
    }
    for id in s.handed_off_sessions_gone().unwrap_or_default() {
        out.push(Follow {
            kind: "session".into(),
            id,
            state: if muted("session", id) {
                FollowState::Muted
            } else {
                FollowState::Killed
            },
            name: format!("#{id}"),
            host_alias: None,
            muted: muted("session", id),
        });
    }
    for id in s.handed_off_mission_ids().unwrap_or_default() {
        let Ok(Some(m)) = s.get_mission(id) else {
            continue;
        };
        let mu = muted("mission", id);
        out.push(Follow {
            kind: "mission".into(),
            id,
            state: mission_follow_state(&m.state, mu),
            name: m.name.clone(),
            host_alias: None,
            muted: mu,
        });
    }
    out
}

/// The session ids Control follows now (see [`follows`]).
pub fn followed_sessions(s: &Store, exclude: Option<i64>, now: i64) -> BTreeSet<i64> {
    follows(s, exclude, now)
        .into_iter()
        .filter(|f| f.kind == "session" && f.state.following())
        .map(|f| f.id)
        .collect()
}

/// Whether the mission `id` is muted.
pub fn mission_muted(s: &Store, id: i64) -> bool {
    s.control_mutes()
        .unwrap_or_default()
        .iter()
        .any(|(k, m)| k == "mission" && *m == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    fn live(last_seen: i64) -> SessionFacts {
        SessionFacts {
            last_seen,
            ..Default::default()
        }
    }

    #[test]
    fn a_session_is_followed_until_it_stops_mattering() {
        let now = 100 * H;
        assert_eq!(
            follow_state(&live(now - H), now, 24 * H),
            FollowState::Following
        );
        assert_eq!(
            follow_state(&live(now - 24 * H), now, 24 * H),
            FollowState::Quiet
        );
        let working = SessionFacts {
            working: true,
            ..live(0)
        };
        assert_eq!(follow_state(&working, now, 24 * H), FollowState::Following);
        let done = SessionFacts {
            done: true,
            ..live(now)
        };
        assert_eq!(follow_state(&done, now, 24 * H), FollowState::Done);
        let merged = SessionFacts {
            merged: true,
            done: true,
            ..live(now)
        };
        assert_eq!(follow_state(&merged, now, 24 * H), FollowState::Merged);
        let killed = SessionFacts {
            killed: true,
            merged: true,
            ..live(now)
        };
        assert_eq!(follow_state(&killed, now, 24 * H), FollowState::Killed);
        let muted = SessionFacts {
            muted: true,
            working: true,
            ..live(now)
        };
        assert_eq!(follow_state(&muted, now, 24 * H), FollowState::Muted);
    }

    #[test]
    fn a_mission_is_followed_until_it_finishes_or_is_muted() {
        assert!(mission_follow_state("active", false).following());
        assert!(mission_follow_state("paused", false).following());
        assert_eq!(mission_follow_state("completed", false), FollowState::Done);
        assert_eq!(mission_follow_state("cancelled", false), FollowState::Done);
        assert_eq!(mission_follow_state("active", true), FollowState::Muted);
    }

    fn world() -> (Store, i64, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let mk = |name: &str| {
            s.upsert_session(name, "h", None, None, 1, 1, "running", None)
                .unwrap()
        };
        let (op, started, briefed) = (mk("fleet-operator"), mk("started"), mk("briefed"));
        mk("other");
        s.conn_ref()
            .execute(
                "UPDATE sessions SET origin = 'operator' WHERE id = ?1",
                [started],
            )
            .unwrap();
        s.insert_control_handoff(
            &crate::store::NewHandoff {
                tool: "send_prompt".into(),
                session_id: Some(briefed),
                ..Default::default()
            },
            1_000,
        )
        .unwrap();
        (s, op, started, briefed)
    }

    #[test]
    fn follows_reads_the_candidates_and_their_facts() {
        let (s, op, started, briefed) = world();
        let now = 2_000;
        let ids = followed_sessions(&s, Some(op), now);
        assert_eq!(ids, [started, briefed].into_iter().collect());
        // Muted, then merged after the brief: neither followed.
        s.set_control_mute("session", started, true, now).unwrap();
        s.conn_ref()
            .execute(
                "INSERT INTO session_events (session_id, at, kind, detail) VALUES (?1, 1500, 'pr_merged', 'u')",
                [briefed],
            )
            .unwrap();
        let all = follows(&s, Some(op), now);
        let state = |id: i64| all.iter().find(|f| f.id == id).unwrap().state;
        assert_eq!(state(started), FollowState::Muted);
        assert_eq!(state(briefed), FollowState::Merged);
        assert!(followed_sessions(&s, Some(op), now).is_empty());
        // A new brief after the merge follows it again.
        s.insert_control_handoff(
            &crate::store::NewHandoff {
                tool: "send_prompt".into(),
                session_id: Some(briefed),
                ..Default::default()
            },
            1_600,
        )
        .unwrap();
        assert!(followed_sessions(&s, Some(op), now).contains(&briefed));
        // Quiet for a day: not followed.
        assert!(!followed_sessions(&s, Some(op), 1_600 + 24 * H).contains(&briefed));
        // Its own row is never a candidate.
        assert!(follows(&s, Some(op), now).iter().all(|f| f.id != op));
    }
}
