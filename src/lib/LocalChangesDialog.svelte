<script lang="ts">
  // Review changes (local workspace Phase 2): git's own list of uncommitted
  // changes in the linked worktree, which side the sync carried each one
  // from, the diff of the one picked, and what to do with them: ask the
  // agent, commit, discard, or dismiss the activity.
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import DiffView from './DiffView.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import {
    ASK_INTENTS,
    loadLocalChanges,
    loadLocalDiff,
    commitLocalWorkspace,
    discardLocalChanges,
    dismissLocalActivity,
    askAiAboutLocalChanges,
    type AskIntent,
    type FileDiff,
    type LocalChanges,
    type LocalWorkspace,
  } from './local_workspaces';
  import { push } from './toasts';

  let { link, onclose }: { link: LocalWorkspace; onclose: () => void } = $props();

  let changes = $state<LocalChanges | null>(null);
  let loading = $state(true);
  let selected = $state(new Set<string>());
  let current = $state<string | null>(null);
  let diff = $state<FileDiff | null>(null);
  let busy = $state(false);
  let intent = $state<AskIntent>('continue');
  let question = $state('');
  let message = $state('');
  let confirmDiscard = $state(false);

  const ORIGIN_LABEL: Record<string, string> = { local: 'you', remote: 'agent' };

  async function reload() {
    loading = true;
    const c = await loadLocalChanges(link.id);
    loading = false;
    changes = c;
    selected = new Set((c?.files ?? []).map((f) => f.path));
    if (current && !c?.files.some((f) => f.path === current)) {
      current = null;
      diff = null;
    }
  }

  onMount(() => {
    void reload();
  });

  async function show(path: string) {
    current = path;
    diff = null;
    const d = await loadLocalDiff(link.id, path);
    if (current === path) diff = d;
  }

  function toggle(path: string) {
    const next = new Set(selected);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    selected = next;
  }

  async function run(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } finally {
      busy = false;
    }
  }

  const paths = $derived([...selected]);
  const files = $derived(changes?.files ?? []);

  function ask() {
    void run(async () => {
      const r = await askAiAboutLocalChanges(link.id, intent, {
        question: question.trim() || undefined,
        paths,
      });
      if (r) {
        push({ kind: 'info', message: 'Sent to the agent.' });
        onclose();
      }
    });
  }

  function commit() {
    void run(async () => {
      const r = await commitLocalWorkspace(link.id, message.trim(), paths);
      if (r) {
        push({ kind: 'info', message: `Committed ${r.commit.slice(0, 9)}.` });
        message = '';
        await reload();
      }
    });
  }
</script>

