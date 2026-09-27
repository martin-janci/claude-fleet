//! Work graph M13.2: the counts behind `work_admin { usage }`
//! (`service::work::usage`), read from rows the work graph already keeps.
//!
//! One statement per group, over a half-open window `[since, until)` in
//! unix seconds. Every statement returns counts, or values from a fixed
//! vocabulary (a link's `source`, an event's `kind`, the reason an auto-tidy
//! wrote into its detail) — never a title, key, path or free text.
//!
//! These are full scans of `work_links` and `session_events`: neither has an
//! index on the columns a window filters by, and an index there would tax
//! every write of two hot tables for a read an administrator runs by hand.
//! The scale fixture's budget test (`service/work/scale_tests.rs`) bounds
//! what that costs.

use super::Store;
use crate::ipc_error::IpcError;

/// Counts of the detection outcomes in a window (`work_links`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DetectionCounts {
    pub suggested: u64,
    pub confirmed_by_person: u64,
    /// Confirmed by an agent (a per-host token or the operator): source
    /// `agent` (D34).
    pub confirmed_by_agent: u64,
    pub promoted: u64,
    pub rejected: u64,
    pub expired: u64,
}

/// Counts of the journal rows in a window (`work_journal`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JournalCounts {
    pub briefs_queued: u64,
    pub briefs_delivered: u64,
    pub resume_briefs: u64,
    pub compact_summaries: u64,
}

