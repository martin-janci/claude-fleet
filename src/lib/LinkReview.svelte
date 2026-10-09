<script lang="ts">
  import { viewKey } from './shortcuts';
  // Work graph M4.4: link suggestions, decided in bulk, and the Undo of an
  // automatic link.
  //
  // - A segment of the sidebar's attention line, "N links to review" (only
  //   when there are any; redesign 1.2), opens a sheet listing each session's top suggestion with its
  //   why. j/k (or ↓/↑) move, y (or ↵) confirms, n (or ⌫) rejects — sticky,
  //   never suggested again. Deciding one brings that session's next
  //   suggestion, if it has one, through the row update; a toast says what
  //   was decided and what comes next.
  // - A session whose primary work becomes an automatic link (a trusted
  //   branch key, a sole ticket URL) gets a toast "Linked NAME → KEY
  //   (branch) · Undo"; Undo is "Not this" for that link.
  // Rows present when the app opened are the baseline and are not toasted.
  // - Clicking a row narrows the sidebar to that session and opens it, to
  //   look at it in detail before deciding; closing the sheet lifts that.
  import { onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions, sessionsLoaded, showFriendlyNames, type SessionRow } from './sessions';
  import {
    autoLinkSnapshot,
    confirmSessionWork,
    newAutoLinks,
    rejectWorkLink,
    rowsWithSuggestions,
    sourceLabel,
    workWhy,
  } from './work';
  import { push, pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked, type SessionAction } from './share';
  import { clearSessionFocus, focusSession } from './session_focus';

  const pending = $derived(rowsWithSuggestions($sessions));
  const blocked = $derived(hubActionBlocked('confirm_session_work', $hubStatus, $hubConnection));
  /**
   * The access half (multi-user M1, F2a), per ROW: this sheet holds every
   * session with a suggestion, which on a paired desktop mixes this person's
   * sessions with the ones shared with them, so one answer for the sheet would
   * gate the wrong thing. `confirm_session_work` / `reject_session_work` are
   * `drive` in `share.ts::SESSION_TIER`, the same pair `SessionRowItem` asks
   * about for its own work chip.
   */
  /**
   * Why this client may not decide `r`'s suggestion. Asked for the action it is
   * actually about to perform (multi-user M1, F2b): `y` confirms and `n`
   * rejects, two different `SESSION_TIER` rows, and this used to answer for
   * `confirm_session_work` in both cases. Both are `drive`, so the sentence
   * does not change — but a table that stops being read is a table that stops
   * being true, and the sweep now keys on the write.
   */
  function rowBlocked(r: SessionRow, action: SessionAction = 'confirm_session_work'): string | null {
    return blocked ?? $sessionBlocked(r, action);
  }
  /** How many rows here belong to somebody else, so the hint can say so. */
  const notMine = $derived(pending.filter((r) => rowBlocked(r) !== null).length);
  let open = $state(false);
  let cursor = $state(0);
  let busy = $state(false);
  let sheet = $state<HTMLDivElement | null>(null);
  // Whether a click here set the sidebar focus: only then does closing the
  // sheet clear it (the tidy-up sheet may own it).
  let focused = false;

  function rowName(r: SessionRow): string {
    return get(showFriendlyNames) && r.friendly_name ? r.friendly_name : r.tmux_name;
  }

  // Whatever had focus when the sheet opened: closing hands focus back to it.
  let opener: HTMLElement | null = null;

  async function openSheet() {
    if (!open) opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    open = true;
    cursor = 0;
    await tick();
    sheet?.focus();
  }

  function closeSheet() {
    const hadFocus = !!sheet && sheet.contains(document.activeElement);
    open = false;
    if (focused) clearSessionFocus();
    focused = false;
    if (hadFocus && opener?.isConnected) opener.focus();
    opener = null;
  }

  function focusAt(i: number) {
    cursor = i;
    const r = pending[i];
    if (!r) return;
    if (focusSession(r.id, rowName(r))) focused = true;
  }

  async function decideAt(i: number, yes: boolean) {
    const r = pending[i];
    const sg = r?.work_suggested;
    // Per row, re-asked at the call: y/n decide the cursor row from the
    // keyboard, past any button's `disabled`.
    if (!r || !sg || busy || rowBlocked(r, yes ? 'confirm_session_work' : 'reject_session_work') !== null)
      return;
    busy = true;
    const name = rowName(r);
    const key = sg.key ?? sg.title;
    const res = yes ? await confirmSessionWork(r.id, sg.link_id) : await rejectWorkLink(r.id, sg.link_id);
    busy = false;
    if (!res.ok) {
      pushError(res.error, yes ? 'Confirm failed' : 'Not this failed');
      return;
    }
    // The session's next suggestion takes this row's place under the same
    // name, so without a word the click looks like it did nothing.
    const next = res.value.work_suggested;
    const more = next ? ` · next: ${next.key ?? next.title}? (${next.suggestions ?? 1} left)` : '';
    push({ kind: 'info', message: yes ? `Linked ${name} → ${key}${more}` : `${name}: not ${key}${more}` });
  }

  function onSheetKey(e: KeyboardEvent) {
    // The keys are the registry's `link-review` rows (step 0.1).
    const act = viewKey('link-review', e);
    if (!act) return;
    // The chords act only from the sheet itself or a row. A keydown that
    // bubbles up from a focused button (close, Confirm, Not this) keeps that
    // button's own meaning: Enter activates it, and never decides the cursor
    // row — which need not be the row whose button has focus.
    // Escape closes the sheet from anywhere inside it; it is never destructive.
    if (act === 'link-review.close') {
      closeSheet();
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const target = e.target as HTMLElement | null;
    if (target !== e.currentTarget && !target?.classList.contains('review-row')) {
      // A select owns its keys; any other control keeps its activating keys
      // (Enter, Space, y/n/Backspace) but j/k and the arrows have no meaning
      // on a checkbox, link or button, so they still move the cursor.
      if (target?.tagName === 'SELECT') return;
      if (act !== 'link-review.down' && act !== 'link-review.up') return;
    }
    const n = pending.length;
    switch (act) {
      case 'link-review.down':
        cursor = Math.min(n - 1, cursor + 1);
        break;
      case 'link-review.up':
        cursor = Math.max(0, cursor - 1);
        break;
      case 'link-review.yes':
        void decideAt(cursor, true);
        break;
      case 'link-review.no':
        void decideAt(cursor, false);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  $effect(() => {
    // The list shrinks as rows are decided: keep the cursor on a row.
    if (cursor > 0 && cursor >= pending.length) cursor = Math.max(0, pending.length - 1);
    if (open && pending.length === 0) closeSheet();
  });

  onMount(() => {
    let prev: Map<number, number> | null = null;
    return sessions.subscribe((rows) => {
      if (prev === null || !get(sessionsLoaded)) {
        prev = autoLinkSnapshot(rows);
        return;
      }
      const fresh = newAutoLinks(prev, rows);
      prev = autoLinkSnapshot(rows);
      for (const r of fresh) {
        const w = r.work;
        if (!w) continue;
        const linkId = w.link_id;
        push({
          kind: 'info',
          message: `Linked ${rowName(r)} → ${w.key ?? w.title} (${sourceLabel(w.source)})`,
          action: {
            label: 'Undo',
            run: () => {
              // The toast outlives the render that made it, so the gate is
              // asked when Undo is pressed and not when the link appeared
              // (multi-user M1, F2b). `rejectWorkLink` is `reject_session_work`,
              // `drive`.
              const why = $sessionBlocked(r, 'reject_session_work');
              if (why !== null) {
                push({ kind: 'error', message: why });
                return;
              }
              void rejectWorkLink(r.id, linkId).then((res) => {
                if (!res.ok) pushError(res.error, 'Undo failed');
              });
            },
          },
        });
      }
    });
  });
</script>

{#if pending.length > 0}
  <button
    class="al-seg"
    data-testid="link-review-pill"
    title="Sessions fleet thinks are working on a ticket: confirm or reject each (j/k, y/n)"
    onclick={() => void openSheet()}
  >{pending.length} link{pending.length === 1 ? '' : 's'} to review</button>
{/if}

{#if open}
  <div
    class="review-sheet"
    data-testid="link-review-sheet"
    role="listbox"
    aria-label="Link suggestions"
    tabindex="-1"
    bind:this={sheet}
    onkeydown={onSheetKey}
  >
    <div class="review-head">
      <span>Link suggestions</span>
      <span class="hint">j/k move · y confirm · n not this · esc close</span>
      <button class="btn btn--quiet" data-testid="link-review-close" onclick={closeSheet}>Close</button>
    </div>
    {#if blocked}<p class="hint" role="note">{blocked}</p>{/if}
    {#if !blocked && notMine > 0}
      <p class="hint" data-testid="link-review-not-mine">
        {notMine} of these session{notMine === 1 ? ' is' : 's are'} shared with you: deciding its
        work needs drive.
      </p>
    {/if}
    {#each pending as r, i (r.id)}
      {@const sg = r.work_suggested}
      {#if sg}
        {@const mine = rowBlocked(r)}
        <div
          class="review-row"
          class:cursor={i === cursor}
          data-testid="link-review-row"
          data-session-id={r.id}
          role="option"
          aria-selected={i === cursor}
          tabindex="-1"
          title="Show only this session in the sidebar"
          onclick={() => focusAt(i)}
          onkeydown={() => {}}
        >
          <span class="name">{rowName(r)}</span>
          <span class="key">{sg.key ?? sg.title}?</span>
          <span class="why">{workWhy({ ...sg, state: 'suggested' })}{(sg.suggestions ?? 1) > 1 ? ` · ${sg.suggestions} suggestions` : ''}</span>
          <button class="btn btn--quiet is-bounded" data-testid="link-review-yes" disabled={busy || mine !== null} title={mine ?? ''}
            onclick={(e) => { e.stopPropagation(); void decideAt(i, true); }}>Confirm</button>
          <button class="btn btn--quiet" data-testid="link-review-no" disabled={busy || mine !== null} title={mine ?? ''}
            onclick={(e) => { e.stopPropagation(); void decideAt(i, false); }}>Not this</button>
        </div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  /* A segment of the attention line (SidebarFilters' .attention-line). */
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
  .review-sheet {
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
  .review-sheet:focus-visible {
    border-color: var(--accent);
  }
  .review-head {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    flex: 1;
  }
  .review-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    padding: 0.15rem 0.3rem;
    border-radius: var(--radius-sm);
  }
  .review-row.cursor {
    background: var(--bg-hover);
  }
  .key {
    font-family: var(--font-mono);
  }
  .why {
    color: var(--fg-muted);
    flex: 1 1 8rem;
  }
</style>
