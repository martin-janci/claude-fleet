<script lang="ts">
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { readErrorText } from './work_view';
  import { promoteWorkItem } from './task_detail';
  import type { PromoteTarget } from './work_view';

  // Promote a task of fleet's own to a tracker's ticket (owner decision
  // 2026-10-11). The hub offers the targets: a subtask of a ticket goes
  // under it in its tracker (nothing to pick); a task at the top picks a
  // tracker and, for Jira, the project. The task becomes the ticket once
  // the tracker answers; until then the page says it is being created.
  let {
    itemId,
    title,
    targets,
    onclose,
    ondone,
  }: {
    itemId: number;
    title: string;
    targets: PromoteTarget[];
    onclose: () => void;
    ondone?: () => void;
  } = $props();

  let trackerId = $state<number | null>(untrack(() => targets[0]?.tracker_id ?? null));
  const target = $derived(targets.find((t) => t.tracker_id === trackerId) ?? null);
  const containers = $derived(target?.containers ?? []);
  let container = $state('');
  // A tracker with one place to put it picks it; another tracker starts
  // over.
  $effect(() => {
    const only = containers.length === 1 ? containers[0][0] : '';
    container = target?.container_required ? only : '';
  });
  let busy = $state(false);
  let failure = $state<string | null>(null);

  const blocked = $derived(hubActionBlocked('promote_work_item', $hubStatus, $hubConnection));
  const canSubmit = $derived(
    !busy && blocked === null && target !== null && (!target.container_required || container.trim() !== ''),
  );
  /** What a container is called on this tracker. */
  const containerLabel = $derived(
    target?.provider === 'asana'
      ? 'Project (optional)'
      : target?.provider === 'linear'
        ? 'Team'
        : target?.provider === 'github'
          ? 'Repository'
          : 'Project',
  );

  async function submit(e?: Event) {
    e?.preventDefault();
    if (!canSubmit || !target) return;
    busy = true;
    failure = null;
    const r = target.parent_key
      ? await promoteWorkItem(itemId)
      : await promoteWorkItem(itemId, target.tracker_id, container.trim() || null);
    busy = false;
    if (!r.ok) {
      failure = readErrorText(r.error);
      return;
    }
    ondone?.();
    onclose();
  }
</script>

<Modal title="Promote to a ticket" {onclose} width="440px" testid="promote-task-dialog">
  <form class="form" onsubmit={submit}>
    <p class="note">
      “{title}” becomes a ticket. Its sessions, comments, subtasks and sprint stay with it; its fleet key
      still finds it.
    </p>
    {#if target?.parent_key}
      <p data-testid="promote-task-under">
        A subtask of <strong>{target.parent_key}</strong> in {target.tracker}.
      </p>
    {:else}
      {#if targets.length > 1}
        <label class="field">
          <span>Tracker</span>
          <select bind:value={trackerId} data-testid="promote-task-tracker">
            {#each targets as t (t.tracker_id)}
              <option value={t.tracker_id}>{t.tracker}</option>
            {/each}
          </select>
        </label>
      {:else if target}
        <p>In {target.tracker}.</p>
      {/if}
      {#if containers.length > 0}
        <label class="field">
          <span>{containerLabel}</span>
          <select bind:value={container} data-testid="promote-task-container">
            {#if !target?.container_required}<option value="">None: my tasks in the workspace</option>{/if}
            {#if target?.container_required && containers.length > 1}<option value="" disabled>Pick one</option>{/if}
            {#each containers as [id, name] (id)}
              <option value={id}>{name}</option>
            {/each}
          </select>
        </label>
      {:else if target?.container_required}
        <!-- A GitHub tracker narrowed to no repositories: any in its scope. -->
        <label class="field">
          <span>{containerLabel}</span>
          <input
            type="text"
            bind:value={container}
            placeholder={target.provider === 'github' ? 'owner/repo' : ''}
            autocomplete="off"
            spellcheck="false"
            data-testid="promote-task-container-text"
          />
        </label>
      {/if}
    {/if}
    <p class="note">The description goes with it, signed with your name. The tracker numbers it.</p>
    {#if blocked}
      <p class="err" data-testid="promote-task-blocked">{blocked}</p>
    {/if}
    {#if failure}
      <p class="err" role="alert" data-testid="promote-task-error">{failure}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button type="submit" class="primary" disabled={!canSubmit} title={blocked ?? ''} data-testid="promote-task-submit"
        >Create ticket</button
      >
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.6rem; }
  .form p { margin: 0; font-size: var(--text-xs); }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field span { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field select,
  .field input {
    font: inherit;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .form p.note { font-size: var(--text-2xs); color: var(--fg-muted); }
  .form p.err { color: var(--danger); font-size: var(--text-2xs); }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