/// `'a','b'` for a list of compile-time vocabulary words. Only lowercase
/// ASCII letters, digits and `_` pass, so nothing a caller supplies can reach
/// the SQL text.
fn sql_list(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| {
            assert!(
                !w.is_empty()
                    && w.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "not a vocabulary word: {w:?}"
            );
            format!("'{w}'")
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn count(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

impl Store {
    /// Links created in the window, per `source`.
    pub fn usage_link_sources(
        &self,
        since: i64,
        until: i64,
    ) -> Result<Vec<(String, u64)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT source, COUNT(*) FROM work_links
              WHERE created_at >= ?1 AND created_at < ?2
              GROUP BY source ORDER BY source",
        )?;
        let rows = stmt.query_map([since, until], |r| {
            Ok((r.get::<_, String>(0)?, count(r.get(1)?)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Detection outcomes in the window. A suggestion is a link a resolver
    /// rule made (`rule` set) that was not confirmed on the spot; a person's
    /// decision rewrites `source` to one of `person` and keeps the rule, an
    /// agent's to `agent`; a
    /// promotion keeps an `auto` source and is decided after it was made;
    /// an expired suggestion is one whose session ended undecided. Withdrawn
    /// and decayed suggestions are deleted, so `suggested` is a floor.
    pub fn usage_detection(
        &self,
        since: i64,
        until: i64,
        auto: &[&str],
        person: &[&str],
    ) -> Result<DetectionCounts, IpcError> {
        let (auto, person) = (sql_list(auto), sql_list(person));
        let sql = format!(
            "SELECT
               COALESCE(SUM(rule IS NOT NULL AND created_at >= ?1 AND created_at < ?2
                   AND NOT (state = 'confirmed' AND source IN ({auto})
                            AND decided_at = created_at)), 0),
               COALESCE(SUM(state = 'confirmed' AND source IN ({person}) AND rule IS NOT NULL
                   AND decided_at >= ?1 AND decided_at < ?2), 0),
               COALESCE(SUM(state = 'confirmed' AND source IN ({auto})
                   AND decided_at > created_at
                   AND decided_at >= ?1 AND decided_at < ?2), 0),
               COALESCE(SUM(state = 'rejected' AND rule IS NOT NULL
                   AND decided_at >= ?1 AND decided_at < ?2), 0),
               COALESCE(SUM(state = 'suggested' AND ended_at >= ?1 AND ended_at < ?2), 0),
               COALESCE(SUM(state = 'confirmed' AND source = 'agent' AND rule IS NOT NULL
                   AND decided_at >= ?1 AND decided_at < ?2), 0)
             FROM work_links"
        );
        Ok(self.conn.query_row(&sql, [since, until], |r| {
            Ok(DetectionCounts {
                suggested: count(r.get(0)?),
                confirmed_by_person: count(r.get(1)?),
                confirmed_by_agent: count(r.get(5)?),
                promoted: count(r.get(2)?),
                rejected: count(r.get(3)?),
                expired: count(r.get(4)?),
            })
        })?)
    }

    /// Seconds from suggestion to a person's decision, for every suggestion
    /// a person confirmed or rejected in the window, ascending.
    pub fn usage_decision_secs(
        &self,
        since: i64,
        until: i64,
        person: &[&str],
    ) -> Result<Vec<i64>, IpcError> {
        let sql = format!(
            "SELECT MAX(decided_at - created_at, 0) FROM work_links
              WHERE rule IS NOT NULL AND source IN ({})
                AND state IN ('confirmed', 'rejected')
                AND decided_at >= ?1 AND decided_at < ?2
              ORDER BY 1",
            sql_list(person)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([since, until], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Timeline events of `kinds` in the window, per kind.
    pub fn usage_event_kinds(
        &self,
        since: i64,
        until: i64,
        kinds: &[&str],
    ) -> Result<Vec<(String, u64)>, IpcError> {
        let sql = format!(
            "SELECT kind, COUNT(*) FROM session_events
              WHERE at >= ?1 AND at < ?2 AND kind IN ({})
              GROUP BY kind ORDER BY kind",
            sql_list(kinds)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([since, until], |r| {
            Ok((r.get::<_, String>(0)?, count(r.get(1)?)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// `gc_tidied` events in the window by who tidied: `("manual", n)` for
    /// Tidy-up's apply, `("auto:<reason>", n)` per auto-tidy reason (the
    /// second field of the event's `source:action:outcome` detail), and
    /// `("other", n)` for anything else. The caller checks each reason
    /// against the known list before it reports one.
    pub fn usage_tidied(&self, since: i64, until: i64) -> Result<Vec<(String, u64)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT CASE
                      WHEN detail LIKE 'manual:%' THEN 'manual'
                      WHEN detail LIKE 'auto:%' THEN
                        'auto:' || substr(detail, 6, instr(substr(detail, 6) || ':', ':') - 1)
                      ELSE 'other'
                    END AS who,
                    COUNT(*)
               FROM session_events
              WHERE kind = 'gc_tidied' AND at >= ?1 AND at < ?2
              GROUP BY who ORDER BY who",
        )?;
        let rows = stmt.query_map([since, until], |r| {
            Ok((r.get::<_, String>(0)?, count(r.get(1)?)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Briefs queued (`handover` rows) and delivered, the briefs a resume
    /// queued (their meta names the resumed `link_id`), and harvested
    /// compaction summaries, in the window.
    pub fn usage_journal(&self, since: i64, until: i64) -> Result<JournalCounts, IpcError> {
        Ok(self.conn.query_row(
            "SELECT
               COALESCE(SUM(kind = 'handover' AND at >= ?1 AND at < ?2), 0),
               COALESCE(SUM(kind = 'handover' AND delivered_at >= ?1 AND delivered_at < ?2), 0),
               COALESCE(SUM(kind = 'handover' AND at >= ?1 AND at < ?2
                   AND (CASE WHEN json_valid(meta)
                             THEN json_extract(meta, '$.link_id') END) IS NOT NULL), 0),
               COALESCE(SUM(kind = 'compact_summary' AND at >= ?1 AND at < ?2), 0)
             FROM work_journal
            WHERE kind IN ('handover', 'compact_summary')",
            [since, until],
            |r| {
                Ok(JournalCounts {
                    briefs_queued: count(r.get(0)?),
                    briefs_delivered: count(r.get(1)?),
                    resume_briefs: count(r.get(2)?),
                    compact_summaries: count(r.get(3)?),
                })
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vocabulary_list_is_quoted() {
        assert_eq!(sql_list(&["a_1", "pr"]), "'a_1','pr'");
    }

    #[test]
    #[should_panic(expected = "not a vocabulary word")]
    fn anything_else_is_refused_before_it_reaches_sql() {
        sql_list(&["x' OR 1=1 --"]);
    }
}
