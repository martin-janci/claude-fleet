<!--
  Toolkit › Prompts & snippets (UX audit 2026-10-09; Martin's call): the
  quick-action chips above the prompt box, moved here from Settings ›
  Sessions & agents, which now links to this page.
-->
<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import {
    composerPresets,
    resetComposerPresets,
    addPreset,
    updatePreset,
    removePreset,
    movePreset,
    flushComposerPresets,
    refreshComposerPresetsIfIdle,
    presetsConflict,
  } from './composer_presets';

  // The editor starts from the fleet's current list, not the one read at
  // launch: no event announces a chip saved on the phone, and an edit made
  // from a stale list would be refused with E_CONFLICT on its first
  // keystroke. Skipped while an edit of ours is still pending.
  onMount(() => {
    void refreshComposerPresetsIfIdle();
  });

  // Chip edits are debounced (the editor saves on every keystroke), so a
  // page left straight after the last character would otherwise leave that
  // character's save to a timer on an unmounted component. Fire-and-forget:
  // it is the same write, only sooner.
  onDestroy(() => {
    void flushComposerPresets();
  });

  // A stable key per chip row, so a move re-orders the rows instead of
  // rewriting every field in place under the cursor (keyed by index, the
  // focused ↑ stayed at its index while the chip it moved went elsewhere).
  // Kept beside the list rather than in it: the list is the backend's, and an
  // id riding in it would be sent to the hub and cached as fleet state. A
  // change of length the editor did not make (a reload, a conflict) deals
  // fresh ids; the fields' values come from the list either way.
  let nextRowId = 0;
  let presetRowIds = $state<number[]>([]);
  $effect(() => {
    const n = $composerPresets.length;
    if (presetRowIds.length !== n) presetRowIds = Array.from({ length: n }, () => nextRowId++);
  });
  const presetKey = (i: number) => presetRowIds[i] ?? `new-${i}`;
  let presetRows: HTMLDivElement | undefined = $state();

  async function onMovePreset(i: number, dir: -1 | 1) {
    const to = i + dir;
    if (to < 0 || to >= presetRowIds.length) return;
    const id = presetRowIds[i];
    const ids = [...presetRowIds];
    [ids[i], ids[to]] = [ids[to], ids[i]];
    presetRowIds = ids;
    movePreset(i, dir);
    await tick();
    // Keep the keyboard on the chip that moved: its same arrow, or — at the
    // end of the list, where that one is now disabled — the other one.
    const row = presetRows?.querySelector<HTMLElement>(`[data-row-id="${id}"]`);
    const same = row?.querySelector<HTMLButtonElement>(`[data-testid="preset-${dir === -1 ? 'up' : 'down'}"]`);
    const other = row?.querySelector<HTMLButtonElement>(`[data-testid="preset-${dir === -1 ? 'down' : 'up'}"]`);
    (same && !same.disabled ? same : other)?.focus();
  }

  function onRemovePreset(i: number) {
    presetRowIds = presetRowIds.filter((_, k) => k !== i);
    removePreset(i);
  }

  function onAddPreset() {
    presetRowIds = [...presetRowIds, nextRowId++];
    addPreset();
  }
</script>

<div class="prompts" data-testid="composer-section">
  <div class="bar">
    <h2>Prompts & snippets</h2>
    <p class="sub" data-testid="toolkit-prompts-summary">
      {$composerPresets.length}
      {$composerPresets.length === 1 ? 'chip' : 'chips'}
    </p>
  </div>
  <p class="desc">
    Quick-action chips above the prompt box in the Conversation tab and on the
    phone, in this order. A click fills the box; with <em>Send</em> ticked it
    sends at once (Shift+click does the other one) — except while the session
    waits on an answer, when it only fills. Chips with an empty label or text
    are not shown.
  </p>
  {#if $presetsConflict}
    <p class="desc" role="status" data-testid="preset-conflict">
      Another device changed the chips first. This is its list now; make your change again.
    </p>
  {/if}
  <div class="preset-rows" bind:this={presetRows}>
    {#each $composerPresets as p, i (presetKey(i))}
      <div class="preset-row" data-row-id={presetKey(i)}>
        <input
          class="preset-label"
          data-testid="preset-label"
          placeholder="Label"
          value={p.label}
          oninput={(e) => updatePreset(i, { label: e.currentTarget.value })}
        />
        <textarea
          class="preset-text"
          data-testid="preset-text"
          rows="1"
          placeholder="Prompt or /command"
          value={p.text}
          oninput={(e) => updatePreset(i, { text: e.currentTarget.value })}
        ></textarea>
        <label class="preset-send" title="Send on click instead of only filling the box">
          <input
            type="checkbox"
            data-testid="preset-auto-send"
            checked={p.auto_send === true}
            onchange={(e) => updatePreset(i, { auto_send: e.currentTarget.checked })}
          />
          Send
        </label>
        <button
          class="row-btn"
          data-testid="preset-up"
          title="Move up"
          aria-label="Move up"
          disabled={i === 0}
          onclick={() => void onMovePreset(i, -1)}>↑</button
        >
        <button
          class="row-btn"
          data-testid="preset-down"
          title="Move down"
          aria-label="Move down"
          disabled={i === $composerPresets.length - 1}
          onclick={() => void onMovePreset(i, 1)}>↓</button
        >
        <button class="row-btn" data-testid="preset-remove" title="Remove" onclick={() => onRemovePreset(i)}>×</button>
      </div>
    {/each}
  </div>
  <div class="preset-actions">
    <button class="row-btn" data-testid="preset-add" onclick={onAddPreset}>Add chip</button>
    <button class="row-btn" data-testid="preset-reset" onclick={resetComposerPresets}>Reset to defaults</button>
  </div>
</div>

<style>
  .prompts {
    max-width: var(--prose-max);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .bar {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
  }
  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
  }
  .sub,
  .desc {
    margin: 0;
    color: var(--fg-muted);
  }
  .sub {
    font-size: var(--text-sm);
  }
  .desc {
    font-size: var(--text-xs);
  }
  /* A handle for focus lookups only: the rows stay items of .prompts. */
  .preset-rows {
    display: contents;
  }
  .preset-row {
    display: flex;
    gap: 0.4rem;
    align-items: flex-start;
  }
  .preset-label {
    flex: 0 0 9rem;
  }
  .preset-text {
    flex: 1 1 auto;
    min-height: 1.9rem;
    resize: vertical;
    font: inherit;
    font-size: var(--text-sm);
  }
  .preset-label,
  .preset-text {
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .preset-send {
    display: flex;
    align-items: center;
    gap: 0.2rem;
    font-size: var(--text-2xs);
    white-space: nowrap;
    padding-top: 0.3rem;
  }
  .preset-actions {
    display: flex;
    gap: 0.4rem;
    margin-top: 0.4rem;
  }
  .row-btn {
    align-self: flex-start;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: var(--text-2xs);
    border-radius: var(--radius-sm);
  }
  .row-btn:hover:not(:disabled) {
    border-color: var(--accent);
  }
  .row-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
