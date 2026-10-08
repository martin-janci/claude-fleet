//! Cost per account and per model (Orbit Fleet redesign step 4.2).
//!
//! Spend is `usage_daily_account` (migration 130), booked beside
//! `usage_daily` in every usage pass, keyed by the session's account
//! (`sessions.account_uuid`: the host's login, or the session's profile
//! login) and the model the transcript line names. Live rows only: a
//! backfill row is history a first read of a transcript booked, not spend
//! in the window, as `org_spend` and the usage report count it.
//!
//! It sums over every session on an account, other people's private ones
//! among them, so it is served only where the caller sees the whole fleet:
//! the desktop's own store (the `account_spend` command is local only).

use crate::ipc_error::{lock, IpcError};
use crate::service::usage::day_string;
use crate::store::{Store, UsageTotals};
use serde::Serialize;
use std::sync::Mutex;

const SECS_PER_DAY: i64 = 86_400;

/// One model's live spend on an account over the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSpend {
    /// The model id; `""` when a transcript line named none.
    pub model: String,
    pub totals: UsageTotals,
}

/// One UTC day's live spend on an account, every model summed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaySpend {
    /// `YYYY-MM-DD`.
    pub day: String,
    pub cost_micros: i64,
}

/// One account's live spend over the window: the total, the split by
/// model (most expensive first) and the series by day (oldest first, days
/// with no spend left out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSpend {
    /// `""` gathers the sessions whose login fleet has not read yet.
    pub account_uuid: String,
    pub cost_micros: i64,
    pub models: Vec<ModelSpend>,
    pub by_day: Vec<DaySpend>,
}

/// `account_spend`: each account's live spend since `since` (unix seconds,
/// counted from the start of its UTC day), or one account's when
/// `account_uuid` is given, most expensive account first. An account with
/// no spend in the window is left out.
pub fn account_spend(
    since: i64,
    account_uuid: Option<&str>,
    store: &Mutex<Store>,
) -> Result<Vec<AccountSpend>, IpcError> {
    let since_day = since.div_euclid(SECS_PER_DAY);
    let s = lock(store)?;
    let mut out: Vec<AccountSpend> = Vec::new();
    for row in s.account_model_cost_since(since_day, account_uuid)? {
        if out.last().map(|a| a.account_uuid.as_str()) != Some(row.account_uuid.as_str()) {
            out.push(AccountSpend {
                account_uuid: row.account_uuid.clone(),
                cost_micros: 0,
                models: Vec::new(),
                by_day: Vec::new(),
            });
        }
        let a = out.last_mut().expect("pushed above");
        a.cost_micros += row.totals.cost_micros;
        a.models.push(ModelSpend {
            model: row.model,
            totals: row.totals,
        });
    }
    for a in &mut out {
        a.models
            .sort_by_key(|m| std::cmp::Reverse(m.totals.cost_micros));
        a.by_day = s
            .account_live_cost_by_day(&a.account_uuid, since_day)?
            .into_iter()
            .map(|(day, cost_micros)| DaySpend {
                day: day_string(day),
                cost_micros,
            })
            .collect();
    }
    out.sort_by(|x, y| {
        y.cost_micros
            .cmp(&x.cost_micros)
            .then_with(|| x.account_uuid.cmp(&y.account_uuid))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(s: &Store, day: i64, acct: &str, model: &str, backfill: bool, cost: i64) {
        s.conn_for_test()
            .execute(
                "INSERT INTO usage_daily_account (day, account_uuid, model, backfill, \
                 input_tokens, cost_micros) VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                rusqlite::params![day, acct, model, backfill as i64, cost],
            )
            .unwrap();
    }

    #[test]
    fn spend_groups_by_account_most_expensive_first_and_skips_history() {
        let s = Store::open_in_memory().unwrap();
        book(&s, 100, "acc-a", "claude-opus-5", false, 30);
        book(&s, 101, "acc-a", "claude-sonnet-5", false, 70);
        book(&s, 101, "acc-a", "claude-opus-5", true, 9_000);
        book(&s, 101, "acc-b", "claude-opus-5", false, 500);
        book(&s, 99, "acc-b", "claude-opus-5", false, 1);
        let store = Mutex::new(s);

        let all = account_spend(100 * SECS_PER_DAY + 7, None, &store).unwrap();
        assert_eq!(
            all.iter()
                .map(|a| (a.account_uuid.as_str(), a.cost_micros))
                .collect::<Vec<_>>(),
            vec![("acc-b", 500), ("acc-a", 100)],
            "day 99 is before the window; the backfill row is history"
        );
        let a = &all[1];
        assert_eq!(
            a.models
                .iter()
                .map(|m| (m.model.as_str(), m.totals.cost_micros))
                .collect::<Vec<_>>(),
            vec![("claude-sonnet-5", 70), ("claude-opus-5", 30)]
        );
        assert_eq!(
            a.by_day,
            vec![
                DaySpend {
                    day: day_string(100),
                    cost_micros: 30
                },
                DaySpend {
                    day: day_string(101),
                    cost_micros: 70
                },
            ]
        );

        let one = account_spend(0, Some("acc-b"), &store).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].cost_micros, 501);
        assert!(account_spend(0, Some("nobody"), &store).unwrap().is_empty());
    }
}
