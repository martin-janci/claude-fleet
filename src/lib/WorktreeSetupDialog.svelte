<script lang="ts">
  // New worktree setup (docs/work-graph.md#new-worktree-setup), from the New
  // session picker's project menu: the files a fresh worktree of this project
  // gets from its base checkout, and the command run in it before the agent
  // starts. A failure stops that start with "Setup failed" and keeps the
  // worktree; nothing runs for an existing worktree.
  import { onMount } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
  import { errorText } from './error_copy';
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
  /** What the setup held when it was read: closing with other text asks
   *  "Discard changes?". */
  let saved = $state({ globs: '', command: '' });
  let busy = $state(false);
  let failure = $state<string | null>(null);

  onMount(async () => {
    const r = await getWorktreeSetup(projectId);
    if (r.ok) {
      globsText = r.value.copy.join('\n');
      command = r.value.command ?? '';
      saved = { globs: globsText, command };
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
  const blocked = $derived(hubActionBlocked('worktree_setup', $hubStatus, $hubConnection));
  const canSave = $derived(loaded && !busy && blocked === null && globProblem === null);
  const dirty = $derived(loaded && (globsText !== saved.globs || command !== saved.command));
  const why = $derived(blocked ?? (globProblem !== null ? 'Fix the globs first.' : null));

  async function submit() {
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
</script>

<DialogSheet
  title="New worktree setup"
  lead={`Runs for each new worktree of ${title}, before its agent starts. An existing worktree is left as it is.`}
  verb="Save"
  busyVerb="Saving…"
  onconfirm={submit}
  {onclose}
  canConfirm={canSave}
  {busy}
  error={failure}
  {dirty}
  width="460px"
  testid="worktree-setup-dialog"
  confirmTestid="worktree-setup-save"
  errorTestid="worktree-setup-error"
  confirmTitle={why}
>
    <label class="field">
      <span class="field-label">Copy from the project folder</span>
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
    <p class="field-note">One path or glob per line, relative to the project folder. Files git ignores, like .env, are the usual ones.</p>
    {#if globProblem}
      <p class="err" data-testid="worktree-setup-glob-error">{globProblem}</p>
    {/if}
    <label class="field">
      <span class="field-label">Then run</span>
      <input
        type="text"
        bind:value={command}
        placeholder="pnpm install --frozen-lockfile"
        spellcheck="false"
        autocomplete="off"
        disabled={!loaded}
        data-testid="worktree-setup-command"
      />
    </label>
    <p class="field-note">
      In the new worktree, on its host, for up to 4 minutes. If it fails, the start stops with "Setup failed" and its
      output; the worktree stays.
    </p>
</DialogSheet>

<style>
  .field input,
  .field textarea {
    font-family: var(--font-mono, monospace);
  }
  .field [aria-invalid='true'] { border-color: var(--danger); }
  .err { color: var(--danger); font-size: var(--text-xs); margin: 0; }
</style>
