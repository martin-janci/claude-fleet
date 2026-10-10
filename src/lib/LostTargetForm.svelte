<!--
  Orbit Fleet 4.12, board HostDetail "Lost & found": "Adopt into papaya-pos
  ▾ · Proposed by Jev · why · Change" and "Restore into Pick a project ▾ ·
  Jev was unsure, so nothing is filled in". The project is prefilled from
  `lost_target` (the rule's, else Jev's at assist); nothing happens until a
  person presses the button and confirms. No loader: the proposal appears
  when ready or not at all, and the select works meanwhile.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { preselect } from './ai_proposal';
  import { projects, loadProjects } from './projects';
  import {
    UNSURE_NOTE,
    confirmTitle,
    lostTarget,
    pickableProjects,
    projectLabel,
    proposalOf,
    ticketLabel,
    ticketProposalOf,
    showsUnsure,
    type LostTarget,
    type LostTargetArgs,
  } from './lost_found';

  let {
    action,
    entry,
    args,
    requireProject = false,
    onsubmit,
    oncancel,
    onignore,
  }: {
    /** The button: Adopt (a pane) or Restore (a conversation). */
    action: 'Adopt' | 'Restore';
    /** The entry's name, for the confirmation: `fleet-trn-scratch`. */
    entry: string;
    args: LostTargetArgs;
    /** Restore cannot resume without a project; Adopt can go without one. */
    requireProject?: boolean;
    /** Do it, linking the new session to `ticket` when the person kept
     *  the proposed ticket ticked (J10, a found conversation only);
     *  resolves to an error message, or null when done. */
    onsubmit: (projectId: number | null, ticket: string | null) => Promise<string | null>;
    oncancel: () => void;
    /** "Ignore" (gap plan G2.7): leave this entry out from now on. Absent,
     *  the form offers no Ignore. */
    onignore?: () => void;
  } = $props();

  const list = $derived(pickableProjects($projects.map((p) => p.project)));
  let target = $state<LostTarget | null>(null);
  let selected = $state('');
  /** The person chose for themselves: a late proposal never overwrites it. */
  let touched = $state(false);
  /** J10's other half: link the restored session to the ticket its branch
   *  names. Prefilled ticked; nothing links until the person confirms. */
  let linkTicket = $state(true);
  let confirming = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const proposal = $derived(proposalOf(target, list));
  /** The chip stays only while the field still holds what it proposed. */
  const shownProposal = $derived(proposal && proposal.value === selected ? proposal : null);
  const unsure = $derived(!touched && selected === '' && showsUnsure(target));
  const chosen = $derived(list.find((p) => String(p.id) === selected) ?? null);
  const ticket = $derived(target?.ticket ?? null);
  const ticketProposal = $derived(ticket ? ticketProposalOf(ticket) : null);

  onMount(() => {
    if ($projects.length === 0) void loadProjects();
    void (async () => {
      const r = await lostTarget(args);
      if (!r.ok) return; // the form only loses its prefill
      target = r.value;
      const pre = preselect('project', proposalOf(r.value, list));
      if (!touched && pre !== null) selected = pre;
    })();
  });

  function change() {
    touched = true;
    selected = '';
  }

  async function confirm() {
    busy = true;
    error = null;
    const err = await onsubmit(chosen ? chosen.id : null, ticket && linkTicket ? ticket.key : null);
    busy = false;
    confirming = false;
    if (err) error = err;
  }
</script>

<div class="form" data-testid="lost-target-form">
  <label class="row">
    <span class="label">{action} into</span>
    <select
      bind:value={selected}
      class:ai-pre={shownProposal !== null && !touched}
      onchange={() => (touched = true)}
      aria-label="{action} into"
      data-testid="lost-target-project"
    >
      <option value="">{requireProject ? 'Pick a project' : 'No project'}</option>
      {#each list as p (p.id)}
        <option value={String(p.id)}>{projectLabel(p)}</option>
      {/each}
    </select>
  </label>
  <ProposedBy proposal={shownProposal} field="project" onchange={change} testid="lost-target-proposed" />
  {#if unsure}
    <p class="muted" data-testid="lost-target-unsure">{UNSURE_NOTE}</p>
  {/if}
  {#if ticket}
    <label class="row">
      <input type="checkbox" bind:checked={linkTicket} data-testid="lost-target-ticket" />
      <span class="label">Link to {ticketLabel(ticket)}</span>
    </label>
    {#if linkTicket}
      <ProposedBy proposal={ticketProposal} field="ticket" testid="lost-target-ticket-proposed" />
    {/if}
  {/if}
  {#if error}<p class="error" data-testid="lost-target-error">{error}</p>{/if}
  <div class="actions">
    {#if onignore}
      <button
        type="button"
        class="small ignore"
        disabled={busy}
        title="Leave it out of this list from now on, on this device. Nothing on the host changes."
        data-testid="lost-target-ignore"
        onclick={onignore}>Ignore</button
      >
    {/if}
    <button
      type="button"
      class="small primary"
      disabled={busy || (requireProject && chosen === null)}
      data-testid="lost-target-submit"
      onclick={() => (confirming = true)}>{action}…</button
    >
    <button type="button" class="small" disabled={busy} data-testid="lost-target-cancel" onclick={oncancel}
      >Cancel</button
    >
  </div>
</div>

{#if confirming}
  <ConfirmDialog
    title={confirmTitle(action, entry, chosen)}
    message={action === 'Adopt'
      ? 'Fleet runs it from now on. The pane and what runs in it stay as they are.'
      : 'Claude resumes the conversation in a new pane in that project. The transcript is copied, never moved.'}
    confirmLabel={action}
    {busy}
    confirmTestId="lost-target-confirm"
    onconfirm={() => void confirm()}
    oncancel={() => (confirming = false)}
  />
{/if}

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px 0 2px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .label {
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .muted {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .error {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--danger);
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .ignore {
    margin-right: auto;
  }
</style>
