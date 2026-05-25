<script lang="ts">
  import { untrack } from 'svelte';
  import type { ProjectTreeRow, WorktreeRow } from './projects';
  import { newSessionAbortable, type SessionRow } from './sessions';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';
  import { slugifyBranch, finalizeBranchSlug } from './branch-slug';

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

  // "work" runs Claude Code in the pane; "shell" runs a plain login shell.
  let chosenKind = $state<'work' | 'shell'>('work');
  // Optional command run on start for a shell session (empty = bare shell).
  let startCommand = $state<string>('');

  function defaultName(wt: WorktreeRow | null): string {
    const base = `dev-${project.project.owner}-${project.project.repo}`;
    const suffix = chosenKind === 'shell' ? '-term' : '';
    if (!wt || wt.name === 'main') return base + suffix;
    return `${base}--${wt.name}${suffix}`;
  }

  function defaultNameForNew(newName: string): string {
    const base = `dev-${project.project.owner}-${project.project.repo}`;
    const suffix = chosenKind === 'shell' ? '-term' : '';
    if (!newName.trim()) return base + suffix;
    return `${base}--${newName.trim()}${suffix}`;
  }

  // Inverse of slugify-ish: take a worktree/branch name and produce a
  // sentence-cased label so the friendly-name field is pre-filled with
  // something readable when the user picks an existing worktree.
  function humanize(branch: string): string {
    if (!branch || branch === 'main') return '';
    const spaced = branch.replace(/[-_]+/g, ' ').trim();
    if (!spaced) return '';
    return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
  }

  let chosenWorktreeId = $state<number | null>(untrack(() => project.worktrees[0]?.id ?? null));
  let newWorktreeName = $state<string>('');
  // Base branch to fork the new worktree from. Empty = the repo's default
  // branch (the backend falls back to default if the named branch is missing).
  let baseBranch = $state<string>('');
  // Track whether the user has manually edited the slug; once they do, we
  // stop auto-syncing it from the friendly-name field so their override
  // sticks. Reset on worktree-mode changes.
  let slugDirty = $state<boolean>(false);
  let friendlyName = $state<string>(
    untrack(() => humanize(project.worktrees[0]?.name ?? '')),
  );
  let name = $state(untrack(() => defaultName(project.worktrees[0] ?? null)));

  // Re-derive the tmux name when the kind toggles so the `-term` suffix tracks it.
  function onPickKind(kind: 'work' | 'shell') {
    chosenKind = kind;
    if (inNewMode) {
      name = defaultNameForNew(newWorktreeName);
    } else {
      const wt = project.worktrees.find((w) => w.id === chosenWorktreeId) ?? null;
      name = defaultName(wt);
    }
  }
  let busy = $state(false);
  let error: string | null = $state(null);
  let createController: AbortController | null = null;

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    newWorktreeName = '';
    baseBranch = '';
    slugDirty = false;
    const wt = project.worktrees.find((w) => w.id === id) ?? null;
    name = defaultName(wt);
    friendlyName = humanize(wt?.name ?? '');
  }

  function onPickNew() {
    chosenWorktreeId = null;
    newWorktreeName = '';
    baseBranch = '';
    slugDirty = false;
    friendlyName = '';
    name = defaultNameForNew('');
  }

  function onFriendlyNameInput(value: string) {
    friendlyName = value;
    if (inNewMode && !slugDirty) {
      // Use the canonical branch-slug helper so the auto-derived slug is
      // git-safe the same way the user's direct edits are. Run it through
      // `finalizeBranchSlug` (not the live `slugifyBranch`) because the
      // friendly-name field's "word in progress" trailing space already
      // collapses to a stable form here — no point preserving a trailing
      // dash for a value the user isn't directly typing into the slug.
      const slug = finalizeBranchSlug(value);
      newWorktreeName = slug;
      name = defaultNameForNew(slug);
    }
  }

  function onNewWorktreeNameInput(value: string) {
    // Auto-correct free-form input ("fix login bug") into a git-safe slug
    // ("fix-login-bug") as the user types.
    newWorktreeName = slugifyBranch(value);
    name = defaultNameForNew(newWorktreeName);
    // Any divergence from the friendly-name-derived slug means the user has
    // taken manual control — stop auto-syncing future friendly-name edits.
    slugDirty = newWorktreeName !== finalizeBranchSlug(friendlyName);
  }

  function onNewWorktreeNameBlur() {
    const cleaned = finalizeBranchSlug(newWorktreeName);
    if (cleaned !== newWorktreeName) {
      newWorktreeName = cleaned;
      name = defaultNameForNew(cleaned);
    }
  }

  // Re-derive: new-worktree mode is active when chosenWorktreeId is null
  let inNewMode = $derived(chosenWorktreeId === null);

  async function submit() {
    if (inNewMode) {
      // Strip any trailing dash the live slugifier left in place so the
      // backend sees a fully-finalized branch name.
      const cleaned = finalizeBranchSlug(newWorktreeName);
      if (cleaned !== newWorktreeName) {
        newWorktreeName = cleaned;
        name = defaultNameForNew(cleaned);
      }
    }
    if (!name.trim()) {
      error = 'Session name required';
      return;
    }
    if (inNewMode && !newWorktreeName.trim()) {
      error = 'Worktree name required';
      return;
    }
    busy = true;
    error = null;
    createController = new AbortController();
    const r = await newSessionAbortable(
      {
        host_alias: chosenHost,
        project_id: project.project.id,
        worktree_id: inNewMode ? null : chosenWorktreeId,
        name: name.trim(),
        new_worktree: inNewMode ? newWorktreeName.trim() || null : null,
        base_branch: inNewMode ? baseBranch.trim() || null : null,
        kind: chosenKind,
        start_command:
          chosenKind === 'shell' ? startCommand.trim() || null : null,
        friendly_name: friendlyName.trim() || null,
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

  <label for="friendly-name">Friendly name</label>
  <input
    id="friendly-name"
    data-testid="friendly-name"
    value={friendlyName}
    oninput={(e) => onFriendlyNameInput((e.target as HTMLInputElement).value)}
    placeholder="e.g. fix friendly name fallback"
    maxlength="80"
  />

  <label for="kind-picker">Type</label>
  <div class="kind-row" id="kind-picker" role="group">
    <button
      class="kind-pick"
      class:active={chosenKind === 'work'}
      data-testid="kind-work"
      onclick={() => onPickKind('work')}
    >
      Claude
    </button>
    <button
      class="kind-pick"
      class:active={chosenKind === 'shell'}
      data-testid="kind-shell"
      onclick={() => onPickKind('shell')}
    >
      Shell
    </button>
  </div>

  {#if chosenKind === 'shell'}
    <label for="start-command">start command (optional)</label>
    <input
      id="start-command"
      data-testid="start-command"
      bind:value={startCommand}
      placeholder="e.g. pnpm test"
    />
  {/if}

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
    <button
      class="wt-pick wt-new"
      class:active={inNewMode}
      data-testid="new-worktree-chip"
      onclick={onPickNew}
    >
      + new
    </button>
  </div>

  {#if inNewMode}
    <label for="new-wt-name">new branch / worktree name</label>
    <input
      id="new-wt-name"
      data-testid="new-worktree-name"
      value={newWorktreeName}
      oninput={(e) => onNewWorktreeNameInput((e.target as HTMLInputElement).value)}
      onblur={onNewWorktreeNameBlur}
      placeholder="fix login bug → fix-login-bug"
    />
    <label for="new-wt-base">base branch</label>
    <input
      id="new-wt-base"
      data-testid="new-worktree-base"
      bind:value={baseBranch}
      placeholder="default branch"
    />
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
      <button onclick={submit} disabled={!name.trim() || (inNewMode && !newWorktreeName.trim())}>Create</button>
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
  .wt-new { font-style: italic; }
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
  .kind-row { display: flex; gap: 0.3rem; }
  .kind-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.7rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .kind-pick.active { color: var(--fg); border-color: var(--accent); }
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
