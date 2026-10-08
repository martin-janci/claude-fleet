//! The cron grammar a routine's schedule is written in: five fields,
//! `minute hour day-of-month month weekday`, each `*`, a number, a range
//! `a-b`, a step `*/n` or `a-b/n`, or a comma list of those; weekday 0 and 7
//! are Sunday. `@hourly`, `@daily` (`@midnight`), `@weekly`, `@monthly` and
//! `@yearly` (`@annually`) stand for their usual lines. When both day
//! fields are restricted a day matching either fires, as in every cron.
//!
//! A line is read at a fixed offset from UTC (minutes east), the one the
//! person's device had when they saved it: there is no time-zone database
//! in the build, so a daylight-saving change moves the wall-clock time of a
//! fire by an hour until the routine is saved again.

/// A parsed line: one bit per allowed value of each field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    days_restricted: bool,
    weekdays_restricted: bool,
}

/// The widest offset from UTC a schedule may name (UTC−12 … UTC+14).
pub const MAX_OFFSET_MIN: i64 = 14 * 60;
/// How far ahead the next fire is looked for: past four years every valid
/// line has fired (29 February included).
const SEARCH_DAYS: i64 = 4 * 366 + 1;

fn field(text: &str, name: &str, lo: u32, hi: u32) -> Result<(u64, bool), String> {
    let mut bits = 0u64;
    let mut restricted = false;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let s: u32 = s
                    .parse()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("{name}: step {s:?} is not a positive number"))?;
                (r, s)
            }
            None => (part, 1),
        };
        // A field that starts with `*` (`*/2` included) leaves the other day
        // field in charge, as in Vixie cron.
        let (a, b) = if range == "*" {
            (lo, hi)
        } else {
            restricted = true;
            let num = |v: &str| -> Result<u32, String> {
                v.parse::<u32>()
                    .ok()
                    .filter(|n| (lo..=hi).contains(n))
                    .ok_or_else(|| format!("{name}: {v:?} is not a number from {lo} to {hi}"))
            };
            match range.split_once('-') {
                Some((a, b)) => {
                    let (a, b) = (num(a)?, num(b)?);
                    if a > b {
                        return Err(format!("{name}: the range {range} runs backwards"));
                    }
                    (a, b)
                }
                // `5/15` reads as 5, 20, 35, 50, as in Vixie cron.
                None if step > 1 => (num(range)?, hi),
                None => {
                    let n = num(range)?;
                    (n, n)
                }
            }
        };
        let mut v = a;
        while v <= b {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok((bits, restricted))
}

impl Cron {
    pub fn parse(line: &str) -> Result<Cron, String> {
        let line = line.trim();
        let expanded = match line {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let parts: Vec<&str> = expanded.split_whitespace().collect();
        let [mi, h, d, mo, w] = parts.as_slice() else {
            return Err(format!(
                "a schedule has five fields (minute hour day month weekday), got {}",
                parts.len()
            ));
        };
        let (minutes, _) = field(mi, "minute", 0, 59)?;
        let (hours, _) = field(h, "hour", 0, 23)?;
        let (days, days_restricted) = field(d, "day of month", 1, 31)?;
        let (months, _) = field(mo, "month", 1, 12)?;
        let (mut weekdays, weekdays_restricted) = field(w, "weekday", 0, 7)?;
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1;
        }
        Ok(Cron {
            minutes,
            hours,
            days,
            months,
            weekdays,
            days_restricted,
            weekdays_restricted,
        })
    }

    fn day_matches(&self, day: u32, weekday: u32) -> bool {
        let d = self.days & (1 << day) != 0;
        let w = self.weekdays & (1 << weekday) != 0;
        if self.days_restricted && self.weekdays_restricted {
            d || w
        } else {
            d && w
        }
    }

    /// The first fire strictly after `after` (unix seconds), read at
    /// `offset_min` east of UTC. `None` for a line that never fires
    /// (`0 0 31 2 *`).
    pub fn next_after(&self, after: i64, offset_min: i64) -> Option<i64> {
        let offset = offset_min * 60;
        let first_minute = (after + offset).div_euclid(60) + 1;
        let first_day = first_minute.div_euclid(1440);
        for day in first_day..first_day + SEARCH_DAYS {
            let (_, month, dom) = civil_from_days(day);
            if self.months & (1 << month) == 0 {
                continue;
            }
            let weekday = (day + 4).rem_euclid(7) as u32;
            if !self.day_matches(dom, weekday) {
                continue;
            }
            for h in 0..24 {
                if self.hours & (1 << h) == 0 {
                    continue;
                }
                for m in 0..60 {
                    if self.minutes & (1 << m) == 0 {
                        continue;
                    }
                    let minute = day * 1440 + h * 60 + m;
                    if minute >= first_minute {
                        return Some(minute * 60 - offset);
                    }
                }
            }
        }
        None
    }
}

