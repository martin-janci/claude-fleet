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
    /// The last turn ended in an API error (a `StopFailure`: rate limit,
    /// auth, …); the `stop_failure` timeline entry says which. Re-prompt.
    StopFailed,
    /// Claude reported a failed turn (`claude agents`, a pane-less agent).
    Failed,
    /// The context window is at or past `health.context_red_pct`: compact
    /// or hand over before the next turn does it for you.
    ContextFull,
    /// The tick demoted a `working` row nothing had moved for
    /// `reconcile.stale_working_secs`: look at what it was doing. Ends on
    /// the next hook, an attach, the row `working` / `blocked` again, or
    /// after `reconcile.stale_working_ttl_secs`.
    StaleWorking,
    /// Idle with a PR whose checks are failing.
    CiFailing,
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
            Reason::StopFailed => "stop_failed",
            Reason::Failed => "failed",
            Reason::ContextFull => "context_full",
            Reason::StaleWorking => "stale_working",
            Reason::CiFailing => "ci_failing",
            Reason::Lifecycle => "lifecycle",
        }
    }
}

impl Reason {
    /// The attention state this reason puts a session in.
    pub fn state(self) -> State {
        match self {
            Reason::Waiting | Reason::Stuck | Reason::ContextFull | Reason::StaleWorking => {
                State::ActionRequired
            }
            Reason::StopFailed | Reason::Failed | Reason::CiFailing => State::Failed,
            // A ghost, a lost row or a pending safe kill: nobody can answer
            // it, so it leaves the badge (step 1.1 folds a mass loss into one
            // Restore row instead).
            Reason::Lifecycle => State::Paused,
        }
    }
}

/// The seven attention states of the Orbit Fleet redesign (step 0.4), in
/// urgency order. The twelve triage buckets fold into these, and the badge
/// counts only the first three. The table is shared with the desktop
/// through `src/lib/attention_states.json`, which both test suites check.
///
/// Not on the wire yet: hub contract 11 (step 2.6) carries the state beside
/// the reason, and `Blocked` gets its first reasons in step 2.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// A person has to answer or act: shown as "Needs you".
    ActionRequired,
    Failed,
    /// Waiting on something outside the session (a host, credentials, a
    /// limit, another task); shown as "Needs you" with its reason line.
    Blocked,
    Working,
    Paused,
    Done,
    Idle,
}

impl State {
    pub const ALL: [State; 7] = [
        State::ActionRequired,
        State::Failed,
        State::Blocked,
        State::Working,
        State::Paused,
        State::Done,
        State::Idle,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::ActionRequired => "action_required",
            State::Failed => "failed",
            State::Blocked => "blocked",
            State::Working => "working",
            State::Paused => "paused",
            State::Done => "done",
            State::Idle => "idle",
        }
    }

    /// Whether a session in this state raises the Needs you badge.
    pub fn counts_toward_badge(self) -> bool {
        matches!(self, State::ActionRequired | State::Failed | State::Blocked)
    }
}

/// Every triage bucket, in the desktop's `TRIAGE_BUCKETS` order, and its
/// state. The hub decides the eight [`Reason`]s; `done_unread`, `idle_long`,
/// `working` and `idle` are the desktop's own buckets, listed so the whole
/// map lives in one table.
pub const BUCKET_STATES: [(&str, State); 12] = [
    ("waiting", State::ActionRequired),
    ("stuck", State::ActionRequired),
    ("stop_failed", State::Failed),
    ("failed", State::Failed),
    ("context_full", State::ActionRequired),
    ("stale_working", State::ActionRequired),
    ("ci_failing", State::Failed),
    ("done_unread", State::Done),
    ("lifecycle", State::Paused),
    ("idle_long", State::Idle),
    ("working", State::Working),
    ("idle", State::Idle),
];

/// A session's attention state: its [`Reason`]'s state when it needs a
/// person, else `Working` or `Idle` from `claude_status`.
pub fn state_with(row: &SessionRow, context_red_pct: f64) -> State {
    match needs_attention_with(row, context_red_pct) {
        Some(a) => a.reason.state(),
        // A shell has no agent in it, whatever its status column says.
        None if row.kind != "shell" && row.claude_status.as_deref() == Some("working") => {
            State::Working
        }
        None => State::Idle,
    }
}

/// The one context threshold, when no store is at hand to read
/// `health.context_red_pct`: `fleet_health.context_red`, `context_full`
/// here and the desktop's chip all count from the same number.
pub const DEFAULT_CONTEXT_RED_PCT: f64 = 85.0;

/// A session that needs a person, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attention {
    pub reason: Reason,
    /// Best-effort unix second the session entered this state. Falls back to
    /// `last_activity_at`, which is always present, so a client can always
    /// draw an age.
    pub since: i64,
}

/// [`needs_attention_with`] at [`DEFAULT_CONTEXT_RED_PCT`]. Callers with a
/// store read the setting (`service::health::context_red_pct`) instead.
pub fn needs_attention(row: &SessionRow) -> Option<Attention> {
    needs_attention_with(row, DEFAULT_CONTEXT_RED_PCT)
}