<Modal title="Changes in {link.worktree_key}" {onclose} width="860px" testid="lw-changes-dialog">
  {#if loading && !changes}
    <p class="muted">Syncing and reading git status on {link.host_alias}…</p>
  {:else if !changes}
    <p class="muted">The changes could not be read.</p>
  {:else if files.length === 0}
    <p class="muted" data-testid="lw-changes-empty">
      Nothing uncommitted on {changes.branch || 'this worktree'}.
    </p>
    {#if changes.activity.length > 0}
      <button
        class="ghost"
        disabled={busy}
        onclick={() => run(async () => (await dismissLocalActivity(link.id)) && onclose())}
      >Clear the change counts</button>
    {/if}
  {:else}
    <p class="muted small">
      Uncommitted on <code>{changes.branch}</code>, as git sees it on {link.host_alias}.
    </p>
    <div class="split">
      <ul class="files" data-testid="lw-changes-files">
        {#each files as f (f.path)}
          <li class:current={current === f.path}>
            <input
              type="checkbox"
              checked={selected.has(f.path)}
              onchange={() => toggle(f.path)}
              aria-label="Include {f.path}"
            />
            <button class="file" onclick={() => show(f.path)} data-testid="lw-change">
              <span class="st">{f.status}</span>
              <code title={f.path}>{f.path}</code>
            </button>
            {#if f.origin}
              <span class="origin origin-{f.origin}">{ORIGIN_LABEL[f.origin] ?? f.origin}</span>
            {/if}
          </li>
        {/each}
      </ul>
      <div class="diff">
        {#if current && !diff}
          <p class="muted small">Loading the diff…</p>
        {:else if diff?.binary}
          <p class="muted small">{diff.path} is a binary file.</p>
        {:else if diff}
          <DiffView diff={diff.diff} testid="lw-diff" />
          {#if diff.truncated}<p class="muted small">The diff was cut short.</p>{/if}
        {:else}
          <p class="muted small">Pick a file to see its diff.</p>
        {/if}
      </div>
    </div>

    <div class="actions">
      <div class="row">
        <select bind:value={intent} aria-label="What to ask Claude" data-testid="lw-ask-intent" disabled={busy}>
          {#each ASK_INTENTS as i (i.intent)}
            <option value={i.intent}>{i.label}</option>
          {/each}
        </select>
        <button
          class="primary"
          data-testid="lw-ask"
          disabled={busy || paths.length === 0 || (intent === 'custom' && !question.trim())}
          onclick={ask}
        >Ask AI</button>
      </div>
      <textarea
        rows="2"
        bind:value={question}
        placeholder={intent === 'custom'
          ? 'What should the agent look at in these changes?'
          : 'Anything to add? (optional)'}
        aria-label={intent === 'custom' ? 'Question' : 'Optional question'}
        data-testid="lw-ask-question"
      ></textarea>
      <div class="row">
        <input
          class="msg"
          type="text"
          bind:value={message}
          placeholder="Commit message"
          data-testid="lw-commit-message"
        />
        <button
          class="ghost"
          data-testid="lw-commit"
          disabled={busy || paths.length === 0 || !message.trim()}
          onclick={commit}
        >Commit {paths.length}</button>
        <button
          class="ghost danger"
          data-testid="lw-discard"
          disabled={busy || paths.length === 0}
          onclick={() => (confirmDiscard = true)}
        >Discard…</button>
        <button
          class="ghost"
          disabled={busy || changes.activity.length === 0}
          title="Mark these as looked at; the files stay as they are"
          onclick={() => run(async () => (await dismissLocalActivity(link.id)) && onclose())}
        >Dismiss</button>
      </div>
    </div>
  {/if}
</Modal>

{#if confirmDiscard}
  <ConfirmDialog
    title="Discard {paths.length} change{paths.length === 1 ? '' : 's'}?"
    message={`They go back to the last commit on ${link.host_alias}, and the next sync carries that to ${link.local_path}. This cannot be undone.`}
    confirmLabel="Discard"
    danger
    {busy}
    confirmTestId="lw-discard-confirm"
    onconfirm={() => {
      confirmDiscard = false;
      const chosen = paths;
      void run(async () => {
        if (await discardLocalChanges(link.id, chosen)) await reload();
      });
    }}
    oncancel={() => (confirmDiscard = false)}
  />
{/if}

<style>
  .muted { margin: 0; color: var(--fg-muted); font-size: var(--text-xs); }
  .small { font-size: var(--text-2xs); }
  .split {
    display: grid;
    grid-template-columns: minmax(14rem, 1fr) 2fr;
    gap: 0.6rem;
    margin: 0.5rem 0;
    min-height: 16rem;
    max-height: 55vh;
  }
  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .files li {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.2rem 0.35rem;
    font-size: var(--text-2xs);
  }
  .files li.current { background: var(--bg-hover); }
  .file {
    flex: 1;
    min-width: 0;
    display: flex;
    gap: 0.35rem;
    background: none;
    border: none;
    color: var(--fg);
    cursor: pointer;
    text-align: left;
    padding: 0;
    font: inherit;
  }
  .file code { font-family: var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .st { font-family: var(--mono); color: var(--fg-muted); width: 1.4rem; flex: none; }
  .origin {
    font-size: var(--text-2xs);
    padding: 0 0.35rem;
    border-radius: var(--radius-xs);
    border: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .origin-local { border-color: var(--usage-warn); color: var(--usage-warn); }
  .origin-remote { border-color: var(--accent); color: var(--accent); }
  .diff { overflow: auto; min-width: 0; }
  .actions { display: flex; flex-direction: column; gap: 0.4rem; }
  .row { display: flex; gap: 0.4rem; flex-wrap: wrap; align-items: center; }
  select, .msg, textarea {
    font-size: var(--text-xs);
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .msg { flex: 1; min-width: 12rem; }
  textarea { width: 100%; box-sizing: border-box; font-family: inherit; }
  .ghost, .primary {
    font-size: var(--text-xs);
    padding: 0.35rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost:hover:not(:disabled) { border-color: var(--accent); }
  .ghost.danger { color: var(--usage-crit); }
  .primary { background: var(--accent); border-color: var(--accent); color: var(--accent-fg); }
  button:disabled { opacity: 0.55; cursor: default; }
</style>