/// `(year, month 1–12, day 1–31)` of a day number since 1970-01-01
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The start of the local day `at` falls in, as unix seconds.
pub fn local_day_start(at: i64, offset_min: i64) -> i64 {
    let offset = offset_min * 60;
    (at + offset).div_euclid(86_400) * 86_400 - offset
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-08 00:00 UTC, a Thursday.
    const OCT8: i64 = 1_791_417_600;

    fn at(day_offset: i64, h: i64, m: i64) -> i64 {
        OCT8 + day_offset * 86_400 + h * 3600 + m * 60
    }

    #[test]
    fn the_anchor_is_a_thursday_in_october() {
        assert_eq!(civil_from_days(OCT8 / 86_400), (2026, 10, 8));
        assert_eq!((OCT8 / 86_400 + 4).rem_euclid(7), 4);
    }

    #[test]
    fn a_weekday_morning_line_fires_on_the_next_weekday() {
        let c = Cron::parse("0 9 * * 1-5").unwrap();
        // Thursday 08:59 → Thursday 09:00.
        assert_eq!(c.next_after(at(0, 8, 59), 0), Some(at(0, 9, 0)));
        // Exactly at the fire: strictly after, so Friday.
        assert_eq!(c.next_after(at(0, 9, 0), 0), Some(at(1, 9, 0)));
        // Friday after nine: Monday.
        assert_eq!(c.next_after(at(1, 9, 30), 0), Some(at(4, 9, 0)));
    }

    #[test]
    fn the_offset_moves_the_wall_clock_not_the_line() {
        let c = Cron::parse("0 9 * * *").unwrap();
        // 09:00 at UTC+2 is 07:00 UTC.
        assert_eq!(c.next_after(at(0, 0, 0), 120), Some(at(0, 7, 0)));
        // 09:00 at UTC−5 is 14:00 UTC.
        assert_eq!(c.next_after(at(0, 0, 0), -300), Some(at(0, 14, 0)));
    }

    #[test]
    fn steps_lists_and_ranges() {
        let c = Cron::parse("*/15 * * * *").unwrap();
        assert_eq!(c.next_after(at(0, 10, 1), 0), Some(at(0, 10, 15)));
        let c = Cron::parse("5/20 8-10 * * *").unwrap();
        assert_eq!(c.next_after(at(0, 8, 6), 0), Some(at(0, 8, 25)));
        assert_eq!(c.next_after(at(0, 10, 45), 0), Some(at(1, 8, 5)));
        let c = Cron::parse("0 12,18 * * *").unwrap();
        assert_eq!(c.next_after(at(0, 12, 0), 0), Some(at(0, 18, 0)));
    }

    #[test]
    fn both_day_fields_restricted_means_either() {
        // The 10th, or any Sunday: from Thursday the 8th, Saturday the 10th.
        let c = Cron::parse("0 0 10 * 0").unwrap();
        assert_eq!(c.next_after(at(0, 1, 0), 0), Some(at(2, 0, 0)));
        // From the 10th: Sunday the 11th.
        assert_eq!(c.next_after(at(2, 1, 0), 0), Some(at(3, 0, 0)));
        // Weekday 7 is Sunday too.
        assert_eq!(
            Cron::parse("0 0 * * 7").unwrap().next_after(at(0, 1, 0), 0),
            Some(at(3, 0, 0))
        );
    }

    #[test]
    fn aliases_and_a_leap_day() {
        assert_eq!(
            Cron::parse("@daily").unwrap(),
            Cron::parse("0 0 * * *").unwrap()
        );
        assert_eq!(
            Cron::parse("@hourly").unwrap().next_after(at(0, 3, 0), 0),
            Some(at(0, 4, 0))
        );
        // 29 February 2028.
        let leap = Cron::parse("0 0 29 2 *")
            .unwrap()
            .next_after(OCT8, 0)
            .unwrap();
        assert_eq!(civil_from_days(leap / 86_400), (2028, 2, 29));
        assert_eq!(Cron::parse("0 0 31 2 *").unwrap().next_after(OCT8, 0), None);
    }

    #[test]
    fn bad_lines_say_what_is_wrong() {
        for (line, says) in [
            ("0 9 * *", "five fields"),
            ("60 * * * *", "minute"),
            ("0 24 * * *", "hour"),
            ("0 0 0 * *", "day of month"),
            ("0 0 * 13 *", "month"),
            ("0 0 * * 8", "weekday"),
            ("*/0 * * * *", "step"),
            ("0 10-8 * * *", "backwards"),
            ("x * * * *", "minute"),
        ] {
            let e = Cron::parse(line).unwrap_err();
            assert!(e.contains(says), "{line}: {e}");
        }
    }

    #[test]
    fn local_day_start_follows_the_offset() {
        assert_eq!(local_day_start(at(0, 23, 0), 0), at(0, 0, 0));
        // 23:00 UTC is 01:00 the next day at UTC+2, which began 22:00 UTC.
        assert_eq!(local_day_start(at(0, 23, 0), 120), at(0, 22, 0));
        assert_eq!(local_day_start(at(0, 1, 0), -300), at(-1, 5, 0));
    }
}
