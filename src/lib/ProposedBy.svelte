<!--
  Redesign step 3.11: "Proposed by Jev · why · Change". Shown beside a field
  the proposal pre-selected; nothing at all when it pre-selected nothing
  (below the floor, unsure, or a target AI never decides). No loader: a
  proposal appears when ready or not at all.
-->
<script lang="ts">
  import { neverDecides, preselect, proposedByLabel, type ProposalLike } from './ai_proposal';

  let {
    proposal,
    field,
    floor,
    stated = false,
    changeLabel = 'Change',
    onchange,
    testid = 'proposed-by',
  }: {
    proposal: ProposalLike | null | undefined;
    /** What the proposal would pre-select (`project`, `host`...); a
     *  target in NEVER_DECIDES never shows. */
    field: string;
    /** The use case confidence floor, in whole percent. */
    floor?: number;
    /** The proposal is written into what is asked (a form option's
     *  `proposed`), not scored: no confidence, so no floor. AI's
     *  never-decides targets still never show. */
    stated?: boolean;
    /** The undo link: "Change", "Not waiting". */
    changeLabel?: string;
    /** The person wants another value: the consumer clears the field. */
    onchange?: () => void;
    testid?: string;
  } = $props();

  const shown = $derived(
    proposal && (stated ? !neverDecides(field) && !!proposal.value : preselect(field, proposal, floor) != null) ? proposal : null,
  );
</script>

{#if shown}
  <div class="why" data-testid={testid} data-source={shown.source}>
    <span class="pill">{proposedByLabel(shown.source)}</span>
    {#if shown.reason}<span class="reason">{shown.reason}</span>{/if}
    {#if shown.confidence_pct != null}<span class="pct" title="Confidence">{shown.confidence_pct}%</span>{/if}
    {#if onchange}
      <span aria-hidden="true">·</span>
      <button type="button" class="link" data-testid="{testid}-change" onclick={() => onchange?.()}
        >{changeLabel}</button
      >
    {/if}
  </div>
{/if}

<style>
  .why {
    font-size: var(--text-xs);
    line-height: 16px;
    color: var(--fg-muted);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    color: var(--accent);
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    white-space: nowrap;
  }
  .pill::before {
    content: '\2726';
    font-size: var(--text-2xs);
  }
  .pct {
    font-variant-numeric: tabular-nums;
  }
  .link {
    font: inherit;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .link:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
</style>
