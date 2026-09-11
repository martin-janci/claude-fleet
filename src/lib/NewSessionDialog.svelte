<script lang="ts">
  import { untrack } from 'svelte';
  import type { ProjectTreeRow, WorktreeRow } from './projects';
  import { newSessionAbortable, sessions, type SessionRow } from './sessions';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';
  import { slugifyBranch, finalizeBranchSlug } from './branch-slug';
  import { generateName, nameWords, tmuxNameSuffix } from './names';
  import Modal from './Modal.svelte';
  import PickerList from './PickerList.svelte';
  import type { PickerItem } from './PickerList.svelte';

  let {
    project,
    onCreate,
    onCancel,
    initialName,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
    /** Pre-fill the friendly name (the quick switcher's query). */
    initialName?: string;
  } = $props();

  // The project is fixed for the dialog's lifetime (the parent remounts for
  // a different one), so these snapshots are intentional.
  const projectId = untrack(() => project.project.id);
  const owner = untrack(() => project.project.owner);
  const repo = untrack(() => project.project.repo);
  const base = `dev-${owner}-${repo}`;

  // ── Remembered choices ───────────────────────────────────────────────
  // Global last host (existing pref) is the fallback; per-project memory
  // (`newsession.project.<id>`) wins so a project that always runs on
  // `hetzner` in a fresh worktree opens that way.
  const isString = (v: unknown): v is string => typeof v === 'string';
  interface ProjectMemory {
    host: string;
    worktree: number | 'new';
    kind: 'work' | 'shell';
  }
  const isMemory = (v: unknown): v is ProjectMemory =>
    typeof v === 'object' &&
    v !== null &&
    typeof (v as ProjectMemory).host === 'string' &&
    ((v as ProjectMemory).worktree === 'new' || typeof (v as ProjectMemory).worktree === 'number') &&
    ((v as ProjectMemory).kind === 'work' || (v as ProjectMemory).kind === 'shell');
  const memoryKey = `newsession.project.${projectId}`;
  const memory = readPref<ProjectMemory | null>(memoryKey, null, (v): v is ProjectMemory | null => v === null || isMemory(v));

  let chosenHost = $state<string>(
    untrack(() => memory?.host ?? readPref('last-host', 'local', isString)),
  );
  $effect(() => {
    writePref('last-host', chosenHost);
  });

  // "work" runs Claude Code in the pane; "shell" runs a plain login shell.
  let chosenKind = $state<'work' | 'shell'>(untrack(() => memory?.kind ?? 'work'));
  // Optional command run on start for a shell session (empty = bare shell).
  let startCommand = $state<string>('');

  // Inverse of slugify-ish: take a worktree/branch name and produce a
  // sentence-cased label so the friendly-name field is pre-filled with
  // something readable when the user picks an existing worktree.
  function humanize(branch: string): string {
    if (!branch || branch === 'main') return '';
    const spaced = branch.replace(/[-_]+/g, ' ').trim();
    if (!spaced) return '';
    return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
  }

  // ── Generated names ──────────────────────────────────────────────────
  // Every slug already in use on this project: worktree names, the suffix
  // of every tmux name, and slugified friendly names. The generator avoids
  // all of them so "blue sirius" is never offered twice.
  const takenSlugs = $derived.by(() => {
    const set = new Set<string>();
    for (const w of project.worktrees) set.add(w.name.toLowerCase());
    for (const s of $sessions) {
      if (s.project_id !== project.project.id) continue;
      const suffix = tmuxNameSuffix(s.tmux_name, owner, repo);
      if (suffix) set.add(suffix.toLowerCase());
      if (s.friendly_name) set.add(finalizeBranchSlug(s.friendly_name));
    }
    return set;
  });
  const takenFriendly = $derived(
    new Set(
      $sessions
        .filter((s) => s.project_id === project.project.id && s.friendly_name)
        .map((s) => s.friendly_name!.trim().toLowerCase()),
    ),
  );

  function freshName(): string {
    return nameWords(generateName(takenSlugs));
  }

  // Default friendly name for a worktree pick: the humanised branch unless
  // it is empty (main) or already used by a session on this project — then
  // a generated pair, so the user never has to invent "work 3".
  function defaultFriendly(wt: WorktreeRow | null): string {
    const h = humanize(wt?.name ?? '');
    if (h && !takenFriendly.has(h.toLowerCase())) return h;
    return freshName();
  }

  // ── Worktree choice ──────────────────────────────────────────────────
  function initialWorktree(): number | null {
    if (memory?.worktree === 'new') return null;
    if (typeof memory?.worktree === 'number' && project.worktrees.some((w) => w.id === memory.worktree)) {
      return memory.worktree;
    }
    return project.worktrees[0]?.id ?? null;
  }
  let chosenWorktreeId = $state<number | null>(untrack(initialWorktree));
  let inNewMode = $derived(chosenWorktreeId === null);
  let chosenWorktree = $derived(project.worktrees.find((w) => w.id === chosenWorktreeId) ?? null);

  let newWorktreeName = $state<string>('');
  // Base branch to fork the new worktree from. Empty = the repo's default
  // branch (the backend falls back to default if the named branch is missing).
  let baseBranch = $state<string>('');
  // Track whether the user has manually edited the slug; once they do, we
  // stop auto-syncing it from the friendly-name field so their override
  // sticks. Reset on worktree-mode changes.
  let slugDirty = $state<boolean>(false);
  let friendlyName = $state<string>('');
  // A hand-edited tmux name sticks until the next mode/kind change; null
  // means "derived from the other fields" (the normal case).
  let nameOverride = $state<string | null>(null);

  // Initial fill (untracked: reads stores once, on open).
  untrack(() => {
    const wt = project.worktrees.find((w) => w.id === chosenWorktreeId) ?? null;
    if (initialName?.trim()) {
      friendlyName = initialName.trim();
    } else if (chosenWorktreeId === null) {
      friendlyName = freshName();
    } else {
      friendlyName = defaultFriendly(wt);
    }
    if (chosenWorktreeId === null) newWorktreeName = finalizeBranchSlug(friendlyName);
  });

  const friendlySlug = $derived(finalizeBranchSlug(friendlyName));
  const termSuffix = $derived(chosenKind === 'shell' ? '-term' : '');

  // The tmux name: `dev-<owner>-<repo>--<worktree>` (or the bare base for
  // main), and when that name is already live on the chosen host — a second
  // session on the same worktree — `…--<worktree>--<name-slug>` so the two
  // never collide. In new-worktree mode the slug IS the worktree name.
  const derivedName = $derived.by(() => {
    if (inNewMode) {
      const slug = finalizeBranchSlug(newWorktreeName);
      return slug ? `${base}--${slug}${termSuffix}` : `${base}${termSuffix}`;
    }
    const wt = chosenWorktree;
    const isMain = !wt || wt.name === 'main';
    const deterministic = isMain ? `${base}${termSuffix}` : `${base}--${wt.name}${termSuffix}`;
    const taken = $sessions.some((s) => s.host_alias === chosenHost && s.tmux_name === deterministic);
    if (!taken || !friendlySlug) return deterministic;
    return isMain
      ? `${base}--${friendlySlug}${termSuffix}`
      : `${base}--${wt.name}--${friendlySlug}${termSuffix}`;
  });
  const name = $derived(nameOverride ?? derivedName);

  // Where the pane's cwd will be. Local paths come from the DB; remote ones
  // follow proj-clean's `~/projects/github.com/<owner>/<repo>` convention
  // (see `remote_project_path` in service/sessions.rs). New worktrees land
  // in whichever of `.worktrees` / `.claude/worktrees` the repo already uses.
  const worktreeDir = $derived(
    project.worktrees.some((w) => w.path.includes('/.claude/worktrees/')) ? '.claude/worktrees' : '.worktrees',
  );
  const pathPreview = $derived.by(() => {
    const root = chosenHost === 'local' ? project.project.base_path : `~/projects/github.com/${owner}/${repo}`;
    if (inNewMode) {
      const slug = finalizeBranchSlug(newWorktreeName);
      return slug ? `${root}/${worktreeDir}/${slug}` : root;
    }
    const wt = chosenWorktree;
    if (!wt || wt.name === 'main') return root;
    return chosenHost === 'local' ? wt.path : `${root}/.claude/worktrees/${wt.name}`;
  });

  const worktreeItems: PickerItem[] = $derived([
    ...project.worktrees.map((wt) => ({
      key: String(wt.id),
      label: wt.name,
      description: wt.branch && wt.branch !== wt.name ? wt.branch : undefined,
      meta: $sessions.some((s) => s.worktree_id === wt.id && s.status !== 'ghost') ? 'in use' : undefined,
      testid: 'worktree-row',
    })),
    { key: 'new', label: '+ new worktree', description: 'fresh branch from the base branch', testid: 'new-worktree-chip' },
  ]);

  let busy = $state(false);
  let error: string | null = $state(null);
  let createController: AbortController | null = null;

  function onPickKind(kind: 'work' | 'shell') {
    chosenKind = kind;
    nameOverride = null;
  }

  function onPickWorktreeKey(key: string) {
    if (key === 'new') {
      onPickNew();
      return;
    }
    onPickWorktree(Number(key));
  }

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    newWorktreeName = '';
    baseBranch = '';
    slugDirty = false;
    nameOverride = null;
    const wt = project.worktrees.find((w) => w.id === id) ?? null;
    friendlyName = defaultFriendly(wt);
  }

  function onPickNew() {
    chosenWorktreeId = null;
    baseBranch = '';
    slugDirty = false;
    nameOverride = null;
    friendlyName = freshName();
    newWorktreeName = finalizeBranchSlug(friendlyName);
  }

  function reroll() {
    friendlyName = freshName();
    slugDirty = false;
    nameOverride = null;
    if (inNewMode) newWorktreeName = finalizeBranchSlug(friendlyName);
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
      newWorktreeName = finalizeBranchSlug(value);
    }
  }

  function onNewWorktreeNameInput(value: string) {
    // Auto-correct free-form input ("fix login bug") into a git-safe slug
    // ("fix-login-bug") as the user types.
    newWorktreeName = slugifyBranch(value);
    // Any divergence from the friendly-name-derived slug means the user has
    // taken manual control — stop auto-syncing future friendly-name edits.
    slugDirty = newWorktreeName !== finalizeBranchSlug(friendlyName);
  }

  function onNewWorktreeNameBlur() {
    const cleaned = finalizeBranchSlug(newWorktreeName);
    if (cleaned !== newWorktreeName) newWorktreeName = cleaned;
  }

  function onNameInput(value: string) {
    nameOverride = value;
  }

  function remember() {
    writePref<ProjectMemory>(memoryKey, {
      host: chosenHost,
      worktree: chosenWorktreeId === null ? 'new' : chosenWorktreeId,
      kind: chosenKind,
    });
  }

  async function submit() {
    if (busy) return;
    if (inNewMode) {
      // Strip any trailing dash the live slugifier left in place so the
      // backend sees a fully-finalized branch name.
      const cleaned = finalizeBranchSlug(newWorktreeName);
      if (cleaned !== newWorktreeName) newWorktreeName = cleaned;
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
        // An empty tmux name is legal: the backend mints one with the same
        // generator (see `fill_session_name`).
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
    remember();
    onCreate(r.value);
  }

  function cancelCreate() {
    createController?.abort();
  }

  // Enter in any field creates; Cmd/Ctrl+R re-rolls the name (and never
  // reloads the webview). Escape is handled by <Modal>.
  function onKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'r') {
      e.preventDefault();
      reroll();
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === 'INPUT') {
        e.preventDefault();
        void submit();
      }
    }
  }
