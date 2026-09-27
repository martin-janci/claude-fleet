# Desktop UX Quick Wins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development` (or `superpowers:executing-plans`) to implement this plan task by task. Every step below is a checkbox; tick it only after the named command printed the named result.

**Goal:** Make the hub-client desktop tell the truth about the live fleet's 63 rows — lost rows render as lost, `bg:<uuid>` rows get a name, the tree keeps a stable in-project order, agent-transport hosts open Conversation instead of a failing ssh, the footer names the app and the hub, Refresh stops reconciling the hub, the operator row is marked and its panel follows the live row — and add the one hub tool the desktop needs for bulk ghost dismissal.

**Architecture:** Tasks 1–7 are TypeScript/Svelte only (pure helpers in `src/lib/*.ts` with Vitest tests, wired into `Sidebar.svelte`, `SessionRowItem.svelte`, `TerminalView.svelte`, `AgentPanel.svelte`, `App.svelte`); Task 5 adds three desktop-internal fields to `HubStatus` (the struct between this app's halves, never on the hub wire) fed from the hub's `ready` frame. Task 8 adds one MCP tool + Tauri command `dismiss_ghost_sessions` through the usual lanes: service fn → `#[tool]` → `TOOL_POLICIES` → verdict row → `tests_routing` case → regenerated verdicts/reference → `ROUTED_ACTIONS` → store wrapper → UI. Nothing here bumps `CONTRACT_REVISION`: no hub→desktop struct gains a field.

**Tech Stack:** Svelte 5 runes, TypeScript, Vitest + `@testing-library/svelte`; Rust (fleet-core `service/` + `mcp/tools/`, `src-tauri` commands/backend), `rmcp` `#[tool]`, serde.

**Spec:** `docs/ux/2026-09-27-live-instance-analysis/README.md` themes T2/T7, code-table rows 7, 8, 15, 16, 17, 18, 23; evidence `ux.md` F-01, F-02 (display half), F-04, F-05, F-06, F-07, F-08, F-11, F-16, F-17, F-18, F-19, F-20; `lifecycle.md` F4/F6 (data side, relied on but not changed here). Audit ids referenced: UX-13 (`docs/ux/2026-09-21-audit/iterations/consolidation-01.md:51`, `consolidation-02.md:131`), UX-92 (`consolidation-02.md:80`), UXPR-13/22/26 (`consolidation-02.md:437,446,450` — the parity lane; NOT a dependency of Tasks 1–7).

**Code base read at:** `origin/main @ 7dad1665` (package.json 0.3.1). Every `file:line` below is from that tree. Two items of row 7 already landed there (PR #316 `fix/outside-fleet-dead-rows`): `buildOutsideFleet` filters `lost_at`/ghost (`src/lib/sidebar_index.ts:154-170`, test `sidebar_index.test.ts:103-110`) and `lostReasonLabel('local_disabled')` exists (`src/lib/sessions.ts:233-243`). Task 1 covers what is still missing.

## Global Constraints

Copied from `CLAUDE.md` → Conventions, plus the repo gotchas every task below must honour:

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote` (alias `shq`) in `crates/fleet-core/src/shell.rs`. Every value interpolated into an SSH/bash command string MUST be quoted with it. The former duplicate copies (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not reintroduce them.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the guard across an `.await`.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command (a sync command runs on the macOS main thread). PTY input goes to the writer thread through its bounded channel — `E_PTY_BUSY` when it is full, `E_PTY_CLOSED` when the thread is gone; kill / reap / fd teardown runs on the `PtyParts` taken out under the lock, after the guard is released.
- A new wire field on a struct that crosses hub↔desktop needs `#[serde(default)]` (an old hub/desktop otherwise breaks) and fails the hub contract golden test: `REGEN_HUB_CONTRACT=1 cargo test -p fleet-core <test>` (the regen run reports FAILED once; re-run to see green). Bump `CONTRACT_REVISION` ONLY if a call shape changes; if bumped, `MIN_HUB_CONTRACT`/`MAX_HUB_CONTRACT` in `src-tauri/src/backend/contract.rs` move with it. (No task in this plan crosses that line; Task 5's fields live on the desktop-internal `HubStatus`.)
- Any new Tauri command or MCP tool (or edited `#[tool(...)]` description): row in `src-tauri/src/backend/verdicts.rs` + `route`/`refuse_local_only` by command name in `backend/tests_routing.rs`, then `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`, then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`; a LocalOnly command the UI reaches needs a `REASONS` entry or allowlist line in `src/lib/hub_verdicts.test.ts`. A test caps the served MCP tool description budget (`the_served_definition_budget_stays_bounded`, `crates/fleet-core/src/mcp/tools/tests.rs:3123`) — keep new descriptions to one slim clause; every tool parameter needs a doc comment (`every_tool_parameter_is_documented`, `:2201`).
- New `work`/`work_link`/`work_admin` actions need an isolation-matrix row (`mcp/tools/tests_isolation.rs`). (None added here.)
- Hub-routed command args: the whole args struct is serialized; a new arg needs `Serialize` + a non-default row in `tests_routing.rs`; report types need `Deserialize` and no serde defaults except where noted above.
- Status vocabulary lives in the enums in `service/pane_intel.rs`; add values there, never in prose. (None added here.)
- Frontend: Svelte 5 runes stores in `src/lib/*.ts`; row events patch stores via `mergeOne`/`removeOne`; tests are Vitest (`npx vitest run <file>`), type-check `npx svelte-check`; `pnpm test`/`pnpm check` do not work on this Mac (use `npx`). Run `pnpm install --frozen-lockfile` once before the first test.
- Tests: `cargo test -p fleet-core <name>`; `cargo test -p claude-fleet --lib <name>` for `src-tauri`. Export `CARGO_TARGET_DIR` yourself before any cargo command in a worktree (the shared target on the SD card is what the `cargo` shell function forces).
- Git: every commit below is made from the worktree root with `git add <paths>` + `git commit -m "<conventional message>"`; never `git add -A`, never stash.

---

### Task 1: Lost rows render as lost (F-01, F-02 display half, F-05; row 7; UX-92 sibling)

**Files:**
- Modify: `src/lib/attention.ts:123-132` (`TRIAGE_BUCKETS`), `:195-205` (`classify`), `:207-221` (`bucketSince`)
- Modify: `src/lib/sidebar_index.ts:144-170` (add `buildLostOutsideFleet` after `buildOutsideFleet`)
- Modify: `src/lib/sessions.ts:233-243` (`lostReasonLabel`)
- Modify: `src/lib/SessionRowItem.svelte:3-15` (imports), `:465-520` (branch order, Recreate guard)
- Modify: `src/lib/Sidebar.svelte:118-128` (section pref), `:600-607` (derived), `:1253-1270` (new `Lost (n)` section)
- Test: `src/lib/attention.test.ts:217-260`, `src/lib/sidebar_index.test.ts:79-111`, `src/lib/sessions.test.ts:39-46`, `src/lib/SessionRowItem.test.ts:52-80`, `src/lib/Sidebar.test.ts:1613-1624`

**Interfaces:**
- Produces `TRIAGE_BUCKETS` gains `'lost'` as its LAST entry (`[..., 'working', 'idle', 'lost']`); `TriageBucket` widens accordingly; `NEEDS_YOU_BUCKETS = TRIAGE_BUCKETS.slice(0, 6)` and `NEEDS_YOU_COUNTED_BUCKETS = slice(0, 5)` are unchanged.
- Produces `classify(s, opts): TriageBucket` — `external && (lost_at !== null || status === 'ghost')` → `'lost'`; every other external row stays `working`/`idle`; non-external ghosts stay `lifecycle`.
- Produces `buildLostOutsideFleet(sessions: readonly SessionRow[], hostFilter: string, scope: ScopeFilter = null): SessionRow[]` — external rows with `lost_at !== null || status === 'ghost'`, host/scope filtered, sorted `lost_at` desc then id desc.
- Produces `lostReasonLabel`: `local_disabled` → `'host retired'`, `missing` → `'gone from tmux'`, `killed` → `'killed'`; `host_reboot`/`tmux_server_gone` unchanged; unknown → `null`.
- Consumes `hasNoPane` (`src/lib/sessions.ts:776`), `timeAgo` (`session_status.ts:33`).

- [ ] **Step 1.1 — failing test: the `lost` bucket.** Append to `describe('triage rank', …)` in `src/lib/attention.test.ts` (after the `it('classifies every bucket reachable…')` block ending at line 231):

```ts
  it('files a lost or ghosted external row under lost, the last bucket (F-01/F-05)', () => {
    expect(TRIAGE_BUCKETS[TRIAGE_BUCKETS.length - 1]).toBe('lost');
    expect(classify(row({ kind: 'external', lost_at: 5, claude_status: 'working' }), opts)).toBe('lost');
    expect(classify(row({ kind: 'external', status: 'ghost' }), opts)).toBe('lost');
    // A live external row is untouched, and a fleet ghost stays lifecycle.
    expect(classify(row({ kind: 'external', claude_status: 'working' }), opts)).toBe('working');
    expect(classify(row({ status: 'ghost', lost_at: 5 }), opts)).toBe('lifecycle');
    // Never counted as needing you, and it sorts under idle.
    expect(needsYou(row({ kind: 'external', lost_at: 5 }), opts)).toBe(false);
    expect(rank(row({ kind: 'external', lost_at: 5 }), opts).order).toBeGreaterThan(
      rank(row(), opts).order,
    );
  });
```

- [ ] **Step 1.2 — run it.** `npx vitest run src/lib/attention.test.ts` → expected: 1 failed, `AssertionError: expected 'idle' to be 'lost'` (first `expect` on `TRIAGE_BUCKETS[...]` fails with `expected 'idle' to be 'lost'`).

- [ ] **Step 1.3 — implement.** In `src/lib/attention.ts`:

Replace lines 123-132 with:

```ts
export const TRIAGE_BUCKETS = [
  'waiting',
  'stuck',
  'failed',
  'done_unread',
  'lifecycle',
  'idle_long',
  'working',
  'idle',
  // A lost external row (`bg:<uuid>` after a host reboot): fleet cannot
  // restore it, nobody can act on it, so it sorts under everything live.
  // Filed here rather than `lifecycle` so a dozen of them never outrank
  // running work (F-05) and never light the Needs-you pill.
  'lost',
] as const;
```

Replace `classify` (lines 195-205) with:

```ts
export function classify(s: SessionRow, opts: AttentionOptions): TriageBucket {
  if (s.kind === 'external') {
    if (s.lost_at !== null || s.status === 'ghost') return 'lost';
    return s.claude_status === 'working' ? 'working' : 'idle';
  }
  if (isWaiting(s)) return 'waiting';
  if (s.stuck_kind) return 'stuck';
  if (s.claude_status === 'failed') return 'failed';
  if (isDoneUnread(s)) return 'done_unread';
  if (isLifecycleBroken(s)) return 'lifecycle';
  if (isIdleLong(s, opts)) return 'idle_long';
  if (s.claude_status === 'working') return 'working';
  return 'idle';
}
```

In `bucketSince` (lines 207-221) add a case before `default`:

```ts
    case 'lost':
      return s.lost_at ?? s.last_activity_at;
```

- [ ] **Step 1.4 — run.** `npx vitest run src/lib/attention.test.ts` → expected: all tests pass (the existing `needsYou covers every bucket above working` test still passes: `slice(0, 6)` is unchanged).

- [ ] **Step 1.5 — failing test: `buildLostOutsideFleet`.** Add to `src/lib/sidebar_index.test.ts` import list `buildLostOutsideFleet` (line 3) and append after the `buildOutsideFleet` describe (line 111):

```ts
describe('buildLostOutsideFleet', () => {
  it('returns only lost/ghost external rows, host-filtered, newest loss first', () => {
    const live = row({ kind: 'external', host_alias: 'mac' });
    const older = row({ kind: 'external', host_alias: 'mac', status: 'ghost', lost_at: 100 });
    const newer = row({ kind: 'external', host_alias: 'mac', status: 'ghost', lost_at: 200 });
    const elsewhere = row({ kind: 'external', host_alias: 'local', status: 'ghost', lost_at: 300 });
    const fleetGhost = row({ kind: 'work', status: 'ghost', lost_at: 400 });
    expect(buildLostOutsideFleet([live, older, newer, elsewhere, fleetGhost], 'all').map((s) => s.id)).toEqual([
      elsewhere.id, newer.id, older.id,
    ]);
    expect(buildLostOutsideFleet([live, older, newer, elsewhere, fleetGhost], 'mac').map((s) => s.id)).toEqual([
      newer.id, older.id,
    ]);
  });

  it('breaks a lost_at tie by id, newest first, and treats a null lost_at ghost as oldest', () => {
    const a = row({ kind: 'external', status: 'ghost', lost_at: 5 });
    const b = row({ kind: 'external', status: 'ghost', lost_at: 5 });
    const c = row({ kind: 'external', status: 'ghost', lost_at: null });
    expect(buildLostOutsideFleet([c, a, b], 'all').map((s) => s.id)).toEqual([b.id, a.id, c.id]);
  });
});
```

- [ ] **Step 1.6 — run it.** `npx vitest run src/lib/sidebar_index.test.ts` → expected: `SyntaxError: The requested module '/src/lib/sidebar_index.ts' does not provide an export named 'buildLostOutsideFleet'`.

- [ ] **Step 1.7 — implement.** In `src/lib/sidebar_index.ts` after `buildOutsideFleet` (after line 170) add:

```ts
/** The other half of "Outside fleet": external rows that have ended (lost
 *  on a host reboot, or ghosted). Fleet cannot restore them, so they are a
 *  list to dismiss, not to open — the sidebar draws them as a collapsed
 *  `Lost (n)` tail under the live ones. Same host / scope filter as the
 *  live half; newest loss first, ties by id descending. */
export function buildLostOutsideFleet(
  sessions: readonly SessionRow[],
  hostFilter: string,
  scope: ScopeFilter = null,
): SessionRow[] {
  return sessions
    .filter(
      (s) =>
        s.kind === 'external' &&
        (s.lost_at !== null || s.status === 'ghost') &&
        rowMatches(sessionFilterRow(s, scope?.of), { host: hostFilter, scope: scope?.id ?? 'all' }),
    )
    .slice()
    .sort((a, b) => (b.lost_at ?? 0) - (a.lost_at ?? 0) || b.id - a.id);
}
```

- [ ] **Step 1.8 — run.** `npx vitest run src/lib/sidebar_index.test.ts` → expected: all pass.

- [ ] **Step 1.9 — failing test: reason labels.** Replace the test at `src/lib/sessions.test.ts:39-46` with:

```ts
  it('labels every lost_reason the backend writes; null only for an unknown one', () => {
    expect(lostReasonLabel('host_reboot')).toBe('host rebooted');
    expect(lostReasonLabel('tmux_server_gone')).toBe('tmux server stopped');
    expect(lostReasonLabel('local_disabled')).toBe('host retired');
    expect(lostReasonLabel('missing')).toBe('gone from tmux');
    expect(lostReasonLabel('killed')).toBe('killed');
    expect(lostReasonLabel('something_new')).toBeNull();
    expect(lostReasonLabel(null)).toBeNull();
    expect(lostReasonLabel(undefined)).toBeNull();
  });
```

- [ ] **Step 1.10 — run it.** `npx vitest run src/lib/sessions.test.ts` → expected: 1 failed, `expected 'local host is off on this hub' to be 'host retired'`.

- [ ] **Step 1.11 — implement.** Replace `lostReasonLabel` (`src/lib/sessions.ts:233-243`) with:

```ts
/** Human label for a ghost row's `lost_reason`. `local_disabled` is the hub
 *  retiring its `local` host (`retire_local_sessions`); `missing` is the
 *  reconcile pass no longer seeing the pane; `killed` a kill fleet recorded.
 *  Null only for a reason this build does not know. */
export function lostReasonLabel(reason: string | null | undefined): string | null {
  switch (reason) {
    case 'host_reboot':
      return 'host rebooted';
    case 'tmux_server_gone':
      return 'tmux server stopped';
    case 'local_disabled':
      return 'host retired';
    case 'missing':
      return 'gone from tmux';
    case 'killed':
      return 'killed';
    default:
      return null;
  }
}
```

- [ ] **Step 1.12 — run.** `npx vitest run src/lib/sessions.test.ts` → expected: all pass.

- [ ] **Step 1.13 — failing test: the row draws a lost external row as a ghost.** Append to `src/lib/SessionRowItem.test.ts`:

```ts
describe('SessionRowItem lost external row (F-01)', () => {
  const lostExternal: SessionRow = session('mac', 'bg:6141be6b-cff2-4e28-b51b-13b66606ec55', {
    id: 2,
    kind: 'external',
    status: 'ghost',
    claude_status: 'working',
    lost_at: 1,
    lost_reason: 'host_reboot',
  });

  it('renders the ghost skeleton even when readOnly: lost marker and Dismiss, no status chip', async () => {
    render(SessionRowItem, { props: { ...baseProps(lostExternal), readOnly: true } });
    await tick();
    expect(screen.getByTestId('lost-reason').textContent).toContain('host rebooted');
    expect(screen.getByTestId('ghost-dismiss')).toBeTruthy();
    expect(screen.queryByTestId('claude-chip')).toBeNull();
  });

  it('offers no Recreate on a pane-less kind — restore refuses bg/external', async () => {
    render(SessionRowItem, { props: baseProps(lostExternal) });
    await tick();
    expect(screen.queryByTestId('ghost-recreate')).toBeNull();
    render(SessionRowItem, { props: baseProps(sampleSession) });
    await tick();
    expect(screen.getByTestId('ghost-recreate')).toBeTruthy();
  });
});
```

- [ ] **Step 1.14 — run it.** `npx vitest run src/lib/SessionRowItem.test.ts` → expected: 2 failed; first: `TestingLibraryElementError: Unable to find an element by: [data-testid="lost-reason"]`.

- [ ] **Step 1.15 — implement.** In `src/lib/SessionRowItem.svelte`:

Add `hasNoPane,` to the `./sessions` import list (lines 3-15, next to `isInactiveAgent`).

Replace lines 468-520 (from `{#if readOnly}` through the ghost branch's closing `</div>`) so the ghost branch comes first:

```svelte
    {#if sess.status === 'ghost'}
      <!-- Ghost first, for every kind: a lost external row is an ended
           session (F-01), and the only thing to offer it is Dismiss. The
           read-only branch below is for LIVE outside-fleet rows only. -->
      <span class="status-dot status-ghost" title="ghost — session lost" aria-hidden="true"></span>
      <span class="host-badge" data-testid="host-badge" aria-label="host {sess.host_alias}">{sess.host_alias}</span>
      <span class="sess-name" title={sess.tmux_name}>{
        $showFriendlyNames && sess.friendly_name ? sess.friendly_name : sess.tmux_name
      }</span>
      {#if sess.lost_at}
        <span class="lost-at" title="Lost at {new Date(sess.lost_at * 1000).toLocaleString()}">
          lost {timeAgo(sess.lost_at)}{#if lostReasonLabel(sess.lost_reason)}<span data-testid="lost-reason"> · {lostReasonLabel(sess.lost_reason)}</span>{/if}
        </span>
      {/if}
      <div class="row-actions">
        {#if !hasNoPane(sess)}
          <!-- Restore refuses bg/external (`restore.rs`): no pane to recreate into. -->
          <button
            class="icon-btn small"
            data-testid="ghost-recreate"
            onclick={(e) => doRecreate(sess, e)}
            disabled={!hostIsReachable(sess.host_alias) || recreateBlocked !== null}
            title={recreateBlocked ?? (hostIsReachable(sess.host_alias) ? 'Recreate tmux session' : 'Host is offline')}
            aria-label="Recreate"
          >↺</button>
        {/if}
        <button
          class="icon-btn small danger"
          data-testid="ghost-dismiss"
          onclick={(e) => doDismissGhost(sess, e)}
          disabled={ghostDismissBlocked !== null}
          title={ghostDismissBlocked ?? 'Dismiss ghost session'}
          aria-label="Dismiss"
        >×</button>
      </div>
    {:else if readOnly}
      <!-- "Outside fleet": a LIVE Claude session running entirely outside
           tmux. Read-only — name and status chip only, no actions. -->
      <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
      <span class="sess-name" title={sess.tmux_name}>{primaryName}</span>
      {#if sess.stuck_kind}
        <span
          class="claude-chip stuck-chip"
          data-testid="stuck-chip"
          style="background: {STUCK_COLOR}22; color: {STUCK_COLOR}; border-color: {STUCK_COLOR}66;"
          title="Stuck: {stuckKindLabel(sess.stuck_kind)}"
        >⚠ stuck: {stuckKindLabel(sess.stuck_kind)}</span>
      {:else if sess.claude_status}
        <span
          class="claude-chip"
          data-testid="claude-chip"
          style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
          title="Claude: {sess.claude_status}"
        >{claudeStatusLabel(sess.claude_status)}</span>
      {/if}
    {:else}
```

(The `{:else}` live branch that follows is unchanged.)

- [ ] **Step 1.16 — run.** `npx vitest run src/lib/SessionRowItem.test.ts` → expected: all pass.

- [ ] **Step 1.17 — failing test: the sidebar's `Lost (n)` tail.** Append to `describe('Sidebar (sessions-grouped view)', …)` in `src/lib/Sidebar.test.ts` (after the test ending at line 1624):

```ts
  it('lists a lost external row under a collapsed "Lost (n)" tail with Dismiss, not under Outside fleet', async () => {
    const live = { ...sessionFor(null, 'bg:aaaaaa11-0000-0000-0000-000000000000'), kind: 'external', claude_status: 'idle' as const };
    const lost = { ...sessionFor(null, 'bg:bbbbbb22-0000-0000-0000-000000000000'), kind: 'external', status: 'ghost', lost_at: 5, lost_reason: 'host_reboot' };
    mockBackend(fakeProjects, [live, lost]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('outside-fleet').textContent).toContain('Outside fleet (1)');
    const lostToggle = screen.getByTestId('lost-outside');
    expect(lostToggle.textContent).toContain('Lost (1)');
    expect(screen.queryByTestId('ghost-dismiss')).toBeNull();
    await fireEvent.click(lostToggle);
    await tick();
    expect(screen.getByTestId('ghost-dismiss')).toBeTruthy();
    expect(screen.getByTestId('lost-reason').textContent).toContain('host rebooted');
  });
```

- [ ] **Step 1.18 — run it.** `npx vitest run src/lib/Sidebar.test.ts -t "Lost (n)"` → expected: `TestingLibraryElementError: Unable to find an element by: [data-testid="lost-outside"]`.

- [ ] **Step 1.19 — implement.** In `src/lib/Sidebar.svelte`:

Add `buildLostOutsideFleet,` next to `buildOutsideFleet,` in the `./sidebar_index` import (line 43).

After the `outsideOpen` block (lines 126-128) add:

```ts
  // The ended half of Outside fleet (F-01): collapsed by default, same
  // persistence as the live half.
  let lostOpen = $state(readPref('lost-outside-open', false, isBool));
  $effect(() => {
    writePref('lost-outside-open', lostOpen);
  });
```

After the `outsideFleet` derived (lines 603-607) add:

```ts
  // Lost / ghosted external rows: drawn as ghosts (Dismiss only), never as
  // live rows with a stale status chip.
  const lostOutside = $derived(
    focus
      ? []
      : buildLostOutsideFleet($sessions, $hostFilter, scopeSel).filter((s) => !workPredicate || workPredicate(s)),
  );
```

After the Outside fleet section (after line 1270, the `{/if}` closing `{#if outsideFleet.length > 0}`) add:

```svelte
    {#if lostOutside.length > 0}
      <div class="orphan-section" data-testid="lost-outside-section">
        <button
          class="section-header section-toggle"
          data-testid="lost-outside"
          aria-expanded={lostOpen}
          onclick={() => (lostOpen = !lostOpen)}
        >
          <span class="caret" class:collapsed={!lostOpen}>▾</span>
          Lost ({lostOutside.length})
        </button>
        {#if lostOpen}
          {#each lostOutside as sess (sess.id)}
            {@render sessionRow(sess)}
          {/each}
        {/if}
      </div>
    {/if}
```

(Rendered with `readOnly` false on purpose: the ghost branch is the whole row, and the row's own `onclick` guard `sess.status !== 'ghost' || selectMode` at `SessionRowItem.svelte:435` already refuses to open a ghost.)

- [ ] **Step 1.20 — run.** `npx vitest run src/lib/Sidebar.test.ts src/lib/SessionRowItem.test.ts src/lib/attention.test.ts src/lib/sidebar_index.test.ts src/lib/sessions.test.ts` → expected: all pass. Then `npx svelte-check` → expected: `svelte-check found 0 errors`.

- [ ] **Step 1.21 — commit.**
```bash
git add src/lib/attention.ts src/lib/attention.test.ts src/lib/sidebar_index.ts src/lib/sidebar_index.test.ts src/lib/sessions.ts src/lib/sessions.test.ts src/lib/SessionRowItem.svelte src/lib/SessionRowItem.test.ts src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "fix(sidebar): draw a lost external row as a ghost, under a Lost (n) tail, with every lost_reason labelled

A lost bg:<uuid> row rendered through the read-only branch: live status chip,
no lost marker, no Dismiss (ux F-01). The ghost branch now comes first for
every kind, lost external rows get their own collapsed tail under Outside
fleet, classify() files them in a new last bucket 'lost' so they never outrank
running work (F-05), and lostReasonLabel knows local_disabled / missing / killed."
```

---

### Task 2: `bg:<uuid>` display name (F-06; row 23)

**Files:**
- Modify: `src/lib/attention.ts:301-305` (`displayName`; add `modelShort`, `agentLabel`)
- Modify: `src/lib/SessionRowItem.svelte:150-151` (`primaryName`), ghost-branch name (the `<span class="sess-name">` inside the ghost branch written in Task 1)
- Test: `src/lib/attention.test.ts:175-186`

**Interfaces:**
- Produces `modelShort(model: string | null): string | null` — `claude-opus-4-5-20250929` → `opus-4.5`, `claude-opus-5-20260601` → `opus-5`, `claude-sonnet-4-20250514` → `sonnet-4`, `claude-3-5-haiku-20241022` → `haiku-3.5`, unknown shape → the id without `claude-`/date, null → null.
- Produces `agentLabel(s: Pick<SessionRow, 'tmux_name' | 'model' | 'turn_seq'>): string | null` — `bg:` rows → `agent <first 6 of uuid> · <modelShort> · <turn_seq> turns` (model segment omitted when unknown); other rows → null.
- Produces `displayName(s, friendly)` = `friendly && friendly_name` → friendly name; else `agentLabel(s) ?? tmux_name`. `TerminalView.svelte:1112` already calls it, so the terminal header follows for free.
- Consumes nothing new.

- [ ] **Step 2.1 — failing test.** In `src/lib/attention.test.ts` add `agentLabel, modelShort,` to the import list (lines 2-28) and append inside `describe('stuck transitions', …)` after the `stuckMessage` test (line 186):

```ts
  it('names a bg:<uuid> row after its agent, model and turns (F-06)', () => {
    const bg = row({
      tmux_name: 'bg:6141be6b-cff2-4e28-b51b-13b66606ec55',
      kind: 'external',
      model: 'claude-opus-5-20260601',
      turn_seq: 10,
    });
    expect(displayName(bg, true)).toBe('agent 6141be · opus-5 · 10 turns');
    expect(displayName(bg, false)).toBe('agent 6141be · opus-5 · 10 turns');
    expect(displayName({ ...bg, friendly_name: 'Fix login' }, true)).toBe('Fix login');
    expect(agentLabel({ tmux_name: 'bg:6141be6b-x', model: null, turn_seq: 1 })).toBe('agent 6141be · 1 turn');
    expect(agentLabel({ tmux_name: 'dev-foo', model: 'claude-opus-5-20260601', turn_seq: 3 })).toBeNull();
    expect(modelShort('claude-opus-4-5-20250929')).toBe('opus-4.5');
    expect(modelShort('claude-sonnet-4-20250514')).toBe('sonnet-4');
    expect(modelShort('claude-3-5-haiku-20241022')).toBe('haiku-3.5');
    expect(modelShort('some-vendor-model')).toBe('some-vendor-model');
    expect(modelShort(null)).toBeNull();
  });
```

- [ ] **Step 2.2 — run it.** `npx vitest run src/lib/attention.test.ts` → expected: `SyntaxError: The requested module '/src/lib/attention.ts' does not provide an export named 'agentLabel'`.

- [ ] **Step 2.3 — implement.** Replace `displayName` (`src/lib/attention.ts:302-305`) with:

```ts
const BG_NAME_PREFIX = 'bg:';

/** `claude-opus-4-5-20250929` → `opus-4.5`; `claude-3-5-haiku-20241022` →
 *  `haiku-3.5`; an id of an unknown shape loses only the vendor prefix and
 *  the date. Null in, null out. */
export function modelShort(model: string | null): string | null {
  if (!model) return null;
  const bare = model.replace(/^claude-/, '').replace(/-\d{8}$/, '');
  const named = /^(opus|sonnet|haiku)-(\d+)(?:-(\d+))?$/.exec(bare);
  if (named) return named[3] ? `${named[1]}-${named[2]}.${named[3]}` : `${named[1]}-${named[2]}`;
  const legacy = /^(\d+)(?:-(\d+))?-(opus|sonnet|haiku)$/.exec(bare);
  if (legacy) return legacy[2] ? `${legacy[3]}-${legacy[1]}.${legacy[2]}` : `${legacy[3]}-${legacy[1]}`;
  return bare;
}

/** The label for a `bg:<claude session id>` row (a headless agent fleet
 *  discovered, which no prompt ever named): `agent 6141be · opus-5 · 10
 *  turns`. Null for any other row. The uuid stays in the row's tooltip. */
export function agentLabel(s: Pick<SessionRow, 'tmux_name' | 'model' | 'turn_seq'>): string | null {
  if (!s.tmux_name.startsWith(BG_NAME_PREFIX)) return null;
  const uuid6 = s.tmux_name.slice(BG_NAME_PREFIX.length, BG_NAME_PREFIX.length + 6);
  const parts = [`agent ${uuid6}`];
  const model = modelShort(s.model);
  if (model) parts.push(model);
  parts.push(`${s.turn_seq} turn${s.turn_seq === 1 ? '' : 's'}`);
  return parts.join(' · ');
}

/** The label a row shows in the sidebar, the terminal header and
 *  notifications: the friendly name when shown and set, else the agent label
 *  for a `bg:` row, else the tmux name. ONE name policy — do not add another. */
export function displayName(s: SessionRow, friendly: boolean): string {
  if (friendly && s.friendly_name) return s.friendly_name;
  return agentLabel(s) ?? s.tmux_name;
}
```

- [ ] **Step 2.4 — run.** `npx vitest run src/lib/attention.test.ts` → expected: all pass.

- [ ] **Step 2.5 — wire the row.** In `src/lib/SessionRowItem.svelte` add `displayName,` to the `./attention` import (lines 19-30) and replace line 151:

```ts
  const primaryName = $derived(displayName(sess, $showFriendlyNames));
```

In the ghost branch written in Task 1, replace the name span with:

```svelte
      <span class="sess-name" title={sess.tmux_name}>{primaryName}</span>
```

- [ ] **Step 2.6 — regression test for the row.** Append to `src/lib/SessionRowItem.test.ts`:

```ts
describe('SessionRowItem bg:<uuid> name', () => {
  it('shows the agent label on line 1 and keeps the uuid in the tooltip', async () => {
    const bg = session('mac', 'bg:6141be6b-cff2-4e28-b51b-13b66606ec55', {
      id: 3, kind: 'external', status: 'running', model: 'claude-opus-5-20260601', turn_seq: 10,
    });
    render(SessionRowItem, { props: { ...baseProps(bg), readOnly: true } });
    await tick();
    const name = document.querySelector('.sess-name') as HTMLElement;
    expect(name.textContent).toBe('agent 6141be · opus-5 · 10 turns');
    expect(name.title).toBe('bg:6141be6b-cff2-4e28-b51b-13b66606ec55');
  });
});
```

- [ ] **Step 2.7 — run.** `npx vitest run src/lib/SessionRowItem.test.ts src/lib/attention.test.ts src/lib/Sidebar.test.ts src/lib/TerminalView.hub.test.ts src/lib/agent_context.test.ts` → expected: all pass. `npx svelte-check` → `0 errors`.

- [ ] **Step 2.8 — commit.**
```bash
git add src/lib/attention.ts src/lib/attention.test.ts src/lib/SessionRowItem.svelte src/lib/SessionRowItem.test.ts
git commit -m "feat(sidebar): name bg:<uuid> rows 'agent <uuid6> · <model> · <n> turns' through displayName

Twenty rows read as raw uuids (ux F-06). displayName is the one name policy
already used by the terminal header and notifications; the row now goes
through it too, and a bg: row gets an agent label built from fields the row
already carries. The uuid stays in the tooltip."
```

---

### Task 3: In-project order, `+n idle` fold, no order churn on the focus refetch (F-08, F-16; row 17; UX-92, UXPR-30)

**Files:**
- Modify: `src/lib/sidebar_index.ts:1-9` (imports), after `buildSessionsByProject` (`:142`) add `sortInProject`, `compareInProject`, `foldIdleTail`, `IDLE_FOLD_DEFAULT`
- Modify: `src/lib/sessions.ts:265-289` (add `idleFoldLimit` pref store), `:291-313` (`loadSessions` gains `keepOrder`)
- Modify: `src/lib/Sidebar.svelte:5-17` (imports), `:294-296` (add `idleExpanded`), `:305-322` (`expandAndScrollTo`), `:572-574` (`filteredSessionsByProject`), `:1227-1231` (tree rows + fold row)
- Modify: `src/App.svelte:320` (`loadSessions({ keepOrder: true })`)
- Test: `src/lib/sidebar_index.test.ts`, `src/lib/sessions.test.ts:391-400`, `src/lib/Sidebar.test.ts`

**Interfaces:**
- Produces `compareInProject(a, b, opts: AttentionOptions, friendly: boolean): number` — by `rank().order` (needs-you buckets first, then `working`, `idle`, `lost`); inside `working`/needs-you buckets by `last_activity_at` desc then id; inside `idle`/`lost` by `displayName` (`localeCompare`) then id.
- Produces `sortInProject(rows, opts, friendly): SessionRow[]` (stable copy).
- Produces `foldIdleTail(rows: readonly SessionRow[], opts, limit: number, expanded: boolean): { shown: SessionRow[]; folded: number }` — keeps every non-idle row and the first `limit` idle rows; `folded` = idle rows hidden; `expanded` or `limit <= 0` folds nothing.
- Produces `idleFoldLimit: Writable<number>` (pref `sidebar.idle_fold`, default 8).
- Produces `loadSessions(opts: { force?: boolean; keepOrder?: boolean })` — `keepOrder: true` keeps the store's current positions for rows still listed and appends new rows in list order.
- Consumes `rank`, `classify`, `displayName` (`attention.ts`), `readPref`/`writePref` (`prefs.ts`).

- [ ] **Step 3.1 — failing test: comparator and fold.** Add `compareInProject, foldIdleTail, sortInProject,` to the `./sidebar_index` import in `src/lib/sidebar_index.test.ts` and append:

```ts
describe('sortInProject / foldIdleTail (F-08)', () => {
  const opts = { idleSecs: 0, now: 10_000 };

  it('orders needs-you first, then working by recency, then an alphabetical idle tail', () => {
    const blocked = row({ tmux_name: 'pd-0300', claude_status: 'blocked', last_activity_at: 1 });
    const stuck = row({ tmux_name: 'pd-0200', stuck_kind: 'oom', last_activity_at: 1 });
    const workNew = row({ tmux_name: 'pd-0900', claude_status: 'working', last_activity_at: 500 });
    const workOld = row({ tmux_name: 'pd-0100', claude_status: 'working', last_activity_at: 100 });
    const idleZ = row({ tmux_name: 'pd-0999', claude_status: 'idle', last_activity_at: 900 });
    const idleA = row({ tmux_name: 'pd-0001', claude_status: 'idle', last_activity_at: 1 });
    const sorted = sortInProject([idleZ, workOld, idleA, stuck, workNew, blocked], opts, true);
    expect(sorted.map((s) => s.tmux_name)).toEqual([
      'pd-0300', 'pd-0200', 'pd-0900', 'pd-0100', 'pd-0001', 'pd-0999',
    ]);
  });

  it('is a pure function of content: a rewritten last_activity_at on an idle row does not move it', () => {
    const a = row({ tmux_name: 'a', claude_status: 'idle', last_activity_at: 1 });
    const b = row({ tmux_name: 'b', claude_status: 'idle', last_activity_at: 2 });
    const before = sortInProject([b, a], opts, true).map((s) => s.id);
    const after = sortInProject([{ ...a, last_activity_at: 999 }, b], opts, true).map((s) => s.id);
    expect(after).toEqual(before);
    expect(compareInProject(a, b, opts, true)).toBeLessThan(0);
  });

  it('folds the idle tail beyond the limit and reports how many it hid', () => {
    const rows = [
      row({ claude_status: 'working' }),
      ...Array.from({ length: 5 }, (_, i) => row({ tmux_name: `idle-${i}`, claude_status: 'idle' })),
    ];
    const sorted = sortInProject(rows, opts, true);
    expect(foldIdleTail(sorted, opts, 2, false)).toEqual({ shown: sorted.slice(0, 3), folded: 3 });
    expect(foldIdleTail(sorted, opts, 5, false)).toEqual({ shown: sorted, folded: 0 });
    expect(foldIdleTail(sorted, opts, 2, true)).toEqual({ shown: sorted, folded: 0 });
    expect(foldIdleTail(sorted, opts, 0, false)).toEqual({ shown: sorted, folded: 0 });
  });
});
```

- [ ] **Step 3.2 — run it.** `npx vitest run src/lib/sidebar_index.test.ts` → expected: `SyntaxError: The requested module '/src/lib/sidebar_index.ts' does not provide an export named 'compareInProject'`.

- [ ] **Step 3.3 — implement.** In `src/lib/sidebar_index.ts` add to the imports (after line 9):

```ts
import { classify, displayName, rank, type AttentionOptions } from './attention';
```

and after `buildSessionsByProject` (after line 142):

```ts
/** In-project order (F-08 / F-16): what needs you first, in triage order,
 *  then `working` by recency, then the idle tail alphabetically by display
 *  name. Alphabetical on purpose: `last_activity_at` is rewritten by every
 *  reconcile pass, so an order that read it moved idle rows under the
 *  cursor for no visible reason. This one is a pure function of row content. */
export function compareInProject(
  a: SessionRow,
  b: SessionRow,
  opts: AttentionOptions,
  friendly: boolean,
): number {
  const ra = rank(a, opts);
  const rb = rank(b, opts);
  if (ra.order !== rb.order) return ra.order - rb.order;
  if (ra.bucket === 'idle' || ra.bucket === 'lost') {
    return displayName(a, friendly).localeCompare(displayName(b, friendly)) || a.id - b.id;
  }
  return b.last_activity_at - a.last_activity_at || a.id - b.id;
}

export function sortInProject(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  friendly: boolean,
): SessionRow[] {
  return rows.slice().sort((a, b) => compareInProject(a, b, opts, friendly));
}

export const IDLE_FOLD_DEFAULT = 8;

/** The `+n idle ▸` fold: every non-idle row and the first `limit` idle rows
 *  of an already-sorted project; `folded` is what the toggle hides. */
export function foldIdleTail(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  limit: number,
  expanded: boolean,
): { shown: SessionRow[]; folded: number } {
  if (expanded || limit <= 0) return { shown: rows.slice(), folded: 0 };
  const firstIdle = rows.findIndex((s) => classify(s, opts) === 'idle');
  if (firstIdle < 0) return { shown: rows.slice(), folded: 0 };
  const idle = rows.length - firstIdle;
  if (idle <= limit) return { shown: rows.slice(), folded: 0 };
  return { shown: rows.slice(0, firstIdle + limit), folded: idle - limit };
}
```

- [ ] **Step 3.4 — run.** `npx vitest run src/lib/sidebar_index.test.ts` → expected: all pass.

- [ ] **Step 3.5 — failing test: `keepOrder`.** In `src/lib/sessions.test.ts` after the test ending at line 400 add:

```ts
  it('loadSessions({ keepOrder: true }) keeps the store order for listed rows and appends new ones', async () => {
    sessions.set([{ ...base, id: 1, row_version: 1 }, { ...base, id: 2, row_version: 1 }]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'list_sessions'
        ? [
            { ...base, id: 3, row_version: 2 },
            { ...base, id: 2, row_version: 2, friendly_name: 'renamed' },
            { ...base, id: 1, row_version: 2 },
          ]
        : null,
    );
    await loadSessions({ keepOrder: true });
    expect(get(sessions).map((s) => s.id)).toEqual([1, 2, 3]);
    expect(get(sessions).find((s) => s.id === 2)?.friendly_name).toBe('renamed');
  });

  it('loadSessions({ keepOrder: true }) still drops rows the list no longer has', async () => {
    sessions.set([{ ...base, id: 1 }, { ...base, id: 2, tmux_name: 'gone' }]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'list_sessions' ? [{ ...base, id: 1, row_version: 1 }] : null,
    );
    await loadSessions({ keepOrder: true });
    expect(get(sessions).map((s) => s.id)).toEqual([1]);
  });
```

- [ ] **Step 3.6 — run it.** `npx vitest run src/lib/sessions.test.ts` → expected: 1 failed, `expected [ 3, 2, 1 ] to deeply equal [ 1, 2, 3 ]`.

- [ ] **Step 3.7 — implement.** Replace `loadSessions` (`src/lib/sessions.ts:291-313`) with:

```ts
// `force: true` (the sidebar Refresh button, standalone) makes the backend
// run a fleet reconcile pass now; the default returns stored rows while the
// last pass is within the configured interval. `keepOrder: true` (the 30 s
// focus refetch) takes CONTENT from the list but leaves every listed row at
// its current position and appends new rows — the in-project sort is a pure
// function of content (sidebar_index.ts), so the backend's
// `ORDER BY last_activity_at DESC`, rewritten every reconcile, must not move
// the tree under the cursor on a refetch the user did not ask for.
export async function loadSessions(
  opts: { force?: boolean; keepOrder?: boolean } = {},
): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions', { force: opts.force ?? false });
  if (r.ok) {
    // Events own CONTENT: a `session:updated` that raced this call and is
    // strictly newer still wins over the listed row.
    sessions.update((cur) => {
      const byId = new Map(cur.map((s) => [s.id, s] as const));
      const listed = new Map<number, SessionRow>();
      for (const row of r.value) {
        if (rows.isTombstoned(row.id)) continue;
        const current = byId.get(row.id);
        listed.set(row.id, current && sessionIsStale(row, current) ? current : row);
      }
      if (!opts.keepOrder) return [...listed.values()];
      const next: SessionRow[] = [];
      for (const s of cur) {
        const kept = listed.get(s.id);
        if (kept) {
          next.push(kept);
          listed.delete(s.id);
        }
      }
      return next.concat([...listed.values()]);
    });
    sessionsLoaded.set(true);
  }
  return r;
}
```

Add after the `sidebarGroupBy` block (after line 289):

```ts
// `+n idle ▸` per project (F-08): idle rows beyond this many fold behind a
// toggle. 0 disables the fold.
const isFold = (v: unknown): v is number => typeof v === 'number' && Number.isInteger(v) && v >= 0;
export const idleFoldLimit = writable<number>(readPref('sidebar.idle_fold', 8, isFold));
idleFoldLimit.subscribe((v) => writePref('sidebar.idle_fold', v));
```

- [ ] **Step 3.8 — run.** `npx vitest run src/lib/sessions.test.ts` → expected: all pass (including the untouched `loadSessions adopts the list's order`).

- [ ] **Step 3.9 — failing test: the tree.** Append to `describe('Sidebar (sessions-grouped view)', …)` in `src/lib/Sidebar.test.ts`:

```ts
  it('sorts a project blocked → working → idle (alphabetical) and folds the idle tail past the limit', async () => {
    idleFoldLimit.set(2);
    const rows = [
      { ...sessionFor(1, 'pd-0999'), claude_status: 'idle' as const },
      { ...sessionFor(1, 'pd-0001'), claude_status: 'idle' as const },
      { ...sessionFor(1, 'pd-0500'), claude_status: 'idle' as const },
      { ...sessionFor(1, 'pd-0100'), claude_status: 'working' as const },
      { ...sessionFor(1, 'pd-0300'), claude_status: 'blocked' as const },
    ];
    mockBackend(fakeProjects, rows);
    render(Sidebar);
    await tick(); await tick();
    const names = () => screen.getAllByTestId('sess-row').map((r) => r.querySelector('.sess-name')?.textContent);
    expect(names()).toEqual(['pd-0300', 'pd-0100', 'pd-0001', 'pd-0500']);
    const fold = screen.getByTestId('idle-fold');
    expect(fold.textContent).toContain('+1 idle');
    await fireEvent.click(fold);
    await tick();
    expect(names()).toEqual(['pd-0300', 'pd-0100', 'pd-0001', 'pd-0500', 'pd-0999']);
    idleFoldLimit.set(8);
  });
```

and add `idleFoldLimit` to the `./sessions` import at line 65 of the test file.

- [ ] **Step 3.10 — run it.** `npx vitest run src/lib/Sidebar.test.ts -t "folds the idle tail"` → expected: `AssertionError: expected [ 'pd-0999', 'pd-0001', … ] to deeply equal [ 'pd-0300', 'pd-0100', 'pd-0001', 'pd-0500' ]`.

- [ ] **Step 3.11 — implement.** In `src/lib/Sidebar.svelte`:

Add `idleFoldLimit, showFriendlyNames,` to the `./sessions` import (lines 5-17) if `showFriendlyNames` is not already there, and `foldIdleTail, sortInProject,` to the `./sidebar_index` import (line 43).

After `let collapsedWork` (line 296) add:

```ts
  // Projects whose `+n idle ▸` fold the user opened. Not persisted: the fold
  // is a reading aid for this sitting, and the limit itself is the pref.
  let idleExpanded: Set<number> = $state(new Set());
  function toggleIdleFold(projectId: number, e?: Event) {
    e?.stopPropagation();
    const next = new Set(idleExpanded);
    if (next.has(projectId)) next.delete(projectId);
    else next.add(projectId);
    idleExpanded = next;
  }
```

In `expandAndScrollTo` (lines 305-322) add after the `collapsed` block:

```ts
    // A selected row hidden behind its project's idle fold must be visible.
    if (sess.project_id !== null && !idleExpanded.has(sess.project_id)) {
      idleExpanded = new Set([...idleExpanded, sess.project_id]);
    }
```

Replace `filteredSessionsByProject` (lines 572-574) with:

```ts
  const filteredSessionsByProject = $derived.by(() => {
    const m = buildSessionsByProject($sessions, viewHost, viewBg, treePredicate, viewScope);
    for (const [pid, rows] of m) m.set(pid, sortInProject(rows, attentionOpts, $showFriendlyNames));
    return m;
  });
```

Replace the tree's row loop (lines 1227-1231):

```svelte
            {#if !isCollapsed}
              {@const fold = foldIdleTail(projectSessions, attentionOpts, $idleFoldLimit, idleExpanded.has(row.project.id))}
              {#each fold.shown as sess (sess.id)}
                {@render sessionRow(sess)}
              {/each}
              {#if fold.folded > 0}
                <button
                  class="fold-row"
                  data-testid="idle-fold"
                  onclick={(e) => toggleIdleFold(row.project.id, e)}
                >+{fold.folded} idle ▸</button>
              {:else if idleExpanded.has(row.project.id) && $idleFoldLimit > 0 && projectSessions.length > $idleFoldLimit}
                <button
                  class="fold-row"
                  data-testid="idle-fold"
                  onclick={(e) => toggleIdleFold(row.project.id, e)}
                >fold idle ▾</button>
              {/if}
            {/if}
```

Add to the `<style>` block next to `.section-toggle`:

```css
  .fold-row {
    display: block;
    width: 100%;
    text-align: left;
    padding: 2px 8px 2px 28px;
    font-size: 11px;
    color: var(--muted, #888);
    background: none;
    border: none;
    cursor: pointer;
  }
  .fold-row:hover { color: inherit; }
```

- [ ] **Step 3.12 — the focus refetch.** In `src/App.svelte` line 320 replace `void loadSessions();` with `void loadSessions({ keepOrder: true });`.

- [ ] **Step 3.13 — run.** `npx vitest run src/lib/Sidebar.test.ts src/lib/sessions.test.ts src/lib/sidebar_index.test.ts src/App.hub.test.ts src/App.test.ts` → expected: all pass (`Sidebar.test.ts:1198` "500 sessions … without quadratic blow-up" must still pass: the sort runs once per store change inside the derived). `npx svelte-check` → `0 errors`.

- [ ] **Step 3.14 — commit.**
```bash
git add src/lib/sidebar_index.ts src/lib/sidebar_index.test.ts src/lib/sessions.ts src/lib/sessions.test.ts src/lib/Sidebar.svelte src/lib/Sidebar.test.ts src/App.svelte
git commit -m "feat(sidebar): triage order inside a project, alphabetical idle tail with a +n idle fold, order kept on the focus refetch

The tree kept the backend's ORDER BY last_activity_at, rewritten every
reconcile, so 17 idle rows reshuffled under the cursor every 30 s (ux F-08,
F-16; UX-92). Rows now sort by triage bucket, then recency for working, then
alphabetically for idle — a pure function of content — with idle rows past a
pref'd limit (default 8) behind a per-project fold, and the focus refetch
keeps positions instead of re-adopting the backend order."
```

---

### Task 4: Agent-transport hosts default to Conversation, with "Try anyway" (F-17; row 16)

**Files:**
- Create: `src/lib/ssh_route.ts`
- Modify: `src/lib/session_view.ts:12-26`
- Modify: `src/App.svelte:441`, `:532-536`
- Modify: `src/lib/TerminalView.svelte:94` (state), `:389-398` (open guard), `:1237-1250` (note markup)
- Test: `src/lib/session_view.test.ts`, `src/lib/ssh_route.test.ts` (new), `src/lib/TerminalView.hub.test.ts:118-187`

**Interfaces:**
- Produces `noSshRoute(transport: 'ssh' | 'agent' | undefined, alias: string, overrides: ReadonlySet<string>): boolean` (pure) — true only for `agent` transport without an override.
- Produces stores `sshTryAnyway: Writable<ReadonlySet<string>>`, `noSshRouteFor: Readable<(alias: string) => boolean>` (derived from `hostByAlias` + overrides), fn `tryAnyway(alias)`, `resetSshRouteForTests()`.
- Produces `resolveSessionView(pref, noPane, hasClaudeId, noSshRoute = false)` — after the `noPane` rule: `noSshRoute && hasClaudeId` → `'conversation'`.
- Consumes `hostByAlias` (`hosts.ts:42`).

- [ ] **Step 4.1 — failing tests.** Append to `describe('resolveSessionView', …)` in `src/lib/session_view.test.ts`:

```ts
  it('forces Conversation on a host this machine has no SSH route to, when there is a transcript (F-17)', () => {
    expect(resolveSessionView('terminal', false, true, true)).toBe('conversation');
    expect(resolveSessionView('conversation', false, true, true)).toBe('conversation');
    // No transcript: nothing to show but the terminal's own note.
    expect(resolveSessionView('terminal', false, false, true)).toBe('terminal');
    // The default keeps every existing caller's behaviour.
    expect(resolveSessionView('terminal', false, true)).toBe('terminal');
  });
```

Create `src/lib/ssh_route.test.ts`:

```ts
import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { noSshRoute, noSshRouteFor, tryAnyway, resetSshRouteForTests } from './ssh_route';
import { hosts } from './hosts';
import { host } from './hosts_fixture';

beforeEach(() => {
  resetSshRouteForTests();
  hosts.set([host('trn', { transport: 'agent' }), host('mefistos', { transport: 'ssh' })]);
});

describe('noSshRoute', () => {
  it('is true only for an agent-transport host without an override', () => {
    expect(noSshRoute('agent', 'trn', new Set())).toBe(true);
    expect(noSshRoute('agent', 'trn', new Set(['trn']))).toBe(false);
    expect(noSshRoute('ssh', 'mefistos', new Set())).toBe(false);
    // An unknown host (row not loaded yet) is attached, not refused on a guess.
    expect(noSshRoute(undefined, 'ghost-host', new Set())).toBe(false);
  });

  it('noSshRouteFor reads the hosts store and flips after Try anyway, for this process only', () => {
    expect(get(noSshRouteFor)('trn')).toBe(true);
    expect(get(noSshRouteFor)('mefistos')).toBe(false);
    tryAnyway('trn');
    expect(get(noSshRouteFor)('trn')).toBe(false);
  });
});
```

- [ ] **Step 4.2 — run them.** `npx vitest run src/lib/session_view.test.ts src/lib/ssh_route.test.ts` → expected: `session_view.test.ts` 1 failed (`expected 'terminal' to be 'conversation'`); `ssh_route.test.ts` fails to load: `Error: Failed to resolve import "./ssh_route"`.

- [ ] **Step 4.3 — implement.** Create `src/lib/ssh_route.ts`:

```ts
// Whether THIS machine can `ssh <alias>` at all (F-17).
//
// `transport: 'agent'` says the HUB cannot dial the host — that is why the
// host dials out through fleet-agent. It says nothing certain about this
// machine, which may have an ssh config entry with a ProxyCommand. So the
// default is Conversation (fully hub-routed) and a one-line note, with a
// "Try anyway" that remembers the choice for the life of the process.
import { derived, writable, type Readable, type Writable } from 'svelte/store';
import { hostByAlias } from './hosts';

/** Hosts the user chose to attach anyway. Process lifetime, never persisted:
 *  a route that existed yesterday is not evidence about today. */
export const sshTryAnyway: Writable<ReadonlySet<string>> = writable(new Set());

export function tryAnyway(alias: string): void {
  sshTryAnyway.update((s) => new Set([...s, alias]));
}

export function resetSshRouteForTests(): void {
  sshTryAnyway.set(new Set());
}

/** PURE: no SSH route from here. An unknown transport (host row not loaded)
 *  is treated as `ssh`: attach and let ssh itself say no. */
export function noSshRoute(
  transport: 'ssh' | 'agent' | undefined,
  alias: string,
  overrides: ReadonlySet<string>,
): boolean {
  return transport === 'agent' && !overrides.has(alias);
}

export const noSshRouteFor: Readable<(alias: string) => boolean> = derived(
  [hostByAlias, sshTryAnyway],
  ([$hosts, $overrides]) =>
    (alias: string) =>
      noSshRoute($hosts.get(alias)?.transport, alias, $overrides),
);
```

Replace `resolveSessionView` (`src/lib/session_view.ts:12-26`) with:

```ts
export function resolveSessionView(
  pref: SessionView,
  noPane: boolean,
  hasClaudeId: boolean,
  noSshRoute = false,
): SessionView {
  // No tmux pane (a background agent, an external Claude session) means no
  // PTY. This wins over the missing-transcript case below: a row that is
  // both has nothing else to offer, and ConversationPanel has its own empty
  // state for exactly that.
  if (noPane) return 'conversation';
  // The session has not reported a Claude session id, so there is no
  // transcript to render.
  if (!hasClaudeId) return 'terminal';
  // A pane exists but this machine cannot reach it over ssh (an agent-
  // transport host, F-17): Conversation is hub-routed and loses nothing;
  // the terminal tab keeps a one-line note with "Try anyway".
  if (noSshRoute) return 'conversation';
  return pref;
}
```

- [ ] **Step 4.4 — run.** `npx vitest run src/lib/session_view.test.ts src/lib/ssh_route.test.ts` → expected: all pass.

- [ ] **Step 4.5 — wire App.svelte.** Add `import { noSshRouteFor } from './lib/ssh_route';` next to the `session_view` import (line 56). Near `selNoPane`/`selHasClaudeId` (search `const selNoPane` in `src/App.svelte`) add:

```ts
  const selNoSshRoute = $derived($selectedSession ? $noSshRouteFor($selectedSession.host_alias) : false);
```

Line 441 becomes:

```ts
  const effectiveView = $derived(resolveSessionView($sessionView, selNoPane, selHasClaudeId, selNoSshRoute));
```

Lines 532-536 (`setSessionView`) become:

```ts
  function setSessionView(v: SessionView) {
    if (resolveSessionView(v, selNoPane, selHasClaudeId, selNoSshRoute) !== v) return;
    if (!selNoPane && selHasClaudeId && !selNoSshRoute) sessionView.set(v);
    showSession();
  }
```

- [ ] **Step 4.6 — failing terminal tests.** In `src/lib/TerminalView.hub.test.ts` add `import { tryAnyway, resetSshRouteForTests } from './ssh_route';` after the `hub` import (line 18), call `resetSshRouteForTests();` first thing in the file's `beforeEach`, and REPLACE the test at lines 149-157 (`attaches an agent host rather than refusing on its transport alone`) with:

```ts
  // `transport: 'agent'` says the HUB cannot dial the host; this machine
  // rarely can either (F-17: 30 of 43 sessions, an ssh failure on every
  // selection). So the pane no longer spawns ssh up front: it shows a
  // one-line note and a "Try anyway" that attaches for real.
  it('does not spawn ssh for an agent host; a one-line note offers Try anyway', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'agent' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    const note = screen.getByTestId('terminal-no-attach');
    expect(note.textContent).toContain('no SSH route to trn');
    await fireEvent.click(screen.getByTestId('terminal-try-anyway'));
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });
```

(add `fireEvent` to the `@testing-library/svelte` import on line 1), and in the test at lines 159-174 (`explains the agent transport when the attach actually fails`) insert `tryAnyway('trn');` right after `hosts.set([...])` so the attach is attempted.

- [ ] **Step 4.7 — run them.** `npx vitest run src/lib/TerminalView.hub.test.ts` → expected: the new test fails with `expected true to be false` (pty_open was called).

- [ ] **Step 4.8 — implement.** In `src/lib/TerminalView.svelte`:

Add `import { noSshRouteFor, tryAnyway } from './ssh_route';` with the other `./` imports. After line 94 (`let openError…`) add:

```ts
  /** The host the pane declined to ssh to (F-17); null once attached or when
   *  the selection can be reached. */
  let noRoute: string | null = $state(null);
```

In `openTerm` (line 389), right after `if (!container) return;` (line 395) and before `const target = …`, add:

```ts
    if ($noSshRouteFor(sess.host_alias)) {
      // No ssh from here: say so in one line instead of spawning `ssh` and
      // reporting its failure after the fact. `Try anyway` lifts this for
      // the host and re-runs the open.
      await closeTerm();
      openError = null;
      noRoute = sess.host_alias;
      return;
    }
    noRoute = null;
```

Replace the error markup (lines 1237-1250) with:

```svelte
    {#if noRoute && $selectedSession}
      <div class="err" data-testid="terminal-no-attach">
        Terminal: no SSH route to {noRoute} from this machine (it reaches the hub through fleet-agent).
        Conversation is fully routed.
        <button
          type="button"
          class="link"
          data-testid="terminal-try-anyway"
          onclick={() => { tryAnyway(noRoute!); void openTerm(); }}
        >Try anyway</button>
      </div>
    {:else if openError}
      {#if selectedIsAgentHost && $selectedSession}
        <!-- The attach was tried and failed. `transport: 'agent'` is not a
             claim that no route exists anywhere — only that the hub has
             none — so name what is actually missing: a route from HERE. -->
        <div class="err" data-testid="terminal-agent-transport">
          {$selectedSession.host_alias} is reached through fleet-agent, not SSH, and this machine
          has no SSH route to it. Give it one — an ssh config entry for {$selectedSession.host_alias},
          through a host that can reach it — or move the session somewhere you can attach.
          ({openError})
        </div>
      {:else}
        <div class="err">{openError}</div>
      {/if}
    {/if}
```

- [ ] **Step 4.9 — run.** `npx vitest run src/lib/TerminalView.hub.test.ts src/lib/session_view.test.ts src/lib/ssh_route.test.ts src/App.test.ts src/App.hub.test.ts` → expected: all pass. `npx svelte-check` → `0 errors`.

- [ ] **Step 4.10 — commit.**
```bash
git add src/lib/ssh_route.ts src/lib/ssh_route.test.ts src/lib/session_view.ts src/lib/session_view.test.ts src/App.svelte src/lib/TerminalView.svelte src/lib/TerminalView.hub.test.ts
git commit -m "feat(terminal): default an agent-transport host to Conversation, with a one-line 'no SSH route · Try anyway'

Selecting any of the 30 sessions on claude-fleet-trn spawned ssh, failed, and
explained afterwards, every time (ux F-17). The session view now resolves to
Conversation for a host this machine has no ssh route to, and the terminal
tab shows one line with Try anyway, remembered per host for the process."
```

---

### Task 5: Footer `app · hub · contract`, `hub_version`/`hub_contract` on `HubStatus`, skew banner (F-11; row 15; T7)

**Files:**
- Modify: `src-tauri/src/backend/connection.rs:97-132` (trait `ConnectionReporter`), `:147-217` (`HubConnectionStatus` + `HubReady`)
- Modify: `src-tauri/src/backend/contract.rs:151-162` (add `hub_version`)
- Modify: `src-tauri/src/backend/events.rs:412-416`
- Modify: `src-tauri/src/commands/hub.rs:42-73` (struct), `:88-94` (command), `:212-250` (`logic::status`), `:368`, `:479` (call sites)
- Modify: `src/lib/hub.ts:31-52` (type), add `versionSkew`
- Modify: `src/App.svelte:8` (import), `:919-923` (footer)
- Test: `src-tauri/src/backend/tests_connection.rs`, `src-tauri/src/commands/hub.rs` tests (from `:483`), `src/lib/hub.test.ts`, `src/App.hub.test.ts:127-143`

**Interfaces:**
- Produces Rust `pub struct HubReady { pub version: String, pub contract: u32 }` (connection.rs), `HubConnectionStatus::record_ready(&self, HubReady)`, `HubConnectionStatus::ready(&self) -> Option<HubReady>`, trait default `fn record_ready(&self, _: HubReady) {}`.
- Produces `contract::hub_version(ready_frame_data: &str) -> Option<String>` (reads `"version"` the hub writes at `crates/fleet-core/src/mcp/events_route.rs:665`).
- Produces `HubStatus` fields: `app_version: String` (from `env!("CARGO_PKG_VERSION")`), `#[serde(default)] hub_version: Option<String>`, `#[serde(default)] hub_contract: Option<u32>`; `logic::status(backend, store, tokens, ready: Option<HubReady>)`.
- Produces TS `HubStatus` gains `app_version?: string; hub_version?: string | null; hub_contract?: number | null;` and `versionSkew(app: string | undefined, hub: string | null | undefined): 'hub_ahead' | 'app_ahead' | null` (major.minor compare; null when either is missing/unparseable or equal).
- Consumes `Health.version` (`src/lib/ipc.ts:4-10`) as the hub-version fallback in remote mode (health_check routes to `fleet_health`).

- [ ] **Step 5.1 — failing Rust test: the ready frame is remembered.** Append to `src-tauri/src/backend/tests_connection.rs`:

```rust
#[test]
fn record_ready_keeps_the_hubs_version_and_contract_for_hub_status() {
    let (s, _) = status("cl_t");
    assert!(s.ready().is_none(), "nothing until a ready frame");
    s.record_ready(HubReady {
        version: "0.3.1".into(),
        contract: 4,
    });
    let r = s.ready().expect("recorded");
    assert_eq!(r.version, "0.3.1");
    assert_eq!(r.contract, 4);
    assert!(HubConnectionStatus::standalone().ready().is_none());
}
```

(with `use super::connection::HubReady;` added to the file's imports — check the existing `use` block at lines 1-16 and add the name to it.)

- [ ] **Step 5.2 — run it.** `cargo test -p claude-fleet --lib record_ready_keeps` → expected: `error[E0432]: unresolved import` / `error[E0599]: no method named `ready` found`.

- [ ] **Step 5.3 — implement (connection + contract + events).** In `src-tauri/src/backend/connection.rs`:

Before `pub struct HubConnectionStatus` (line 147) add:

```rust
/// What the hub's `ready` frame said about itself: its version and its wire
/// contract. Kept beside the connection so `hub_status` can show `hub 0.3.1
/// · contract 4` next to this app's own version (ux F-11) — the footer used
/// to print the hub's `fleet_health.version` as if it were the app's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubReady {
    pub version: String,
    pub contract: u32,
}
```

Add the field to the struct (after `confirmed`):

```rust
    /// The last `ready` frame's version and contract; `None` until one
    /// arrives. Survives a socket drop (the hub did not change), replaced by
    /// the next `ready`.
    ready: Mutex<Option<HubReady>>,
```

Initialise `ready: Mutex::new(None),` in both `standalone()` (lines 190-198) and `remote()` (lines 200-208). Add to `impl HubConnectionStatus` after `current()`:

```rust
    pub fn record_ready(&self, ready: HubReady) {
        if let Ok(mut r) = self.ready.lock() {
            *r = Some(ready);
        }
    }

    pub fn ready(&self) -> Option<HubReady> {
        self.ready.lock().ok().and_then(|r| r.clone())
    }
```

Add to trait `ConnectionReporter` (line 97 area) a default method so the test fakes in `tests_routing.rs` need no change:

```rust
    /// The hub's `ready` frame's version and contract, for `hub_status`.
    /// Default no-op: only the real status keeps it.
    fn record_ready(&self, _ready: HubReady) {}
```

and in `impl ConnectionReporter for HubConnectionStatus` (lines 126-132):

```rust
    fn record_ready(&self, ready: HubReady) {
        HubConnectionStatus::record_ready(self, ready)
    }
```

In `src-tauri/src/backend/contract.rs` after `hub_contract_revision` (line 162) add:

```rust
/// The hub's own version from its `ready` frame (`"version"`, written by
/// `fleet_core::mcp::events_route`). `None` when absent or not a string.
pub fn hub_version(ready_frame_data: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(ready_frame_data)
        .ok()
        .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(str::to_owned))
}
```

In `src-tauri/src/backend/events.rs` after line 415 (`let hub_contract = …;`) add:

```rust
                    self.status.record_ready(super::connection::HubReady {
                        version: contract::hub_version(&frame.data).unwrap_or_default(),
                        contract: hub_contract,
                    });
```

- [ ] **Step 5.4 — run.** `cargo test -p claude-fleet --lib record_ready_keeps` → expected: `test backend::tests_connection::record_ready_keeps_the_hubs_version_and_contract_for_hub_status ... ok`.

- [ ] **Step 5.5 — failing Rust test: `HubStatus` carries the three fields.** In `src-tauri/src/commands/hub.rs`'s `#[cfg(test)] mod` (from line 483) find the existing test that calls `logic::status(...)` on a standalone backend (grep `status(` inside the test module) and add beside it:

```rust
    #[test]
    fn status_names_this_apps_version_and_the_hubs_ready_frame() {
        let (backend, store, tokens) = standalone_fixture();
        let none = logic::status(&backend, &store, tokens.as_ref(), None).unwrap();
        assert_eq!(none.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(none.hub_version, None);
        assert_eq!(none.hub_contract, None);
        let ready = crate::backend::connection::HubReady { version: "0.3.1".into(), contract: 4 };
        let some = logic::status(&backend, &store, tokens.as_ref(), Some(ready)).unwrap();
        assert_eq!(some.hub_version.as_deref(), Some("0.3.1"));
        assert_eq!(some.hub_contract, Some(4));
    }
```

(`standalone_fixture()` names whatever helper the existing tests in that module use to build `(Backend, Mutex<Store>, Arc<dyn TokenStore>)`; use that helper's real name — it is the one the neighbouring `status` test already calls.)

- [ ] **Step 5.6 — run it.** `cargo test -p claude-fleet --lib status_names_this_apps_version` → expected: `error[E0061]: this function takes 3 arguments but 4 arguments were supplied`.

- [ ] **Step 5.7 — implement (`commands/hub.rs`).** Add to `HubStatus` (after `unavailable`, line 72):

```rust
    /// This app's own version (`CARGO_PKG_VERSION`) — the footer shows it
    /// beside the hub's, which `health_check` (routed to `fleet_health`)
    /// used to print as if it were the app's (ux F-11).
    pub app_version: String,
    /// The hub's version from its `ready` frame; `None` standalone or before
    /// the first frame.
    #[serde(default)]
    pub hub_version: Option<String>,
    /// The hub's wire-contract revision from the same frame.
    #[serde(default)]
    pub hub_contract: Option<u32>,
```

Change the command (lines 88-94) to:

```rust
#[tauri::command]
pub fn hub_status(
    backend: State<'_, Backend>,
    store: State<'_, Arc<Mutex<Store>>>,
    tokens: State<'_, Arc<dyn TokenStore>>,
    conn: State<'_, Arc<crate::backend::connection::HubConnectionStatus>>,
) -> Result<HubStatus, IpcError> {
    logic::status(&backend, &store, tokens.inner().as_ref(), conn.ready())
}
```

Change `logic::status` signature (line 212) to `pub fn status(backend: &Backend, store: &Mutex<Store>, tokens: &dyn TokenStore, ready: Option<crate::backend::connection::HubReady>) -> Result<HubStatus, IpcError>` and add to the `Ok(HubStatus { … })` literal (line 232):

```rust
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            hub_version: ready.as_ref().map(|r| r.version.clone()),
            hub_contract: ready.as_ref().map(|r| r.contract),
```

At the two internal call sites (`:368` in `pair`, `:479` in `disconnect`) pass `None` as the fourth argument (a pairing/disconnect has no live frame to report; the next `hub_status` read does).

- [ ] **Step 5.8 — run.** `cargo test -p claude-fleet --lib hub` → expected: all `commands::hub` and `backend::tests_connection` tests pass; `cargo clippy -p claude-fleet --all-targets -- -D warnings` → clean; `cargo fmt --all --check` → clean.

- [ ] **Step 5.9 — failing TS test: `versionSkew` and the footer.** In `src/lib/hub.test.ts` add `versionSkew,` to the import (lines 7-19) and append:

```ts
describe('versionSkew', () => {
  it('compares major.minor only, and says which side is behind', () => {
    expect(versionSkew('0.2.42', '0.3.1')).toBe('hub_ahead');
    expect(versionSkew('0.3.1', '0.3.7')).toBeNull();
    expect(versionSkew('0.3.1', '0.3.1')).toBeNull();
    expect(versionSkew('1.0.0', '0.9.9')).toBe('app_ahead');
    expect(versionSkew(undefined, '0.3.1')).toBeNull();
    expect(versionSkew('0.3.1', null)).toBeNull();
    expect(versionSkew('0.3.1', 'dev')).toBeNull();
  });
});
```

In `src/App.hub.test.ts` replace the test at lines 127-143 with:

```ts
  it('names the app, the hub and the contract in the footer, and flags a hub a minor ahead', async () => {
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return { ...remote, app_version: '0.2.42', hub_version: '0.3.1', hub_contract: 4 };
      if (cmd === 'health_check') return { version: '0.3.1', db_ready: true, schema_version: 41 };
      return undefined;
    });
    try {
      render(App);
      const footer = await screen.findByTestId('footer-versions');
      expect(footer.textContent).toBe('app v0.2.42 · hub v0.3.1 · contract 4');
      const skew = await screen.findByTestId('version-skew');
      expect(skew.textContent).toContain('0.3.1');
      expect(skew.getAttribute('title')).toContain('v0.3.1');
    } finally {
      restore();
    }
  });

  it('falls back to health.version for the hub before the first ready frame, and shows no skew badge', async () => {
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return { ...remote, app_version: '9.9.9' };
      if (cmd === 'health_check') return { version: '9.9.9', db_ready: true, schema_version: 41 };
      return undefined;
    });
    try {
      render(App);
      await waitFor(() => expect(screen.getByText(/hub v9\.9\.9/)).toBeInTheDocument());
      expect(screen.queryByTestId('version-skew')).toBeNull();
    } finally {
      restore();
    }
  });
```

- [ ] **Step 5.10 — run them.** `npx vitest run src/lib/hub.test.ts src/App.hub.test.ts` → expected: `hub.test.ts`: `does not provide an export named 'versionSkew'`; `App.hub.test.ts`: `Unable to find an element by: [data-testid="footer-versions"]`.

- [ ] **Step 5.11 — implement (TS).** In `src/lib/hub.ts` add to `HubStatus` (after `unavailable`, line 51):

```ts
  /** This app's own version. Absent only on a status built client-side. */
  app_version?: string;
  /** The hub's version / wire contract from its `ready` frame; null before
   *  the first frame and standalone. */
  hub_version?: string | null;
  hub_contract?: number | null;
```

(`STANDALONE` needs no change: the fields are optional.) Append to `hub.ts`:

```ts
/** major.minor skew between this app and the hub (ux F-11). Null when equal
 *  on major.minor, or when either side is unknown or not `x.y.z`. */
export function versionSkew(
  app: string | undefined,
  hub: string | null | undefined,
): 'hub_ahead' | 'app_ahead' | null {
  const parse = (v: string | null | undefined): [number, number] | null => {
    const m = /^(\d+)\.(\d+)\.\d+/.exec(v ?? '');
    return m ? [Number(m[1]), Number(m[2])] : null;
  };
  const a = parse(app);
  const h = parse(hub);
  if (!a || !h) return null;
  if (h[0] !== a[0]) return h[0] > a[0] ? 'hub_ahead' : 'app_ahead';
  if (h[1] !== a[1]) return h[1] > a[1] ? 'hub_ahead' : 'app_ahead';
  return null;
}
```

In `src/App.svelte` add `versionSkew` to the `./lib/hub` import (search `from './lib/hub'`), then replace line 923 (`<span>v{health.version} · …</span>`) with:

```svelte
    {#if $hubStatus.remote}
      {@const hubVersion = $hubStatus.hub_version ?? health.version}
      {@const skew = versionSkew($hubStatus.app_version, hubVersion)}
      <!-- A hub client: this app's version, the hub's, and the wire contract
           between them. `health.version` is the hub's `fleet_health` answer,
           the fallback until the first `ready` frame names it. -->
      <span data-testid="footer-versions"
        >app v{$hubStatus.app_version ?? '?'} · hub v{hubVersion} · contract {$hubStatus.hub_contract ?? '?'}</span
      >
      {#if skew === 'hub_ahead'}
        <span
          class="hub-badge err"
          data-testid="version-skew"
          title="The hub runs v{hubVersion}; this app is v{$hubStatus.app_version}. Update this app (release tag v{hubVersion})."
          >⬆ app older than hub {hubVersion}</span
        >
      {/if}
    {:else}
      <span data-testid="footer-versions">v{health.version} · db: {health.db_ready ? 'ok' : 'fail'} · schema {health.schema_version}</span>
    {/if}
```

- [ ] **Step 5.12 — run.** `npx vitest run src/lib/hub.test.ts src/App.hub.test.ts src/App.test.ts src/lib/SettingsDialog.hub.test.ts src/lib/TerminalView.hub.test.ts src/lib/Sidebar.test.ts` → expected: all pass (the `App.hub.test.ts:57` standalone test still finds `/schema/`). `npx svelte-check` → `0 errors`.

- [ ] **Step 5.13 — commit.**
```bash
git add src-tauri/src/backend/connection.rs src-tauri/src/backend/contract.rs src-tauri/src/backend/events.rs src-tauri/src/backend/tests_connection.rs src-tauri/src/commands/hub.rs src/lib/hub.ts src/lib/hub.test.ts src/App.svelte src/App.hub.test.ts
git commit -m "feat(footer): show app · hub · contract from HubStatus, and a badge when the hub is a minor ahead

The footer printed the hub's fleet_health.version as if it were the app's:
v0.3.1 at the bottom of a 0.2.42 app (ux F-11). HubStatus now carries
app_version (CARGO_PKG_VERSION) plus hub_version / hub_contract remembered
from the hub's ready frame; the footer reads app v0.2.42 · hub v0.3.1 ·
contract 4 and flags a hub whose major.minor is ahead. Desktop-internal
struct only: no wire contract change."
```

---

### Task 6: Sidebar Refresh in hub mode = Re-list with "updated n s ago" (F-18; row 18)

**Files:**
- Modify: `src/lib/sessions.ts` (add `sessionsUpdatedAt`, `updatedAgo`; stamp in `loadSessions` and `applySessionEvents`)
- Modify: `src/lib/Sidebar.svelte:388-403` (`onRefresh`), `:1001-1021` (props to `SidebarFilters`)
- Modify: `src/lib/SidebarFilters.svelte:60-97` (props), `:132-134` (button)
- Test: `src/lib/sessions.test.ts`, `src/lib/Sidebar.test.ts`

**Interfaces:**
- Produces `sessionsUpdatedAt: Writable<number | null>` (ms epoch of the last list or applied row event).
- Produces `updatedAgo(updatedAtMs: number | null, nowMs: number): string | null` — `null` → null; `< 60 s` → `updated 12 s ago`; else `updated 3m ago` via `timeAgo`.
- Produces `Sidebar.onRefresh(e?: MouseEvent)` — hub mode: `force` only with `altKey`; standalone: `force: true` as today. `SidebarFilters` props gain `relist: boolean`, `updatedAt: number | null`, `nowSec: number`; `onRefresh: (e?: MouseEvent) => void`.
- Consumes `timeAgo` (`session_status.ts:33`), `$hubStatus.remote`.

- [ ] **Step 6.1 — failing test.** Append to `src/lib/sessions.test.ts` (import `sessionsUpdatedAt, updatedAgo` from `./sessions`):

```ts
describe('sessionsUpdatedAt (F-18)', () => {
  it('is stamped by a list and by an applied row event', async () => {
    sessionsUpdatedAt.set(null);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([base]);
    const before = Date.now();
    await loadSessions();
    expect(get(sessionsUpdatedAt)).toBeGreaterThanOrEqual(before);
    sessionsUpdatedAt.set(null);
    applySessionEvents([{ type: 'updated', row: { ...base, id: 1, row_version: 9 } }]);
    expect(get(sessionsUpdatedAt)).toBeGreaterThanOrEqual(before);
  });

  it('updatedAgo reads in seconds under a minute, then minutes', () => {
    expect(updatedAgo(null, 100_000)).toBeNull();
    expect(updatedAgo(88_000, 100_000)).toBe('updated 12 s ago');
    expect(updatedAgo(100_000 - 3 * 60_000, 100_000)).toBe('updated 3m ago');
  });
});
```

- [ ] **Step 6.2 — run it.** `npx vitest run src/lib/sessions.test.ts` → expected: `does not provide an export named 'sessionsUpdatedAt'`.

- [ ] **Step 6.3 — implement.** In `src/lib/sessions.ts` after `sessionsLoaded` (line 257) add:

```ts
/** Ms epoch of the last time the store took rows from the backend — a list
 *  or an applied row event. The sidebar's "updated 12 s ago" (F-18): in hub
 *  mode the hub ticks every 20 s and the button only re-lists, so the user
 *  needs to see that rows are arriving without pressing anything. */
export const sessionsUpdatedAt = writable<number | null>(null);

export function updatedAgo(updatedAtMs: number | null, nowMs: number): string | null {
  if (updatedAtMs === null) return null;
  const secs = Math.max(0, Math.floor((nowMs - updatedAtMs) / 1000));
  if (secs < 60) return `updated ${secs} s ago`;
  return `updated ${timeAgo(Math.floor(updatedAtMs / 1000), nowMs)}`;
}
```

with `import { timeAgo } from './session_status';` added at the top (check `session_status.ts` does not import `sessions.ts` — it imports types only; if it does import the module, move `updatedAgo` into `session_status.ts` instead and re-export nothing). In `loadSessions` after `sessionsLoaded.set(true);` add `sessionsUpdatedAt.set(Date.now());`. In `applySessionEvents` (line 574) after the `sessions.update(...)` call add `sessionsUpdatedAt.set(Date.now());`.

- [ ] **Step 6.4 — run.** `npx vitest run src/lib/sessions.test.ts` → expected: all pass.

- [ ] **Step 6.5 — failing Sidebar test.** Append to `describe('Sidebar (sessions-grouped view)', …)` in `src/lib/Sidebar.test.ts` (the file already imports `hubStatus`, `STANDALONE` and has a `remote` fixture at `:1812-1821`; `hubConnection` too):

```ts
  it('in hub mode the button re-lists (no force), alt-click forces, and it reads "updated n s ago"', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    const btn = screen.getByTestId('sidebar-refresh');
    expect(btn.getAttribute('title')).toContain('Re-list');
    const calls = () =>
      (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_sessions').map((c) => c[1]);
    const n = calls().length;
    await fireEvent.click(btn);
    await tick();
    expect(calls()[n]).toEqual({ force: false });
    await fireEvent.click(btn, { altKey: true });
    await tick();
    expect(calls()[n + 1]).toEqual({ force: true });
    expect(screen.getByTestId('sidebar-updated').textContent).toMatch(/^updated \d+ s ago$/);
  });

  it('standalone the button still forces a reconcile', async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    const calls = () =>
      (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_sessions').map((c) => c[1]);
    const n = calls().length;
    await fireEvent.click(screen.getByTestId('sidebar-refresh'));
    await tick();
    expect(calls()[n]).toEqual({ force: true });
    expect(screen.queryByTestId('sidebar-updated')).toBeNull();
  });
```

- [ ] **Step 6.6 — run it.** `npx vitest run src/lib/Sidebar.test.ts -t "Re-list"` → expected: `expected 'Refresh' to contain 'Re-list'`.

- [ ] **Step 6.7 — implement.** In `src/lib/Sidebar.svelte` replace `onRefresh` (lines 388-403):

```ts
  // Standalone this process owns the fleet and Refresh means "reconcile
  // now". A hub client's fleet is reconciled by the hub's own 20 s tick, so
  // the button only re-lists (F-18); the forced reconcile stays reachable
  // behind ⌥-click, and per host in HostDetail's re-probe.
  async function onRefresh(e?: MouseEvent) {
    loading = true;
    loadError = null;
    const pr = await refreshProjects();
    const force = !$hubStatus.remote || !!e?.altKey;
    const sr = await loadSessions({ force });
    loading = false;
    if (!pr.ok) {
      loadError = pr.error.message;
      pushError(pr.error, 'Refresh projects failed');
    } else if (!sr.ok) {
      loadError = sr.error.message;
      pushError(sr.error, 'Refresh sessions failed');
    }
  }
```

Add `sessionsUpdatedAt,` to the `./sessions` import. In the `<SidebarFilters …/>` block (lines 1001-1021) add:

```svelte
    relist={$hubStatus.remote}
    updatedAt={$sessionsUpdatedAt}
    {nowSec}
```

In `src/lib/SidebarFilters.svelte` add to the props destructuring and type (lines 60-97):

```ts
    relist = false,
    updatedAt = null,
    nowSec = 0,
```
```ts
    onRefresh: (e?: MouseEvent) => void;
    /** Hub mode: the button re-lists; ⌥-click forces a reconcile. */
    relist?: boolean;
    /** Ms epoch of the last list / row event, for "updated n s ago". */
    updatedAt?: number | null;
    nowSec?: number;
```

add `import { updatedAgo } from './sessions';` (the file already imports `sessions` from `./sessions`; extend that import), and replace the button (lines 132-134):

```svelte
    <button
      class="icon-btn"
      onclick={(e) => onRefresh(e)}
      disabled={loading}
      data-testid="sidebar-refresh"
      title={relist ? 'Re-list sessions (the hub reconciles every 20 s; ⌥-click to force a reconcile now)' : 'Refresh'}
    >
      {#if loading}…{:else}↻{/if}
    </button>
    {#if relist && updatedAgo(updatedAt, nowSec * 1000)}
      <span class="updated" data-testid="sidebar-updated">{updatedAgo(updatedAt, nowSec * 1000)}</span>
    {/if}
```

with `.updated { font-size: 10px; color: var(--muted, #888); white-space: nowrap; }` in the style block.

- [ ] **Step 6.8 — run.** `npx vitest run src/lib/Sidebar.test.ts src/lib/sessions.test.ts` → expected: all pass. `npx svelte-check` → `0 errors`.

- [ ] **Step 6.9 — commit.**
```bash
git add src/lib/sessions.ts src/lib/sessions.test.ts src/lib/Sidebar.svelte src/lib/Sidebar.test.ts src/lib/SidebarFilters.svelte
git commit -m "feat(sidebar): Re-list instead of a hub-wide reconcile in hub mode, with 'updated n s ago'

The sidebar's Refresh sent list_sessions { force: true } to the hub — a
reconcile of every reachable host on top of the hub's own 20 s tick, with
nothing on screen saying when rows last arrived (ux F-18). In hub mode the
button re-lists; ⌥-click keeps the forced pass; the store stamps every list
and applied row event and the header reads it."
```

---

### Task 7: Operator row: badge, opens the panel, panel state from the live row, honest `token_revoked` copy (F-19, F-20; row 18; UX-13)

**Files:**
- Modify: `src/lib/sessions.ts` (add `OPERATOR_TMUX_NAME`, `isOperatorRow`)
- Modify: `src/lib/attention.ts` (`displayName` → `Fleet operator`)
- Modify: `src/lib/operator.ts:8-9` (types), `:53-63` (after `operatorRow`: `operatorViewOf`, `operatorView`, `operatorViewLabel`), `:100-105` (`token_revoked` copy)
- Modify: `src/lib/AgentPanel.svelte:1-30` (imports), `:67-80` (blocked/action), `:215-237` (header badge, waiting state)
- Modify: `src/lib/SessionRowItem.svelte:532-541` (badge)
- Modify: `src/lib/Sidebar.svelte:249-256` (`toggleSelected`), `:725-731` (`onSelectSession`)
- Test: `src/lib/operator.test.ts`, `src/lib/AgentPanel.test.ts`, `src/lib/attention.test.ts`, `src/lib/Sidebar.test.ts:818-846`

**Interfaces:**
- Produces `OPERATOR_TMUX_NAME = 'fleet-operator'` (mirrors `crates/fleet-core/src/service/operator.rs:125`), `isOperatorRow(s: Pick<SessionRow, 'tmux_name' | 'kind'>, project: { system: boolean } | undefined): boolean`.
- Produces `type OperatorView = 'unknown' | 'waking' | 'ready' | 'waiting' | OperatorBlocked`; `operatorViewOf(state, row: SessionRow | null): OperatorView` (pure: blocked/unknown/waking states pass through; `row.lost_at` → `'lost'`; `row.claude_status === 'blocked'` → `'waiting'`; else the state); `operatorView: Readable<OperatorView>` derived from `operatorState` + `operatorRow`; `operatorViewLabel(v): string`.
- Produces `blockedCopy('token_revoked')` → `{ title: "The agent's token was revoked, so it can no longer reach the fleet. Restart the agent: a restart mints a new token.", action: 'Restart the agent' }`; `AgentPanel` runs `restartOperator()` for it (the service refuses `kill_session` on the operator — `operator.rs:97-112` — and `restart_session` is the one recovery it allows: `restartOperator` already exists).
- Produces `displayName` → `'Fleet operator'` for a `kind: 'work'` row named `fleet-operator` (the name is fixed and rename-refused, `operator.rs:94-96`), unless a friendly name is shown.
- Consumes `projectById` (`projects.ts:44`), `openAgent` (`operator.ts:148`), `pendingInputFor` (`pending_input.ts:52`), `AnswerPrompt` props `{ session, view, compact?, onOpenTerminal? }`.

- [ ] **Step 7.1 — failing tests (pure).** In `src/lib/operator.test.ts` add `operatorViewOf, operatorView, operatorViewLabel,` to the import (lines 7-20) and append:

```ts
describe('operatorView (F-19)', () => {
  it('derives waiting / lost from the live row while the status call said ready', () => {
    expect(operatorViewOf('ready', row({ claude_status: 'blocked' }))).toBe('waiting');
    expect(operatorViewOf('ready', row({ lost_at: 5 }))).toBe('lost');
    expect(operatorViewOf('ready', row({ claude_status: 'working' }))).toBe('ready');
    expect(operatorViewOf('ready', null)).toBe('ready');
    // A blocked kind or a transitional state is the status call's to clear.
    expect(operatorViewOf('token_revoked', row({ claude_status: 'blocked' }))).toBe('token_revoked');
    expect(operatorViewOf('waking', row({ claude_status: 'blocked' }))).toBe('waking');
    expect(operatorViewOf('unknown', row({ lost_at: 5 }))).toBe('unknown');
  });

  it('the store follows the row-event bus', () => {
    operatorState.set('ready');
    operatorSession.set(row());
    sessions.set([row()]);
    expect(get(operatorView)).toBe('ready');
    applySessionEvents([{ type: 'updated', row: row({ claude_status: 'blocked' }) }]);
    expect(get(operatorView)).toBe('waiting');
  });

  it('labels every view for the header badge', () => {
    expect(operatorViewLabel('waiting')).toBe('waiting for you');
    expect(operatorViewLabel('lost')).toBe('session lost');
    expect(operatorViewLabel('absent')).toBe('not running');
    expect(operatorViewLabel('no_mcp')).toBe('no tools');
    expect(operatorViewLabel('token_revoked')).toBe('no access');
    expect(operatorViewLabel('no_host')).toBe('no host');
    expect(operatorViewLabel('ready')).toBe('ready');
  });
});
```

and replace the assertion for `token_revoked` in the test at lines 80-92 (`absent and lost offer a button; no_mcp, token_revoked and no_host do not`) so that `token_revoked` is expected to OFFER `'Restart the agent'` and the title no longer mentions `Kill`:

```ts
    expect(blockedCopy('token_revoked').action).toBe('Restart the agent');
    expect(blockedCopy('token_revoked').title).not.toMatch(/kill/i);
```

In `src/lib/attention.test.ts` add to the `bg:<uuid>` test from Task 2:

```ts
    expect(displayName(row({ tmux_name: 'fleet-operator', kind: 'work' }), true)).toBe('Fleet operator');
    expect(displayName(row({ tmux_name: 'fleet-operator', kind: 'work', friendly_name: 'Ops' }), true)).toBe('Ops');
```

- [ ] **Step 7.2 — run them.** `npx vitest run src/lib/operator.test.ts src/lib/attention.test.ts` → expected: `operator.test.ts`: `does not provide an export named 'operatorViewOf'`; `attention.test.ts`: `expected 'fleet-operator' to be 'Fleet operator'`.

- [ ] **Step 7.3 — implement (pure).** In `src/lib/sessions.ts` append:

```ts
/** The operator's fixed tmux name (`service/operator.rs` OPERATOR_TMUX_NAME);
 *  rename is refused on it, so the name IS the identity. */
export const OPERATOR_TMUX_NAME = 'fleet-operator';

/** The UX agent's own row: the fixed name, `kind: work`, under the system
 *  project (`fleet/operator`). */
export function isOperatorRow(
  s: Pick<SessionRow, 'tmux_name' | 'kind'>,
  project: { system: boolean } | undefined,
): boolean {
  return s.kind === 'work' && s.tmux_name === OPERATOR_TMUX_NAME && project?.system === true;
}
```

In `src/lib/attention.ts` add `OPERATOR_TMUX_NAME` to the `./sessions` import and change `displayName`:

```ts
export function displayName(s: SessionRow, friendly: boolean): string {
  if (friendly && s.friendly_name) return s.friendly_name;
  if (s.kind === 'work' && s.tmux_name === OPERATOR_TMUX_NAME) return 'Fleet operator';
  return agentLabel(s) ?? s.tmux_name;
}
```

In `src/lib/operator.ts` after `operatorRow` (line 63) add:

```ts
/** What the panel shows: the status call's answer, refined by the LIVE row
 *  (F-19). `waiting` is the row's `claude_status: blocked` — Claude is at a
 *  permission prompt or a question — and is not one of the blocked KINDS
 *  (F-20: those are about the agent's setup, this is about its turn). */
export type OperatorView = 'unknown' | 'waking' | 'ready' | 'waiting' | OperatorBlocked;

export function operatorViewOf(
  state: 'unknown' | 'waking' | 'ready' | OperatorBlocked,
  row: SessionRow | null,
): OperatorView {
  if (state !== 'ready') return state;
  if (!row) return 'ready';
  if (row.lost_at !== null) return 'lost';
  if (row.claude_status === 'blocked') return 'waiting';
  return 'ready';
}

export const operatorView: Readable<OperatorView> = derived(
  [operatorState, operatorRow],
  ([$state, $row]) => operatorViewOf($state, $row),
);

export function operatorViewLabel(v: OperatorView): string {
  switch (v) {
    case 'waiting':
      return 'waiting for you';
    case 'lost':
      return 'session lost';
    case 'absent':
      return 'not running';
    case 'no_mcp':
      return 'no tools';
    case 'token_revoked':
      return 'no access';
    case 'no_host':
      return 'no host';
    default:
      return v;
  }
}
```

Replace the `token_revoked` arm of `blockedCopy` (lines 100-105) with:

```ts
    case 'token_revoked':
      // Kill is refused on the operator's own row (`refuse_if_operator`);
      // restart is the recovery the service allows, and `ensure_operator_on`
      // mints a fresh token for the session it (re)creates.
      return {
        title:
          "The agent's token was revoked, so it can no longer reach the fleet. Restart the agent: a restart mints a new token.",
        action: 'Restart the agent',
      };
```

and update the doc comment above `blockedCopy` (lines 66-80) to drop the sentence saying `token_revoked` has no button.

- [ ] **Step 7.4 — run.** `npx vitest run src/lib/operator.test.ts src/lib/attention.test.ts` → expected: all pass.

- [ ] **Step 7.5 — failing panel test.** Append to `describe('AgentPanel', …)` in `src/lib/AgentPanel.test.ts`:

```ts
  it('says "waiting for you" and offers the dialog choices when the live row is blocked (F-19)', async () => {
    operatorState.set('ready');
    operatorSession.set(row());
    sessions.set([
      row({
        claude_status: 'blocked',
        pending_input: { kind: 'permission', question: 'Allow Bash?', options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: 'No', selected: false }] },
      }),
    ]);
    render(AgentPanel);
    expect(screen.getByTestId('agent-state').textContent).toBe('waiting for you');
    expect(screen.getByTestId('agent-waiting')).toBeTruthy();
    expect(screen.getByRole('button', { name: /^1/ })).toBeTruthy();
  });

  it('a revoked token offers Restart, which restarts the session', async () => {
    operatorState.set('token_revoked');
    invoke.mockResolvedValue(row());
    render(AgentPanel);
    await fireEvent.click(screen.getByRole('button', { name: /restart/i }));
    expect(invoke).toHaveBeenCalledWith('restart_session', expect.anything());
  });
```

- [ ] **Step 7.6 — run it.** `npx vitest run src/lib/AgentPanel.test.ts` → expected: `Unable to find an element by: [data-testid="agent-state"]`.

- [ ] **Step 7.7 — implement (panel).** In `src/lib/AgentPanel.svelte`:

Import `operatorView, operatorViewLabel` from `./operator`, `pendingInputFor` from `./pending_input`, and `AnswerPrompt` from `./AnswerPrompt.svelte`. Replace the `blocked`/`blockedAction` deriveds (lines 67-80):

```ts
  const blocked = $derived(
    $operatorView !== 'ready' && $operatorView !== 'waking' && $operatorView !== 'unknown' && $operatorView !== 'waiting'
      ? blockedCopy($operatorView as OperatorBlocked, $operatorHost)
      : null,
  );
  // `absent` wakes, `lost` and `token_revoked` restart; the rest have no button.
  const blockedAction = $derived(
    $operatorView === 'absent'
      ? () => void openAgent()
      : $operatorView === 'lost' || $operatorView === 'token_revoked'
        ? () => void restartOperator()
        : null,
  );
  // The dialog the operator's pane is showing, when the row says blocked.
  const answerView = $derived(
    session
      ? pendingInputFor({ rowStatus: session.claude_status, rowStuck: session.stuck_kind, rowPending: session.pending_input, probe: null })
      : null,
  );
```

In the header (line 216) after `<span class="who">Agent</span>` add:

```svelte
      <span class="state" data-testid="agent-state">{operatorViewLabel($operatorView)}</span>
```

Replace the `{#if blocked}` block (lines 233-237) with:

```svelte
    {#if blocked}
      <p class="blocked">{blocked.title}</p>
      {#if blocked.action && blockedAction}
        <button onclick={blockedAction}>{blocked.action}</button>
      {/if}
    {:else if $operatorView === 'waiting' && session}
      <p class="blocked" data-testid="agent-waiting">The agent is waiting for you.</p>
      {#if answerView}
        <AnswerPrompt {session} view={answerView} />
      {/if}
    {/if}
    {#if !blocked && session}
```

and close that new `{#if !blocked && session}` where the old `{:else if session}` branch's `{/if}` was (the `ConversationPanel` stays mounted under a waiting state so the answer, once sent, shows in the conversation). Add `.state { margin-left: 8px; font-size: 11px; color: var(--muted, #888); }` to the style block.

- [ ] **Step 7.8 — run.** `npx vitest run src/lib/AgentPanel.test.ts src/lib/AgentPanel.integration.test.ts` → expected: all pass.

- [ ] **Step 7.9 — failing sidebar test.** After the test at `src/lib/Sidebar.test.ts:818-846` (`the project picker hides the UX agent's system project, but the tree still shows its session`) add:

```ts
  it('the operator row carries a badge, reads "Fleet operator", opens the agent panel on click and is never bulk-selected', async () => {
    agentPanelOpen.set(false);
    const operatorProject = {
      project: { id: 9, owner: 'fleet', repo: 'operator', base_path: '/o', last_session_at: 1, adopted: false, system: true },
      worktrees: [],
    };
    const op = sessionFor(9, 'fleet-operator');
    mockBackend([...fakeProjects, operatorProject], [op]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    expect(screen.getByTestId('operator-badge')).toBeTruthy();
    expect(row.querySelector('.sess-name')?.textContent).toBe('Fleet operator');
    await fireEvent.click(row);
    await tick();
    expect(get(agentPanelOpen)).toBe(true);
    expect(get(selectedSession)).toBeNull();
    await fireEvent.click(row, { shiftKey: true });
    await tick();
    expect(screen.queryByTestId('bulk-bar')).toBeNull();
  });
```

(import `agentPanelOpen` from `./operator` at the top of the test file; `mockBackend` answers `operator_status`/`ensure_operator` with its generic sentinel — `openAgent` tolerates a non-ready answer.)

- [ ] **Step 7.10 — run it.** `npx vitest run src/lib/Sidebar.test.ts -t "operator row"` → expected: `Unable to find an element by: [data-testid="operator-badge"]`.

- [ ] **Step 7.11 — implement (row + sidebar).** In `src/lib/SessionRowItem.svelte` add `isOperatorRow,` to the `./sessions` import and `import { projectById } from './projects';`, then next to `primaryName` (line 151):

```ts
  const isOperator = $derived(isOperatorRow(sess, $projectById.get(sess.project_id ?? -1)?.project));
```

and in the live branch after the `bg` badge (line 540) add:

```svelte
          {#if isOperator}
            <span class="operator-badge" data-testid="operator-badge" role="img" title="Fleet operator — the agent acting for you; click opens its panel" aria-label="fleet operator">◎</span>
          {/if}
```

with `.operator-badge { font-size: 11px; color: var(--accent, #6aa9ff); }` in the style block.

In `src/lib/Sidebar.svelte` add `isOperatorRow,` to the `./sessions` import, `import { projectById } from './projects';` (or extend the existing `./projects` import on line 4), `import { openAgent } from './operator';`, and:

in `toggleSelected` (line 249) after the external guard:

```ts
    if (isOperatorRow(sess, $projectById.get(sess.project_id ?? -1)?.project)) return;
```

in `onSelectSession` (line 725) as the FIRST statement:

```ts
    // The operator's row opens its panel (F-19); the terminal is inside the
    // panel's Conversation, and a bulk action on it would only be refused.
    if (isOperatorRow(sess, $projectById.get(sess.project_id ?? -1)?.project)) {
      void openAgent();
      return;
    }
```

- [ ] **Step 7.12 — run.** `npx vitest run src/lib/Sidebar.test.ts src/lib/SessionRowItem.test.ts src/lib/operator.test.ts src/lib/AgentPanel.test.ts src/lib/attention.test.ts src/lib/agent_context.test.ts` → expected: all pass. `npx svelte-check` → `0 errors`.

- [ ] **Step 7.13 — commit.**
```bash
git add src/lib/sessions.ts src/lib/attention.ts src/lib/attention.test.ts src/lib/operator.ts src/lib/operator.test.ts src/lib/AgentPanel.svelte src/lib/AgentPanel.test.ts src/lib/SessionRowItem.svelte src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "feat(operator): mark the operator row, open its panel on click, derive the panel state from the live row

The agent acting for the user was an unmarked row under 'operator' (ux F-19)
and the panel's state was a snapshot from when it opened (UX-13). The row now
reads 'Fleet operator' with a badge, opens the panel, and stays out of bulk
selection; operatorView derives waiting / lost from the live row and the
waiting state offers the pane's dialog choices. token_revoked no longer sends
the user to a Kill the service refuses (F-20): it offers Restart."
```

---

### Task 8: `dismiss_ghost_sessions` — hub tool + Tauri command + "Dismiss all lost (n)" (F-04; row 8; T2)

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/lifecycle.rs:1485-1511` (add `DismissGhostSessionsArgs`, `dismiss_ghost_sessions` after the singular)
- Modify: `crates/fleet-core/src/mcp/tools/session_ops.rs:569-588` (add the tool after the singular)
- Modify: `crates/fleet-core/src/mcp/guard.rs:317-323` (add a `ToolPolicy`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs:1560-1600` (add a fence test after the singular's)
- Regen: `docs/control-api-reference.md`
- Modify: `src-tauri/src/commands/sessions.rs:219-226` (command), `:656-671` (routed)
- Modify: `src-tauri/src/lib.rs:353`
- Modify: `src-tauri/src/backend/verdicts.rs:503-508`
- Modify: `src-tauri/src/backend/tests_routing.rs:1030-1040` (import), `:1550-1562` (case)
- Regen: `src/lib/hub_verdicts.generated.json`, `docs/hub.md` table
- Modify: `src/lib/hub.ts:299-320` (`ROUTED_ACTIONS`), `src/lib/sessions.ts:733-739` (add `dismissGhostSessions`)
- Modify: `src/lib/Sidebar.svelte:241-263` (selection), `:1001-1021` + `SidebarFilters.svelte:339-360` (bulk Dismiss), Task 1's `Lost (n)` header (Dismiss all)
- Modify: `src/lib/HostDetail.svelte:79` (confirm union), `:84-89` (`lostRows`), `:350-375` (button), `:525-560` (dialog)
- Test: `crates/fleet-core/src/service/sessions/lifecycle.rs` tests, `mcp/tools/tests.rs`, `src-tauri/src/backend/tests_routing.rs`, `src/lib/sessions.test.ts`, `src/lib/HostDetail.test.ts`, `src/lib/Sidebar.test.ts`, `src/lib/hub_verdicts.test.ts`

**Interfaces:**
- Produces Rust `DismissGhostSessionsArgs { host_alias: Option<String>, session_ids: Option<Vec<i64>>, all_lost: bool }` (`Serialize, Deserialize, JsonSchema`, every field `#[serde(default)]` — it is an ARGS struct, not a report; the brief's "no serde defaults" applies to report types) and `dismiss_ghost_sessions(args, &Mutex<Store>) -> Result<Vec<i64>, IpcError>`: refuses `E_INVALID` when neither `session_ids` nor `all_lost`; refuses `E_INVALID_STATE` naming the first listed id that is not a ghost (nothing deleted); `E_NOTFOUND` for a listed id that does not exist; deletes every matching ghost through `Store::delete_session` (emits `session:killed` per row) and returns the ids in the order deleted.
- Produces MCP tool `dismiss_ghost_sessions` → `{"dismissed":[…]}`; `Access::Client`, `readonly: false`, `confirm: false` (same as the singular at `guard.rs:317-323` — the desktop confirms in its own dialog, M9.7; see Self-review), `Deadline::Quick`; a per-host token is fenced to its host: an explicit `host_alias` must equal the token's host (`require_host`), an absent one is filled in with the token's host, and every listed id goes through `resolve_target`.
- Produces Tauri command `dismiss_ghost_sessions(args) -> Vec<i64>`, `Verdict::Routed { tool: "dismiss_ghost_sessions" }`.
- Produces TS `dismissGhostSessions(opts: { hostAlias?: string; sessionIds?: number[]; allLost?: boolean }): Promise<Result<number[]>>` — optimistic `removeSession(id)` per returned id; `'dismiss_ghost_sessions'` in `ROUTED_ACTIONS`.
- UI: ghost rows are selectable in select mode (already: `SessionRowItem.svelte:435-439` + `toggleSelected`); the bulk bar shows `Dismiss (n)` when every selected row is a ghost; "Dismiss all lost (n)" in the `Lost (n)` header (Task 1) and in HostDetail's Sessions block; both confirm-gated in the desktop with `ConfirmDialog`.

- [ ] **Step 8.1 — failing service test.** In `crates/fleet-core/src/service/sessions/lifecycle.rs`'s `#[cfg(test)]` module (find the existing tests for `dismiss_ghost_session` — `grep -n "fn dismiss_ghost" crates/fleet-core/src/service/sessions/lifecycle.rs` — and mirror their store fixture; the store helper the module uses is the one those tests call) add:

```rust
    #[test]
    fn dismiss_ghost_sessions_deletes_matching_ghosts_and_refuses_a_live_row_before_deleting_anything() {
        let store = test_store();
        let g1 = insert_ghost(&store, "mac", "bg:aaa");
        let g2 = insert_ghost(&store, "mac", "bg:bbb");
        let g3 = insert_ghost(&store, "local", "bg:ccc");
        let live = insert_live(&store, "mac", "dev-live");

        // Neither ids nor all_lost: refused.
        let err = dismiss_ghost_sessions(
            DismissGhostSessionsArgs { host_alias: None, session_ids: None, all_lost: false },
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);

        // A live id among the list: refused, nothing deleted.
        let err = dismiss_ghost_sessions(
            DismissGhostSessionsArgs { host_alias: None, session_ids: Some(vec![g1, live]), all_lost: false },
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(store.lock().unwrap().get_session_by_id(g1).unwrap().is_some());

        // all_lost on one host.
        let done = dismiss_ghost_sessions(
            DismissGhostSessionsArgs { host_alias: Some("mac".into()), session_ids: None, all_lost: true },
            &store,
        )
        .unwrap();
        assert_eq!(done, vec![g1, g2]);
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(g3).unwrap().is_some(), "other host untouched");
        assert!(s.get_session_by_id(live).unwrap().is_some(), "live row untouched");
    }
```

(`test_store`, `insert_ghost`, `insert_live`: use the module's existing helpers by their real names — the singular's tests insert a ghost row; if no `insert_live` exists, insert a row with `status: "running"` through the same `Store` API those tests use.)

- [ ] **Step 8.2 — run it.** `cargo test -p fleet-core dismiss_ghost_sessions_deletes` → expected: `error[E0425]: cannot find function `dismiss_ghost_sessions` in this scope`.

- [ ] **Step 8.3 — implement (service).** After `dismiss_ghost_session` (line 1511) add:

```rust
#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DismissGhostSessionsArgs {
    /// Only ghosts on this host (default: every host the caller may see).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only these fleet session ids; each must be a ghost.
    #[serde(default)]
    pub session_ids: Option<Vec<i64>>,
    /// Dismiss every ghost the other filters select; required when session_ids is absent.
    #[serde(default)]
    pub all_lost: bool,
}

/// The plural of [`dismiss_ghost_session`] (ux F-04: 19 ghosts were 19
/// calls). Validates the whole selection first — a listed id that is not a
/// ghost refuses the call with nothing deleted — then deletes through
/// `Store::delete_session`, which emits `session:killed` per row. Returns
/// the ids removed, in deletion order.
pub fn dismiss_ghost_sessions(
    args: DismissGhostSessionsArgs,
    store: &Mutex<Store>,
) -> Result<Vec<i64>, IpcError> {
    if args.session_ids.is_none() && !args.all_lost {
        return Err(IpcError::new(
            codes::E_INVALID,
            "dismiss_ghost_sessions: pass session_ids, or all_lost: true",
        ));
    }
    if let Some(h) = &args.host_alias {
        crate::validate::host_alias(h)?;
    }
    let s = lock(store)?;
    let candidates = match &args.host_alias {
        Some(h) => s.list_sessions_for_host(h)?,
        None => s.list_all_sessions()?,
    };
    let targets: Vec<i64> = match &args.session_ids {
        Some(ids) => {
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let row = candidates
                    .iter()
                    .find(|r| r.id == *id)
                    .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {id} not found")))?;
                if row.status != "ghost" {
                    return Err(IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("session {} is not a ghost (status={})", row.id, row.status),
                    ));
                }
                out.push(row.id);
            }
            out
        }
        None => candidates.iter().filter(|r| r.status == "ghost").map(|r| r.id).collect(),
    };
    let mut dismissed = Vec::with_capacity(targets.len());
    for id in targets {
        s.delete_session(id)?;
        dismissed.push(id);
    }
    Ok(dismissed)
}
```

- [ ] **Step 8.4 — run.** `cargo test -p fleet-core dismiss_ghost_sessions_deletes` → expected: `ok`.

- [ ] **Step 8.5 — failing MCP fence test.** In `crates/fleet-core/src/mcp/tools/tests.rs`, right after the test containing lines 1560-1600 (the per-host `forbidden(...)` on `dismiss_ghost_session`), add a test in the same style (`t`, `a` = a per-host caller bound to host `a`, `ghost_b` = a ghost on host `b`, `Caller::master()` — reuse the fixture of the neighbouring test):

```rust
#[tokio::test]
async fn dismiss_ghost_sessions_is_fenced_to_the_tokens_host() {
    let t = fixture_with_two_hosts().await;
    let a = t.caller_for_host("a");
    let ghost_a = t.insert_ghost("a", "bg:a1");
    let ghost_b = t.insert_ghost("b", "bg:b1");

    // Naming the other host: E_FORBIDDEN.
    forbidden(
        t.dismiss_ghost_sessions(
            Extension(a.clone()),
            Parameters(sessions::DismissGhostSessionsArgs {
                host_alias: Some("b".into()),
                session_ids: None,
                all_lost: true,
            }),
        )
        .await
        .unwrap_err(),
    );
    // Naming the other host's ghost by id: E_FORBIDDEN, nothing deleted.
    forbidden(
        t.dismiss_ghost_sessions(
            Extension(a.clone()),
            Parameters(sessions::DismissGhostSessionsArgs {
                host_alias: None,
                session_ids: Some(vec![ghost_b]),
                all_lost: false,
            }),
        )
        .await
        .unwrap_err(),
    );
    assert!(t.store.lock().unwrap().get_session_by_id(ghost_b).unwrap().is_some());

    // all_lost with no host: scoped to the token's own host.
    let out = t
        .dismiss_ghost_sessions(
            Extension(a),
            Parameters(sessions::DismissGhostSessionsArgs {
                host_alias: None,
                session_ids: None,
                all_lost: true,
            }),
        )
        .await
        .unwrap();
    assert_eq!(text_of(&out), format!(r#"{{"dismissed":[{ghost_a}]}}"#));
    let s = t.store.lock().unwrap();
    assert!(s.get_session_by_id(ghost_a).unwrap().is_none());
    assert!(s.get_session_by_id(ghost_b).unwrap().is_some());
}
```

(`fixture_with_two_hosts`, `caller_for_host`, `insert_ghost`, `text_of`: the names the neighbouring test at 1560-1600 uses for the same three things — read that test first and use its helpers verbatim; `forbidden` is the assertion it already calls.)

- [ ] **Step 8.6 — run it.** `cargo test -p fleet-core dismiss_ghost_sessions_is_fenced` → expected: `error[E0599]: no method named `dismiss_ghost_sessions` found`.

- [ ] **Step 8.7 — implement (tool + policy).** In `crates/fleet-core/src/mcp/tools/session_ops.rs` after the singular (line 588) add:

```rust
    #[tool(description = "Delete many ghost rows at once: session_ids, or \
        all_lost: true (optionally host_alias). Returns the ids removed.")]
    pub(super) async fn dismiss_ghost_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::DismissGhostSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "dismiss_ghost_sessions",
            &format!(
                "host={:?} ids={:?} all_lost={}",
                args.host_alias, args.session_ids, args.all_lost
            ),
        );
        // A per-host token acts on its own host only: an explicit host must
        // be that host, an absent one becomes it, and every listed id is
        // resolved through the same gate the singular uses.
        let args = match (&caller.host_alias, &args.host_alias) {
            (Some(_), Some(given)) => {
                require_host(&caller, given, "the ghosts to dismiss")?;
                args
            }
            (Some(own), None) => sessions::DismissGhostSessionsArgs {
                host_alias: Some(own.clone()),
                ..args
            },
            (None, _) => args,
        };
        if let Some(ids) = &args.session_ids {
            for id in ids {
                self.resolve_target(&caller, Some(*id), None, None, "the ghost to dismiss")?;
            }
        }
        let dismissed =
            sessions::dismiss_ghost_sessions(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({ "dismissed": dismissed }))
    }
```

(`require_host` is what `discover_lost_sessions` at `:563` calls in this same file; it comes through `use super::*;`.)

In `crates/fleet-core/src/mcp/guard.rs` after the `dismiss_ghost_session` policy (line 323) add:

```rust
    ToolPolicy {
        name: "dismiss_ghost_sessions",
        access: Access::Client,
        readonly: false,
        confirm: false,
        deadline: Deadline::Quick,
    },
```

- [ ] **Step 8.8 — run.** `cargo test -p fleet-core dismiss_ghost_sessions` → expected: both new tests `ok`. Then `cargo test -p fleet-core every_tool_parameter_is_documented the_served_definition_budget_stays_bounded` → expected: both `ok`. If the budget test fails, it prints the measured surface; raise the constant in `tests.rs` (the test's own doc says to) to the printed number plus 100 and add one sentence to its comment: `Raised for dismiss_ghost_sessions (F-04): <measured> measured, plus 100.` Re-run → `ok`.

- [ ] **Step 8.9 — regenerate the reference.** `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (reports FAILED once while writing), then `cargo test -p fleet-core reference_is_current` → `ok`. `git diff --stat docs/control-api-reference.md` shows the new `### dismiss_ghost_sessions` section.

- [ ] **Step 8.10 — failing routing test.** In `src-tauri/src/backend/tests_routing.rs` add `DismissGhostSessionsArgs,` to the `fleet_core::service::sessions::{…}` import inside `routed_mutation_cases()` (lines 1030-1034) and after the `dismiss_ghost_session` case (lines 1550-1562) add:

```rust
        (
            "dismiss_ghost_sessions",
            "dismiss_ghost_sessions",
            // Every field non-default, so a dropped one fails here.
            json!({ "host_alias": "trn", "session_ids": [7, 9], "all_lost": true }),
            r#"{"dismissed":[7,9]}"#,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::dismiss_ghost_sessions(
                    b,
                    DismissGhostSessionsArgs {
                        host_alias: Some("trn".into()),
                        session_ids: Some(vec![7, 9]),
                        all_lost: true,
                    },
                    s,
                ))
                .map(|ids| assert_eq!(ids, vec![7, 9]))
            }),
        ),
```

- [ ] **Step 8.11 — run it.** `cargo test -p claude-fleet --lib every_routed_mutation_names_its_tool_and_arguments` → expected: `error[E0425]: cannot find function `dismiss_ghost_sessions` in module `commands::sessions::routed``.

- [ ] **Step 8.12 — implement (desktop).** In `src-tauri/src/commands/sessions.rs` add `DismissGhostSessionsArgs` to the `fleet_core::service::sessions::{…}` import; after the `dismiss_ghost_session` command (line 226) add:

```rust
/// The plural: `session_ids`, or `all_lost` (optionally per host). Routes
/// one-to-one onto the hub tool; returns the ids removed.
#[tauri::command]
pub async fn dismiss_ghost_sessions(
    args: DismissGhostSessionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<i64>, IpcError> {
    routed::dismiss_ghost_sessions(&backend, args, &store).await
}
```

and in `mod routed` after `dismiss_ghost_session` (line 671):

```rust
    pub async fn dismiss_ghost_sessions(
        backend: &FleetBackend,
        args: DismissGhostSessionsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<i64>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                #[derive(serde::Deserialize)]
                struct Dismissed {
                    dismissed: Vec<i64>,
                }
                let d: Dismissed = hub.route("dismiss_ghost_sessions", &args).await?;
                Ok(d.dismissed)
            }
            None => sessions::dismiss_ghost_sessions(args, store),
        }
    }
```

In `src-tauri/src/lib.rs` after line 353 (`commands::sessions::dismiss_ghost_session,`) add `commands::sessions::dismiss_ghost_sessions,`.

In `src-tauri/src/backend/verdicts.rs` after the `dismiss_ghost_session` row (line 508) add:

```rust
    (
        "dismiss_ghost_sessions",
        Verdict::Routed {
            tool: "dismiss_ghost_sessions",
        },
    ),
```

- [ ] **Step 8.13 — run.** `cargo test -p claude-fleet --lib backend::tests_routing` → expected: all `ok` (including `every_command_has_a_verdict`, `every_routed_row_is_driven_by_a_case`, `every_routed_tool_is_a_tool_the_hub_serves`). Then `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` (FAILED once while writing), then `cargo test -p claude-fleet --lib verdict_gen` → `ok`; `git diff --stat src/lib/hub_verdicts.generated.json docs/hub.md` shows the new routed entry. `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` → clean.

- [ ] **Step 8.14 — failing TS store test.** Append to `src/lib/sessions.test.ts` (import `dismissGhostSessions`):

```ts
describe('dismissGhostSessions', () => {
  it('passes host_alias / session_ids / all_lost and removes every returned id from the store', async () => {
    sessions.set([{ ...base, id: 1, status: 'ghost' }, { ...base, id: 2, status: 'ghost' }, { ...base, id: 3 }]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([1, 2]);
    const r = await dismissGhostSessions({ hostAlias: 'mac', allLost: true });
    expect(r.ok && r.value).toEqual([1, 2]);
    expect(mockedInvoke).toHaveBeenCalledWith('dismiss_ghost_sessions', {
      args: { host_alias: 'mac', session_ids: null, all_lost: true },
    });
    expect(get(sessions).map((s) => s.id)).toEqual([3]);
  });

  it('defaults: no host, ids as given, all_lost false', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([7]);
    await dismissGhostSessions({ sessionIds: [7] });
    expect(mockedInvoke).toHaveBeenCalledWith('dismiss_ghost_sessions', {
      args: { host_alias: null, session_ids: [7], all_lost: false },
    });
  });
});
```

and to `src/lib/hub_verdicts.test.ts` nothing — but `npx vitest run src/lib/hub_verdicts.test.ts` must pass after `ROUTED_ACTIONS` gains the entry.

- [ ] **Step 8.15 — run it.** `npx vitest run src/lib/sessions.test.ts` → expected: `does not provide an export named 'dismissGhostSessions'`.

- [ ] **Step 8.16 — implement (TS store).** In `src/lib/sessions.ts` after `dismissGhostSession` (line 739) add:

```ts
/** The plural (F-04): every ghost on a host, or the listed ids. The command
 *  answers the ids it removed; each is dropped from the store here, and the
 *  `session:killed` events the hub emits per row are then no-ops. */
export async function dismissGhostSessions(opts: {
  hostAlias?: string;
  sessionIds?: number[];
  allLost?: boolean;
}): Promise<Result<number[]>> {
  const r = await invokeCmd<number[]>('dismiss_ghost_sessions', {
    args: {
      host_alias: opts.hostAlias ?? null,
      session_ids: opts.sessionIds ?? null,
      all_lost: opts.allLost ?? false,
    },
  });
  if (r.ok) for (const id of r.value) removeSession(id);
  return r;
}
```

In `src/lib/hub.ts` `ROUTED_ACTIONS` (line 299-320) add `'dismiss_ghost_sessions',` after `'dismiss_ghost_session',`.

- [ ] **Step 8.17 — run.** `npx vitest run src/lib/sessions.test.ts src/lib/hub_verdicts.test.ts src/lib/hub.test.ts` → expected: all pass.

- [ ] **Step 8.18 — failing HostDetail test.** Append to `describe('HostDetail restore lost sessions', …)` in `src/lib/HostDetail.test.ts` (the file mocks `./sessions`; add `dismissGhostSessions: vi.fn(),` to that mock at lines 6-14 and `const mockedDismiss = dismissGhostSessions as unknown as ReturnType<typeof vi.fn>;` beside the others, importing it):

```ts
  it('offers "Dismiss all lost (n)" for every lost row, confirms, and dismisses by host', async () => {
    mockedDismiss.mockReset();
    mockedDismiss.mockResolvedValue({ ok: true, value: [1, 2, 3] });
    const rows = [lost('mefistos', 'a'), lost('mefistos', 'b', { kind: 'external' }), lost('mefistos', 'c', { claude_session_id: null })];
    mount('mefistos', { hostSessions: rows });
    const btn = screen.getByTestId('dismiss-all-lost');
    expect(btn.textContent).toBe('Dismiss all lost (3)…');
    await fireEvent.click(btn);
    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('3 lost');
    await fireEvent.click(within(dialog).getByTestId('confirm-dismiss-lost'));
    expect(mockedDismiss).toHaveBeenCalledWith({ hostAlias: 'mefistos', allLost: true });
  });

  it('hides the dismiss button with no lost rows', () => {
    mount('mefistos', { hostSessions: [] });
    expect(screen.queryByTestId('dismiss-all-lost')).toBeNull();
  });
```

- [ ] **Step 8.19 — run it.** `npx vitest run src/lib/HostDetail.test.ts` → expected: `Unable to find an element by: [data-testid="dismiss-all-lost"]`.

- [ ] **Step 8.20 — implement (HostDetail).** In `src/lib/HostDetail.svelte`: add `dismissGhostSessions,` to the `./sessions` import (lines 14-21); change line 79 to `let confirm = $state<'rotate' | 'remove' | 'restore' | 'dismiss' | null>(null);`; after `restorable` (line 89) add:

```ts
  // Every lost row on the host, restorable or not (F-03/F-04): the agent rows
  // without a pane can only be dismissed.
  const lostRows = $derived(hostSessions.filter((s) => s.lost_at !== null || s.status === 'ghost'));
  let dismissError = $state<string | null>(null);
  async function onDismissLost() {
    confirm = null;
    busy = true;
    dismissError = null;
    const r = await dismissGhostSessions({ hostAlias: host.alias, allLost: true });
    busy = false;
    if (!r.ok) dismissError = r.error.message;
  }
```

In the Sessions block's `.actions` (after the restore button, line 364) add:

```svelte
        {#if lostRows.length > 0}
          <button
            type="button"
            class="small danger"
            disabled={busy || hubActionBlocked('dismiss_ghost_sessions', $hubStatus, $hubConnection) !== null}
            title={hubActionBlocked('dismiss_ghost_sessions', $hubStatus, $hubConnection) ?? 'Delete every lost row on this host'}
            data-testid="dismiss-all-lost"
            onclick={() => (confirm = 'dismiss')}
            >Dismiss all lost ({lostRows.length})…</button
          >
        {/if}
```

after the `restoreError` paragraph add `{#if dismissError}<p class="error" data-testid="dismiss-error">{dismissError}</p>{/if}`, and after the `restore` ConfirmDialog (line 560) add:

```svelte
{:else if confirm === 'dismiss'}
  <ConfirmDialog
    title="Dismiss {lostRows.length} lost session{lostRows.length === 1 ? '' : 's'} on {host.alias}?"
    body="Their rows are deleted ({restorable.length} of them could still be restored). Nothing on the host is touched."
    confirmLabel="Dismiss"
    confirmTestId="confirm-dismiss-lost"
    danger
    onconfirm={onDismissLost}
    oncancel={() => (confirm = null)}
  />
```

(match the prop names the neighbouring `restore` dialog at `:547-560` uses — `title`/`body`/`confirmLabel`/`confirmTestId`/`onconfirm`/`oncancel` are the ones `Sidebar.svelte:1352-1360` passes; copy that shape.)

- [ ] **Step 8.21 — run.** `npx vitest run src/lib/HostDetail.test.ts` → expected: all pass.

- [ ] **Step 8.22 — failing Sidebar tests (bulk + header).** Append to `describe('Sidebar (sessions-grouped view)', …)`:

```ts
  it('select mode: ghosts are selectable and an all-ghost selection offers Dismiss, which confirms and calls the plural', async () => {
    const g1 = { ...sessionFor(1, 'dev-g1'), status: 'ghost', lost_at: 5 };
    const g2 = { ...sessionFor(1, 'dev-g2'), status: 'ghost', lost_at: 6 };
    mockBackend(fakeProjects, [g1, g2]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [g1, g2];
      if (cmd === 'dismiss_ghost_sessions') return [g1.id, g2.id];
      return [];
    });
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('select-mode'));
    const boxes = screen.getAllByTestId('select-box');
    expect(boxes).toHaveLength(2);
    await fireEvent.click(boxes[0]);
    await fireEvent.click(boxes[1]);
    await tick();
    expect(screen.queryByTestId('bulk-kill')).toBeNull();
    await fireEvent.click(screen.getByTestId('bulk-dismiss'));
    await fireEvent.click(await screen.findByTestId('confirm-bulk-dismiss'));
    await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'dismiss_ghost_sessions');
    expect(call?.[1]).toEqual({ args: { host_alias: null, session_ids: [g1.id, g2.id], all_lost: false } });
  });

  it('the Lost (n) header offers "Dismiss all lost (n)" for the listed external ghosts', async () => {
    const lost = { ...sessionFor(null, 'bg:bbbbbb22-0000-0000-0000-000000000000'), kind: 'external', status: 'ghost', lost_at: 5 };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [lost];
      if (cmd === 'dismiss_ghost_sessions') return [lost.id];
      return [];
    });
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('dismiss-all-lost-outside'));
    await fireEvent.click(await screen.findByTestId('confirm-bulk-dismiss'));
    await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'dismiss_ghost_sessions');
    expect(call?.[1]).toEqual({ args: { host_alias: null, session_ids: [lost.id], all_lost: false } });
  });
```

- [ ] **Step 8.23 — run them.** `npx vitest run src/lib/Sidebar.test.ts -t "Dismiss"` → expected: `Unable to find an element by: [data-testid="bulk-dismiss"]`.

- [ ] **Step 8.24 — implement (Sidebar + SidebarFilters).** In `src/lib/Sidebar.svelte`:

Add `dismissGhostSessions,` to the `./sessions` import. In `toggleSelected` (line 249) replace the external guard with:

```ts
    // Outside-fleet rows are read-only — except a lost one, which Dismiss
    // can act on (F-04). A live external row still cannot be selected.
    if (sess.kind === 'external' && sess.status !== 'ghost') return;
```

After `bulkPromptOpen` (line 246) add:

```ts
  /** The ids a pending bulk Dismiss will delete; null when no dialog is up. */
  let bulkDismiss: number[] | null = $state(null);
  const selectedAllGhosts = $derived(
    selectedRows.length > 0 && selectedRows.every((s) => s.status === 'ghost'),
  );
  async function confirmBulkDismiss() {
    const ids = bulkDismiss ?? [];
    bulkDismiss = null;
    clearSelected();
    const r = await dismissGhostSessions({ sessionIds: ids });
    if (!r.ok) pushError(r.error, 'Dismiss lost sessions failed');
  }
```

In `<SidebarFilters …/>` (lines 1001-1021) add:

```svelte
    canBulkDismiss={selectedAllGhosts}
    onBulkDismiss={() => (bulkDismiss = selectedRows.map((s) => s.id))}
```

In Task 1's `Lost (n)` header, after the toggle button (inside the `orphan-section` div, before `{#if lostOpen}`) add:

```svelte
        <button
          class="icon-btn small danger"
          data-testid="dismiss-all-lost-outside"
          disabled={hubActionBlocked('dismiss_ghost_sessions', $hubStatus, $hubConnection) !== null}
          title={hubActionBlocked('dismiss_ghost_sessions', $hubStatus, $hubConnection) ?? `Dismiss all ${lostOutside.length} lost`}
          aria-label="Dismiss all lost"
          onclick={(e) => { e.stopPropagation(); bulkDismiss = lostOutside.map((s) => s.id); }}
        >Dismiss all lost ({lostOutside.length})</button>
```

After the `bulkKillOpen` ConfirmDialog (lines 1352-1360) add:

```svelte
{#if bulkDismiss}
  <ConfirmDialog
    title="Dismiss {bulkDismiss.length} lost session{bulkDismiss.length === 1 ? '' : 's'}?"
    body="Their rows are deleted. Nothing on any host is touched; a restorable row can no longer be restored from fleet."
    confirmLabel="Dismiss"
    confirmTestId="confirm-bulk-dismiss"
    danger
    onconfirm={confirmBulkDismiss}
    oncancel={() => (bulkDismiss = null)}
  />
{/if}
```

(same `ConfirmDialog` prop shape as the `bulkKillOpen` dialog directly above it.)

In `src/lib/SidebarFilters.svelte` add props `canBulkDismiss = false, onBulkDismiss` (types `canBulkDismiss?: boolean; onBulkDismiss?: () => void;`) and in the bulk bar (lines 339-360) wrap the Kill / Send buttons:

```svelte
      {#if canBulkDismiss}
        <button class="pill danger" data-testid="bulk-dismiss" onclick={onBulkDismiss}>Dismiss</button>
      {:else}
        <!-- existing bulk-send and bulk-kill buttons, unchanged -->
      {/if}
```

(keep the existing `bulk-send`/`bulk-kill` buttons inside the `{:else}`; the `clearSelected` button stays outside the branch.)

- [ ] **Step 8.25 — run.** `npx vitest run src/lib/Sidebar.test.ts src/lib/HostDetail.test.ts src/lib/sessions.test.ts src/lib/hub_verdicts.test.ts src/lib/SessionRowItem.test.ts` → expected: all pass (`Sidebar.test.ts:1476` shift-click bulk kill and `:1499` select mode still pass: their rows are not ghosts). `npx svelte-check` → `0 errors`. `npx vitest run` (whole suite) → all pass.

- [ ] **Step 8.26 — commit.**
```bash
git add crates/fleet-core/src/service/sessions/lifecycle.rs crates/fleet-core/src/mcp/tools/session_ops.rs crates/fleet-core/src/mcp/guard.rs crates/fleet-core/src/mcp/tools/tests.rs docs/control-api-reference.md src-tauri/src/commands/sessions.rs src-tauri/src/lib.rs src-tauri/src/backend/verdicts.rs src-tauri/src/backend/tests_routing.rs src/lib/hub_verdicts.generated.json docs/hub.md src/lib/hub.ts src/lib/sessions.ts src/lib/sessions.test.ts src/lib/Sidebar.svelte src/lib/Sidebar.test.ts src/lib/SidebarFilters.svelte src/lib/HostDetail.svelte src/lib/HostDetail.test.ts
git commit -m "feat(sessions): dismiss_ghost_sessions — bulk ghost dismissal as a hub tool, a routed command and 'Dismiss all lost (n)'

19 ghosts were 19 clicks, 16 of which had no click at all (ux F-04). One
tool/command takes session_ids or all_lost (per host), validates the whole
selection before deleting, is fenced to a per-host token's host, and returns
the ids removed; the desktop confirms in its own dialog and drops each id
optimistically. Ghosts are selectable in select mode, an all-ghost selection
offers Dismiss, and both the Lost (n) tail and HostDetail offer Dismiss all."
```

---

## Self-review

### Spec coverage

| Finding / row | Task |
|---|---|
| F-01 lost `bg:` row drawn as live, no Dismiss (row 7) | 1 (ghost branch first; `Lost (n)` tail; `classify` → `lost`; labels) — `buildOutsideFleet` filter and `local_disabled` label were already on main (PR #316); Task 1 re-words the label to "host retired" |
| F-02 display half (hidden host's rows leak) | 1, partially: lost `local` rows now sit in the collapsed `Lost (n)` tail with a host badge and Dismiss, instead of reading as live. The `sessionVisible` hidden-host rule and hub-side reaping/dedupe are slice A/B work (see "left out") |
| F-04 bulk dismiss (row 8) | 8 |
| F-05 lost outranks running work | 1 (`lost` bucket, last) — only for external rows; see "left out" |
| F-06 `bg:<uuid>` names (row 23) | 2 |
| F-07 agent fold / retire (row 23) | not built — see "left out" |
| F-08 / F-16 in-project order, fold, focus refetch (row 17) | 3 |
| F-11 footer / `HubStatus` version (row 15, T7) | 5 |
| F-17 agent-transport default (row 16) | 4 |
| F-18 Re-list + "updated n s ago" | 6 |
| F-19 operator row + live panel state (row 18) | 7 |
| F-20 `token_revoked` copy / state labels | 7 |
| UX-13 (panel state changed between openings) | 7 (`operatorView` is derived from the live row) |
| UX-92 (`byTriage` exported, never called) | 3 (`rank` drives `compareInProject`) |
| UXPR-13/22/26 | not a dependency: Tasks 1–7 are TS-only or desktop-internal; Task 8 adds a *routed* tool (no LocalOnly row, no `REASONS` entry) |
| lifecycle F4/F6 | relied on for the data side (a lost row keeps its stale `claude_status`; external ghosts keep the 14-day TTL). Task 1 renders through the ghost branch so the stale chip is never shown; Task 8 gives the user the exit F6 says the TTL withholds |

### Deliberately left out (and why)

- **`confirm: true` on `dismiss_ghost_sessions`.** The task brief said "confirm-gated like `dismiss_ghost_session`", but on main the singular is `confirm: false` (`guard.rs:317-323`). A confirm-gated tool on a hub has to be approved on the hub (`hubNextStep`, `hub.ts`), which would make every desktop "Dismiss all lost" a dead end in hub mode. The plan keeps `confirm: false` to match the singular and confirms in the desktop's own `ConfirmDialog` (the M9.7 rule). Flip to `confirm: true` only if the operator wants the hub's approval flow for it.
- **F-05's aging of fleet ghosts (`lifecycle` → `lost` after an hour).** `lost` is used for external rows only. Aging a restorable ghost out of `Needs you` changes the `NEEDS_YOU` semantics `Sidebar.test.ts:1445` pins and belongs with the attention-model work (row 5).
- **F-07: the `Agents (n · m working)` per-project fold, `agents.retire_after_secs`, routing `dismiss_agent_session` (UXPR-26).** Hub setting + parity lane; row 23's second half.
- **F-08's `2 ⚡ · 1 ⏸ · 14` project-header counts.** Not asked for by the task; the fold row already says how many idle rows are hidden.
- **F-11 (b)/(c): Settings → Hub showing the versions and the `E_HUB_CONTRACT` sentence.** The fields are on `HubStatus` now; adding two lines to `SettingsDialog`'s Hub section is a follow-up that needs no further backend work.
- **F-19 (c): automatic `refreshOperator()` on a row event while the panel is in a blocked kind** (so `token_revoked`/`no_host` clear themselves). `operatorView` already follows the row for `waiting`/`lost`; the blocked KINDS come only from `operator_status`, and re-polling it from the event bus is a behaviour change to `AgentFab`'s wake path worth its own review.
- **Tab / window title for the `bg:` name.** Nothing on main writes `document.title` (searched `src/`); there is no tab title to change. The terminal header (`TerminalView.svelte:1112`) already goes through `displayName`.
- **Backend default `friendly_name` for `bg:` rows (UXPR-04).** Wire-visible; the TS-only label is what row 23 asks for first.
- **Hub-side pieces of F-02 (hide_host ghosts with `lost_reason = host_hidden`, reap hidden hosts, dedupe by `claude_session_id`)** — data-sync / lifecycle slices.
