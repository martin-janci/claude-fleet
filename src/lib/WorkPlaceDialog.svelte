<script lang="ts">
  // "Place in group…" (work graph M14): put one task under a group of the
  // Work view — an existing group's label or a new one — with an optional
  // note. Local placement only: a tracker's project is never changed. After
  // it is placed, "Make a rule for similar tasks…" is offered as its own,
  // separate step (a rule reaches beyond this task, so it is never implied).
  // The Group field is a combobox (gap plan G2.2): the existing groups with
  // how many tasks each holds, filtered as you type, and a "+ New group"
  // row when what is typed is not one of them.
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    conflictNotice,
    placementNote,
    placeWork,
    ruleDraftFor,
    taskLabel,
    workTreeMeta,
    type ConflictNotice,
    type WorkRuleDraft,
    type WorkTask,
  } from './work_view';
  import WorkConflictNotice from './WorkConflictNotice.svelte';

  let {
    task,
    currentNote = null,
    onclose,
    ondone,
    onreload,
    onmakerule,
  }: {
    task: WorkTask;
    /** The placement's note as last read (`TaskDetail.placement.note`):
     *  kept unless edited — a placement sent without one clears it. */
    currentNote?: string | null;
    onclose: () => void;
    /** After a successful placement, with the task as the hub answered and
     *  the note sent (null: none). */
    ondone?: (t: WorkTask, note: string | null) => void;
    /** The placement changed elsewhere: the parent re-reads the task. */
    onreload?: () => void;
    /** "Make a rule for similar tasks…", prefilled. */
    onmakerule?: (d: WorkRuleDraft) => void;
  } = $props();

  const blocked = $derived(hubActionBlocked('place_work', $hubStatus, $hubConnection));
  // The task as it was before this placement: the rule is built from where
  // it came from (its tracker project, key prefix or repository).
  const before = untrack(() => task);

  let label = $state(untrack(() => (task.group?.source === 'manual' ? task.group.label : '')));
  let note = $state(untrack(() => currentNote ?? ''));
  let busy = $state(false);
  let failure = $state<string | ConflictNotice | null>(null);
  let placed = $state<string | null>(null);

  // Each group once, its count summed over the orgs it appears in.
  const groups = $derived.by(() => {
    const m = new Map<string, number>();
    for (const g of $workTreeMeta.groups) {
      if (g.group.source === 'none') continue;
      m.set(g.group.label, (m.get(g.group.label) ?? 0) + (g.count ?? 0));
    }
    return [...m].map(([l, count]) => ({ label: l, count })).sort((a, b) => a.label.localeCompare(b.label));
  });
  const typed = $derived(label.trim());
  const existing = $derived(groups.find((g) => g.label.toLowerCase() === typed.toLowerCase()) ?? null);
  const shown = $derived(
    (typed === '' || existing ? groups : groups.filter((g) => g.label.toLowerCase().includes(typed.toLowerCase()))).slice(0, 8),
  );
  const offerNew = $derived(typed !== '' && existing === null);
  // The option the arrow keys are on: an index into `shown`, then the
  // "+ New group" row last; -1 none.
  let activeIdx = $state(-1);
  const optionCount = $derived(shown.length + (offerNew ? 1 : 0));
  function pick(l: string) {
    label = l;
    activeIdx = -1;
  }
  function onGroupKey(e: KeyboardEvent) {
    if (optionCount === 0) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      activeIdx = (activeIdx + 1) % optionCount;
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      activeIdx = activeIdx <= 0 ? optionCount - 1 : activeIdx - 1;
    } else if (e.key === 'Enter' && activeIdx >= 0) {
      e.preventDefault();
      pick(activeIdx < shown.length ? shown[activeIdx].label : typed);
    }
  }
  function tasks(n: number): string {
    return `${n} task${n === 1 ? '' : 's'}`;
  }
  const isManual = $derived(task.group?.source === 'manual' || (task.placement_version ?? 0) > 0);

  async function place(group: string) {
    if (busy || blocked !== null) return;
    busy = true;
    failure = null;
    const r = await placeWork(task.task_id, group, task.placement_version ?? 0, note);
    busy = false;
    if (!r.ok) {
      const c = conflictNotice(r.error, 'Its placement');
      if (c) {
        failure = {
          ...c,
          text: 'This task was placed elsewhere in the meantime; reloaded — check where it is now and try again.',
        };
        onreload?.();
      } else {
        failure = r.error.message;
      }
      return;
    }
    ondone?.(r.value, group.trim() === '' ? null : note.trim() || null);
    if (group.trim() === '') {
      onclose();
      return;
    }
    placed = group.trim();
  }
</script>

