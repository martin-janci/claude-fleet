//! `work_admin { action: usage, days? }` (work graph M13.2, decision D24):
//! how the work graph is actually used, over a window, as counts.
//!
//! Counted from rows the work graph already keeps (`work_links`,
//! `session_events`, `work_journal`) and the tracker sync's in-memory
//! totals. Nothing is recorded for it, nothing leaves the machine, and it
//! holds counts and ids only — never a title, key, path or free text: a
//! link's `source` and an auto-tidy's reason are reported only when they are
//! words of the known vocabulary, anything else as `other`. Master-only (the
//! `work_admin` tool), read-only.
//!
//! What the stored rows cannot tell is listed in [`UsageSummary::unrecorded`]
//! instead of being guessed; see `docs/work-graph.md` → *Usage summary*.

use crate::ipc_error::{codes, IpcError};
use crate::service::gc::tidy::TidyReason;
use crate::service::trackers::sync::SyncMetrics;
use crate::store::{Store, PERSON_SOURCES, WORK_SUGGESTION_WITHDRAWN};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `days` when none is given.
pub const DEFAULT_DAYS: u32 = 30;
/// The longest window.
pub const MAX_DAYS: u32 = 365;

/// Every link `source` the work graph writes. Anything else is `other`.
pub const KNOWN_SOURCES: &[&str] = &[
    "manual",
    "started",
    "agent",
    "agent_started",
    "branch",
    "pr",
    "trailer",
    "url",
    "prompt",
    "agent_inferred",
    "resumed",
    "forked",
    "inherited",
];

/// What the store does not record, so `usage` cannot count it.
pub const UNRECORDED: &[&str] = &[
    "suggestions shown (only made, confirmed, rejected and expired are stored)",
    "handovers refused as busy (a refusal writes nothing)",
    "resume mode last vs fresh (only resumes with a brief are told apart)",
    "transcript probe outcomes",
    "tidy suggestions per reason (the planner stores nothing until applied)",
    "multi-start runs and repos per run (each start is its own started link)",
];

const HANDOVER_KINDS: &[&str] = &[
    "handover_requested",
    "handover_written",
    "handover_missing",
    "handover_send_failed",
];
const OTHER_KINDS: &[&str] = &[
    "tidy_kept",
    "work_classify_nudge",
    WORK_SUGGESTION_WITHDRAWN,
];

/// The window, in days: [`DEFAULT_DAYS`] when absent, `1..=`[`MAX_DAYS`].
pub fn parse_days(days: Option<i64>) -> Result<u32, IpcError> {
    match days {
        None => Ok(DEFAULT_DAYS),
        Some(d) if (1..=i64::from(MAX_DAYS)).contains(&d) => Ok(d as u32),
        Some(d) => Err(IpcError::new(
            codes::E_INVALID,
            format!("days must be 1..={MAX_DAYS}, not {d}"),
        )),
    }
}

