//! Token-usage cursors, per-session totals and the daily roll-up.

use super::*;

impl Store {
    /// Usage cursors of the live sessions on `host_alias` that have a Claude
    /// session id (migration 025), least recently updated first: a host's
    /// per-pass byte budget goes to the stalest sessions, and a session that
    /// just made progress moves to the back, so a large backlog rotates.
    pub fn list_usage_cursors(
        &self,
        host_alias: &str,
    ) -> Result<Vec<UsageCursor>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {USAGE_CURSOR_COLUMNS} FROM sessions \
             WHERE host_alias = ?1 AND lost_at IS NULL AND claude_session_id IS NOT NULL \
             ORDER BY COALESCE(usage_updated_at, 0), id"
        ))?;
        let rows = stmt.query_map(rusqlite::params![host_alias], map_usage_cursor)?;
        rows.collect()
    }

    /// A moved session (#57 `move_session`) keeps the source's Claude session
    /// id and gets a whole-line byte prefix of its transcript. Start the
    /// target's usage cursor where the source's stands, so the copied history
    /// is not counted again: `usage_source` = `<claude id>.jsonl`; the offset
    /// is the source's (capped at `copied_bytes`) when the source was reading
    /// that same file, else `copied_bytes`; the last message id and its
    /// counted usage come from the source. `usage_daily` is untouched.
    /// Returns false (no-op) when the source row or its Claude id is missing.
    pub fn inherit_usage_cursor(
        &self,
        target_id: i64,
        source_id: i64,
        copied_bytes: i64,
    ) -> Result<bool, rusqlite::Error> {
        type Src = (
            Option<String>,
            Option<String>,
            i64,
            Option<String>,
            Option<String>,
            i64,
        );
        let src: Option<Src> = self
            .conn
            .query_row(
                "SELECT claude_session_id, usage_source, usage_offset_bytes, usage_last_msg_id, \
                 usage_last_msg_usage, usage_backfill_until FROM sessions WHERE id = ?1",
                rusqlite::params![source_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((Some(claude_id), source_file, offset, last_id, last_usage, backfill_until)) = src
        else {
            return Ok(false);
        };
        let file = format!("{claude_id}.jsonl");
        let copied = copied_bytes.max(0);
        let offset = if source_file.as_deref() == Some(file.as_str()) {
            offset.clamp(0, copied)
        } else {
            copied
        };
        // The source's backfill mark (history still to read) carries over
        // when it was reading this file, capped like the offset.
        let until = if source_file.as_deref() == Some(file.as_str()) {
            backfill_until.clamp(0, copied)
        } else {
            0
        };
        let n = self.conn.execute(
            "UPDATE sessions SET usage_source = ?1, usage_offset_bytes = ?2, \
             usage_last_msg_id = ?3, usage_last_msg_usage = ?4, usage_backfill_until = ?6 \
             WHERE id = ?5",
            rusqlite::params![file, offset, last_id, last_usage, target_id, until],
        )?;
        Ok(n == 1)
    }

    /// Add the source's lifetime usage totals to the target (a move that
    /// KILLS the source, so the spend follows the session). Never call it
    /// under `keep_source`: both rows would then report the same spend.
    /// A missing source or `target == source` is a no-op. `usage_daily` is
    /// untouched (that spend was already bucketed).
    #[cfg(test)]
    pub fn carry_usage_totals(
        &self,
        target_id: i64,
        source_id: i64,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        if target_id == source_id {
            return fetch_session_by_id(&self.conn, target_id);
        }
        match fetch_session_by_id(&self.conn, source_id)? {
            Some(src) => self.add_usage_totals(target_id, &src.usage),
            None => fetch_session_by_id(&self.conn, target_id),
        }
    }

    /// One session's usage cursor (same shape as `list_usage_cursors`),
    /// whether or not the row is live. `None` for an unknown id.
    pub fn usage_cursor(&self, id: i64) -> Result<Option<UsageCursor>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {USAGE_CURSOR_COLUMNS} FROM sessions WHERE id = ?1"),
                rusqlite::params![id],
                map_usage_cursor,
            )
            .optional()
    }

    /// Close a move's window: a usage pass on the source between
    /// `inherit_usage_cursor` and the kill counted lines the target's
    /// inherited cursor still points before. Raise the target's cursor to
    /// the source's pre-kill snapshot when both read the same transcript
    /// file name and the source got further (never lowers it). Returns
    /// whether the cursor moved.
    pub fn raise_usage_cursor(
        &self,
        target_id: i64,
        src: &UsageCursor,
    ) -> Result<bool, rusqlite::Error> {
        let Some(file) = src.source.as_deref() else {
            return Ok(false);
        };
        let n = self.conn.execute(
            "UPDATE sessions SET usage_offset_bytes = ?2, usage_last_msg_id = ?3, \
             usage_last_msg_usage = ?4, \
             usage_backfill_until = MAX(usage_backfill_until, ?6) \
             WHERE id = ?1 AND usage_source = ?5 AND usage_offset_bytes < ?2",
            rusqlite::params![
                target_id,
                src.offset_bytes,
                src.last_msg_id,
                src.last_msg_usage,
                file,
                src.backfill_until
            ],
        )?;
        Ok(n == 1)
    }

    /// Add a usage snapshot (taken from a row that is about to be killed) to
    /// `target_id`'s totals; the model and `usage_updated_at` fill in when
    /// the target has none / an older one. `usage_daily` is untouched. Emits
    /// `session_updated` for the target.
    pub fn add_usage_totals(
        &self,
        target_id: i64,
        u: &SessionUsage,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET \
             usage_input_tokens = usage_input_tokens + ?1, \
             usage_output_tokens = usage_output_tokens + ?2, \
             usage_cache_write_tokens = usage_cache_write_tokens + ?3, \
             usage_cache_read_tokens = usage_cache_read_tokens + ?4, \
             usage_cost_micros = usage_cost_micros + ?5, \
             usage_model = COALESCE(usage_model, ?6), \
             usage_updated_at = CASE WHEN ?7 IS NULL THEN usage_updated_at \
                                     ELSE MAX(COALESCE(usage_updated_at, 0), ?7) END \
             WHERE id = ?8",
            rusqlite::params![
                u.usage_input_tokens,
                u.usage_output_tokens,
                u.usage_cache_write_tokens,
                u.usage_cache_read_tokens,
                u.usage_cost_micros,
                u.usage_model,
                u.usage_updated_at,
                target_id
            ],
        )?;
        self.emit_session(target_id)
    }

    /// Apply one usage pass to a session: add (or, on `reset`, replace) the
    /// totals, move the cursor, and add the growth to the host's
    /// `usage_daily` bucket for the UTC day of `now`. Returns whether the
    /// totals or model changed; only then is `usage_updated_at` stamped and
    /// `session_updated` emitted (a pass that only moves the offset is
    /// silent). An unknown session id is a no-op. Opens its own transaction;
    /// a caller that already holds one (usage collection's per-host batch)
    /// uses [`Store::apply_usage_in_tx`] instead.
    pub fn apply_usage(
        &self,
        session_id: i64,
        host_alias: &str,
        d: &UsageDelta,
    ) -> Result<bool, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = self.apply_usage_body(session_id, host_alias, d)?;
        tx.commit()?;
        if changed {
            self.emit_session(session_id)?;
        }
        Ok(changed)
    }

    /// Same as [`Store::apply_usage`] but assumes the caller already holds a
    /// transaction — usage collection's per-host batch, run inside
    /// [`Store::atomically`], where a second `BEGIN` on the same connection
    /// would error. Still emits `session_updated` when the row changed:
    /// `atomically` holds bus events until its own commit, so this never
    /// announces a write that later rolls back.
    pub fn apply_usage_in_tx(
        &self,
        session_id: i64,
        host_alias: &str,
        d: &UsageDelta,
    ) -> Result<bool, rusqlite::Error> {
        let changed = self.apply_usage_body(session_id, host_alias, d)?;
        if changed {
            self.emit_session(session_id)?;
        }
        Ok(changed)
    }

    /// Shared body of [`Store::apply_usage`] / [`Store::apply_usage_in_tx`]:
    /// writes without opening or committing a transaction. A pass whose
    /// token totals and model are unchanged AND whose offset does not move
    /// writes nothing — there is no new byte read to account for and no
    /// cursor progress to persist, so neither the row nor `usage_daily` is
    /// touched (and `usage_last_msg_id` / `usage_source` are not clobbered
    /// with the empty values a no-growth read reports).
    fn apply_usage_body(
        &self,
        session_id: i64,
        host_alias: &str,
        d: &UsageDelta,
    ) -> Result<bool, rusqlite::Error> {
        let old: Option<(UsageTotals, Option<String>, i64)> = self
            .conn
            .query_row(
                "SELECT usage_input_tokens, usage_output_tokens, usage_cache_write_tokens, \
                 usage_cache_read_tokens, usage_cost_micros, usage_model, usage_offset_bytes \
                 FROM sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |r| {
                    Ok((
                        UsageTotals {
                            input_tokens: r.get(0)?,
                            output_tokens: r.get(1)?,
                            cache_write_tokens: r.get(2)?,
                            cache_read_tokens: r.get(3)?,
                            cost_micros: r.get(4)?,
                        },
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((before, old_model, old_offset)) = old else {
            return Ok(false);
        };
        let after = if d.reset {
            d.totals
        } else {
            let mut t = before;
            t.add(&d.totals);
            t
        };
        let daily = if d.reset {
            after.growth_since(&before)
        } else {
            d.totals
        };
        let model = d.model.clone().or_else(|| old_model.clone());
        let changed = after != before || model != old_model;
        let new_offset = d.offset.max(0);
        if !changed && new_offset == old_offset {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE sessions SET usage_input_tokens = ?1, usage_output_tokens = ?2, \
             usage_cache_write_tokens = ?3, usage_cache_read_tokens = ?4, usage_cost_micros = ?5, \
             usage_model = ?6, usage_offset_bytes = ?7, usage_source = ?8, usage_last_msg_id = ?9, \
             usage_updated_at = CASE WHEN ?10 THEN ?11 ELSE usage_updated_at END, \
             usage_last_msg_usage = ?13, \
             usage_backfill_until = COALESCE(?14, usage_backfill_until) WHERE id = ?12",
            rusqlite::params![
                after.input_tokens,
                after.output_tokens,
                after.cache_write_tokens,
                after.cache_read_tokens,
                after.cost_micros,
                model,
                new_offset,
                d.source,
                d.last_msg_id,
                changed,
                d.now,
                session_id,
                d.last_msg_usage,
                d.backfill_until
            ],
        )?;
        // A reader without `D` lines books everything to the day of `now`.
        // A rewritten file (`reset`) has one growth figure over what was
        // already counted: it is split over the pass's day slices by
        // [`split_reset_growth`], so a rewrite's history lands in backfill
        // rows like a first read's. Otherwise each day's slice goes to its
        // own row, history from a fresh cursor to the backfill row.
        let today = d.now.div_euclid(86_400);
        // Org administration phase C: the same slices again, keyed by the
        // session's org as it is now, for the org's spend and budget.
        let org: Option<i64> = self.conn.query_row(
            concat!(
                "SELECT ",
                crate::session_org_sql!("s"),
                " FROM sessions s WHERE s.id = ?1"
            ),
            rusqlite::params![session_id],
            |r| r.get(0),
        )?;
        let book = |day: i64, backfill: bool, t: &UsageTotals| -> Result<(), rusqlite::Error> {
            self.add_usage_daily(day, host_alias, backfill, t)?;
            if let Some(org) = org {
                self.add_usage_daily_org(day, org, backfill, t)?;
            }
            Ok(())
        };
        if d.by_day.is_empty() {
            if !daily.is_zero() {
                book(today, false, &daily)?;
            }
        } else if d.reset {
            for (day, backfill, t) in split_reset_growth(daily, &d.by_day, today) {
                book(day, backfill, &t)?;
            }
        } else {
            for slice in &d.by_day {
                if !slice.totals.is_zero() {
                    book(slice.day, slice.backfill, &slice.totals)?;
                }
            }
        }
        Ok(changed)
    }

    /// Add `t` to one `usage_daily_org` row (migration 106), keyed `(day,
    /// org_id, backfill)`.
    fn add_usage_daily_org(
        &self,
        day: i64,
        org_id: i64,
        backfill: bool,
        t: &UsageTotals,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO usage_daily_org (day, org_id, backfill, input_tokens, output_tokens, \
             cache_write_tokens, cache_read_tokens, cost_micros) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(day, org_id, backfill) DO UPDATE SET \
             input_tokens = input_tokens + excluded.input_tokens, \
             output_tokens = output_tokens + excluded.output_tokens, \
             cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens, \
             cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens, \
             cost_micros = cost_micros + excluded.cost_micros",
            rusqlite::params![
                day,
                org_id,
                backfill as i64,
                t.input_tokens,
                t.output_tokens,
                t.cache_write_tokens,
                t.cache_read_tokens,
                t.cost_micros
            ],
        )?;
        Ok(())
    }

    /// Each org's LIVE spend (backfill rows left out: that is history a
    /// first read booked, not spend in the window) in micro-USD, summed over
    /// `since_day..` (UTC day numbers): `org id → cost_micros`.
    pub fn org_live_cost_since(
        &self,
        since_day: i64,
    ) -> Result<std::collections::BTreeMap<i64, i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT org_id, SUM(cost_micros) FROM usage_daily_org \
             WHERE day >= ?1 AND backfill = 0 GROUP BY org_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![since_day], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    /// One org's LIVE spend per UTC day over `since_day..`, in micro-USD:
    /// `day → cost_micros`, days with no row left out.
    pub fn org_live_cost_by_day(
        &self,
        org: i64,
        since_day: i64,
    ) -> Result<std::collections::BTreeMap<i64, i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT day, SUM(cost_micros) FROM usage_daily_org \
             WHERE org_id = ?1 AND day >= ?2 AND backfill = 0 GROUP BY day",
        )?;
        let rows = stmt.query_map(rusqlite::params![org, since_day], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    /// Add `t` to one `usage_daily` row, keyed `(day, host_alias, backfill)`.
    fn add_usage_daily(
        &self,
        day: i64,
        host_alias: &str,
        backfill: bool,
        t: &UsageTotals,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO usage_daily (day, host_alias, backfill, input_tokens, output_tokens, \
             cache_write_tokens, cache_read_tokens, cost_micros) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(day, host_alias, backfill) DO UPDATE SET \
             input_tokens = input_tokens + excluded.input_tokens, \
             output_tokens = output_tokens + excluded.output_tokens, \
             cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens, \
             cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens, \
             cost_micros = cost_micros + excluded.cost_micros",
            rusqlite::params![
                day,
                host_alias,
                backfill as i64,
                t.input_tokens,
                t.output_tokens,
                t.cache_write_tokens,
                t.cache_read_tokens,
                t.cost_micros
            ],
        )?;
        Ok(())
    }

    /// `usage_daily` rows with `day >= since_day` (UTC day numbers),
    /// optionally for one host, oldest first: `(day, host_alias, backfill,
    /// totals)`, the live row (`backfill = false`) and the backfill row of a
    /// day beside each other.
    pub fn usage_daily_since(
        &self,
        since_day: i64,
        host_alias: Option<&str>,
    ) -> Result<Vec<(i64, String, bool, UsageTotals)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT day, host_alias, backfill, input_tokens, output_tokens, cache_write_tokens, \
             cache_read_tokens, cost_micros FROM usage_daily \
             WHERE day >= ?1 AND (?2 IS NULL OR host_alias = ?2) \
             ORDER BY day, host_alias, backfill",
        )?;
        let rows = stmt.query_map(rusqlite::params![since_day, host_alias], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)? != 0,
                UsageTotals {
                    input_tokens: r.get(3)?,
                    output_tokens: r.get(4)?,
                    cache_write_tokens: r.get(5)?,
                    cache_read_tokens: r.get(6)?,
                    cost_micros: r.get(7)?,
                },
            ))
        })?;
        rows.collect()
    }
}

