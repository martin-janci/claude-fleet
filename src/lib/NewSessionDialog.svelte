<script lang="ts">
  import type { ProjectTreeRow, WorktreeRow } from './projects';
  import { newSession, type SessionRow } from './sessions';

  let {
    project,
    onCreate,
    onCancel,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
  } = $props();

  const owner = project.project.owner;
  const repo = project.project.repo;
  const projectId = project.project.id;
  const worktrees = project.worktrees;

  function defaultName(wt: WorktreeRow | null): string {
    const base = `dev-${owner}-${repo}`;
    if (!wt || wt.name === 'main') return base;
    return `${base}--${wt.name}`;
  }

  const initialWorktree = worktrees[0] ?? null;
  let chosenWorktreeId = $state<number | null>(initialWorktree?.id ?? null);
  let name = $state(defaultName(initialWorktree));
  let busy = $state(false);
  let error: string | null = $state(null);

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    const wt = worktrees.find((w) => w.id === id) ?? null;
    name = defaultName(wt);
  }

  async function submit() {
    if (!name.trim()) {
      error = 'Session name required';
      return;
    }
    busy = true;
    error = null;
    const r = await newSession({
      project_id: projectId,
      worktree_id: chosenWorktreeId,
      name: name.trim(),
    });
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onCreate(r.value);
  }
</script>

<div class="dialog" role="dialog" aria-label="New session">
  <h3>New session — {owner}/{repo}</h3>

  {#if worktrees.length > 1}
    <label for="wt-picker">Worktree</label>
    <div class="worktree-row" id="wt-picker" role="group">
      {#each worktrees as wt (wt.id)}
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
    <button onclick={submit} disabled={busy || !name.trim()}>Create</button>
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
