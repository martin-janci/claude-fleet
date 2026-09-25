<script lang="ts">
  // Work graph M7.3: Tidy up and Reopened, in the attention strip.
  //
  // - "Tidy up · n" appears only when fleet has something to suggest. It is
  //   neutral: it never counts toward Needs you. It opens a sheet grouped by
  //   reason, rows preselected, a per-row choice (Safe kill by default for
  //   the kill reasons, Archive only, Snooze 7 d, Never for this work), and
  //   the footer "Tidy n · Cancel". Keyboard: j/k move, space toggles, ↵
  //   applies, esc closes. Clicking a row narrows the sidebar to that
  //   session and opens it, to look before tidying; closing lifts that.
  // - "Reopened · n" (accent) lists work that came back after being done,
  //   with its past sessions and Resume; it stays until resumed, done again
  //   or dismissed. A newly reopened item also toasts once.
  import { onDestroy, onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import ResumeButton from './ResumeButton.svelte';
  import {
    applyItems,
    applyTidy,
    choicesFor,
    defaultChoice,
    dismissReopened,
    formatIdle,
    groupByReason,
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
    type TidyCandidate,
    type TidyChoice,
  } from './tidy';
  import { push, pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessions } from './sessions';
  import { effectiveScope, scopeOf } from './orgs';
  import { inScope } from './tidy';
  import { clearSessionFocus, focusSession } from './session_focus';

  /** How often the candidates are re-read (they change on the scale of hours). */
  const REFRESH_MS = 60_000;

  // The sidebar's scope (work graph M5) narrows the view, like every other
  // list: a candidate shows when its session is in the chosen scope.
  const candidates = $derived(
    inScope($tidyReport.candidates, $sessions, $effectiveScope, $scopeOf),
  );
  // A request from the Today view's Stale section (M10.4) narrows the sheet
  // to those sessions until "Show all"; the pill's own opening shows all.
  let only = $state<Set<number> | null>(null);
  const shown = $derived(only ? candidates.filter((c) => only!.has(c.session_id)) : candidates);
  const groups = $derived(groupByReason(shown));
  /** Sheet order, flattened: what j/k walk. */
  const ordered = $derived(groups.flatMap((g) => g.items));
  const blocked = $derived(hubActionBlocked('tidy_apply', $hubStatus, $hubConnection));

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

  const tickedCount = $derived(ordered.filter((c) => ticked.has(c.session_id)).length);

  function rowName(c: TidyCandidate): string {
    return c.label || c.tmux_name;
  }

  async function openSheet(requested: number[] = []) {
    only = requestedOnly(candidates, requested);
    ticked =
      requested.length > 0
        ? requestedTicks(candidates, { sessionIds: requested, at: 0 })
        : new Set(candidates.filter(preselected).map((c) => c.session_id));
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
    open = false;
    only = null;
    if (focused) clearSessionFocus();
    focused = false;
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

  async function apply() {
    if (busy || blocked !== null) return;
    const items = applyItems(ordered, ticked, choice);
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
      });
    } else {
      push({ kind: 'info', message: `Tidied ${done} session${done === 1 ? '' : 's'}` });
    }
    closeSheet();
  }

  function onSheetKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    // The chords act only from the sheet itself or a row. A keydown that
    // bubbles up from a focused control (Cancel, Resume, the PR link, a
    // checkbox, the choice select) keeps that control's own meaning: Enter
    // activates it and Space toggles the checkbox under the caret, never the
    // cursor row's — and never applies the tidy.
    // Escape closes the sheet from anywhere inside it; it is never destructive.
    if (e.key === 'Escape') {
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
      if (!['j', 'k', 'ArrowDown', 'ArrowUp'].includes(e.key)) return;
    }
    const n = ordered.length;
    switch (e.key) {
      case 'j':
      case 'ArrowDown':
        cursor = Math.min(n - 1, cursor + 1);
        break;
      case 'k':
      case 'ArrowUp':
        cursor = Math.max(0, cursor - 1);
        break;
      case ' ': {
        const c = ordered[cursor];
        if (c) toggle(c.session_id);
        break;
      }
      case 'Enter':
        void apply();
        break;
      case 'Escape':
        closeSheet();
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
    timer = setInterval(() => void refreshTidy(), REFRESH_MS);
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

{#if candidates.length > 0 || $reopenedWork.length > 0}
  <div class="tidy-pills">
    {#if candidates.length > 0}
      <button
        class="pill tidy-pill"
        data-testid="tidy-pill"
        title="Finished or duplicate sessions fleet suggests cleaning up — nothing happens until you confirm"
        onclick={() => void openSheet()}
      >Tidy up · {candidates.length}</button>
    {/if}
    {#if $reopenedWork.length > 0}
      <button
        class="pill reopened-pill"
        data-testid="reopened-pill"
        title="Work that came back after being done"
        onclick={() => {
          reopenedOpen = !reopenedOpen;
          closeSheet();
        }}
      >Reopened · {$reopenedWork.length}</button>
    {/if}
  </div>
{/if}

{#if reopenedOpen}
  <div class="tidy-sheet" data-testid="reopened-list">
    <div class="sheet-head">
      <span>Reopened</span>
      <span class="hint">moved out of done in the tracker</span>
      <button class="pill" onclick={() => (reopenedOpen = false)}>close</button>
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
          class="pill"
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
    role="listbox"
    aria-label="Tidy up"
    aria-multiselectable="true"
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
          class="pill"
          data-testid="tidy-show-all"
          onkeydown={(e) => e.stopPropagation()}
          onclick={() => (only = null)}>Show all</button
        >
      </p>
    {/if}
    {#if blocked}<p class="hint" role="note">{blocked}</p>{/if}
    {#each groups as g (g.reason)}
      <div class="group-head" data-testid="tidy-group">{tidyReasonLabel(g.reason)} · {g.items.length}</div>
      {#each g.items as c (c.session_id)}
        {@const i = ordered.indexOf(c)}
        {@const choices = choicesFor(c)}
        <div
          class="tidy-row"
          class:cursor={i === cursor}
          data-testid="tidy-row"
          data-session-id={c.session_id}
          role="option"
          aria-selected={ticked.has(c.session_id)}
          tabindex="-1"
          title="Show only this session in the sidebar"
          onclick={(e) => focusAt(i, e)}
          onkeydown={() => {}}
        >
          <input
            type="checkbox"
            data-testid="tidy-check"
            checked={ticked.has(c.session_id)}
            disabled={choices.length === 0}
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
            <span class="meta">idle {formatIdle(c.idle_secs)}</span>
          {/if}
          {#if (c.secondary ?? []).length > 0}
            <span class="meta">also: {(c.secondary ?? []).map(tidyReasonLabel).join(', ')}</span>
          {/if}
          {#if c.action === 'safe_kill'}
            <span class="warn" title="Claude is asked to commit and push first; the worktree is removed only if that succeeds"
              >commits &amp; pushes first</span
            >
          {/if}
          {#if c.action === 'resume_or_expire' && c.key}<ResumeButton workKey={c.key} />{/if}
          {#if choices.length > 0}
            <select
              data-testid="tidy-choice"
              value={choice.get(c.session_id) ?? defaultChoice(c)}
              onchange={(e) => setChoice(c.session_id, (e.currentTarget as HTMLSelectElement).value)}
            >
              {#each choices as ch (ch)}<option value={ch}>{TIDY_CHOICE_LABELS[ch]}</option>{/each}
            </select>
          {/if}
        </div>
      {/each}
    {/each}
    <div class="sheet-foot">
      <button
        class="pill primary"
        data-testid="tidy-apply"
        disabled={busy || tickedCount === 0 || blocked !== null}
        onclick={() => void apply()}>Tidy {tickedCount}</button
      >
      <button class="pill" data-testid="tidy-cancel" onclick={closeSheet}>Cancel</button>
    </div>
  </div>
{/if}

<style>
  .tidy-pills {
    display: flex;
    gap: 0.4rem;
    margin: 0.2rem 0.5rem;
  }
  .tidy-pill {
    color: var(--fg-muted);
  }
  .reopened-pill {
    color: var(--accent, #3b82f6);
    border-color: var(--accent, #3b82f6);
  }
  .tidy-sheet {
    margin: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.3rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.75rem;
    outline: none;
  }
  .tidy-sheet:focus-visible {
    border-color: var(--accent, #3b82f6);
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
  .group-head {
    color: var(--fg-muted);
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding-top: 0.2rem;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 0.65rem;
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
    border-radius: 4px;
  }
  .tidy-row.cursor {
    background: var(--bg-hover, rgba(127, 127, 127, 0.15));
  }
  .key {
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .meta {
    color: var(--fg-muted);
  }
  .warn {
    color: var(--usage-warn, #b7791f);
    font-size: 0.65rem;
  }
  .badge {
    color: var(--accent, #3b82f6);
  }
  .primary {
    font-weight: 600;
  }
</style>