<Modal title="Place in group" {onclose} width="440px" testid="work-place-dialog">
  {#if placed}
    <div class="form">
      <p data-testid="work-place-done">{taskLabel(task)} is now under <strong>{placed}</strong>.</p>
      <p class="note">Tasks like it can go there too, with a rule you preview before it is saved.</p>
      <div class="actions">
        {#if onmakerule}
          <button
            type="button"
            class="btn"
            data-testid="work-place-make-rule"
            onclick={() => {
              const d = ruleDraftFor(before, placed ?? '', placed ?? '');
              onclose();
              onmakerule?.(d);
            }}>Make a rule for similar tasks…</button
          >
        {/if}
        <button type="button" class="btn btn--primary" data-autofocus="" onclick={onclose}>Done</button>
      </div>
    </div>
  {:else}
    <form
      class="form"
      onsubmit={(e) => {
        e.preventDefault();
        if (label.trim()) void place(label);
      }}
    >
      <p class="what">{taskLabel(task)}</p>
      <p class="note" data-testid="work-place-note">{placementNote(task.group, task)}</p>
      <label class="field">
        <span>Group</span>
        <input
          type="text"
          role="combobox"
          aria-expanded={optionCount > 0}
          aria-controls="work-place-groups"
          aria-autocomplete="list"
          aria-activedescendant={activeIdx >= 0 ? `work-place-opt-${activeIdx}` : undefined}
          bind:value={label}
          oninput={() => (activeIdx = -1)}
          onkeydown={onGroupKey}
          placeholder="Pick a group or type a new one"
          data-autofocus=""
          maxlength="80"
          data-testid="work-place-group"
        />
      </label>
      {#if existing}
        <p class="note" data-testid="work-place-existing">existing · {tasks(existing.count)}</p>
      {:else if offerNew}
        <p class="note" data-testid="work-place-existing">new group</p>
      {/if}
      {#if optionCount > 0}
        <ul class="options" id="work-place-groups" role="listbox" aria-label="Groups">
          {#each shown as g, i (g.label)}
            <li
              id="work-place-opt-{i}"
              role="option"
              tabindex="-1"
              class:active={activeIdx === i}
              aria-selected={existing?.label === g.label}
              data-testid="work-place-label"
              data-label={g.label}
              onmousedown={(e) => e.preventDefault()}
              onclick={() => pick(g.label)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  pick(g.label);
                }
              }}
            >
              <span class="opt-label">{g.label}</span>
              <span class="opt-count" title={tasks(g.count)}>{g.count}</span>
            </li>
          {/each}
          {#if offerNew}
            <li
              id="work-place-opt-{shown.length}"
              role="option"
              tabindex="-1"
              class="new"
              class:active={activeIdx === shown.length}
              aria-selected="false"
              data-testid="work-place-new-group"
              onmousedown={(e) => e.preventDefault()}
              onclick={() => pick(typed)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  pick(typed);
                }
              }}
            >+ New group “{typed}”</li>
          {/if}
        </ul>
      {/if}
      <label class="field">
        <span>Note (optional)</span>
        <input type="text" bind:value={note} maxlength="200" data-testid="work-place-note-input" />
      </label>
      {#if failure}
        <p class="err" role="alert" data-testid="work-place-error">
          {#if typeof failure === 'string'}{failure}{:else}<WorkConflictNotice notice={failure} onreload={onreload} />{/if}
        </p>
      {/if}
      {#if blocked}<p class="note">{blocked}</p>{/if}
      <div class="actions">
        {#if isManual}
          <button
            type="button"
            class="btn btn--quiet"
            data-testid="work-place-clear"
            disabled={busy || blocked !== null}
            title="Fall back to where fleet would put it"
            onclick={() => void place('')}>Clear placement</button
          >
        {/if}
        <button type="button" class="btn btn--quiet" onclick={onclose}>Cancel</button>
        <button type="submit" class="btn btn--primary" data-testid="work-place-submit" disabled={busy || !label.trim() || blocked !== null}
          >Place</button
        >
      </div>
    </form>
  {/if}
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.5rem; font-size: var(--text-xs); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .field span { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input {
    font: inherit;
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .what { margin: 0; font-weight: 600; overflow-wrap: anywhere; }
  .note { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .options {
    list-style: none;
    margin: 0;
    padding: 0.15rem 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    max-height: 12rem;
    overflow: auto;
  }
  .options li {
    display: flex;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.25rem 0.45rem;
    cursor: pointer;
  }
  .options li:hover,
  .options li.active { background: var(--bg-hover); }
  .options li[aria-selected='true'] { font-weight: 600; }
  .opt-label { overflow-wrap: anywhere; }
  .opt-count { color: var(--fg-muted); font-variant-numeric: tabular-nums; }
  .options .new { color: var(--accent); }
  .err { color: var(--danger); margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; flex-wrap: wrap; }
  p { margin: 0; }
</style>
