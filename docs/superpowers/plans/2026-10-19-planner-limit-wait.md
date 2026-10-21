# The planner at a usage limit (TASK-291)

Mission #4's planner hit its account's usage limit and every ask after it was
logged as `refused` ("not a JSON array: the answer begins \"You've hit your
limit · resets 3pm…\""), so nothing said the mission was only waiting, and the
loop kept asking into the limit.

## What changes

1. **Recognise the answer** — `claude_print::usage_limit(said)` reads the
   opening of a reply (as `says_signed_out` does) for Claude Code's limit
   words (`Claude AI usage limit reached|<epoch>`, `5-hour limit reached ∙
   resets 3pm`, `You've hit your limit · resets …`, `Weekly limit reached`)
   and returns the reset epoch when the answer carries one, and the reset
   phrase as Claude said it. A command array never begins that way, so it is
   asked only of an answer that does not parse (or of stderr).
2. **Pick a login with headroom first** — `orchestrate::limit::choose`: the
   planner's candidate places are the hosts `planner_host` already allowed
   (the policy host alone, else the grant's hosts, else the workers' hosts
   that see the org), and on each the grant's login (or the host's own) first,
   then the host's other signed-in profiles. A login is passed over while it
   is at `accounts.pause_at` (`account_limits::over_limit`) or while it has an
   open `limit_wait` (its reset is still ahead). The script runs under the
   chosen profile (`login_env`).
3. **Wait instead of refusing** — an answer at the limit records a
   `limit_wait` mission event (`host`, `profile`, `resets_at`,
   `reset_known`, `said`, `why`) instead of `refused`, and the planner tries
   the next login (at most `LIMIT_ATTEMPTS` runs per ask). With no login
   left, one more `limit_wait` with no login says the mission waits until the
   earliest reset, and `PlanOutcome.waiting` carries it. `resets_at` is, in
   order: the epoch in Claude's answer, the account's usage reading
   (`routines::backoff::limit_resume_at`), else `LIMIT_WAIT_GUESS_SECS`.
4. **Ask again after the reset** — `planner_wanted`: while the planner's
   newest word (of `planned` / `limit_wait`) is a wait, it asks again as soon
   as `choose` finds a login (a reset passed, or another login has headroom),
   and not before. The mission's lease wakes at the earliest reset.
5. **Show it** — `eventSentence('limit_wait')`, `plannerWaiting()` for the
   Plan button's result, a "waiting on a usage limit" line in the mission
   room.

Not here: parsing a wall-clock reset ("resets 3pm (Europe/Bratislava)")
without a reading — fleet has no time-zone database; the phrase is shown as
Claude said it and the guess is used.
