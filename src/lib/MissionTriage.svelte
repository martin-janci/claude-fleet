<!--
  Redesign step 9.10: a stuck mission's card in Missions. Fleet's facts say
  why it is stuck; Jev proposes the outcome so far and the next step
  ("Proposed by Jev · why"); the LLM writes the card's words only on Draft.
  The step buttons hand the choice to the action the person already has
  (`onstep`): nothing here completes, cancels or verifies anything.
-->
<script lang="ts">
  import DraftField from './DraftField.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import StatusChip from './kit/StatusChip.svelte';
  import { preselect } from './ai_proposal';
  import type { Draft } from './drafts';
  import {
    HUB_HAS_NO_TRIAGE,
    NEXT_STEPS,
    STEP_HINTS,
    asProposal,
    countsLine,
    missionTriage,
    outcomeLabel,
    outcomeState,
    stepLabel,
    type NextStep,
    type Triage,
  } from './mission_triage';

  let {
    missionId,
    reload = 0,
    onstep,
  }: {
    missionId: number;
    /** Bumped by the parent when the mission changed: ask again. */
    reload?: number;
    /** A person picked a step. */
    onstep: (step: NextStep) => void;
  } = $props();

  let triage = $state<Triage | null>(null);
  let draft = $state<Draft | null>(null);
  let text = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let gen = 0;

  async function load(id: number) {
    const mine = ++gen;
    const r = await missionTriage(id);
    if (mine !== gen) return;
    if (r.ok) triage = r.value;
    else if (HUB_HAS_NO_TRIAGE.includes(r.error.code)) triage = null;
    else error = r.error.message;
  }

  async function drawCard() {
    busy = true;
    error = null;
    const r = await missionTriage(missionId, true);
    busy = false;
    if (r.ok) {
      triage = r.value;
      draft = r.value.card ?? null;
      text = draft?.text ?? '';
    } else {
      error = r.error.message;
    }
  }

  $effect(() => {
    void reload;
    draft = null;
    text = '';
    void load(missionId);
  });

  const outcome = $derived(asProposal(triage?.outcome));
  const next = $derived(asProposal(triage?.next));
  const proposedStep = $derived(preselect('mission_next_step', next));
  const proposedOutcome = $derived(preselect('mission_outcome', outcome));
</script>

{#if triage?.stuck}
  <section class="triage" aria-label="Stuck mission" data-testid="mission-triage">
    <p class="why"><strong>Failed · stuck</strong> · <span data-testid="mission-triage-why">{triage.stuck.why}</span></p>
    <p class="muted small" data-testid="mission-triage-counts">{countsLine(triage.stuck)}</p>
    {#if triage.stuck.last_failure}
      <p class="muted small failure" data-testid="mission-triage-failure">{triage.stuck.last_failure}</p>
    {/if}
    {#if proposedOutcome}
      <p class="outcome" data-testid="mission-triage-outcome">Outcome so far: <StatusChip state={outcomeState(proposedOutcome)} label={outcomeLabel(proposedOutcome)} /></p>
      <ProposedBy proposal={outcome} field="mission_outcome" testid="mission-triage-outcome-by" />
    {/if}
    {#if triage.may_change}
      <div class="steps" role="group" aria-label="Next step">
        {#each NEXT_STEPS as step (step)}
          <button
            type="button"
            class="btn"
            class:btn--primary={proposedStep === step}
            class:btn--quiet={proposedStep !== step}
            title={STEP_HINTS[step]}
            data-testid="mission-triage-step-{step}"
            data-proposed={proposedStep === step}
            onclick={() => onstep(step)}>{stepLabel(step)}</button
          >
        {/each}
      </div>
      <ProposedBy proposal={next} field="mission_next_step" testid="mission-triage-next-by" />
      {#if draft || busy}
        <DraftField
          bind:value={text}
          label="Card"
          model={draft?.model}
          host={draft?.host_alias}
          from={draft ? `from ${draft.from}` : null}
          {busy}
          rows={3}
          onregenerate={() => void drawCard()}
          onclear={() => (draft = null)}
          testid="mission-triage-card"
        />
      {:else}
        <button class="btn btn--quiet" type="button" data-testid="mission-triage-draft" onclick={() => void drawCard()}
          >Draft the card</button
        >
      {/if}
    {/if}
    {#if error}<p class="error" role="alert" data-testid="mission-triage-error">{error}</p>{/if}
  </section>
{/if}

<style>
  .triage {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 8px 0;
  }
  .why,
  .outcome {
    margin: 0;
  }
  .failure {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .small {
    font-size: var(--text-xs);
    margin: 0;
  }
  .error {
    color: var(--danger);
    font-size: var(--text-xs);
    margin: 0;
  }
</style>
