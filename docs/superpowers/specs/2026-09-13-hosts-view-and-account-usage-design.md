# Hosts view and per-account usage

Date: 2026-09-13. Status: approved in conversation. UX designed with a
product-design expert; decisions below are binding.

## Problem

1. Settings is a fixed 600px modal. Its Hosts table has eight columns and does
   not fit.
2. There is no screen for inspecting one host.
3. The user wants to see, per Claude account, how much of the **5-hour window**
   and the **weekly window** is left — to decide which account to start or keep
   work on before hitting a limit.

## User decisions (from the conversation)

- Build the full design below.
- In compact surfaces, show **% left and the reset time with equal weight**
  (the user sometimes moves to another account, sometimes waits for a reset).
- Accounts get **short nicknames**, because `m-janci@users.noreply.github.com` and
  `mj-janci@users.noreply.github.com` are easy to confuse in narrow labels.

## Facts that shape the design

- **Usage belongs to an account, not a host.** Real mapping: `mefistos` and
  `claude-fleet-oci` share `admin-janci@users.noreply.github.com`; `claude-fleet-htz` →
  `m-janci@users.noreply.github.com`; `claude-fleet-trn` and `local` → `mj-janci@users.noreply.github.com`.
- **Bug:** `local` is logged in (`~/.claude.json` has `oauthAccount` for
  `mj-janci@users.noreply.github.com`) but its `hosts.account_uuid` is empty. `probe_local` in
  `service/hosts.rs` reads the file and the `OauthAccount` field types match
  the JSON (`seatTier: null` is fine for `Option<String>`), so the cause is
  elsewhere in how the local probe result reaches `set_host_account`. Fix it
  with a regression test.