/// Book a rewrite's growth (`growth`: field-wise new totals minus what the
/// row already held) over the pass's day slices. Live slices take their
/// share first, newest day first, so today's real spend stays live; what is
/// left goes to the backfill slices, newest first; any remainder (rounding
/// between the per-model and per-day prices) to today's live row. No slice
/// takes more of a field than it counted. Returns the non-zero
/// `(day, backfill, totals)` bookings.
fn split_reset_growth(
    growth: UsageTotals,
    slices: &[DayDelta],
    today: i64,
) -> Vec<(i64, bool, UsageTotals)> {
    let mut order: Vec<&DayDelta> = slices.iter().collect();
    order.sort_by_key(|s| (s.backfill, std::cmp::Reverse(s.day)));
    let mut left = growth;
    let mut out = Vec::new();
    for s in order {
        let take = |left: &mut i64, want: i64| {
            let t = (*left).min(want.max(0)).max(0);
            *left -= t;
            t
        };
        let t = UsageTotals {
            input_tokens: take(&mut left.input_tokens, s.totals.input_tokens),
            output_tokens: take(&mut left.output_tokens, s.totals.output_tokens),
            cache_write_tokens: take(&mut left.cache_write_tokens, s.totals.cache_write_tokens),
            cache_read_tokens: take(&mut left.cache_read_tokens, s.totals.cache_read_tokens),
            cost_micros: take(&mut left.cost_micros, s.totals.cost_micros),
        };
        if !t.is_zero() {
            out.push((s.day, s.backfill, t));
        }
    }
    if !left.is_zero() {
        out.push((today, false, left));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(cost: i64, now: i64) -> UsageDelta {
        UsageDelta {
            reset: false,
            totals: UsageTotals {
                input_tokens: cost / 10,
                cost_micros: cost,
                ..Default::default()
            },
            model: Some("claude-opus-5".into()),
            offset: cost,
            source: "s.jsonl".into(),
            last_msg_id: None,
            last_msg_usage: None,
            now,
            by_day: Vec::new(),
            backfill_until: None,
        }
    }

    /// Org administration phase C: a session in an org books its spend into
    /// `usage_daily_org` too, beside its host's row; a session in no org
    /// books nothing there.
    #[test]
    fn spend_is_booked_to_the_sessions_org_beside_its_host() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("a", Some("a")).unwrap();
        s.insert_host("b", Some("b")).unwrap();
        let org = s.add_org("Acme", None, false).unwrap();
        s.set_host_org("a", Some(org.id)).unwrap();
        let in_org = s
            .upsert_session("x", "a", None, None, 1, 1, "running", None)
            .unwrap();
        let loose = s
            .upsert_session("y", "b", None, None, 1, 1, "running", None)
            .unwrap();
        let day = 20_000;
        s.apply_usage(in_org, "a", &delta(500, day * 86_400))
            .unwrap();
        s.apply_usage(in_org, "a", &delta(800, day * 86_400 + 60))
            .unwrap();
        s.apply_usage(loose, "b", &delta(900, day * 86_400))
            .unwrap();
        let spent = s.org_live_cost_since(day).unwrap();
        // Each pass's totals are its growth: 500 then 800 more.
        assert_eq!(spent.get(&org.id), Some(&1_300), "{spent:?}");
        assert_eq!(spent.len(), 1, "no org, nothing booked: {spent:?}");
        assert!(s.org_live_cost_since(day + 1).unwrap().is_empty());
        // The host's roll-up is untouched by the second booking.
        let total: i64 = s
            .usage_daily_since(0, None)
            .unwrap()
            .iter()
            .map(|(_, _, _, t)| t.cost_micros)
            .sum();
        assert_eq!(total, 2_200);
        // Removing the org takes its roll-up with it.
        assert!(s.remove_org(org.id).unwrap());
        assert!(s.org_live_cost_since(0).unwrap().is_empty());
    }

    #[test]
    fn inherit_usage_cursor_and_carry_totals_follow_a_moved_session() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("a", Some("a")).unwrap();
        s.insert_host("b", Some("b")).unwrap();
        let src = s
            .upsert_session("dev-x", "a", None, None, 1, 1, "running", None)
            .unwrap();
        let dst = s
            .upsert_session("dev-x", "b", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(src, "uuid-m").unwrap();
        s.set_claude_session_id(dst, "uuid-m").unwrap();
        let totals = UsageTotals {
            input_tokens: 40,
            output_tokens: 4,
            cost_micros: 400,
            ..Default::default()
        };
        s.apply_usage(
            src,
            "a",
            &UsageDelta {
                reset: false,
                totals,
                model: Some("claude-opus-5".into()),
                offset: 900,
                source: "uuid-m.jsonl".into(),
                last_msg_id: Some("msg_9".into()),
                last_msg_usage: Some("40,4,0,0,0".into()),
                now: 86_400,
                by_day: Vec::new(),
                backfill_until: None,
            },
        )
        .unwrap();
        let daily_before = s.usage_daily_since(0, None).unwrap();

        // Source read the same file: its offset (<= copied) carries over.
        assert!(s.inherit_usage_cursor(dst, src, 1_000).unwrap());
        let c = &s.list_usage_cursors("b").unwrap()[0];
        assert_eq!(c.offset_bytes, 900);
        assert_eq!(c.source.as_deref(), Some("uuid-m.jsonl"));
        assert_eq!(c.last_msg_id.as_deref(), Some("msg_9"));
        assert_eq!(c.last_msg_usage.as_deref(), Some("40,4,0,0,0"));
        // Capped at the copied prefix.
        assert!(s.inherit_usage_cursor(dst, src, 500).unwrap());
        assert_eq!(s.list_usage_cursors("b").unwrap()[0].offset_bytes, 500);
        // The source was reading another file: start at the copied size.
        s.conn
            .execute(
                "UPDATE sessions SET usage_source = 'other.jsonl' WHERE id = ?1",
                [src],
            )
            .unwrap();
        assert!(s.inherit_usage_cursor(dst, src, 1_000).unwrap());
        let c = &s.list_usage_cursors("b").unwrap()[0];
        assert_eq!(c.offset_bytes, 1_000);
        assert_eq!(c.source.as_deref(), Some("uuid-m.jsonl"));
        // Unknown source: no-op.
        assert!(!s.inherit_usage_cursor(dst, 9_999, 1).unwrap());

        // Carry: the target now reports the source's lifetime spend.
        let row = s.carry_usage_totals(dst, src).unwrap().unwrap();
        assert_eq!(
            row.usage.totals(),
            UsageTotals {
                cost_micros: 400,
                ..totals
            }
        );
        assert_eq!(row.usage.usage_model.as_deref(), Some("claude-opus-5"));
        // Self-carry is refused (no doubling).
        let row = s.carry_usage_totals(dst, dst).unwrap().unwrap();
        assert_eq!(row.usage.usage_input_tokens, 40);
        // Neither helper touches the daily roll-up.
        assert_eq!(s.usage_daily_since(0, None).unwrap(), daily_before);
    }

    #[test]
    fn raise_usage_cursor_closes_the_window_between_inherit_and_kill() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("a", Some("a")).unwrap();
        s.insert_host("b", Some("b")).unwrap();
        let src = s
            .upsert_session("dev-y", "a", None, None, 1, 1, "running", None)
            .unwrap();
        let dst = s
            .upsert_session("dev-y", "b", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(src, "uuid-w").unwrap();
        s.set_claude_session_id(dst, "uuid-w").unwrap();
        let pass = |offset: i64, last: &str| UsageDelta {
            reset: false,
            totals: UsageTotals::default(),
            model: None,
            offset,
            source: "uuid-w.jsonl".into(),
            last_msg_id: Some(last.into()),
            last_msg_usage: Some("1,0,0,0,0".into()),
            by_day: Vec::new(),
            backfill_until: None,
            now: 1,
        };
        s.apply_usage(src, "a", &pass(100, "msg_a")).unwrap();
        assert!(s.inherit_usage_cursor(dst, src, 500).unwrap());
        assert_eq!(s.usage_cursor(dst).unwrap().unwrap().offset_bytes, 100);
        // A source pass between the inherit and the kill reached 300.
        s.apply_usage(src, "a", &pass(300, "msg_b")).unwrap();
        let snap = s.usage_cursor(src).unwrap().unwrap();
        assert!(s.raise_usage_cursor(dst, &snap).unwrap());
        let c = s.usage_cursor(dst).unwrap().unwrap();
        assert_eq!(c.offset_bytes, 300);
        assert_eq!(c.last_msg_id.as_deref(), Some("msg_b"));
        // Never lowers the target.
        let behind = UsageCursor {
            offset_bytes: 50,
            ..snap.clone()
        };
        assert!(!s.raise_usage_cursor(dst, &behind).unwrap());
        // A different transcript file leaves it alone.
        let other = UsageCursor {
            offset_bytes: 900,
            source: Some("other.jsonl".into()),
            ..snap.clone()
        };
        assert!(!s.raise_usage_cursor(dst, &other).unwrap());
        assert_eq!(s.usage_cursor(dst).unwrap().unwrap().offset_bytes, 300);
        assert!(s.usage_cursor(9_999).unwrap().is_none());
    }

    #[test]
    fn usage_accumulates_resets_rolls_up_daily_and_emits_only_on_change() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("vps").unwrap();
        let id = s
            .upsert_session("a", "vps", None, None, 1, 1, "running", None)
            .unwrap();
        // No claude id yet: no cursor.
        assert!(s.list_usage_cursors("vps").unwrap().is_empty());
        s.set_claude_session_id(id, "uuid-a").unwrap();
        let c = &s.list_usage_cursors("vps").unwrap()[0];
        assert_eq!((c.session_id, c.offset_bytes), (id, 0));
        assert_eq!(c.source, None);
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.usage, SessionUsage::default());

        let t = |i: i64, cost: i64| UsageTotals {
            input_tokens: i,
            output_tokens: 1,
            cache_write_tokens: 2,
            cache_read_tokens: 3,
            cost_micros: cost,
        };
        let delta = |reset: bool, totals: UsageTotals, offset: i64, now: i64| UsageDelta {
            reset,
            totals,
            model: Some("claude-opus-5".into()),
            offset,
            source: "uuid-a.jsonl".into(),
            last_msg_id: Some("msg_1".into()),
            last_msg_usage: Some("1,1,2,3,0".into()),
            now,
            by_day: Vec::new(),
            backfill_until: None,
        };
        let _ = bus.take();
        let day0 = 20_000 * 86_400;
        assert!(s
            .apply_usage(id, "vps", &delta(false, t(10, 50), 100, day0))
            .unwrap());
        assert_eq!(bus.take(), vec![format!("session:updated:{id}")]);
        assert!(s
            .apply_usage(id, "vps", &delta(false, t(5, 25), 200, day0 + 86_400))
            .unwrap());
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(
            row.usage.totals(),
            UsageTotals {
                input_tokens: 15,
                output_tokens: 2,
                cache_write_tokens: 4,
                cache_read_tokens: 6,
                cost_micros: 75
            }
        );
        assert_eq!(row.usage.usage_model.as_deref(), Some("claude-opus-5"));
        assert_eq!(row.usage.usage_updated_at, Some(day0 + 86_400));
        let c = &s.list_usage_cursors("vps").unwrap()[0];
        assert_eq!(c.offset_bytes, 200);
        assert_eq!(c.source.as_deref(), Some("uuid-a.jsonl"));
        assert_eq!(c.last_msg_id.as_deref(), Some("msg_1"));
        // The wire row carries the flattened fields.
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["usage_cost_micros"], 75);
        assert_eq!(v["usage_input_tokens"], 15);
        assert!(v.get("usage").is_none());

        // Offset-only pass: nothing changed, no event, stamp kept.
        let _ = bus.take();
        let mut idle = delta(false, UsageTotals::default(), 300, day0 + 2 * 86_400);
        idle.model = None;
        assert!(!s.apply_usage(id, "vps", &idle).unwrap());
        assert!(bus.take().is_empty());
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.usage.usage_updated_at, Some(day0 + 86_400));
        assert_eq!(s.list_usage_cursors("vps").unwrap()[0].offset_bytes, 300);

        // Reset: totals replaced; only the growth reaches the daily roll-up.
        assert!(s
            .apply_usage(id, "vps", &delta(true, t(20, 100), 40, day0 + 86_400))
            .unwrap());
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.usage.usage_input_tokens, 20);
        assert_eq!(row.usage.usage_cost_micros, 100);
        let days = s.usage_daily_since(20_000, None).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!((days[0].0, days[0].3.cost_micros), (20_000, 50));
        assert_eq!(days[1].3.cost_micros, 25 + 25);
        assert_eq!(days[1].3.input_tokens, 5 + 5);
        assert!(s.usage_daily_since(20_002, None).unwrap().is_empty());
        assert!(s.usage_daily_since(0, Some("other")).unwrap().is_empty());
        assert_eq!(s.usage_daily_since(0, Some("vps")).unwrap().len(), 2);

        // Unknown session: no-op.
        assert!(!s
            .apply_usage(9_999, "vps", &delta(false, t(1, 1), 1, 0))
            .unwrap());
        // Lost sessions have no cursor.
        s.conn
            .execute("UPDATE sessions SET lost_at = 1 WHERE id = ?1", [id])
            .unwrap();
        assert!(s.list_usage_cursors("vps").unwrap().is_empty());
    }

    #[test]
    fn apply_usage_books_each_transcript_day_and_keeps_backfill_apart() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        s.upsert_session("a", "vps", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("a", "vps").unwrap().unwrap().id;
        let t = |cost: i64| UsageTotals {
            input_tokens: 1,
            output_tokens: 1,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            cost_micros: cost,
        };
        let mut all = t(100);
        all.add(&t(7));
        let d = UsageDelta {
            reset: false,
            totals: all,
            model: Some("claude-opus-5".into()),
            offset: 10,
            source: "x.jsonl".into(),
            last_msg_id: None,
            last_msg_usage: None,
            now: 20_716 * 86_400 + 5,
            by_day: vec![
                DayDelta {
                    day: 20_714,
                    totals: t(100),
                    backfill: true,
                },
                DayDelta {
                    day: 20_716,
                    totals: t(7),
                    backfill: false,
                },
            ],
            backfill_until: None,
        };
        assert!(s.apply_usage(id, "vps", &d).unwrap());
        let rows: Vec<(i64, bool, i64)> = s
            .usage_daily_since(0, Some("vps"))
            .unwrap()
            .into_iter()
            .map(|(day, _, backfill, t)| (day, backfill, t.cost_micros))
            .collect();
        assert_eq!(rows, vec![(20_714, true, 100), (20_716, false, 7)]);
        let days = crate::service::usage::daily_totals(&s, 0, Some("vps")).unwrap();
        assert_eq!(days[0].day, "2026-09-18");
        assert_eq!(days[0].backfill_cost_micros, 100);
        assert_eq!(
            days[0].totals.cost_micros, 0,
            "backfill never inflates the live day"
        );
        assert_eq!(days[1].totals.cost_micros, 7);
        assert_eq!(days[1].backfill_cost_micros, 0);
        // A delta without day slices books to the day of `now`, as before.
        let plain = UsageDelta {
            by_day: Vec::new(),
            totals: t(1),
            offset: 11,
            ..d.clone()
        };
        assert!(s.apply_usage(id, "vps", &plain).unwrap());
        let live_today = s
            .usage_daily_since(20_716, Some("vps"))
            .unwrap()
            .into_iter()
            .find(|(_, _, b, _)| !b)
            .unwrap()
            .3
            .cost_micros;
        assert_eq!(live_today, 8);
    }

    /// A rewritten transcript (`reset`) re-reads its history from byte 0:
    /// its growth over what the row held is split over the pass's day
    /// slices — today's live slice first, the rest to the backfill rows —
    /// instead of all landing on today's live row.
    #[test]
    fn a_reset_books_its_history_growth_as_backfill() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        s.upsert_session("a", "vps", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("a", "vps").unwrap().unwrap().id;
        let t = |n: i64| UsageTotals {
            input_tokens: n,
            cost_micros: n * 10,
            ..Default::default()
        };
        let today = 20_716;
        let now = today * 86_400 + 5;
        let base = UsageDelta {
            reset: false,
            totals: t(30),
            model: Some("claude-opus-5".into()),
            offset: 10,
            source: "x.jsonl".into(),
            last_msg_id: None,
            last_msg_usage: None,
            now,
            by_day: Vec::new(),
            backfill_until: None,
        };
        // 30 already counted, live today.
        assert!(s.apply_usage(id, "vps", &base).unwrap());
        // The file is rewritten: 100 of history two days back, 7 today.
        let reset = UsageDelta {
            reset: true,
            totals: t(107),
            offset: 5,
            by_day: vec![
                DayDelta {
                    day: today - 2,
                    totals: t(100),
                    backfill: true,
                },
                DayDelta {
                    day: today,
                    totals: t(7),
                    backfill: false,
                },
            ],
            backfill_until: Some(5),
            ..base.clone()
        };
        assert!(s.apply_usage(id, "vps", &reset).unwrap());
        let rows: Vec<(i64, bool, i64)> = s
            .usage_daily_since(0, Some("vps"))
            .unwrap()
            .into_iter()
            .map(|(day, _, backfill, t)| (day, backfill, t.input_tokens))
            .collect();
        assert_eq!(
            rows,
            vec![(today - 2, true, 70), (today, false, 37)],
            "growth 77: 7 live today, 70 to history"
        );
        let row = s.get_session("a", "vps").unwrap().unwrap();
        assert_eq!(row.usage.usage_input_tokens, 107, "the totals are replaced");
        assert_eq!(s.usage_cursor(id).unwrap().unwrap().backfill_until, 5);
    }

    #[test]
    fn split_reset_growth_never_books_more_than_a_slice_counted() {
        let t = |n: i64| UsageTotals {
            input_tokens: n,
            ..Default::default()
        };
        let slices = [
            DayDelta {
                day: 1,
                totals: t(5),
                backfill: true,
            },
            DayDelta {
                day: 2,
                totals: t(5),
                backfill: true,
            },
            DayDelta {
                day: 3,
                totals: t(2),
                backfill: false,
            },
        ];
        assert_eq!(
            split_reset_growth(t(9), &slices, 3),
            vec![(3, false, t(2)), (2, true, t(5)), (1, true, t(2))],
            "live first, then history newest first"
        );
        assert_eq!(
            split_reset_growth(t(14), &slices, 3).last(),
            Some(&(3, false, t(2))),
            "a remainder past every slice books live today"
        );
    }
}