</script>

<Modal label="New session" onclose={onCancel} width="420px">
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dialog" onkeydown={onKeydown}>
  <h3>New session — {owner}/{repo}</h3>

  <div class="fields">
    <label for="friendly-name">Name</label>
    <div class="name-row">
      <input
        id="friendly-name"
        data-testid="friendly-name"
        data-autofocus
        value={friendlyName}
        oninput={(e) => onFriendlyNameInput((e.target as HTMLInputElement).value)}
        placeholder="blue sirius"
        maxlength="80"
      />
      <button
        type="button"
        class="dice"
        data-testid="reroll-name"
        onclick={reroll}
        title="Roll a new name (Ctrl/⌘+R)"
        aria-label="Roll a new name"
      >🎲</button>
    </div>

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
          onclick={() => {
            chosenHost = h.alias;
            nameOverride = null;
          }}
        >
          {h.alias}
        </button>
      {/each}
    </div>

    <label for="wt-picker">Worktree</label>
    <PickerList
      items={worktreeItems}
      activeKey={inNewMode ? 'new' : String(chosenWorktreeId)}
      onpick={onPickWorktreeKey}
      maxHeight="9rem"
      ariaLabel="Worktree"
      testid="wt-picker"
    />

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
    <input
      id="session-name"
      data-testid="new-session-name"
      value={name}
      oninput={(e) => onNameInput((e.target as HTMLInputElement).value)}
      placeholder="empty = let fleet pick one"
    />
    <p class="preview" data-testid="path-preview" title={pathPreview}>
      <span class="k">cwd</span> <code>{pathPreview}</code>
    </p>

    {#if error}
      <p class="err">{error}</p>
    {/if}
  </div>

  <div class="actions">
    <span class="hint">↵ create · Ctrl/⌘R re-roll</span>
    <button onclick={onCancel} disabled={busy}>Cancel</button>
    {#if busy}
      <button type="button" data-testid="cancel-create" onclick={cancelCreate}>Cancel creation</button>
    {:else}
      <button class="primary" onclick={submit} disabled={inNewMode && !newWorktreeName.trim()}>Create</button>
    {/if}
  </div>
</div>
</Modal>

<style>
  /* The dialog owns its height budget: the field stack scrolls, the
     Create/Cancel row is pinned, so no number of worktrees or hosts can push
     the buttons off-screen. Modal's body caps at 85vh and adds 1rem padding
     per side; stay inside that. */
  .dialog {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-height: calc(85vh - 2rem);
    min-height: 0;
  }
  .dialog h3 { margin: 0 0 0.3rem 0; font-size: 0.95rem; flex: 0 0 auto; }
  .fields {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    overflow-y: auto;
    min-height: 0;
    flex: 1 1 auto;
    padding-right: 0.2rem;
  }
  label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  input {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    min-width: 0;
  }
  .name-row { display: flex; gap: 0.3rem; }
  .name-row input { flex: 1 1 auto; }
  .dice {
    font-size: 1rem;
    line-height: 1;
    padding: 0.2rem 0.45rem;
    border: 1px solid var(--border);
    background: transparent;
    border-radius: 4px;
    cursor: pointer;
  }
  .dice:hover { border-color: var(--accent); }
  .host-row {
    display: flex;
    gap: 0.3rem;
    flex-wrap: wrap;
    max-height: 5.2rem;
    overflow-y: auto;
  }
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
  .preview {
    margin: 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .preview .k { text-transform: uppercase; font-size: 0.65rem; margin-right: 0.3rem; }
  .preview code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions {
    display: flex;
    gap: 0.4rem;
    justify-content: flex-end;
    align-items: center;
    flex: 0 0 auto;
    padding-top: 0.2rem;
    border-top: 1px solid var(--border);
  }
  .actions .hint { margin-right: auto; font-size: 0.68rem; color: var(--fg-muted); }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button.primary { border-color: var(--accent); }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
