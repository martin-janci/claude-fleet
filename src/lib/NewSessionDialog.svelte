<script lang="ts">
  import { untrack } from 'svelte';
  import type { ProjectTreeRow, WorktreeRow } from './projects';
  import { newSessionAbortable, type SessionRow } from './sessions';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';

  let {
    project,
    onCreate,
    onCancel,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
  } = $props();

  const isString = (v: unknown): v is string => typeof v === 'string';
  let chosenHost = $state<string>(
    readPref('last-host', 'local', isString),
  );
  $effect(() => {
    writePref('last-host', chosenHost);
  });

  function defaultName(wt: WorktreeRow | null): string {
    const base = `dev-${project.project.owner}-${project.project.repo}`;
    if (!wt || wt.name === 'main') return base;
    return `${base}--${wt.name}`;
  }

  let chosenWorktreeId = $state<number | null>(untrack(() => project.worktrees[0]?.id ?? null));
  let name = $state(untrack(() => defaultName(project.worktrees[0] ?? null)));
  let busy = $state(false);
  let error: string | null = $state(null);
  let createController: AbortController | null = null;

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    const wt = project.worktrees.find((w) => w.id === id) ?? null;
    name = defaultName(wt);
  }

  async function submit() {
    if (!name.trim()) {
      error = 'Session name required';
      return;
    }
    busy = true;
    error = null;
    createController = new AbortController();
    const r = await newSessionAbortable(
      {
        host_alias: chosenHost,
        project_id: project.project.id,
        worktree_id: chosenWorktreeId,
        name: name.trim(),
      },
      createController.signal,
    );
    createController = null;
    busy = false;
    if (!r.ok) {
      if (r.error.code !== 'E_CANCELLED') {
        error = r.error.message;
      }
      return;
    }
    onCreate(r.value);
  }

  function cancelCreate() {
    createController?.abort();
  }
</script>

<div class="dialog" role="dialog" aria-label="New session">
  <h3>New session — {project.project.owner}/{project.project.repo}</h3>

  <label for="host-picker">Host</label>
  <div class="host-row" id="host-picker" role="group">
    {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
      <button
        class="host-pick"
        class:active={chosenHost === h.alias}
        disabled={!h.reachable && h.alias !== 'local'}
        onclick={() => (chosenHost = h.alias)}
      >
        {h.alias}
      </button>
    {/each}
  </div>

  {#if project.worktrees.length > 1}
    <label for="wt-picker">Worktree</label>
    <div class="worktree-row" id="wt-picker" role="group">
      {#each project.worktrees as wt (wt.id)}
        <button
          class="wt-pick"
          class:active={chosenWorktreeId === wt.id}
          onclick={() => onPickWorktree(wt.id)}
        >
          {wt.name}
        </button>
      {/each}
    </div>
  {/if}

  <label for="session-name">tmux name</label>
  <input id="session-name" bind:value={name} data-testid="new-session-name" />

  {#if error}
    <p class="err">{error}</p>
  {/if}

  <div class="actions">
    <button onclick={onCancel} disabled={busy}>Cancel</button>
    {#if busy}
      <button type="button" data-testid="cancel-create" onclick={cancelCreate}>Cancel creation</button>
    {:else}
      <button onclick={submit} disabled={!name.trim()}>Create</button>
    {/if}
  </div>
</div>

<style>
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 360px;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .dialog h3 { margin: 0 0 0.3rem 0; font-size: 0.95rem; }
  label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  input {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
  }
  .worktree-row { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .wt-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .wt-pick.active { color: var(--fg); border-color: var(--accent); }
  .host-row { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .host-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .host-pick.active { color: var(--fg); border-color: var(--accent); }
  .host-pick:disabled { opacity: 0.4; cursor: not-allowed; }
  .err { color: #e64a4a; font-size: 0.8rem; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
