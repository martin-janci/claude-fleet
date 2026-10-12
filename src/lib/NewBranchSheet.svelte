<!--
  New branch (Orbit Fleet gap plan step G2.7, the FormsSession board;
  replaces the bare PromptDialog): where it starts ("From fix/x at 4e1a9c2."),
  the name checked on blur with a corrected one to take ("Use
  fix/hub-e2e-windows-2?"), and "Check it out now". A failed create keeps
  the name and says why at the top.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
  import { repoBranches, repoCreateBranch, type Branch } from './history';
  import { suggestBranchName, validateBranchName } from './branch-slug';
  import type { IpcError } from './result';

  let {
    sessionId,
    startPoint = null,
    ondone,
    onclose,
  }: {
    sessionId: number;
    /** A commit to start from (History's "Branch from here"); null starts
     *  from the checked-out branch. */
    startPoint?: string | null;
    /** Created; `checkout` says whether the worktree is on it now. */
    ondone: (name: string, checkout: boolean) => void;
    onclose: () => void;
  } = $props();

  let name = $state('');
  let checkout = $state(true);
  let checked = $state(false);
  let busy = $state(false);
  let error = $state<string | IpcError | null>(null);
  let branches = $state<Branch[]>([]);

  onMount(() => {
    void repoBranches(sessionId).then((r) => {
      if (r.ok) branches = r.value;
    });
  });

  const locals = $derived(branches.filter((b) => !b.isRemote).map((b) => b.name));
  const current = $derived(branches.find((b) => b.isCurrent && !b.isRemote) ?? null);
  const from = $derived(
    startPoint
      ? `From ${startPoint.slice(0, 7)}.`
      : current
        ? `From ${current.name} at ${current.tipHash.slice(0, 7)}.`
        : 'From the checked-out commit.',
  );
  const trimmed = $derived(name.trim());
  const problem = $derived(
    validateBranchName(trimmed) ??
      (locals.includes(trimmed) ? `A branch named ${trimmed} already exists.` : null),
  );
  const suggestion = $derived(problem && trimmed !== '' ? suggestBranchName(trimmed, locals) : null);

  // Enter in the name creates, as the one-field prompt it replaces did
  // (the checkbox makes this a two-control sheet, where only ⌘↵ submits).
  function onNameKey(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.isComposing || e.shiftKey || e.metaKey || e.ctrlKey || e.altKey) return;
    e.preventDefault();
    void create();
  }

  async function create() {
    checked = true;
    if (problem !== null || busy) return;
    busy = true;
    error = null;
    const r = await repoCreateBranch(sessionId, trimmed, { startPoint, checkout });
    busy = false;
    if (!r.ok) {
      error = r.error;
      return;
    }
    ondone(trimmed, checkout);
  }
</script>

<DialogSheet
  title="New branch"
  lead={from}
  verb="Create branch"
  busyVerb="Creating…"
  {busy}
  {error}
  dirty={trimmed !== ''}
  canConfirm={problem === null}
  confirmTitle={trimmed === '' ? 'Name the branch first.' : 'Fix the name first.'}
  onconfirm={() => void create()}
  oninvalid={() => (checked = true)}
  {onclose}
  testid="new-branch-sheet"
  confirmTestid="new-branch-create"
  errorTestid="new-branch-error"
>
  <label class="field">
    <span class="field-label">Name</span>
    <input
      type="text"
      data-testid="new-branch-name"
      bind:value={name}
      placeholder="feature/my-change"
      aria-invalid={checked && problem !== null}
      onblur={() => (checked = trimmed !== '' || checked)}
      onkeydown={onNameKey}
      autocomplete="off"
      spellcheck="false"
    />
    {#if checked && problem}
      <p class="field-note problem" role="alert" data-testid="new-branch-problem">
        {problem}
        {#if suggestion}
          <button type="button" class="use" data-testid="new-branch-suggestion" onclick={() => (name = suggestion)}
            >Use {suggestion}?</button
          >
        {/if}
      </p>
    {/if}
  </label>
  <label class="check">
    <input type="checkbox" bind:checked={checkout} data-testid="new-branch-checkout" />
    Check it out now
  </label>
</DialogSheet>

<style>
  .problem {
    color: var(--danger) !important;
  }
  .use {
    font: inherit;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    cursor: pointer;
    text-decoration: underline;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: var(--text-sm);
    cursor: pointer;
  }
</style>
