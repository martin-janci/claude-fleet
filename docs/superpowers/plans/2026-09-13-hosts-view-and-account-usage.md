# Hosts view and per-account usage — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A Hosts screen grouped by Claude account showing each account's 5-hour and weekly usage honestly, Settings reduced to configuration, account nicknames, and usage glanceable where sessions are started.

**Spec (binding, read first):** `docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md`.

**Architecture:** Backend: fix local account detection; migration 028 for `accounts.nickname` and `accounts.has_extra_usage`; a new `service/account_usage.rs` that fetches usage per account by running a script ON a logged-in host (the OAuth token never leaves it), with a floor-respecting in-memory cache polled from the existing background tick. Frontend: a pure `account_usage.ts` model, `UsageBar`/`UsageBlock` components, a `HostsView` master–detail reusing App's Files-mode overlay, Settings slimmed, and usage in New-session host chips and the footer.

**Plan style note.** This plan states contracts, file paths, tests and acceptance criteria rather than full code: the previous feature showed that implementers who read the current code produce better results than stale inline snippets. Every task still requires failing tests first.

**Conventions for every task:**
- `crate::shell::quote` on every value interpolated into a shell string.
- Never hold the `Store` mutex across an `.await`.
- Rust from `src-tauri/`: `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`.
- Frontend from root: `npx vitest run`, `npx svelte-check --tsconfig ./tsconfig.json`, `pnpm run build` (the pnpm test/check binaries are not on PATH).
- Any new or changed Tauri command or `#[tool]`: `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`.
- A new migration: `NNN_<topic>.sql` ending with `INSERT OR IGNORE INTO schema_version (version) VALUES (NNN);`, registered in `MIGRATIONS` in `src-tauri/src/store/schema.rs` with an `already_applied` guard, and every schema-version test assertion bumped.
- A Rust wire field always gets its TypeScript mirror (`value | null` for `Option`).
- Commit after each task; no attribution lines; subagents never run git pull/push/fetch/rebase/checkout/stash/reset.

---

## Task 1 — Local account detection bug

**Files:** `src-tauri/src/service/hosts.rs` (+ tests).

`local` is logged in (`~/.claude.json` → `oauthAccount` for `mj-janci@users.noreply.github.com`) but `hosts.account_uuid` for `local` is empty. `probe_local` (~line 311) reads the file, and the `OauthAccount` field types already match the JSON (`seatTier: null` deserializes into `Option<String>` fine), so the defect is in how the local probe's account reaches `set_host_account` — or `probe_local` is never used for `local` on the path that runs (re-probe, add, the tick), or `HOME` differs in the GUI process.

- [ ] Reproduce with a test that drives the real local-probe path used for `host_alias == "local"` and asserts the account is set. Make `probe_local` take the home directory as a parameter so a test can point it at a temp `~/.claude.json` without touching the user's.
- [ ] Find the root cause (trace every call site of `probe_local` and `set_host_account`), fix it at the source, and state the cause in the commit body.
- [ ] Commit: `fix(hosts): record the local host's Claude account`.

## Task 2 — Account nickname and extra-usage flag

**Files:** `src-tauri/migrations/028_account_nickname.sql`, `src-tauri/src/store/schema.rs`, `src-tauri/src/store/hosts_accounts.rs`, `src-tauri/src/store/rows.rs`, `src-tauri/src/service/hosts.rs`, a command in `src-tauri/src/commands/` (next to the existing account/host commands), `src/lib/accounts.ts`, `docs/control-api-reference.md`.

- [ ] Migration 028: `ALTER TABLE accounts ADD COLUMN nickname TEXT;` and `ALTER TABLE accounts ADD COLUMN has_extra_usage INTEGER NOT NULL DEFAULT 0;` with the idempotency guard.
- [ ] `OauthAccount` parses `hasExtraUsageEnabled` (`Option<bool>`); `upsert_account` stores it but **never overwrites `nickname`** (the nickname is user data, the probe must not clobber it).
- [ ] `Store::set_account_nickname(uuid, Option<&str>)`: trims; empty clears; max 32 characters; emits `account_upserted`.
- [ ] Command `set_account_nickname { uuid, nickname }` validating the uuid exists (`E_NOTFOUND`) and the length (`E_INVALID`).
- [ ] `AccountRow` gains `nickname` and `has_extra_usage` in Rust and TypeScript; add an exported TS helper `accountLabel(a): string` (nickname, else email, else a short uuid).
- [ ] Tests: migration re-run safety; a probe upsert keeps an existing nickname; set/clear/too-long; `accountLabel` fallbacks.
- [ ] Regenerate the reference. Commit: `feat(accounts): nicknames and the extra-usage flag`.

