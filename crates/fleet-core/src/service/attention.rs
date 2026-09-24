//! Which sessions need a person, decided once, here.
//!
//! # Why this is on the hub
//!
//! "What needs me?" is the question a phone is opened to answer, and until
//! now every client worked it out for itself from rows it had to download
//! first. The desktop has the rich version (`src/lib/attention.ts`,
//! `classify`), fleet-mobile has a two-condition predicate, and the hub's own
//! `fleet_health` roll-up has no notion of it at all — it counts ghosts,
//! stuck rows and context pressure, never "a person is needed here".
//!
//! So a phone downloaded 44 full rows — 51 968 B measured — to find the three
//! that wanted an answer. With the reason on the row and a filter beside it,
//! that is 1 668 B.
//!
//! The divergence between those two classifiers is latent rather than
//! absent: replayed over a 56-row capture they agree exactly, but only
//! because that capture holds no `failed` row, no ghost, no `lost_at` and no
//! `safe_kill_state`. The first one of those would have split them.
//!
//! # What is here and what is not
//!
//! Only the classification. Ranking, scoring and sort order stay with the
//! client: they are how a screen chooses to present the queue, not a fact
//! about the fleet.
//!
//! The desktop's "idle for too long" bucket is deliberately **not** decided
//! here either. It depends on an operator-configured idle threshold,
//! and a hub that guessed one would quietly disagree with the desktop that
//! set it. What this module answers is the states that need no knob to
//! recognise; a client is free to add its own idle rule on top.

use crate::store::SessionRow;

/// Why a session needs a person. The order is the urgency order, and it is
/// the same order the desktop's `TRIAGE_BUCKETS` uses, so the two cannot
/// drift into disagreeing about which of two reasons wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Blocked on a dialog: a permission prompt, a question, an elicitation.
    /// The one state where an answer is all that is wanted.
    Waiting,
    /// Wedged in a way the REPL will not leave on its own — an auth menu, a
    /// reconnect, an OOM. `stuck_kind` says which.
    Stuck,
    /// Claude reported a failed turn.
    Failed,
    /// The session's lifecycle is broken: a safe kill that failed or is still
    /// pending, a ghost row, or a row the fleet has lost track of.
    Lifecycle,
}

impl Reason {
    /// The wire spelling, matching the desktop's bucket names.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Waiting => "waiting",
            Reason::Stuck => "stuck",
            Reason::Failed => "failed",
            Reason::Lifecycle => "lifecycle",
        }
    }
}

/// A session that needs a person, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attention {
    pub reason: Reason,
    /// Best-effort unix second the session entered this state. Falls back to
    /// `last_activity_at`, which is always present, so a client can always
    /// draw an age.
    pub since: i64,
}

/// Whether this row needs a person, and why.
///
/// The order of the checks *is* the precedence: a session that is both
/// blocked and ghosted is reported as blocked, because that is the one a
/// person can do something about right now.
///
/// An `external` session — a Claude running outside fleet entirely — never
/// qualifies, whatever its fields say: it is read-only here, so reporting it
/// as needing a person offers an action that does not exist.
pub fn needs_attention(row: &SessionRow) -> Option<Attention> {
    if row.kind == "external" {
        return None;
    }
    let reason = if row.claude_status.as_deref() == Some("blocked") {
        Reason::Waiting
    } else if row.stuck_kind.is_some() {
        Reason::Stuck
    } else if row.claude_status.as_deref() == Some("failed") {
        Reason::Failed
    } else if is_lifecycle_broken(row) {
        Reason::Lifecycle
    } else {
        return None;
    };
    Some(Attention {
        reason,
        since: since_for(row, reason),
    })
}

fn is_lifecycle_broken(row: &SessionRow) -> bool {
    matches!(
        row.safe_kill_state.as_deref(),
        Some("failed") | Some("requested")
    ) || row.status == "ghost"
        || row.lost_at.is_some()
}