/// `work_admin { usage }`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub days: u32,
    /// The window, `[since, until)`, unix seconds.
    pub since: i64,
    pub until: i64,
    #[serde(default)]
    pub links: LinkUsage,
    #[serde(default)]
    pub detection: DetectionUsage,
    #[serde(default)]
    pub handover: HandoverUsage,
    #[serde(default)]
    pub resume: ResumeUsage,
    #[serde(default)]
    pub journal: JournalUsage,
    #[serde(default)]
    pub tidy: TidyUsage,
    /// Per tracker id, since the syncing process started (not windowed).
    #[serde(default)]
    pub trackers: Vec<TrackerUsage>,
    /// What cannot be counted from the stored rows.
    #[serde(default)]
    pub unrecorded: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkUsage {
    /// Links made in the window.
    #[serde(default)]
    pub created: u64,
    /// Per `source` (a suggestion a person decided reads `manual`, one an
    /// agent decided `agent`).
    #[serde(default)]
    pub by_source: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionUsage {
    /// Suggestions made: the stored ones made in the window, plus those
    /// detection withdrew or let decay in it (their row is gone; a timeline
    /// event counts them). A floor still: retention and the timeline's cap
    /// per session bound the events.
    #[serde(default)]
    pub suggested: u64,
    /// Decided by a person (`manual` / `started`).
    #[serde(default)]
    pub confirmed_by_person: u64,
    /// Confirmed by an agent — a per-host token or the operator (`agent`,
    /// D34); never counted as a person's.
    #[serde(default)]
    pub confirmed_by_agent: u64,
    /// Suggestions detection confirmed later by itself: a sole state
    /// candidate in a trusted project (R3) or a first prompt's sole ticket
    /// URL (R5).
    #[serde(default)]
    pub promoted: u64,
    #[serde(default)]
    pub rejected: u64,
    /// Suggestions detection took back undecided: its state signal moved on
    /// (R7) or an event suggestion decayed at a conversation boundary (R6).
    /// Counted from `work_suggestion_withdrawn` timeline events (D34).
    #[serde(default)]
    pub withdrawn: u64,
    /// Suggestions fleet settled by carrying the same work onto the
    /// session (a resume, a fork, a review or worker inheriting its
    /// parent's): `work_suggestion_withdrawn` events with reason `carried`.
    #[serde(default)]
    pub carried: u64,
    /// Suggestions whose session ended undecided.
    #[serde(default)]
    pub expired: u64,
    /// Median seconds from suggestion to a person's decision.
    #[serde(default)]
    pub median_decision_secs: Option<i64>,
    /// Classification nudges sent (M4.6).
    #[serde(default)]
    pub nudges: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoverUsage {
    #[serde(default)]
    pub requested: u64,
    #[serde(default)]
    pub written: u64,
    /// The turn ended without a handover in it, or a resume brief could not
    /// be queued.
    #[serde(default)]
    pub missing: u64,
    /// The request could not be sent to the session.
    #[serde(default)]
    pub send_failed: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeUsage {
    #[serde(default)]
    pub resumed: u64,
    /// Fresh with a brief.
    #[serde(default)]
    pub with_brief: u64,
    /// `last`, or fresh without a brief.
    #[serde(default)]
    pub without_brief: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalUsage {
    #[serde(default)]
    pub briefs_queued: u64,
    #[serde(default)]
    pub briefs_delivered: u64,
    #[serde(default)]
    pub compact_summaries: u64,
    /// Session summaries (`work_link { summarize }`).
    #[serde(default)]
    pub summaries: u64,
    /// PR links written back to a tracker.
    #[serde(default)]
    pub write_backs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyUsage {
    /// Applied from Tidy-up.
    #[serde(default)]
    pub applied: u64,
    /// "Keep" answers.
    #[serde(default)]
    pub kept: u64,
    #[serde(default)]
    pub auto_tidied: u64,
    /// Auto-tidies per reason.
    #[serde(default)]
    pub auto_by_reason: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerUsage {
    pub tracker_id: i64,
    #[serde(default)]
    pub passes: u64,
    #[serde(default)]
    pub passes_failed: u64,
    #[serde(default)]
    pub items_failed: u64,
}

fn known_or_other(word: String, known: impl Fn(&str) -> bool) -> String {
    if known(&word) {
        word
    } else {
        "other".to_string()
    }
}

/// The summary for the `days` before `now`. `metrics` is the sync's table
/// (`sync::metrics_for` in production). Reads only.
pub fn usage(
    s: &Store,
    days: u32,
    now: i64,
    metrics: &dyn Fn(&[i64]) -> Vec<SyncMetrics>,
) -> Result<UsageSummary, IpcError> {
    let until = now.saturating_add(1);
    let since = now.saturating_sub(i64::from(days) * 86_400);
    let auto = crate::service::work::resolve::AUTO_SOURCES;

    let mut links = LinkUsage::default();
    for (source, n) in s.usage_link_sources(since, until)? {
        links.created += n;
        let key = known_or_other(source, |w| KNOWN_SOURCES.contains(&w));
        *links.by_source.entry(key).or_default() += n;
    }

    let d = s.usage_detection(since, until, auto, PERSON_SOURCES)?;
    let secs = s.usage_decision_secs(since, until, PERSON_SOURCES)?;
    let events: BTreeMap<String, u64> = s
        .usage_event_kinds(since, until, &[HANDOVER_KINDS, OTHER_KINDS].concat())?
        .into_iter()
        .collect();
    let ev = |k: &str| events.get(k).copied().unwrap_or(0);
    // Withdrawn, decayed and carried suggestions have no row left: their
    // event is the only trace, so they are made AND settled here.
    let gone = ev(WORK_SUGGESTION_WITHDRAWN);
    let carried = s.usage_carried_suggestions(since, until)?.min(gone);
    let detection = DetectionUsage {
        suggested: d.suggested + gone,
        confirmed_by_person: d.confirmed_by_person,
        confirmed_by_agent: d.confirmed_by_agent,
        promoted: d.promoted,
        rejected: d.rejected,
        withdrawn: gone - carried,
        carried,
        expired: d.expired,
        median_decision_secs: median(&secs),
        nudges: ev("work_classify_nudge"),
    };
    let handover = HandoverUsage {
        requested: ev("handover_requested"),
        written: ev("handover_written"),
        missing: ev("handover_missing"),
        send_failed: ev("handover_send_failed"),
    };

    let j = s.usage_journal(since, until)?;
    let resumed = links.by_source.get("resumed").copied().unwrap_or(0);
    let resume = ResumeUsage {
        resumed,
        with_brief: j.resume_briefs,
        without_brief: resumed.saturating_sub(j.resume_briefs),
    };
    let journal = JournalUsage {
        briefs_queued: j.briefs_queued,
        briefs_delivered: j.briefs_delivered,
        compact_summaries: j.compact_summaries,
        summaries: j.summaries,
        write_backs: j.write_backs,
    };

    let mut tidy = TidyUsage {
        kept: ev("tidy_kept"),
        ..Default::default()
    };
    for (who, n) in s.usage_tidied(since, until)? {
        if who == "manual" {
            tidy.applied += n;
        } else if let Some(reason) = who.strip_prefix("auto:") {
            tidy.auto_tidied += n;
            let key = known_or_other(reason.to_string(), |w| TidyReason::parse(w).is_some());
            *tidy.auto_by_reason.entry(key).or_default() += n;
        }
    }

    let ids: Vec<i64> = s.list_trackers()?.iter().map(|t| t.id).collect();
    let trackers = metrics(&ids)
        .into_iter()
        .map(|m| TrackerUsage {
            tracker_id: m.tracker_id,
            passes: m.passes_total,
            passes_failed: m.passes_failed_total,
            items_failed: m.items_failed_total,
        })
        .collect();

    Ok(UsageSummary {
        days,
        since,
        until,
        links,
        detection,
        handover,
        resume,
        journal,
        tidy,
        trackers,
        unrecorded: UNRECORDED.iter().map(|u| u.to_string()).collect(),
    })
}

/// The middle value of an ascending list (the lower of the two middles).
fn median(sorted: &[i64]) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    Some(sorted[(sorted.len() - 1) / 2])
}

/// "4 min", "3 h", "2 d".
fn duration(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s} s"),
        s if s < 7_200 => format!("{} min", s / 60),
        s if s < 172_800 => format!("{} h", s / 3_600),
        s => format!("{} d", s / 86_400),
    }
}

fn pairs(m: &BTreeMap<String, u64>) -> String {
    if m.is_empty() {
        return "none".into();
    }
    m.iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl UsageSummary {
    /// The plain-text form: `fleet-hub work usage`, a run record. One line
    /// per group; counts and ids only.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("work graph usage, last {} d", self.days)];
        out.extend(self.rows().into_iter().map(|(g, v)| format!("{g}: {v}")));
        out
    }

    /// One `(group, what it counted)` pair per line of [`Self::lines`]
    /// after its header: the rows of the `work.usage` data source, so the
    /// desktop's page and the CLI say the same words.
    pub fn rows(&self) -> Vec<(String, String)> {
        let d = &self.detection;
        let mut out: Vec<(String, String)> = vec![
            (
                "links".into(),
                format!(
                    "{} made ({})",
                    self.links.created,
                    pairs(&self.links.by_source)
                ),
            ),
            (
                "detection".into(),
                format!(
                    "{} suggested, {} confirmed by a person, {} confirmed by an agent, \
                     {} promoted, {} rejected, {} withdrawn, {} carried, {} expired; \
                     median decision {}; {} nudges",
                    d.suggested,
                    d.confirmed_by_person,
                    d.confirmed_by_agent,
                    d.promoted,
                    d.rejected,
                    d.withdrawn,
                    d.carried,
                    d.expired,
                    d.median_decision_secs.map_or("n/a".into(), duration),
                    d.nudges
                ),
            ),
            (
                "handover".into(),
                format!(
                    "{} requested, {} written, {} missing, {} send failed",
                    self.handover.requested,
                    self.handover.written,
                    self.handover.missing,
                    self.handover.send_failed
                ),
            ),
            (
                "resume".into(),
                format!(
                    "{} ({} with a brief, {} without)",
                    self.resume.resumed, self.resume.with_brief, self.resume.without_brief
                ),
            ),
            (
                "journal".into(),
                format!(
                    "{} briefs queued, {} delivered; {} compaction summaries, \
                     {} session summaries, {} PR links written",
                    self.journal.briefs_queued,
                    self.journal.briefs_delivered,
                    self.journal.compact_summaries,
                    self.journal.summaries,
                    self.journal.write_backs
                ),
            ),
            (
                "tidy".into(),
                format!(
                    "{} applied, {} kept, {} auto-tidied ({})",
                    self.tidy.applied,
                    self.tidy.kept,
                    self.tidy.auto_tidied,
                    pairs(&self.tidy.auto_by_reason)
                ),
            ),
        ];
        if self.trackers.is_empty() {
            out.push(("trackers".into(), "none".into()));
        }
        for t in &self.trackers {
            out.push((
                format!("tracker {}", t.tracker_id),
                format!(
                    "{} passes, {} failed, {} items skipped (since the sync started)",
                    t.passes, t.passes_failed, t.items_failed
                ),
            ));
        }
        for u in &self.unrecorded {
            out.push(("not recorded".into(), u.clone()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WorkTarget;

    const NOW: i64 = 100 * 86_400;

    fn no_metrics(ids: &[i64]) -> Vec<SyncMetrics> {
        ids.iter()
            .map(|&id| SyncMetrics {
                tracker_id: id,
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn days_default_to_30_and_stay_within_a_year() {
        assert_eq!(parse_days(None).unwrap(), 30);
        assert_eq!(parse_days(Some(1)).unwrap(), 1);
        assert_eq!(parse_days(Some(365)).unwrap(), 365);
        for bad in [0, -1, 366] {
            assert_eq!(parse_days(Some(bad)).unwrap_err().code, codes::E_INVALID);
        }
    }

    #[test]
    fn the_median_is_the_middle_and_none_for_nothing() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[5]), Some(5));
        assert_eq!(median(&[1, 2, 9]), Some(2));
        assert_eq!(median(&[1, 2, 8, 9]), Some(2));
    }

    fn set(s: &Store, sql: &str, p: &[&dyn rusqlite::ToSql]) {
        s.conn_for_test().execute(sql, p).unwrap();
    }

    /// Every group counted from rows, inside the window only, and nothing
    /// but counts and ids in the answer.
    #[test]
    fn it_counts_each_group_inside_the_window_and_carries_no_text() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let sid = s
            .upsert_session("SECRET-TMUX", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let in_window = NOW - 86_400;
        let old = NOW - 90 * 86_400;
        let link = |key: &str, source: &str| {
            s.link_session_work(sid, WorkTarget::Key(key), source)
                .unwrap()
                .id
        };
        // A manual link and a start, now.
        let manual = link("SECRET-1", "manual");
        let started = link("SECRET-2", "started");
        // A suggestion a person confirmed after an hour.
        let confirmed = link("SECRET-3", "manual");
        // One detection promoted by itself, one rejected, one still open
        // when its session ended, one auto-confirmed on the spot.
        let promoted = link("SECRET-4", "manual");
        let rejected = link("SECRET-5", "manual");
        let expired = link("SECRET-6", "manual");
        let spot = link("SECRET-7", "manual");
        // A link from long ago: outside the window.
        let ancient = link("SECRET-8", "manual");
        // A suggestion an agent confirmed after two hours (D34): not a
        // person's decision, and not in the person's median.
        let by_agent = link("SECRET-9", "manual");
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = ?1 + 7200, rule = 'R3b', source = 'agent' WHERE id = ?2", &[&in_window, &by_agent]);
        for id in [manual, started] {
            set(
                &s,
                "UPDATE work_links SET created_at = ?1, decided_at = ?1 WHERE id = ?2",
                &[&in_window, &id],
            );
        }
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = ?1 + 3600, rule = 'R4' WHERE id = ?2", &[&in_window, &confirmed]);
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = ?1 + 60, rule = 'R2', source = 'branch' WHERE id = ?2", &[&in_window, &promoted]);
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = ?1 + 600, rule = 'R3', state = 'rejected' WHERE id = ?2", &[&in_window, &rejected]);
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = NULL, rule = 'R6', source = 'prompt', state = 'suggested', ended_at = ?1 + 5 WHERE id = ?2", &[&in_window, &expired]);
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = ?1, rule = 'R2', source = 'pr' WHERE id = ?2", &[&in_window, &spot]);
        set(&s, "UPDATE work_links SET created_at = ?1, decided_at = ?1, source = 'resumed' WHERE id = ?2", &[&old, &ancient]);
        // Timeline events: handovers, a keep, a nudge, tidies (manual, two
        // auto reasons, one reason nobody knows), one event far outside.
        for (kind, detail, at) in [
            ("handover_requested", Some("n 1"), in_window),
            ("handover_requested", Some("n 2"), in_window),
            ("handover_written", None, in_window),
            ("handover_send_failed", Some("SECRET-ERR"), in_window),
            ("tidy_kept", Some("123"), in_window),
            ("work_classify_nudge", None, in_window),
            ("gc_tidied", Some("manual:archive:archived"), in_window),
            (
                "gc_tidied",
                Some("auto:done_idle:safe_kill:safe_kill_requested"),
                in_window,
            ),
            (
                "gc_tidied",
                Some("auto:done_idle:archive:archived"),
                in_window,
            ),
            (
                "gc_tidied",
                Some("auto:SECRET-REASON:kill:killed"),
                in_window,
            ),
            // Two suggestions detection took back (their rows are gone),
            // one long ago.
            (
                "work_suggestion_withdrawn",
                Some(r#"{"link_id":90,"item_id":null,"rule":"R3b","reason":"withdraw"}"#),
                in_window,
            ),
            (
                "work_suggestion_withdrawn",
                Some(r#"{"link_id":91,"item_id":4,"rule":"R6","reason":"decay"}"#),
                in_window,
            ),
            // One a resume's carry settled (not taken back).
            (
                "work_suggestion_withdrawn",
                Some(r#"{"link_id":92,"item_id":4,"rule":"R6","reason":"carried"}"#),
                in_window,
            ),
            ("work_suggestion_withdrawn", None, old),
            ("handover_requested", None, old),
        ] {
            set(
                &s,
                "INSERT INTO session_events (session_id, at, kind, detail) VALUES (?1, ?2, ?3, ?4)",
                &[&sid, &at, &kind, &detail],
            );
        }
        // Journal: a start brief delivered, a resume brief undelivered, two
        // compaction summaries (one outside), a session summary and a
        // write-back in the window and one of each outside.
        for (kind, meta, at, delivered) in [
            (
                "handover",
                r#"{"key":"SECRET-1","source":"start"}"#,
                in_window,
                Some(in_window + 9),
            ),
            (
                "handover",
                r#"{"key":"SECRET-1","link_id":7}"#,
                in_window,
                None,
            ),
            ("handover", "not json", in_window, None),
            ("compact_summary", "{}", in_window, None),
            ("compact_summary", "{}", old, None),
            ("summary", "{}", in_window, None),
            ("summary", "{}", old, None),
            ("write_back", r#"{"key":"SECRET-1"}"#, in_window, None),
            ("write_back", r#"{"key":"SECRET-1"}"#, old, None),
        ] {
            set(
                &s,
                "INSERT INTO work_journal (claude_session_id, at, kind, source, body, meta, delivered_at) \
                 VALUES ('c', ?1, ?2, 'fleet', 'SECRET-BODY', ?3, ?4)",
                &[&at, &kind, &meta, &delivered],
            );
        }
        let t = s
            .add_tracker("jira", "SECRET-TRACKER", "https://secret.atlassian.net")
            .unwrap();
        let metrics = move |ids: &[i64]| -> Vec<SyncMetrics> {
            ids.iter()
                .map(|&id| SyncMetrics {
                    tracker_id: id,
                    passes_total: 12,
                    passes_failed_total: 2,
                    items_failed_total: 5,
                    last_error: Some("SECRET-ERROR".into()),
                    ..Default::default()
                })
                .collect()
        };

        let u = usage(&s, 30, NOW, &metrics).unwrap();
        assert_eq!((u.days, u.until - u.since), (30, 30 * 86_400 + 1));
        assert_eq!(u.links.created, 8, "{:?}", u.links);
        assert_eq!(
            u.links.by_source,
            BTreeMap::from([
                ("agent".into(), 1),
                ("branch".into(), 1),
                ("manual".into(), 3),
                ("pr".into(), 1),
                ("prompt".into(), 1),
                ("started".into(), 1),
            ])
        );
        assert_eq!(
            u.detection,
            DetectionUsage {
                // confirmed (by a person and by an agent), promoted,
                // rejected, expired — not the spot one — the two
                // withdrawn and the carried one.
                suggested: 8,
                confirmed_by_person: 1,
                confirmed_by_agent: 1,
                promoted: 1,
                rejected: 1,
                withdrawn: 2,
                carried: 1,
                expired: 1,
                // the person's two decisions: 600 s and 3600 s (the
                // agent's 7200 s is not a person's).
                median_decision_secs: Some(600),
                nudges: 1,
            }
        );
        assert_eq!(
            u.handover,
            HandoverUsage {
                requested: 2,
                written: 1,
                missing: 0,
                send_failed: 1
            }
        );
        assert_eq!(
            u.journal,
            JournalUsage {
                briefs_queued: 3,
                briefs_delivered: 1,
                compact_summaries: 1,
                summaries: 1,
                write_backs: 1
            }
        );
        assert_eq!(
            u.resume,
            ResumeUsage {
                resumed: 0,
                with_brief: 1,
                without_brief: 0
            }
        );
        assert_eq!(u.tidy.applied, 1);
        assert_eq!(u.tidy.kept, 1);
        assert_eq!(u.tidy.auto_tidied, 3);
        assert_eq!(
            u.tidy.auto_by_reason,
            BTreeMap::from([("done_idle".into(), 2), ("other".into(), 1)])
        );
        assert_eq!(
            u.trackers,
            vec![TrackerUsage {
                tracker_id: t.id,
                passes: 12,
                passes_failed: 2,
                items_failed: 5
            }]
        );
        assert_eq!(u.unrecorded.len(), UNRECORDED.len());

        assert!(
            u.lines().contains(
                &"journal: 3 briefs queued, 1 delivered; 1 compaction summaries, \
                  1 session summaries, 1 PR links written"
                    .to_string()
            ),
            "{:?}",
            u.lines()
        );

        // Counts and ids only: no title, key, name, detail, body or error.
        let json = serde_json::to_string(&u).unwrap();
        let text = u.lines().join("\n");
        for body in [&json, &text] {
            assert!(!body.contains("SECRET"), "{body}");
        }

        // A longer window reaches the old rows too.
        let year = usage(&s, 365, NOW, &no_metrics).unwrap();
        assert_eq!(year.links.by_source.get("resumed"), Some(&1));
        assert_eq!(year.handover.requested, 3);
        assert_eq!(year.journal.compact_summaries, 2);
        assert_eq!((year.journal.summaries, year.journal.write_backs), (2, 2));
    }

    #[test]
    fn an_empty_store_reads_all_zero_and_says_what_is_not_recorded() {
        let s = Store::open_in_memory().unwrap();
        let u = usage(&s, DEFAULT_DAYS, NOW, &no_metrics).unwrap();
        assert_eq!(u.links, LinkUsage::default());
        assert_eq!(u.detection.median_decision_secs, None);
        assert!(u.trackers.is_empty());
        let text = u.lines();
        assert_eq!(text[0], "work graph usage, last 30 d");
        assert!(text.contains(&"trackers: none".to_string()), "{text:?}");
        assert!(
            text.iter()
                .any(|l| l.starts_with("not recorded: handovers refused as busy")),
            "{text:?}"
        );
        // Additive on the wire: a sparse answer parses.
        let back: UsageSummary = serde_json::from_str(r#"{"days":7,"since":1,"until":2}"#).unwrap();
        assert_eq!(back.days, 7);
    }

    #[test]
    fn a_duration_reads_in_the_largest_whole_unit() {
        assert_eq!(duration(59), "59 s");
        assert_eq!(duration(600), "10 min");
        assert_eq!(duration(3 * 3600), "3 h");
        assert_eq!(duration(5 * 86_400), "5 d");
    }
}