## Task 3 — The usage fetch (security-critical)

**Files:** new `src-tauri/src/service/account_usage.rs`, `src-tauri/src/service/mod.rs`, `src-tauri/src/ipc_error.rs` (codes if needed).

**Absolute rules:** the OAuth token must never leave the host, never be read into fleet's process, never be logged, never appear in argv. Never call the real endpoint in tests — use `FakeSsh`. Never read the macOS Keychain.

- [ ] **Pure script builder** `usage_script(user_agent: &str) -> String` run via `bash -lc` on a host. It:
  - reads `$HOME/.claude/.credentials.json` (also honour `$CLAUDE_CONFIG_DIR/.credentials.json` when set) and extracts `claudeAiOauth.accessToken` with `python3`/`jq` — confirm the real key path against a Linux host's file shape by reading only its KEYS over SSH, never its values, or from Claude Code docs; if unsure, handle both `claudeAiOauth.accessToken` and a top-level `accessToken`;
  - passes the token to `curl` through a header file created with `umask 077` in `mktemp` and removed on exit (`trap`), or via `--config -` on stdin — never on argv;
  - calls `GET https://api.anthropic.com/api/oauth/usage` with `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-fleet/<version>`, `--max-time 15`, and `-w` to append the HTTP status and any `Retry-After`;
  - emits machine-readable markers on stdout, e.g. `__no_credentials__`, `__http_status__=<n>`, `__retry_after__=<s>`, then the body;
  - never uses `set -x` and never echoes the token. A test asserts the script text contains no `set -x`, no `echo` of the token variable, and does not place the token in a `curl` argument.
- [ ] **Pure parser** `parse_usage_output(stdout, now) -> UsageOutcome` classifying: `Ok(AccountUsage)`, `NoCredentials` (file missing — e.g. a macOS host), `TokenExpired` (401/403), `RateLimited { retry_after }` (429), `Unavailable { status, snippet }` (any other non-2xx, or a 2xx body that does not have the expected shape), `Transport(error)`. `AccountUsage { five_hour: Option<Window>, seven_day: Option<Window>, seven_day_opus: Option<Window>, seven_day_sonnet: Option<Window> }` with `Window { utilization: f64 /* 0-100, used */, resets_at: Option<i64 /* unix */> }`. Be tolerant of unknown extra fields and of a missing bucket; clamp utilization into 0..=100.
- [ ] **Host selection** `pick_source_host(account_uuid, hosts, sticky: Option<&str>) -> Vec<String>`: online hosts whose `account_uuid` matches, the sticky host first, `local` last (it has no credentials file on macOS), then alphabetical.
- [ ] **Cache + scheduler** `UsageCache` (in-memory, per account): `last_ok: Option<(AccountUsage, fetched_at)>`, `last_outcome`, `source_host`, `next_try_at`. `due(account, now)` honours the 5-minute floor and backoff: 429 → `max(Retry-After, doubling)` capped at 30 minutes; transport or unavailable → doubling from 5 minutes capped at 30.
- [ ] `fetch_account_usage_with(account_uuid, store, ssh: &dyn SshExec, cache, now, force)`: tries source hosts in order until one returns `Ok` or a non-host-specific failure (`RateLimited`, `Unavailable`), falls back past `NoCredentials`/`TokenExpired`/`Transport`, records which host answered, updates the cache. `force` still respects the floor (the spec forbids bypassing it) and reports the time until the next allowed attempt.
- [ ] Tests (all `FakeSsh`): each outcome parsed from realistic output; the fallback order across two hosts on one account; `local` tried last; floor and backoff arithmetic including `Retry-After`; a 2xx with a changed body shape becomes `Unavailable`; the script-safety assertions above; no test runs a real `curl` to the network.
- [ ] Commit: `feat(usage): fetch each account's 5-hour and weekly usage on its own host`.

## Task 4 — Poller, commands, events

**Files:** `src-tauri/src/service/tick.rs` (or the existing tick loop), `src-tauri/src/service/account_usage.rs`, `src-tauri/src/events.rs`, `src-tauri/src/commands/`, `src-tauri/src/lib.rs`, `src/lib/events.ts`, `docs/control-api-reference.md`.