fn since_for(row: &SessionRow, reason: Reason) -> i64 {
    match reason {
        Reason::Stuck => row.stuck_since.unwrap_or(row.last_activity_at),
        Reason::Lifecycle => row
            .lost_at
            .or(row.safe_kill_requested_at)
            .unwrap_or(row.last_activity_at),
        _ => row.last_activity_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain running session that needs nobody, to vary one field at a time.
    fn row() -> SessionRow {
        SessionRow {
            id: 1,
            row_version: 0,
            tmux_name: "t".to_string(),
            host_alias: "alpha".to_string(),
            project_id: None,
            worktree_id: None,
            created_at: 0,
            last_activity_at: 100,
            status: "running".to_string(),
            notes: None,
            account_uuid: None,
            kind: "work".to_string(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            lost_reason: None,
            claude_session_id: None,
            claude_status: Some("working".to_string()),
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct: None,
            stuck_kind: None,
            friendly_name: None,
            safe_kill_state: None,
            safe_kill_nonce: None,
            safe_kill_detail: None,
            safe_kill_requested_at: None,
            idle_since: None,
            stuck_since: None,
            last_playbook_at: None,
            last_prompt: None,
            started_at: None,
            last_turn_at: None,
            ci_status: None,
            turn_seq: 0,
            last_stop_at: None,
            parent_session_id: None,
            tags: Vec::new(),
            usage: Default::default(),
            context: Default::default(),
            pending_input: None,
            work: None,
            work_rejected: vec![],
        }
    }

    #[test]
    fn a_blocked_session_is_waiting_and_an_ordinary_one_is_nothing() {
        let mut r = row();
        assert_eq!(needs_attention(&r), None, "a working session needs nobody");

        r.claude_status = Some("blocked".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    /// The order of the checks is the precedence, and this is the pair that
    /// makes it matter: a person can answer a blocked session now, and can do
    /// nothing about a ghost row until they are at a terminal.
    #[test]
    fn blocked_outranks_every_other_reason() {
        let mut r = row();
        r.claude_status = Some("blocked".into());
        r.stuck_kind = Some("auth_menu".into());
        r.status = "ghost".into();
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    #[test]
    fn stuck_failed_and_the_three_lifecycle_shapes_each_qualify() {
        let mut r = row();
        r.stuck_kind = Some("oom".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Stuck);

        let mut r = row();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Failed);

        for broken in [
            |r: &mut SessionRow| r.safe_kill_state = Some("failed".into()),
            |r: &mut SessionRow| r.safe_kill_state = Some("requested".into()),
            |r: &mut SessionRow| r.status = "ghost".into(),
            |r: &mut SessionRow| r.lost_at = Some(99),
        ] {
            let mut r = row();
            broken(&mut r);
            assert_eq!(
                needs_attention(&r).map(|a| a.reason),
                Some(Reason::Lifecycle)
            );
        }
    }

    /// An external session is a Claude running outside fleet: nothing here can
    /// act on it, so reporting it would offer an action that does not exist.
    #[test]
    fn an_external_session_never_needs_a_person() {
        let mut r = row();
        r.kind = "external".into();
        r.claude_status = Some("blocked".into());
        r.stuck_kind = Some("oom".into());
        assert_eq!(needs_attention(&r), None);
    }

    /// A client draws an age from `since`, so it must always be a number.
    #[test]
    fn since_prefers_the_state_stamp_and_always_falls_back() {
        let mut r = row();
        r.last_activity_at = 100;
        r.stuck_kind = Some("oom".into());
        assert_eq!(
            needs_attention(&r).unwrap().since,
            100,
            "no stuck_since yet"
        );

        r.stuck_since = Some(140);
        assert_eq!(needs_attention(&r).unwrap().since, 140);

        let mut r = row();
        r.last_activity_at = 100;
        r.lost_at = Some(150);
        assert_eq!(needs_attention(&r).unwrap().since, 150);
    }

    /// The wire spellings are the desktop's bucket names; a rename on one
    /// side without the other would split the two classifiers silently.
    #[test]
    fn the_wire_spellings_match_the_desktop_buckets() {
        let ts = include_str!("../../../../src/lib/attention.ts");
        for r in [
            Reason::Waiting,
            Reason::Stuck,
            Reason::Failed,
            Reason::Lifecycle,
        ] {
            assert!(
                ts.contains(&format!("'{}'", r.as_str())),
                "src/lib/attention.ts does not name the bucket {}",
                r.as_str()
            );
        }
    }
}
