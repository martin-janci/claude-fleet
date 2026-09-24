<script lang="ts">
  // Work graph M4.4: link suggestions, decided in bulk, and the Undo of an
  // automatic link.
  //
  // - An attention pill "N link suggestions · Review" (only when there are
  //   any) opens a sheet listing each session's top suggestion with its
  //   why. j/k (or ↓/↑) move, y (or ↵) confirms, n (or ⌫) rejects — sticky,
  //   never suggested again. Deciding one brings that session's next
  //   suggestion, if it has one, through the row update.
  // - A session whose primary work becomes an automatic link (a trusted
  //   branch key, a sole ticket URL) gets a toast "Linked NAME → KEY
  //   (branch) · Undo"; Undo is "Not this" for that link.
  // Rows present when the app opened are the baseline and are not toasted.
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

  const pending = $derived(rowsWithSuggestions($sessions));
  const blocked = $derived(hubActionBlocked('confirm_session_work', $hubStatus, $hubConnection));
  let open = $state(false);
  let cursor = $state(0);
  let busy = $state(false);
  let sheet = $state<HTMLDivElement | null>(null);

  function rowName(r: SessionRow): string {
    return get(showFriendlyNames) && r.friendly_name ? r.friendly_name : r.tmux_name;
  }

  async function openSheet() {
    open = true;
    cursor = 0;
    await tick();
    sheet?.focus();
  }

  async function decideAt(i: number, yes: boolean) {
    const r = pending[i];
    const sg = r?.work_suggested;
    if (!r || !sg || busy || blocked !== null) return;
    busy = true;
    const res = yes ? await confirmSessionWork(r.id, sg.link_id) : await rejectWorkLink(r.id, sg.link_id);
    busy = false;
    if (!res.ok) pushError(res.error, yes ? 'Confirm failed' : 'Not this failed');
  }

  function onSheetKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const n = pending.length;
    switch (e.key) {
      case 'j':
      case 'ArrowDown':
        cursor = Math.min(n - 1, cursor + 1);
        break;
      case 'k':
      case 'ArrowUp':
        cursor = Math.max(0, cursor - 1);
        break;
      case 'y':
      case 'Enter':
        void decideAt(cursor, true);
        break;
      case 'n':
      case 'Backspace':
        void decideAt(cursor, false);
        break;
      case 'Escape':
        open = false;
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
    if (open && pending.length === 0) open = false;
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
    class="pill review-pill"
    data-testid="link-review-pill"
    title="Sessions fleet thinks are working on a ticket: confirm or reject each (j/k, y/n)"
    onclick={() => void openSheet()}
  >{pending.length} link suggestion{pending.length === 1 ? '' : 's'} · Review</button>
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
      <button class="pill" data-testid="link-review-close" onclick={() => (open = false)}>close</button>
    </div>
    {#if blocked}<p class="hint" role="note">{blocked}</p>{/if}
    {#each pending as r, i (r.id)}
      {@const sg = r.work_suggested}
      {#if sg}
        <div
          class="review-row"
          class:cursor={i === cursor}
          data-testid="link-review-row"
          data-session-id={r.id}
          role="option"
          aria-selected={i === cursor}
          tabindex="-1"
          onclick={() => (cursor = i)}
          onkeydown={() => {}}
        >
          <span class="name">{rowName(r)}</span>
          <span class="key">{sg.key ?? sg.title}?</span>
          <span class="why">{workWhy({ ...sg, state: 'suggested' })}{(sg.suggestions ?? 1) > 1 ? ` · ${sg.suggestions} suggestions` : ''}</span>
          <button class="pill" data-testid="link-review-yes" disabled={busy || blocked !== null}
            onclick={(e) => { e.stopPropagation(); void decideAt(i, true); }}>Confirm</button>
          <button class="pill" data-testid="link-review-no" disabled={busy || blocked !== null}
            onclick={(e) => { e.stopPropagation(); void decideAt(i, false); }}>Not this</button>
        </div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .review-pill {
    margin: 0.2rem 0.5rem;
  }
  .review-sheet {
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
  .review-sheet:focus-visible {
    border-color: var(--accent, #3b82f6);
  }
  .review-head {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 0.65rem;
    flex: 1;
  }
  .review-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    padding: 0.15rem 0.3rem;
    border-radius: 4px;
  }
  .review-row.cursor {
    background: var(--bg-hover, rgba(127, 127, 127, 0.15));
  }
  .key {
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .why {
    color: var(--fg-muted);
    flex: 1 1 8rem;
  }
</style>