- **The usage source is undocumented.** There is no supported CLI, hook or SDK
  API (open requests: anthropics/claude-code#40793, #24459, #48660, #36056). The
  only source is `GET https://api.anthropic.com/api/oauth/usage`, the endpoint
  Claude Code's own `/usage` uses, called with the host's OAuth access token
  and `anthropic-beta: oauth-2025-04-20`. It returns
  `five_hour { utilization, resets_at }`, `seven_day { … }`, and optional
  `seven_day_opus` / `seven_day_sonnet`. It can change, error, or rate-limit at
  any time.
- **Credentials.** On Linux the token is in `~/.claude/.credentials.json`
  (mode 0600). On macOS it is in the Keychain item `Claude Code-credentials`;
  reading that from a third-party process prompts the user every time Claude
  Code refreshes the token, so fleet does NOT read the Keychain.
- `oauthAccount.hasExtraUsageEnabled` (bool) tells whether hitting a limit
  blocks the account or spends pay-as-you-go money. Store it; it changes the
  wording of the limit state.

## Security rules for the usage fetch

- **The token never leaves its host.** Fleet runs one shell script on the
  host over SSH: it reads the token from `~/.claude/.credentials.json` with a
  JSON parser and calls the endpoint with `curl` on that host. Only the JSON
  response comes back. Fleet never reads, stores, logs, or transmits the token
  itself, and the script must not echo it (no `set -x`, token passed via a
  file descriptor or header file, not argv where `ps` could show it).
- **Honest `User-Agent`:** `claude-fleet/<version>`. Do not impersonate Claude
  Code. If that gets throttled, the feature degrades to "unavailable" — it does
  not escalate to impersonation.
- **Local macOS host:** no Keychain access. The account's usage is fetched
  through any other online host logged in to the same account. If there is
  none, local reports "usage is read through another host on this account".
- **Poll gently:** at most one request per account per 5 minutes; honour
  `Retry-After` on 429 with exponential backoff up to 30 minutes.

## Information architecture

- **The Hosts table leaves Settings.** Settings keeps only fleet-wide
  configuration (Projects, Setup guide, Notifications, Automation, Limits,
  Diagnostics), widens to `min(640px, 92vw)`, and shows one line in place of the
  table: `Hosts  5 configured · 1 offline  [Open Hosts ⌘I]`.
- **Hosts is a full view to the right of the sidebar**, reusing the existing
  Files-mode mechanism (`filesMode` in `App.svelte`): the center pane collapses,
  an overlay covers the terminal, and `TerminalView` stays mounted so the PTY
  survives. It is entered from a right-aligned, visually separated `Hosts ⌘I`
  tab (Terminal and Files are session-scoped; Hosts is fleet-scoped and is never
  disabled).
- **Shortcuts:** ⌘I toggles Hosts (capture phase, so it works from inside the
  terminal); ⌘, opens Settings (unbound today); Ctrl+Shift+H toggles Hosts on
  non-mac (Ctrl+Shift+I is the devtools chord). The quick switcher (⌘K) gains
  `host: <alias>` entries that open the view with that host selected. Verify
  none collide with Tauri's default app menu.
- **Leaving:** Esc (when focus is not in an input), ⌘I again, clicking
  Terminal, or selecting a session in the sidebar (intent: "go to it").
- `Sidebar`'s onboarding "Add host" opens the Hosts view instead of Settings.

## The Hosts view

Master–detail. The list is grouped by account; usage is shown once per group.

**List (≈320px):**
- Group header per account: nickname (else email), seat tier, a 5h mini-bar
  and a weekly mini-bar with % left, a freshness mark.
- Host row: status glyph (● online / ○ offline, plus the word "offline"),
  alias, session counts (`6 ⚡2 ⏸1`), one attention mark (hooks stale, token
  missing, claude version drift).
- **Stable order:** accounts alphabetically by label, a "No Claude account"
  group last, hosts alphabetically within a group. Never re-sort by headroom.

**Detail sections, in order:**
1. Header: alias, ssh alias, status and last ping, claude and tmux versions.
2. **Usage** — with "shared with <hosts>" when the account covers several.
3. Sessions on this host, each a jump target.
4. Today: estimated tokens and cost (already computed; kept separate from
   usage — different units).
5. Integration: Control-API token mode and Rotate…, hook health.
6. Danger: Hide host, Remove host….

**Action safety:**
- Safe and idempotent (re-probe, refresh usage, filter sidebar to host): single
  keys, no confirm.
- Reversible (hide, token mode): a detail button, no confirm; hide shows an
  Undo toast.
- Destructive (Rotate token, Remove host): only at the bottom of the detail,
  **no keyboard shortcut**, a confirm dialog with Cancel focused, stating the
  consequence plainly (check `store.delete_host` for cascades and say so).
- No × or 🚫 icons in list rows.

## Showing usage

- **Wording leads with left:** `8% left`; in the detail `92% used` follows,
  muted. Never a bare percentage.
- **Bars fill with used**, matching the existing context-% bar.
- **Reset time, per the user's "equal weight" decision:** both % left and the
  reset time are always shown together in compact surfaces. Format: 5-hour as a
  countdown then clock — `resets in 38 min (15:10)`; weekly as weekday then
  countdown — `resets Thu 09:00 (in 2d 18h)`.
- **Pace tick** on weekly bars only: a thin mark at the fraction of the week
  elapsed (from `resets_at`). The detail says `on pace` / `ahead of pace`.
- **Severity** on the binding window: ok (≥ 50% left) — neutral bar, not green;
  caution (20–50%) — amber, `low soon` in the detail; low (< 20%) — red with a
  diagonal-stripe fill and `▲ LOW`; limit (0%) — red stripes and
  `■ LIMIT · resets 15:10`, or `■ EXTRA USAGE · resets 15:10` when
  `has_extra_usage` is true. Exception: for the 5-hour window, drop one level
  when the reset is under 15 minutes away.
- **Colour-blind and themes:** severity always carried by word, glyph and
  pattern, never colour alone. New tokens `--usage-warn` and `--usage-crit` in
  `app.css` for both themes; bar contrast ≥ 3:1, text ≥ 4.5:1. The existing
  `contextColor` hard-coded colours move onto the same tokens.
- **Model buckets** (`seven_day_opus`, `seven_day_sonnet`): hidden by default;
  never in the list or compact surfaces. In the detail a bucket line appears
  only when it binds (fewer % left than overall weekly) or is below 50%;
  otherwise a collapsed `Per-model ▸`. A binding bucket is named on the weekly
  line: `Weekly 58% left · Opus 29% left ▲`.

## Staleness and failure

A number is shown only while it is valid for a decision.

| window | fresh | stale (dimmed, `~`, age shown) | expired (number withheld, `?`) |
|---|---|---|---|
| 5-hour | ≤ 6 min | 6–30 min | > 30 min, or now > `resets_at` |
| weekly | ≤ 6 min | 6 min – 3 h | > 3 h, or now > `resets_at` |

States and wording:
- **First load:** `checking…`, a dashed empty bar — never an empty solid bar
  (which reads as 100% free). `Asking <host> for usage…`
- **Fresh:** `via <host> · checked 2 min ago` and `u refresh`.
- **Stale:** `◷ 14 min old — last check failed: <reason>. Next try 14:40.`
- **Expired past reset:** `Window reset at 15:10 after the last check. Last
  known: 8% left at 14:30.`
- **Rate-limited:** last-known values under the stale/expired rules, plus
  `⏸ Anthropic is rate-limiting usage checks. Next try 14:52.`
- **Endpoint unavailable or changed:** ONE banner at the top of the Hosts view —
  `⚠ Usage unavailable since 13:10. Anthropic's usage endpoint returned an
  unexpected response (HTTP 404). It's undocumented and may have changed.
  Sessions are unaffected. [Copy details] [Retry 14:40]` — and blocks show `—`.
- **OAuth token expired on the host:** `🔑 Claude login expired on <host> —
  usage can't be checked from it. Run claude /login there.` If another host on
  the account works, show `via <other>` with a muted note instead of an alarm.
- **No online host for the account:** last-known values under the rules, plus
  `○ No online host is logged in to this account (<host> offline since 13:52).`
- **Host with no account:** `Not logged in to Claude on this host — no usage to
  show.`

**Fetch triggers** (all within the 5-minute floor): the background tick, app
focus regained, the Hosts view opening, the New-session dialog opening. Manual
`u` refresh respects the floor and says `refresh available in 2:10`. Each
account is polled through one sticky host so the "via" label does not flap; on
failure it falls back to another host on the same account.

## Glanceable surfaces

- **New-session host chips:** each chip shows the account's binding window as
  `% left` AND its reset time (the user's equal-weight decision), naming the
  window only when weekly binds. The selected chip gets a full line below the
  row (`<nickname> · 5h 91% left, resets 17:05 · weekly 86% left · 2 min ago`).
  Stale chips show `~62% left ◷`; expired show `? left`. If the chosen host's
  account is low, an inline warning names the account and the other hosts
  sharing it. Never auto-switch hosts. Widen the dialog from 420px to ≈520px.
- **Footer segment** (right side of the existing
  `v… · db: ok · schema …` footer) says whether to look, not the numbers:
  `usage ✓ all accounts · 3m`, `usage ▲ <nickname> 5h 8% left · resets 15:10`,
  or `usage ◷ unavailable since 13:10`; clicking it or ⌘I opens Hosts on the
  worst account. After 24 hours unavailable it collapses to a muted
  `usage off`.
- **Not in the sidebar.** Session rows already show a context-% chip meaning
  *used*; a second % with the opposite meaning would be misread.

## Keyboard

On open, remember `document.activeElement`; focus the list; preselect the
selected session's host, else the last-viewed host, else the first host needing
attention. On close, restore focus.

| Key | List | Detail |
|---|---|---|
| ↑↓ / j k, Home/End | move selection (detail follows live) | move between focusable rows |
| Enter / → | focus detail | on a session row: jump to it (closes Hosts) |
| ← | — | back to list |
| Esc | close view | back to list (or clear an input) |
| `r` | re-probe host | same |
| `u` | refresh account usage | same |
| `s` | filter sidebar to this host | same |
| `n` | new session on this host | same |
| `/` | filter hosts | — |
| `?` | key legend | key legend |
| Tab | the only way to reach Hide, Rotate, Remove | |

No type-ahead; stray letters do nothing; no bound letter is destructive.

## Account nicknames

A nullable `nickname` on `accounts` (one migration), set inline from the group
header or the account line in the detail (click or `e` to edit, Enter saves,
Escape cancels, empty clears). Every compact label uses the nickname when set,
else the email. The email is always available in the detail and a tooltip.

## Out of scope (v1)

Usage history charts, sparklines and burn-rate forecasts; auto-routing new
sessions to the account with the most headroom; OS notifications on threshold
crossings (a v1.1 candidate); a separate Accounts screen; logging in or
switching accounts from the UI; usage in sidebar rows, a tray, or the terminal
header; model buckets outside the detail; sortable or custom columns; cost next
to utilization; a manual refresh that bypasses the poll floor; a combined health
score; keyboard shortcuts for destructive actions; reading the macOS Keychain.

## Ergonomic risks to guard (ranked)

1. An old number read as current — mitigated by `~`/◷ when stale and `?` when
   expired or past reset, including in compact chips.
2. Two percentages with opposite meaning — mitigated by always printing
   "left"/"used" and filling every bar with used.
3. Shared-account blindness — mitigated by grouping, "shared with …", and the
   chip warning naming the other hosts.
4. A third mode in the right-hand region muddling Esc and focus — mitigated by
   a separated tab, consistent Esc/⌘I, focus restore, session-select exits.
5. Letters typed into Hosts by someone expecting the terminal — mitigated by
   harmless bindings and no destructive shortcuts.
6. The pace tick mistaken for a limit line — weekly only, thin, with words.
7. Alarm fatigue if the endpoint stays dead — footer collapses after 24 h.
8. Look-alike account labels — mitigated by nicknames.