- [ ] Hold one `UsageCache` in app state. On each background tick, fetch every account whose `due` time has passed (bounded concurrency, e.g. two at a time), without blocking the reconcile work.
- [ ] Wire type `AccountUsageState { account_uuid, usage: Option<AccountUsage>, fetched_at: Option<i64>, source_host: Option<String>, status: "ok"|"no_credentials"|"token_expired"|"rate_limited"|"unavailable"|"no_online_host"|"never_fetched", detail: Option<String>, next_try_at: Option<i64> }` — mirrored in TypeScript.
- [ ] Commands `list_account_usage` (returns every account's state, never triggers a fetch) and `refresh_account_usage { account_uuid }` (fetches if the floor allows, else returns the state with `next_try_at`).
- [ ] Event `account_usage:updated` carrying one `AccountUsageState`, subscribed in `src/lib/events.ts` and patched into a new `accountUsage` store in `src/lib/account_usage_store.ts` (keep the store separate from Task 5's pure module).
- [ ] Tests: the tick fetches only due accounts; a command within the floor does not fetch; the event fires on update.
- [ ] Regenerate the reference. Commit: `feat(usage): poll account usage in the background and expose it`.

## Task 5 — The pure usage model and theme tokens

**Files:** new `src/lib/account_usage.ts` + `src/lib/account_usage.test.ts`, `src/app.css`, `src/lib/attention.ts` (`contextColor`).

Pure, fully unit-tested functions implementing the spec exactly:
- [ ] `freshness(window: '5h'|'weekly', fetchedAt, resetsAt, now) -> 'fresh'|'stale'|'expired'` using the spec's thresholds, expired when `now > resetsAt`.
- [ ] `severity(window, leftPct, resetsAt, now, hasExtraUsage) -> 'ok'|'caution'|'low'|'limit'` including the 5-hour under-15-minutes drop-a-level rule.
- [ ] `bindingWindow(usage) -> '5h'|'weekly'` (fewest % left, weekly model buckets considered for the detail's binding-bucket note).
- [ ] `formatReset(window, resetsAt, now, locale)`: 5-hour `resets in 38 min (15:10)`; weekly `resets Thu 09:00 (in 2d 18h)`.
- [ ] `paceFraction(resetsAt, now)` for the weekly tick, and `paceLabel` (`on pace` / `ahead of pace`).
- [ ] `chipLabel(state, now)` returning the compact chip text with BOTH % left and the reset (the user's equal-weight decision), `~` + ◷ when stale, `? left` when expired.
- [ ] `limitWording(hasExtraUsage)`: `LIMIT` vs `EXTRA USAGE`.
- [ ] Add `--usage-warn` and `--usage-crit` tokens for light and dark in `app.css` with ≥ 3:1 contrast against `--bg-pane`; move `contextColor`'s hard-coded `#e64a4a` / `#d29b4a` onto them without changing its thresholds.
- [ ] Tests cover every threshold boundary, reset past, missing buckets, the extra-usage wording, and a fixed `now` and locale.
- [ ] Commit: `feat(usage): the usage model and theme tokens`.

## Task 6 — `UsageBar` and `UsageBlock`

**Files:** new `src/lib/UsageBar.svelte`, `src/lib/UsageBlock.svelte` (+ tests).

- [ ] `UsageBar { window, usage window, fetchedAt, now, compact }`: fills with used; diagonal stripes at `low`/`limit`; dimmed with a dotted outline when stale; a dashed empty track (never a solid empty bar) when expired or never fetched; the weekly pace tick; `role="meter"` with `aria-valuenow` = used and an `aria-label` that says "N% left".
- [ ] `UsageBlock { account, state, sharedWith: string[], now }` renders the detail block for EVERY state in the spec's staleness section with its exact wording, the `Per-model ▸` disclosure, `via <host> · checked N min ago`, and the refresh affordance showing `refresh available in m:ss` inside the floor.
- [ ] Tests render each state and assert the wording, the `~`/◷/`?` rules, and that no stale or expired number is shown without its marker.
- [ ] Commit: `feat(usage): usage bar and block components`.

## Task 7 — `HostsView`

**Files:** new `src/lib/HostsView.svelte` (split into `HostsList.svelte` and `HostDetail.svelte` if it passes ~400 lines), tests; reuse the host actions currently in `src/lib/HostsTable.svelte` (probe, hide, remove, token mode, rotate, hook health) — move them, do not duplicate.

- [ ] List grouped by account per the spec (stable order, "No Claude account" last), group header with `accountLabel`, both mini-bars, freshness mark; host rows with status glyph + word, session counts, one attention mark.
- [ ] Detail sections in the spec's order; "shared with …" when the account covers several hosts; sessions as jump targets.
- [ ] Nickname editing inline on the group header and the detail's account line (`e` or click; Enter saves; Escape cancels; empty clears) calling `set_account_nickname`.
- [ ] Action safety exactly as specified: Hide with an Undo toast; Rotate token and Remove host behind a `ConfirmDialog` with Cancel focused and consequence copy verified against `store.delete_host` (say what happens to that host's session rows); no shortcut for either.
- [ ] Keyboard table from the spec; no type-ahead; `?` legend.
- [ ] The endpoint-unavailable banner, shown once at the top when every account is `unavailable`, with `[Copy details]` (copies status and snippet, never a token) and `[Retry <time>]`.
- [ ] Tests: grouping and order with the real 4-account/5-host shape (including two hosts sharing an account and `local` on `mj-janci@users.noreply.github.com`); keyboard navigation; nickname edit; destructive confirms with Cancel focused and no shortcut; the banner.
- [ ] Commit: `feat(hosts): the Hosts view`.

## Task 8 — App integration and Settings

**Files:** `src/App.svelte`, `src/lib/SettingsDialog.svelte`, `src/lib/Sidebar.svelte` / `src/lib/OnboardingCard.svelte`, `src/lib/QuickSwitcher.svelte` / `src/lib/quick_switcher.ts`, `src/lib/HostsTable.svelte` (delete once its behaviour lives in Task 7), tests.

- [ ] A `hostsMode` next to `filesMode`, reusing its overlay so `TerminalView` stays mounted; the right-aligned, separated `Hosts ⌘I` tab.
- [ ] ⌘I (capture phase; works while the terminal is focused), Ctrl+Shift+H on non-mac, ⌘, for Settings. Confirm against Tauri's default menu and every existing binding (`grep -rn "metaKey\|ctrlKey" src/`) that nothing collides; document the result in the commit body.
- [ ] Leaving: Esc (not while an input has focus), ⌘I, clicking Terminal, selecting a session in the sidebar. Focus is remembered on open and restored on close; the preselected host follows the spec.
- [ ] Quick switcher gains `host: <alias>` entries.
- [ ] Settings: remove the Hosts table, add the one-line `Hosts  N configured · M offline  [Open Hosts ⌘I]`, width `min(640px, 92vw)`. Onboarding "Add host" opens the Hosts view.
- [ ] Tests: ⌘I toggles from a focused terminal; Esc returns focus; selecting a session exits; the terminal is not unmounted across a round trip; Settings no longer renders the table.
- [ ] Commit: `feat(hosts): open the Hosts view from anywhere; slim Settings`.

## Task 9 — Glanceable surfaces

**Files:** `src/lib/HostChips.svelte`, `src/lib/NewSessionDialog.svelte`, `src/App.svelte` (footer), tests.

- [ ] New-session host chips show `chipLabel` (both % left and the reset), stale/expired markers, and the selected chip's full line; an inline low-headroom warning naming the account and the other hosts sharing it; never auto-switch. Widen the dialog to ≈520px. Opening the dialog triggers `refresh_account_usage` for the visible accounts (floor-respecting).
- [ ] Footer segment per the spec, clicking or ⌘I opens Hosts on the worst account; collapses to a muted `usage off` after 24 hours unavailable.
- [ ] Tests: chip text in fresh/stale/expired; the warning names sharing hosts; no auto-switch; footer states.
- [ ] Commit: `feat(usage): show headroom when starting a session and in the footer`.

## Task 10 — CI, version, build, push

- [ ] Full CI mirror (Rust fmt/clippy/test, reference check, `cargo deny`, frozen install, svelte-check, vitest, build).
- [ ] Bump the three version files to the next patch; commit `chore(release): bump version to X.Y.Z`.
- [ ] Build with `CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet pnpm tauri build --bundles app`; install to `/Applications`, keeping the previous bundle; verify the version; screenshot the Hosts view for the user.
- [ ] `git fetch`; merge `origin/main` if it moved and re-run the CI mirror; `git push origin HEAD:main`; commit the refreshed `Cargo.lock`.

## Self-review

- Spec coverage: IA (T8), Hosts view (T7), usage display (T5–T6), staleness and failure (T3, T5, T6, T7), glanceable surfaces (T9), keyboard (T7, T8), nicknames (T2, T7), the local account bug (T1), extra-usage wording (T2, T5), security rules for the token (T3).
- The contracts shared across tasks — `AccountUsageState`, `AccountRow.nickname`/`has_extra_usage`, `accountLabel`, `chipLabel` — are each defined once, in T2, T4 and T5, and referenced by name afterwards.
