<script lang="ts">
  import { viewKey } from './shortcuts';
  // Work graph M7.3: Tidy up and Reopened, in the attention strip.
  //
  // - "n to tidy", a segment of the sidebar's attention line (redesign 1.2),
  //   appears only when fleet has something to suggest. It is neutral: it never counts toward Needs you. It opens a sheet grouped by
  //   reason, rows preselected, a per-row choice (Clean up by default for
  //   the kill reasons, Archive, Keep for 7 days, Never), and the footer
  //   "Tidy n · Cancel". Under the rows: the cursor row's detail (why, and
  //   what its choice does — the tree a clean up removes and its size), and
  //   the legend "What each choice does". The stopped group has Restore all
  //   (`restore_host_sessions`, per host). G3.12. Keyboard: j/k move, space toggles, ↵
  //   applies, esc closes. Clicking a row narrows the sidebar to that
  //   session and opens it, to look before tidying; closing lifts that.
  // - "Idle, no work linked" (M11.3) rows start unticked and carry their own
  //   Keep 7 d and Safe kill buttons; Safe kill arms first and acts on the
  //   second click. The backend kills such a session only when its worktree
  //   is clean and pushed, and auto-tidy never does (D19).
  // - "n reopened" (accent) lists work that came back after being done,
  //   with its past sessions and Resume; it stays until resumed, done again
  //   or dismissed. A newly reopened item also toasts once.
  import { onDestroy, onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import ResumeButton from './ResumeButton.svelte';
  import Loader from './Loader.svelte';
  import { proposedByLabel } from './ai_proposal';
  import {
    applyItems,
    applyTidy,
    choicesFor,
    defaultChoice,
    dismissReopened,
    formatIdle,
    freedByResults,
    freedKb,
    groupByReason,
    restoreBatches,
    tidyDetail,
    tidyLegend,
    newlyReopened,
    preselected,
    refreshTidy,
    reopenedLoads,
    requestedOnly,
    requestedTicks,
    tidyRequest,
    tidyRequestLive,
    reopenedWork,
    tidyReasonLabel,
    tidyReport,
    TIDY_CHOICE_LABELS,
    KEEP_DAYS,
    tidyEvidence,
    type TidyApplyItem,
    type TidyApplyResult,
    type TidyCandidate,
    type TidyChoice,
  } from './tidy';
  import { push, pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessions } from './sessions';
  import { sessionIdBlocked } from './share';
  import { effectiveScope, scopeOf } from './orgs';
  import { inScope } from './tidy';
  import { clearSessionFocus, focusSession } from './session_focus';
  import { windowHidden } from './window_hidden';
  import { sizeText } from './hosts_table';
  import { restoreHostSessions } from './sessions';

  /** How often the candidates are re-read (they change on the scale of hours). */
  const REFRESH_MS = 60_000;

  // The sidebar's scope (work graph M5) narrows the view, like every other
  // list: a candidate shows when its session is in the chosen scope.
  // Jev's reasons (6.9, Tidy › Duplicates) show with the rest.
  const candidates = $derived(inScope($tidyReport.candidates, $sessions, $effectiveScope, $scopeOf));
  // A request from the Today view's Stale section (M10.4) narrows the sheet
  // to those sessions until "Show all"; the pill's own opening shows all.
  let only = $state<Set<number> | null>(null);
  const shown = $derived(only ? candidates.filter((c) => only!.has(c.session_id)) : candidates);
  const groups = $derived(groupByReason(shown));
  /** Sheet order, flattened: what j/k walk. */
  const ordered = $derived(groups.flatMap((g) => g.items));
  const blocked = $derived(hubActionBlocked('tidy_apply', $hubStatus, $hubConnection));
  /**
   * Both halves, multi-user M1 — and the reason this sheet needed a lookup to
   * get there: a `TidyCandidate` carries a `session_id`, a host and a tmux
   * name, never the row's `owner_person_id`, so the access half cannot be
   * asked about the candidate directly; the row has to be resolved first.
   *
   * `tidy_apply` is `own` in `share.ts::SESSION_TIER` because it can SAFE
   * KILL: until F2a the hub half was the whole gate here, so a `watch` or
   * `drive` grantee could kill a session shared with them from this sheet.
   *
   * ── Not knowing is not permission (F2d) ─────────────────────────────────
   *
   * F2a resolved the row out of `$sessions` by hand and treated "no row" as
   * "nothing to judge, therefore allowed" — `mayTidy` ended `|| !rowById.has(…)`
   * and a candidate this client could not resolve was fully tidyable, SAFE KILL
   * included. The hole is not rare: the candidate list and `$sessions` are
   * fetched independently, the sheet refreshes on its own minute timer, and on a
   * paired desktop the hub fences rows this person may not see off the stream
   * entirely — so "unresolvable" is precisely *someone else's, or gone*.
   *
   * So the resolution is `share.ts::sessionIdBlocked`'s rather than this
   * sheet's: it fails closed with `UNKNOWN_SESSION_REASON` on a fleet this
   * client does not own, and answers `null` on a standalone desktop, where the
   * master owns every row (`access.ts::sessionAccess` rule 1). A paired desktop
   * therefore loses candidates whose row it cannot see; the undo is the
   * backend's, not a wider gate (plan T7: an owner on the link).
   */
  function accessBlocked(sessionId: number): string | null {
    return $sessionIdBlocked(sessionId, 'tidy_apply');
  }
  /** One row's own gate: the hub's refusal first, then this client's access. */
  function rowBlocked(c: TidyCandidate): string | null {
    return blocked ?? accessBlocked(c.session_id);
  }
  /** The candidates this client may actually tidy — the narrowing the footer
   *  counts and `apply` sends, per target rather than once for the sheet: one
   *  sheet mixes this person's sessions with the ones shared with them. */
  const allowed = $derived(ordered.filter((c) => accessBlocked(c.session_id) === null));
  /** How many rows of the sheet belong to somebody else — shown once, so the
   *  footer's smaller count is not a mystery. */
  const notMine = $derived(ordered.length - allowed.length);

  let open = $state(false);
  let reopenedOpen = $state(false);
  let cursor = $state(0);
  let busy = $state(false);
  let ticked = $state<Set<number>>(new Set());
  let choice = $state<Map<number, TidyChoice>>(new Map());
  let sheet = $state<HTMLDivElement | null>(null);
  // Whether a click here set the sidebar focus: only then does closing the
  // sheet clear it (the link-suggestion sheet may own it).
  let focused = false;

  const tickedCount = $derived(allowed.filter((c) => ticked.has(c.session_id)).length);
  /** "frees about 2.1 GB": the measured worktrees the ticked safe kills remove. */
  const freed = $derived(freedKb(allowed, ticked, choice));
  /** "What each choice does": the choices the shown rows offer (G3.12). */
  const legend = $derived(tidyLegend(shown));
  /** The cursor row and what its choice would do (G3.12's row detail). */
  const selected = $derived(ordered[cursor] ?? null);
  const selectedDetail = $derived(
    selected ? tidyDetail(selected, choice.get(selected.session_id) ?? defaultChoice(selected)) : '',
  );
  /** Restore all on the stopped group: the hub's half, then the access half
   *  per session (`restore_host_sessions` is `own`). */
  const restoreBlocked = $derived(hubActionBlocked('restore_host_sessions', $hubStatus, $hubConnection));
  function restoreTargets(items: TidyCandidate[]) {
    return restoreBatches(items, (id) => $sessionIdBlocked(id, 'restore_host_sessions') === null);
  }
  let restoring = $state(false);

  /** Restore every stopped session of the group this client may restore,
   *  one `restore_host_sessions` batch per host. Not destructive: each
   *  resumes its Claude conversation; one that cannot be is reported. */
  async function restoreAll(items: TidyCandidate[]) {
    const batches = restoreTargets(items);
    if (restoring || restoreBlocked !== null || batches.length === 0) return;
    restoring = true;
    let ok = 0;
    let total = 0;
    const failed: string[] = [];
    for (const b of batches) {
      const r = await restoreHostSessions(b.host, { sessionIds: b.ids });
      if (!r.ok) {
        total += b.ids.length;
        failed.push(`${b.host}: ${r.error.message ?? 'restore failed'}`);
        continue;
      }
      for (const x of r.value.results) {
        total += 1;
        if (x.ok) ok += 1;
        else failed.push(`${x.tmux_name}: ${x.error ?? 'unknown error'}`);
      }
    }
    restoring = false;
    void refreshTidy();
    if (failed.length === 0) {
      push({ kind: 'success', message: `Restored ${ok} of ${total} stopped session${total === 1 ? '' : 's'}` });
    } else {
      push({
        kind: 'warning',
        sticky: true,
        message: `Restored ${ok} of ${total} stopped sessions. Failed: ${failed.join('; ')}`,
      });
    }
  }

  function rowName(c: TidyCandidate): string {
    return c.label || c.tmux_name;
  }

  /** The kept session of a `same_work` pair, by its sidebar name. */
  function sameAsName(id: number): string {
    const row = $sessions.find((r) => r.id === id);
    return row ? row.friendly_name || row.tmux_name : `session ${id}`;
  }

  // Whatever had focus when the sheet opened (the pill, a row): closing
  // the sheet hands focus back to it instead of dropping it on <body>.
  let opener: HTMLElement | null = null;

  async function openSheet(requested: number[] = []) {
    if (!open) opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    only = requestedOnly(candidates, requested);
    // Preselection never ticks a row this client may not tidy: a tick that
    // the footer then silently drops would read as a tidy that did nothing.
    const preTicked =
      requested.length > 0
        ? requestedTicks(candidates, { sessionIds: requested, at: 0 })
        : new Set(candidates.filter(preselected).map((c) => c.session_id));
    ticked = new Set([...preTicked].filter((id) => accessBlocked(id) === null));
    choice = new Map();
    cursor = Math.max(
      0,
      ordered.findIndex((c) => requested.includes(c.session_id)),
    );
    open = true;
    reopenedOpen = false;
    await tick();
    sheet?.focus();
  }

  function closeSheet() {
    // Only a sheet that holds focus gives it back: one that empties itself
    // while the person works elsewhere must not pull focus to the pill.
    const hadFocus = !!sheet && sheet.contains(document.activeElement);
    open = false;
    only = null;
    if (focused) clearSessionFocus();
    focused = false;
    if (hadFocus && opener?.isConnected) opener.focus();
    opener = null;
  }

  function focusAt(i: number, e: MouseEvent) {
    cursor = i;
    // The row's own controls (checkbox, choice, PR link, Resume) keep their
    // meaning; only a click on the row itself focuses it.
    if ((e.target as HTMLElement | null)?.closest('input, select, a, button')) return;
    const c = ordered[i];
    if (!c) return;
    if (focusSession(c.session_id, rowName(c))) focused = true;
  }

  function toggle(id: number) {
    // Space ticks the cursor row from the keyboard, past the checkbox's own
    // `disabled`, so the gate is here as well — a tick the footer would then
    // drop is worse than no tick at all.
    if (accessBlocked(id) !== null) return;
    const next = new Set(ticked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    ticked = next;
  }

  function setChoice(id: number, value: string) {
    const next = new Map(choice);
    next.set(id, value as TidyChoice);
    choice = next;
  }

  // The one row whose Safe kill button is armed (a second click applies).
  let armed = $state<number | null>(null);

  /** One row's own button: Keep, or an armed Safe kill. */
  async function applyRow(c: TidyCandidate, action: 'keep' | 'safe_kill') {
    // Re-asked here, not only on the button: a revoke that arrives while the
    // sheet is open must stop the call, and Safe kill is two clicks apart.
    if (busy || rowBlocked(c) !== null) return;
    if (action === 'safe_kill' && armed !== c.session_id) {
      armed = c.session_id;
      return;
    }
    armed = null;
    const item: TidyApplyItem = { session_id: c.session_id, action };
    if (action === 'keep') item.days = KEEP_DAYS;
    busy = true;
    const r = await applyTidy([item]);
    busy = false;
    if (!r.ok) {
      pushError(r.error, action === 'keep' ? 'Keep failed' : 'Safe kill failed');
      return;
    }
    const res = r.value.results[0];
    if (res && !res.ok) {
      push({ kind: 'error', message: `${rowName(c)}: ${res.error ?? action}` });
      return;
    }
    push({
      kind: 'info',
      message: action === 'keep' ? `Kept ${rowName(c)} for ${KEEP_DAYS} days` : `Cleaned up ${rowName(c)}`,
      sub: freedLine(r.value.results),
    });
  }

  /** The toast's second line (Toasts board, "freed 2.1 GB"): what the safe
   *  kills that went through free, as the sheet's footer counts it. */
  function freedLine(results: readonly TidyApplyResult[]): string | undefined {
    const kb = freedByResults(candidates, results);
    return kb !== null && kb > 0 ? `frees about ${sizeText(kb)}` : undefined;
  }

  async function apply() {
    if (busy || blocked !== null) return;
    // Narrowed per target, never one answer for the whole sheet: ↵ applies
    // from anywhere in it, and a session shared with this client at `watch`
    // or `drive` is not this client's to safe-kill.
    const items = applyItems(allowed, ticked, choice);
    if (items.length === 0) return;
    busy = true;
    const r = await applyTidy(items);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Tidy up failed');
      return;
    }
    const failed = r.value.results.filter((x) => !x.ok);
    const done = r.value.results.length - failed.length;
    if (failed.length > 0) {
      push({
        kind: 'error',
        message: `Tidied ${done}; ${failed.length} failed: ${failed
          .map((f) => f.error ?? f.action)
          .join('; ')}`,
        sub: freedLine(r.value.results),
      });
    } else {
      push({ kind: 'info', message: `Tidied ${done} session${done === 1 ? '' : 's'}`, sub: freedLine(r.value.results) });
    }
    closeSheet();
  }

  function onSheetKey(e: KeyboardEvent) {
    // The keys are the registry's `tidy-review` rows (step 0.1).
    const act = viewKey('tidy-review', e);
    if (!act) return;
    // The chords act only from the sheet itself or a row. A keydown that
    // bubbles up from a focused control (Cancel, Resume, the PR link, a
    // checkbox, the choice select) keeps that control's own meaning: Enter
    // activates it and Space toggles the checkbox under the caret, never the
    // cursor row's — and never applies the tidy.
    // Escape closes the sheet from anywhere inside it; it is never destructive.
    if (act === 'tidy-review.close') {
      closeSheet();
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const target = e.target as HTMLElement | null;
    if (target !== e.currentTarget && !target?.classList.contains('tidy-row')) {
      // A select owns its keys; any other control keeps its activating keys
      // (Enter, Space, y/n/Backspace) but j/k and the arrows have no meaning
      // on a checkbox, link or button, so they still move the cursor.
      if (target?.tagName === 'SELECT') return;
      if (act !== 'tidy-review.down' && act !== 'tidy-review.up') return;
    }
    const n = ordered.length;
    switch (act) {
      case 'tidy-review.down':
        cursor = Math.min(n - 1, cursor + 1);
        break;
      case 'tidy-review.up':
        cursor = Math.max(0, cursor - 1);
        break;
      case 'tidy-review.toggle': {
        const c = ordered[cursor];
        if (c) toggle(c.session_id);
        break;
      }
      case 'tidy-review.apply':
        void apply();
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  // A request from elsewhere (the Today view's Stale section): honoured once
  // the candidates are here, dropped when it is too old to still be meant.
  $effect(() => {
    const req = $tidyRequest;
    if (!req) return;
    if (!tidyRequestLive(req)) {
      tidyRequest.set(null);
      return;
    }
    if (candidates.length === 0) return;
    tidyRequest.set(null);
    void openSheet(req.sessionIds);
  });

  $effect(() => {
    if (cursor > 0 && cursor >= ordered.length) cursor = Math.max(0, ordered.length - 1);
    if (open && ordered.length === 0) closeSheet();
    if (reopenedOpen && $reopenedWork.length === 0) reopenedOpen = false;
  });

  let timer: ReturnType<typeof setInterval> | null = null;
  let seenReopened: Set<number> | null = null;
  let unsubscribe: (() => void) | null = null;

  onMount(() => {
    void refreshTidy();
    timer = setInterval(() => {
      if (!windowHidden()) void refreshTidy();
    }, REFRESH_MS);
    // A reopen toasts once; what was already open at the first read is the
    // baseline (the pill shows it).
    unsubscribe = reopenedLoads.subscribe((n) => {
      if (n === 0) return;
      const list = get(reopenedWork);
      if (seenReopened !== null) {
        for (const w of newlyReopened(seenReopened, list)) {
          push({
            kind: 'info',
            message: `${w.key ?? w.title} reopened · ${w.past_sessions} past session${w.past_sessions === 1 ? '' : 's'}`,
            action: { label: 'Show', run: () => (reopenedOpen = true) },
          });
        }
      }
      seenReopened = new Set(list.map((w) => w.item_id));
    });
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
    unsubscribe?.();
  });
</script>

<!-- Segments of the sidebar's attention line (SidebarFilters), siblings of
     the link segment, so the line reads "3 links to review · 4 to tidy". -->
{#if candidates.length > 0}
  <button
    class="al-seg tidy-pill"
    data-testid="tidy-pill"
    title="Finished or duplicate sessions fleet suggests cleaning up — nothing happens until you confirm"
    onclick={() => void openSheet()}
  >{candidates.length} to tidy</button>
{/if}
{#if $reopenedWork.length > 0}
  <button
    class="al-seg reopened-pill"
    data-testid="reopened-pill"
    title="Work that came back after being done"
    onclick={() => {
      reopenedOpen = !reopenedOpen;
      closeSheet();
    }}
  >{$reopenedWork.length} reopened</button>
{/if}

{#if reopenedOpen}
  <div class="tidy-sheet" data-testid="reopened-list">
    <div class="sheet-head">
      <span>Reopened</span>
      <span class="hint">moved out of done in the tracker</span>
      <button class="btn btn--quiet" onclick={() => (reopenedOpen = false)}>Close</button>
    </div>
    {#each $reopenedWork as w (w.item_id)}
      <div class="tidy-row" data-testid="reopened-row">
        <span class="key">{w.key ?? ''}</span>
        <span class="name">{w.title}</span>
        <span class="badge" data-testid="reopened-badge"
          >reopened · {w.past_sessions} past session{w.past_sessions === 1 ? '' : 's'}</span
        >
        {#if w.status_name}<span class="meta">{w.status_name}</span>{/if}
        {#if w.key}<ResumeButton workKey={w.key} />{/if}
        <button
          class="btn btn--quiet is-bounded"
          data-testid="reopened-dismiss"
          disabled={hubActionBlocked('dismiss_reopened', $hubStatus, $hubConnection) !== null}
          onclick={() =>
            void dismissReopened(w.item_id).then((r) => {
              if (!r.ok) pushError(r.error, 'Dismiss failed');
            })}>Dismiss</button
        >
      </div>
    {/each}
  </div>
{/if}

{#if open}
  <div
    class="tidy-sheet"
    data-testid="tidy-sheet"
    role="dialog"
    aria-label="Tidy up"
    tabindex="-1"
    bind:this={sheet}
    onkeydown={onSheetKey}
  >
    <div class="sheet-head">
      <span>Tidy up</span>
      <span class="hint">j/k move · space toggle · ↵ apply · esc close</span>
    </div>
    {#if only && shown.length < candidates.length}
      <p class="hint only" data-testid="tidy-only">
        From Today's Stale · {shown.length} of {candidates.length}
        <!-- Its own keys: the sheet's ↵ would otherwise apply, not widen. -->
        <button
          class="btn btn--quiet"
          data-testid="tidy-show-all"
          onkeydown={(e) => e.stopPropagation()}
          onclick={() => (only = null)}>Show all</button
        >
      </p>
    {/if}
    {#if blocked}<p class="hint" role="note">{blocked}</p>{/if}
    {#if notMine > 0}
      <p class="hint" data-testid="tidy-not-mine">
        {notMine} session{notMine === 1 ? '' : 's'} here {notMine === 1 ? 'is' : 'are'} not yours to
        tidy — shared with you, or no longer visible to this app, so it cannot tell whose
        {notMine === 1 ? 'it is' : 'they are'}. Tidying one is the owner's to do.
      </p>
    {/if}
    {#if busy}
      <!-- Redesign step 5.13: the safe kills check each worktree before it
           goes, so the scan gets the manual's Hex field over the list. -->
      <div class="tidy-scan" data-testid="tidy-scan">
        <Loader name="hex-field" size={48} label="Checking worktrees" />
        <span>Checking {tickedCount} worktree{tickedCount === 1 ? '' : 's'}…</span>
      </div>
    {/if}
    <!-- The rows as a tree (redesign step 7.2): one group per reason, so a
         row's checkbox, buttons and select are its own controls, not the
         hidden children of an option. -->
    <div role="tree" aria-label="Sessions to tidy" aria-multiselectable="true">
    {#each groups as g (g.reason)}
      <div role="group" aria-label={tidyReasonLabel(g.reason)}>
      <div class="group-line">
        <div class="group-head" data-testid="tidy-group">{tidyReasonLabel(g.reason)} · {g.items.length}</div>
        {#if g.reason === 'ghost_expiring'}
          {@const targets = restoreTargets(g.items)}
          <!-- G3.12: the stopped group's Restore all, the same
               `restore_host_sessions` the sidebar's lost fold runs. -->
          <button
            class="btn btn--quiet is-bounded group-action"
            data-testid="tidy-restore-all"
            disabled={restoring || restoreBlocked !== null || targets.length === 0}
            title={restoreBlocked ??
              (targets.length === 0
                ? 'None of these sessions are yours to restore'
                : 'Restore each stopped session; it resumes its Claude conversation')}
            onkeydown={(e) => e.stopPropagation()}
            onclick={() => void restoreAll(g.items)}>{restoring ? 'Restoring…' : 'Restore all'}</button
          >
        {/if}
      </div>
      {#each g.items as c (c.session_id)}
        {@const i = ordered.indexOf(c)}
        {@const choices = choicesFor(c)}
        <!-- The ACCESS half alone for the tick (a down hub leaves the rows
             listed and ticked — only the sending is off, which `blocked`
             already says once at the top of the sheet); both halves for the
             buttons, which actually send. -->
        {@const notYours = accessBlocked(c.session_id)}
        <div
          class="tidy-row"
          class:cursor={i === cursor}
          data-testid="tidy-row"
          data-session-id={c.session_id}
          role="treeitem"
          aria-selected={ticked.has(c.session_id)}
          tabindex="-1"
          title="Show only this session in the sidebar"
          onclick={(e) => focusAt(i, e)}
          onkeydown={() => {}}
        >
          <input
            type="checkbox"
            data-testid="tidy-check"
            checked={ticked.has(c.session_id) && notYours === null}
            disabled={choices.length === 0 || notYours !== null}
            title={notYours ?? ''}
            aria-label="Tidy {rowName(c)}"
            onchange={() => toggle(c.session_id)}
          />
          <span class="name">{rowName(c)}</span>
          <span class="meta">{[c.host_alias, c.branch].filter(Boolean).join(' · ')}</span>
          {#if c.key}<span class="key">{c.key}{c.item_status ? ` · ${c.item_status}` : ''}</span>{/if}
          {#if c.pr_url}<a class="meta" href={c.pr_url} target="_blank" rel="noreferrer">PR</a>{/if}
          {#if c.expires_at}
            <span class="meta">expires {formatIdle(c.expires_at - Math.floor(Date.now() / 1000))}</span>
          {:else}
            <span class="meta" data-testid="tidy-evidence">{tidyEvidence(c)}</span>
          {/if}
          {#if c.reason === 'same_work' && c.same_as != null}
            <span class="meta" data-testid="tidy-same-work">same work as {sameAsName(c.same_as)}</span>
            <span class="jev" data-testid="tidy-same-work-by">{proposedByLabel('jev')}</span>
          {/if}
          {#if (c.secondary ?? []).length > 0}
            <span class="meta">also: {(c.secondary ?? []).map(tidyReasonLabel).join(', ')}</span>
          {/if}
          {#if c.reason === 'idle_unlinked'}
            <span class="warn" title="No work is linked, so fleet will not guess what uncommitted work is for: a dirty or unpushed worktree is refused, not killed"
              >only if clean &amp; pushed</span
            >
            <button
              class="btn btn--quiet is-bounded"
              data-testid="tidy-keep"
              disabled={busy || rowBlocked(c) !== null}
              title={rowBlocked(c) ?? `Leave it out of Tidy up for ${KEEP_DAYS} days`}
              onkeydown={(e) => e.stopPropagation()}
              onclick={() => void applyRow(c, 'keep')}>{TIDY_CHOICE_LABELS.keep}</button
            >
            <button
              class="btn btn--quiet is-bounded"
              class:armed={armed === c.session_id}
              data-testid="tidy-safe-kill"
              disabled={busy || rowBlocked(c) !== null}
              title={rowBlocked(c) ?? ''}
              onkeydown={(e) => e.stopPropagation()}
              onclick={() => void applyRow(c, 'safe_kill')}
              >{armed === c.session_id ? 'Confirm clean up' : TIDY_CHOICE_LABELS.safe_kill}</button
            >
          {:else if c.action === 'safe_kill'}
            <span class="warn" title="Claude is asked to commit and push first; the worktree is removed only if that succeeds"
              >commits &amp; pushes first</span
            >
          {/if}
          {#if c.action === 'resume_or_expire' && c.key}<ResumeButton workKey={c.key} sessionId={c.session_id} />{/if}
          {#if choices.length > 0}
            <select
              aria-label="What to do with {rowName(c)}"
              data-testid="tidy-choice"
              value={choice.get(c.session_id) ?? defaultChoice(c)}
              onchange={(e) => setChoice(c.session_id, (e.currentTarget as HTMLSelectElement).value)}
            >
              {#each choices as ch (ch)}<option value={ch}>{TIDY_CHOICE_LABELS[ch]}</option>{/each}
            </select>
          {/if}
        </div>
      {/each}
      </div>
    {/each}
    </div>
    {#if selected}
      <p class="tidy-detail" data-testid="tidy-detail" aria-live="polite">
        <span class="detail-name">Selected: {rowName(selected)}</span>
        {selectedDetail}
      </p>
    {/if}
    {#if legend.length > 0}
      <details class="tidy-legend" data-testid="tidy-legend">
        <summary>What each choice does</summary>
        <dl>
          {#each legend as l (l.label)}
            <dt>{l.label}</dt>
            <dd>{l.help}</dd>
          {/each}
        </dl>
      </details>
    {/if}
    <div class="sheet-foot">
      {#if tickedCount > 0 && freed !== null && freed > 0}
        <span class="foot-meta" data-testid="tidy-frees">{tickedCount} selected · frees about {sizeText(freed)}</span>
      {/if}
      <button
        class="btn btn--primary"
        data-testid="tidy-apply"
        disabled={busy || tickedCount === 0 || blocked !== null}
        onclick={() => void apply()}>Tidy {tickedCount}</button
      >
      <button class="btn btn--quiet" data-testid="tidy-cancel" onclick={closeSheet}>Cancel</button>
    </div>
  </div>
{/if}

<style>
  .tidy-scan {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  /* Segments of the attention line (SidebarFilters' .attention-line). */
  .al-seg {
    order: 0;
    border: none;
    background: none;
    padding: 0.1rem 0.15rem;
    font: inherit;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    cursor: pointer;
    border-radius: var(--radius-sm);
  }
  .al-seg:hover {
    color: var(--fg);
    text-decoration: underline;
  }
  .al-seg:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .reopened-pill {
    color: var(--accent);
  }
  .tidy-sheet {
    order: 1;
    flex: 1 0 100%;
    box-sizing: border-box;
    margin: 0.25rem 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 0.3rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: var(--text-2xs);
    outline: none;
  }
  .tidy-sheet:focus-visible {
    border-color: var(--accent);
  }
  .sheet-head,
  .sheet-foot {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .sheet-foot {
    justify-content: flex-end;
    padding-top: 0.2rem;
  }
  .foot-meta {
    margin-right: auto;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .group-line {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .group-action {
    margin-left: auto;
    color: var(--accent);
  }
  .tidy-detail {
    margin: 0.2rem 0 0;
    padding: 0.3rem;
    border-top: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .detail-name {
    color: var(--fg);
    font-weight: 500;
    margin-right: 0.3rem;
  }
  .tidy-legend summary {
    cursor: pointer;
    color: var(--fg-muted);
  }
  .tidy-legend dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 0.15rem 0.6rem;
    margin: 0.3rem 0 0;
  }
  .tidy-legend dt {
    color: var(--fg);
  }
  .tidy-legend dd {
    margin: 0;
    color: var(--fg-muted);
  }
  .group-head {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding-top: 0.2rem;
  }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    flex: 1;
  }
  .only {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 2px 0;
  }
  .tidy-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    padding: 0.15rem 0.3rem;
    border-radius: var(--radius-sm);
  }
  .tidy-row.cursor {
    background: var(--bg-hover);
  }
  .key {
    font-family: var(--font-mono);
  }
  .meta {
    color: var(--fg-muted);
  }
  /* Jev's mark on a `same_work` row (6.9), as ProposedBy draws it. */
  .jev {
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    color: var(--accent);
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    white-space: nowrap;
  }
  .jev::before {
    content: '\2726 ';
  }
  .warn {
    color: var(--usage-warn);
    font-size: var(--text-2xs);
  }
  .badge {
    color: var(--accent);
  }
  .armed {
    color: var(--usage-warn);
    border-color: var(--usage-warn);
  }
</style>
