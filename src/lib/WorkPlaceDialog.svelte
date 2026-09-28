<script lang="ts">
  // "Place in group…" (work graph M14): put one task under a group of the
  // Work view — an existing group's label or a new one — with an optional
  // note. Local placement only: a tracker's project is never changed. After
  // it is placed, "Make a rule for similar tasks…" is offered as its own,
  // separate step (a rule reaches beyond this task, so it is never implied).
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    conflictNotice,
    placementNote,
    placeWork,
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

  const labels = $derived(
    [...new Set($workTreeMeta.groups.filter((g) => g.group.source !== 'none').map((g) => g.group.label))].sort((a, b) =>
      a.localeCompare(b),
    ),
  );
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

  function ruleDraft(group: string): WorkRuleDraft {
    const g = before.group;
    const prefix = before.key && /^[A-Za-z][A-Za-z0-9_]*-\d+$/.test(before.key) ? before.key.split('-')[0] : null;
    return {
      name: group,
      enabled: true,
      group,
      expected_version: 0,
      conditions: {
        tracker_id: before.kind === 'tracker' ? (before.tracker_id ?? null) : null,
        container: g?.source === 'tracker' ? (g.tracker_value ?? g.label) : null,
        key_prefix: g?.source !== 'tracker' && prefix ? prefix : null,
        repo: g?.source === 'repo' ? g.label : null,
        title_contains: null,
      },
    };
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
              const d = ruleDraft(placed ?? '');
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
          list="work-place-labels"
          bind:value={label}
          placeholder="Pick a group or type a new one"
          data-autofocus=""
          maxlength="80"
          data-testid="work-place-group"
        />
        <datalist id="work-place-labels">
          {#each labels as l (l)}<option value={l}></option>{/each}
        </datalist>
      </label>
      {#if labels.length > 0}
        <div class="chips">
          {#each labels.slice(0, 12) as l (l)}
            <button type="button" class="btn btn--chip" aria-pressed={label === l} data-testid="work-place-label" onclick={() => (label = l)}>{l}</button>
          {/each}
        </div>
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
  .form { display: flex; flex-direction: column; gap: 0.5rem; font-size: 0.85rem; }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .field span { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input {
    font: inherit;
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
  }
  .what { margin: 0; font-weight: 600; overflow-wrap: anywhere; }
  .note { margin: 0; font-size: 0.8rem; color: var(--fg-muted); }
  .chips { display: flex; gap: 0.25rem; flex-wrap: wrap; }
  .err { color: #e64a4a; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; flex-wrap: wrap; }
  p { margin: 0; }
</style>
