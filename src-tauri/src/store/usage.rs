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
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, transcript_path, claude_session_id, usage_offset_bytes, usage_source, \
             usage_last_msg_id, usage_last_msg_usage FROM sessions \
             WHERE host_alias = ?1 AND lost_at IS NULL AND claude_session_id IS NOT NULL \
             ORDER BY COALESCE(usage_updated_at, 0), id",
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], |r| {
            Ok(UsageCursor {
                session_id: r.get(0)?,
                transcript_path: r.get(1)?,
                claude_session_id: r.get(2)?,
                offset_bytes: r.get(3)?,
                source: r.get(4)?,
                last_msg_id: r.get(5)?,
                last_msg_usage: r.get(6)?,
            })
        })?;
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
        );
        let src: Option<Src> = self
            .conn
            .query_row(
                "SELECT claude_session_id, usage_source, usage_offset_bytes, usage_last_msg_id, \
                 usage_last_msg_usage FROM sessions WHERE id = ?1",
                rusqlite::params![source_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((Some(claude_id), source_file, offset, last_id, last_usage)) = src else {
            return Ok(false);
        };
        let file = format!("{claude_id}.jsonl");
        let copied = copied_bytes.max(0);
        let offset = if source_file.as_deref() == Some(file.as_str()) {
            offset.clamp(0, copied)
        } else {
            copied
        };
        let n = self.conn.execute(
            "UPDATE sessions SET usage_source = ?1, usage_offset_bytes = ?2, \
             usage_last_msg_id = ?3, usage_last_msg_usage = ?4 WHERE id = ?5",
            rusqlite::params![file, offset, last_id, last_usage, target_id],
        )?;
        Ok(n == 1)
    }

    /// Add the source's lifetime usage totals to the target (a move that
    /// KILLS the source, so the spend follows the session). Never call it
    /// under `keep_source`: both rows would then report the same spend.
    /// A missing source or `target == source` is a no-op. `usage_daily` is
    /// untouched (that spend was already bucketed).
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
                "SELECT id, transcript_path, claude_session_id, usage_offset_bytes, usage_source, \
                 usage_last_msg_id, usage_last_msg_usage FROM sessions WHERE id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok(UsageCursor {
                        session_id: r.get(0)?,
                        transcript_path: r.get(1)?,
                        claude_session_id: r.get(2)?,
                        offset_bytes: r.get(3)?,
                        source: r.get(4)?,
                        last_msg_id: r.get(5)?,
                        last_msg_usage: r.get(6)?,
                    })
                },
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
             usage_last_msg_usage = ?4 \
             WHERE id = ?1 AND usage_source = ?5 AND usage_offset_bytes < ?2",
            rusqlite::params![
                target_id,
                src.offset_bytes,
                src.last_msg_id,
                src.last_msg_usage,
                file
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
        let row = fetch_session_by_id(&self.conn, target_id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Apply one usage pass to a session: add (or, on `reset`, replace) the
    /// totals, move the cursor, and add the growth to the host's
    /// `usage_daily` bucket for the UTC day of `now`. Returns whether the
    /// totals or model changed; only then is `usage_updated_at` stamped and
    /// `session_updated` emitted (a pass that only moves the offset is
    /// silent). An unknown session id is a no-op.
    pub fn apply_usage(
        &self,
        session_id: i64,
        host_alias: &str,
        d: &UsageDelta,
    ) -> Result<bool, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let old: Option<(UsageTotals, Option<String>)> = tx
            .query_row(
                "SELECT usage_input_tokens, usage_output_tokens, usage_cache_write_tokens, \
                 usage_cache_read_tokens, usage_cost_micros, usage_model FROM sessions WHERE id = ?1",
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
                    ))
                },
            )
            .optional()?;
        let Some((before, old_model)) = old else {
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
        tx.execute(
            "UPDATE sessions SET usage_input_tokens = ?1, usage_output_tokens = ?2, \
             usage_cache_write_tokens = ?3, usage_cache_read_tokens = ?4, usage_cost_micros = ?5, \
             usage_model = ?6, usage_offset_bytes = ?7, usage_source = ?8, usage_last_msg_id = ?9, \
             usage_updated_at = CASE WHEN ?10 THEN ?11 ELSE usage_updated_at END, \
             usage_last_msg_usage = ?13 WHERE id = ?12",
            rusqlite::params![
                after.input_tokens,
                after.output_tokens,
                after.cache_write_tokens,
                after.cache_read_tokens,
                after.cost_micros,
                model,
                d.offset.max(0),
                d.source,
                d.last_msg_id,
                changed,
                d.now,
                session_id,
                d.last_msg_usage
            ],
        )?;
        if !daily.is_zero() {
            tx.execute(
                "INSERT INTO usage_daily (day, host_alias, input_tokens, output_tokens, \
                 cache_write_tokens, cache_read_tokens, cost_micros) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(day, host_alias) DO UPDATE SET \
                 input_tokens = input_tokens + excluded.input_tokens, \
                 output_tokens = output_tokens + excluded.output_tokens, \
                 cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens, \
                 cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens, \
                 cost_micros = cost_micros + excluded.cost_micros",
                rusqlite::params![
                    d.now.div_euclid(86_400),
                    host_alias,
                    daily.input_tokens,
                    daily.output_tokens,
                    daily.cache_write_tokens,
                    daily.cache_read_tokens,
                    daily.cost_micros
                ],
            )?;
        }
        tx.commit()?;
        if changed {
            if let Some(row) = fetch_session_by_id(&self.conn, session_id)? {
                self.bus.session_updated(&row);
            }
        }
        Ok(changed)
    }

    /// `usage_daily` rows with `day >= since_day` (UTC day numbers),
    /// optionally for one host, oldest first.
    pub fn usage_daily_since(
        &self,
        since_day: i64,
        host_alias: Option<&str>,
    ) -> Result<Vec<(i64, String, UsageTotals)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT day, host_alias, input_tokens, output_tokens, cache_write_tokens, \
             cache_read_tokens, cost_micros FROM usage_daily \
             WHERE day >= ?1 AND (?2 IS NULL OR host_alias = ?2) ORDER BY day, host_alias",
        )?;
        let rows = stmt.query_map(rusqlite::params![since_day, host_alias], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                UsageTotals {
                    input_tokens: r.get(2)?,
                    output_tokens: r.get(3)?,
                    cache_write_tokens: r.get(4)?,
                    cache_read_tokens: r.get(5)?,
                    cost_micros: r.get(6)?,
                },
            ))
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!((days[0].0, days[0].2.cost_micros), (20_000, 50));
        assert_eq!(days[1].2.cost_micros, 25 + 25);
        assert_eq!(days[1].2.input_tokens, 5 + 5);
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
}
