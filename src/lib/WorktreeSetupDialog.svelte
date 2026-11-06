<script lang="ts">
  // New worktree setup (docs/work-graph.md#new-worktree-setup), from the New
  // session picker's project menu: the files a fresh worktree of this project
  // gets from its base checkout, and the command run in it before the agent
  // starts. A failure stops that start with "Setup failed" and keeps the
  // worktree; nothing runs for an existing worktree.
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import { errorText } from './error_copy';
  import { formKeys, platformSubmitHint } from './forms/form_frame';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { SETUP_GLOBS_MAX, getWorktreeSetup, globError, parseGlobs, setWorktreeSetup } from './worktree_setup';

  let {
    projectId,
    title,
    onclose,
  }: {
    projectId: number;
    /** The project as the picker names it. */
    title: string;
    onclose: () => void;
  } = $props();

  let loaded = $state(false);
  let globsText = $state('');
  let command = $state('');
  let busy = $state(false);
  let failure = $state<string | null>(null);

  onMount(async () => {
    const r = await getWorktreeSetup(projectId);
    if (r.ok) {
      globsText = r.value.copy.join('\n');
      command = r.value.command ?? '';
      loaded = true;
    } else {
      failure = errorText(r.error, { read: true });
    }
  });

  const globs = $derived(parseGlobs(globsText));
  const globProblem = $derived(
    globs.map(globError).find((e) => e !== null) ??
      (globs.length > SETUP_GLOBS_MAX ? `At most ${SETUP_GLOBS_MAX} lines` : null),
  );
  const commandProblem = $derived(/[\r\n]/.test(command.trim()) ? 'One line; chain steps with &&' : null);
  const blocked = $derived(hubActionBlocked('worktree_setup', $hubStatus, $hubConnection));
  const canSave = $derived(loaded && !busy && blocked === null && globProblem === null && commandProblem === null);

  async function submit(e?: Event) {
    e?.preventDefault();
    if (!canSave) return;
    busy = true;
    failure = null;
    const r = await setWorktreeSetup(projectId, globs, command);
    busy = false;
    if (!r.ok) {
      failure = errorText(r.error);
      return;
    }
    onclose();
  }

  const hint = platformSubmitHint();
</script>

<Modal title="New worktree setup" {onclose} width="460px" testid="worktree-setup-dialog">
  <form class="form" use:formKeys onsubmit={submit}>
    <p class="note">
      Runs for each new worktree of <strong>{title}</strong>, before its agent starts. An existing worktree is left as
      it is.
    </p>
    <label class="field">
      <span>Copy from the project folder</span>
      <textarea
        rows="4"
        bind:value={globsText}
        placeholder={'.env\nconfig/*.local'}
        spellcheck="false"
        autocomplete="off"
        data-autofocus=""
        disabled={!loaded}
        aria-invalid={globProblem !== null}
        data-testid="worktree-setup-globs"
      ></textarea>
    </label>
    <p class="note">One path or glob per line, relative to the project folder. Files git ignores, like .env, are the usual ones.</p>
    {#if globProblem}
      <p class="err" data-testid="worktree-setup-glob-error">{globProblem}</p>
    {/if}
    <label class="field">
      <span>Then run</span>
      <input
        type="text"
        bind:value={command}
        placeholder="pnpm install --frozen-lockfile"
        spellcheck="false"
        autocomplete="off"
        disabled={!loaded}
        aria-invalid={commandProblem !== null}
        data-testid="worktree-setup-command"
      />
    </label>
    <p class="note">In the new worktree, on its host. If it fails, the start stops with "Setup failed" and its output; the worktree stays.</p>
    {#if commandProblem}
      <p class="err" data-testid="worktree-setup-command-error">{commandProblem}</p>
    {/if}
    {#if blocked}
      <p class="err" data-testid="worktree-setup-blocked">{blocked}</p>
    {/if}
    {#if failure}
      <p class="err" role="alert" data-testid="worktree-setup-error">{failure}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button
        type="submit"
        class="primary"
        disabled={!canSave}
        title={blocked ?? ''}
        data-shortcut={hint.label}
        aria-keyshortcuts={hint.aria}
        data-testid="worktree-setup-save">Save</button
      >
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.5rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field span { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input,
  .field textarea {
    font: inherit;
    font-family: var(--font-mono, monospace);
    font-size: var(--text-xs);
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
    resize: vertical;
  }
  .field [aria-invalid='true'] { border-color: var(--danger); }
  .note { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }
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