/// Whether this row needs a person, and why.
///
/// The order of the checks *is* the precedence: a session that is both
/// blocked and ghosted is reported as blocked, because that is the one a
/// person can do something about right now — and every reason a person can
/// act on comes before `Lifecycle`, which nobody can act on from a phone.
///
/// An `external` session — a Claude running outside fleet entirely — never
/// qualifies, whatever its fields say: it is read-only here, so reporting it
/// as needing a person offers an action that does not exist. Nor does a
/// `shell`: there is no Claude in it.
pub fn needs_attention_with(row: &SessionRow, context_red_pct: f64) -> Option<Attention> {
    if row.kind == "external" || row.kind == "shell" {
        return None;
    }
    let failed = row.claude_status.as_deref() == Some("failed");
    // A dead row keeps its last context reading; only a live one can act on
    // it (or on a stale stamp) — a lost one reads `Lifecycle`.
    let live = row.status != "ghost" && row.lost_at.is_none();
    let idle = row
        .claude_status
        .as_deref()
        .is_some_and(|s| crate::store::IDLE_STATUSES.contains(&s));
    let reason = if row.claude_status.as_deref() == Some("blocked") || row.pending_form.is_some() {
        Reason::Waiting
    } else if row.stuck_kind.is_some() {
        Reason::Stuck
    } else if failed && !crate::store::has_no_pane(&row.kind) {
        Reason::StopFailed
    } else if failed {
        Reason::Failed
    } else if live && row.context_pct.is_some_and(|p| p >= context_red_pct) {
        Reason::ContextFull
    } else if live && row.stale_working_at.is_some() {
        Reason::StaleWorking
    } else if idle && row.ci_status.as_deref() == Some("failing") {
        Reason::CiFailing
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
        Reason::StopFailed => row.last_stop_at.unwrap_or(row.last_activity_at),
        Reason::ContextFull => row.context.context_at.unwrap_or(row.last_activity_at),
        Reason::StaleWorking => row.stale_working_at.unwrap_or(row.last_activity_at),
        Reason::CiFailing => row.idle_since.unwrap_or(row.last_activity_at),
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
            prompt_submit_seq: 0,
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
            stale_working_at: None,
            stale_demoted_at: None,
            work_rev: 0,
            pr_evidence: None,
            pr_checked_at: None,
            owner_person_id: None,
            visibility: crate::store::VISIBILITY_UNCLAIMED.into(),
            claude_profile: None,
            agent: crate::store::AGENT_CLAUDE.into(),
            origin: None,
            origin_ref: None,
            pending_form: None,
            parent_session_id: None,
            tags: Vec::new(),
            usage: Default::default(),
            context: Default::default(),
            pending_input: None,
            work: None,
            work_rejected: vec![],
            work_suggested: None,
            org_id: None,
        }
    }

    #[test]
    fn a_blocked_session_is_waiting_and_an_ordinary_one_is_nothing() {
        let mut r = row();
        assert_eq!(needs_attention(&r), None, "a working session needs nobody");

        r.claude_status = Some("blocked".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    #[test]
    fn a_session_waiting_on_a_form_needs_attention_while_working() {
        let mut r = row();
        assert_eq!(needs_attention(&r), None, "a working session needs nobody");
        r.pending_form = Some(crate::store::PendingForm {
            form_id: "f_x".into(),
            title: "T".into(),
        });
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

        // A tmux row's failed turn is a StopFailure (re-prompt); `Failed`
        // is the pane-less agent's verdict (see the F7 test below).
        let mut r = row();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::StopFailed);

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

    /// F7: the model flagged three ghosts a person can do nothing about and
    /// missed five context-red rows, two stale `working` rows, a 429 and
    /// four idle sessions with failing CI.
    #[test]
    fn a_failed_turn_a_full_context_a_stale_row_and_failing_ci_each_qualify() {
        let mut r = row();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::StopFailed);
        let mut r = row();
        r.kind = "bg".into();
        r.claude_status = Some("failed".into());
        assert_eq!(
            needs_attention(&r).unwrap().reason,
            Reason::Failed,
            "a bg agent's exit is `claude agents`' verdict"
        );

        let mut r = row();
        r.context_pct = Some(85.0);
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::ContextFull);
        assert_eq!(
            needs_attention_with(&r, 90.0),
            None,
            "the threshold is the hub's setting"
        );

        let mut r = row();
        r.claude_status = Some("idle".into());
        r.stale_working_at = Some(50);
        assert_eq!(
            needs_attention(&r).unwrap(),
            Attention {
                reason: Reason::StaleWorking,
                since: 50
            }
        );

        let mut r = row();
        r.claude_status = Some("idle".into());
        r.ci_status = Some("failing".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::CiFailing);
        let mut r = row();
        r.ci_status = Some("failing".into());
        assert_eq!(
            needs_attention(&r),
            None,
            "a working session may be fixing its CI"
        );

        let mut r = row();
        r.kind = "shell".into();
        r.context_pct = Some(99.0);
        assert_eq!(needs_attention(&r), None, "a shell has no Claude in it");
    }

    /// Every reason a person can act on outranks a broken lifecycle.
    #[test]
    fn every_actionable_reason_outranks_lifecycle() {
        for set in [
            (|r: &mut SessionRow| r.context_pct = Some(99.0)) as fn(&mut SessionRow),
            |r: &mut SessionRow| {
                r.claude_status = Some("idle".into());
                r.stale_working_at = Some(1);
            },
            |r: &mut SessionRow| {
                r.claude_status = Some("idle".into());
                r.ci_status = Some("failing".into());
            },
            |r: &mut SessionRow| r.claude_status = Some("failed".into()),
        ] {
            let mut r = row();
            set(&mut r);
            r.safe_kill_state = Some("failed".into());
            assert_ne!(needs_attention(&r).unwrap().reason, Reason::Lifecycle);
        }
    }

    /// A lost row keeps its last context reading and may keep a stale stamp;
    /// neither is anything a person can act on, so it reads `Lifecycle`.
    #[test]
    fn a_dead_rows_context_or_stale_stamp_reads_lifecycle() {
        for dead in [
            (|r: &mut SessionRow| r.status = "ghost".into()) as fn(&mut SessionRow),
            |r: &mut SessionRow| r.lost_at = Some(1),
        ] {
            let mut r = row();
            r.context_pct = Some(99.0);
            r.stale_working_at = Some(1);
            dead(&mut r);
            assert_eq!(needs_attention(&r).unwrap().reason, Reason::Lifecycle);
        }
    }

    /// The wire spellings are the desktop's bucket names; a rename on one
    /// side without the other would split the two classifiers silently.
    #[test]
    fn the_wire_spellings_match_the_desktop_buckets() {
        let ts = crate::repo_files::read("src/lib/attention.ts");
        for r in [
            Reason::Waiting,
            Reason::Stuck,
            Reason::StopFailed,
            Reason::Failed,
            Reason::ContextFull,
            Reason::StaleWorking,
            Reason::CiFailing,
            Reason::Lifecycle,
        ] {
            assert!(
                ts.contains(&format!("'{}'", r.as_str())),
                "src/lib/attention.ts does not name the bucket {}",
                r.as_str()
            );
        }
    }

    /// The shared attention table (redesign step 0.4): the desktop reads
    /// `src/lib/attention_states.json` at runtime, so it must say exactly
    /// what this module does, and its rows must classify the same here as
    /// in `attention.test.ts`.
    #[test]
    fn the_shared_attention_table_matches_this_module() {
        let fixture: serde_json::Value =
            serde_json::from_str(&crate::repo_files::read("src/lib/attention_states.json"))
                .unwrap();

        let states: Vec<(String, bool)> = fixture["states"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["id"].as_str().unwrap().to_string(),
                    s["counted"].as_bool().unwrap(),
                )
            })
            .collect();
        let ours: Vec<(String, bool)> = State::ALL
            .iter()
            .map(|s| (s.as_str().to_string(), s.counts_toward_badge()))
            .collect();
        assert_eq!(states, ours, "states in attention_states.json");

        let buckets: Vec<(String, String)> = fixture["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b[0].as_str().unwrap().to_string(),
                    b[1].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let ours: Vec<(String, String)> = BUCKET_STATES
            .iter()
            .map(|(b, s)| (b.to_string(), s.as_str().to_string()))
            .collect();
        assert_eq!(buckets, ours, "buckets in attention_states.json");

        // Each hub reason's state is its bucket's row in the table.
        let reasons = [
            Reason::Waiting,
            Reason::Stuck,
            Reason::StopFailed,
            Reason::Failed,
            Reason::ContextFull,
            Reason::StaleWorking,
            Reason::CiFailing,
            Reason::Lifecycle,
        ];
        for r in reasons {
            let row = BUCKET_STATES.iter().find(|(b, _)| *b == r.as_str());
            assert_eq!(row.map(|(_, s)| *s), Some(r.state()), "{}", r.as_str());
        }

        let base = serde_json::to_value(row()).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut v = base.clone();
            for (k, val) in case["row"].as_object().unwrap() {
                v[k] = val.clone();
            }
            let r: SessionRow = serde_json::from_value(v).unwrap_or_else(|e| panic!("{name}: {e}"));
            let state = state_with(&r, DEFAULT_CONTEXT_RED_PCT);
            assert_eq!(state.as_str(), case["state"].as_str().unwrap(), "{name}");
            assert_eq!(
                state.counts_toward_badge(),
                case["counted"].as_bool().unwrap(),
                "{name}"
            );
            // Where the hub decides the bucket, it is the same one.
            let reason =
                needs_attention_with(&r, DEFAULT_CONTEXT_RED_PCT).map(|a| a.reason.as_str());
            let bucket = case["bucket"].as_str().unwrap();
            if reasons.iter().any(|r| r.as_str() == bucket) {
                assert_eq!(reason, Some(bucket), "{name}");
            } else {
                assert_eq!(reason, None, "{name}");
            }
        }
    }
}
