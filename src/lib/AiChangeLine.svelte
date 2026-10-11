<!--
  G7.15 (AI patterns board, "When AI changed something"): the one line every
  AI-caused change shows once it is made, "✓ Linked to PD-2592 · Proposed by
  Jev · you confirmed · Undo", so a change AI proposed is one move to take
  back wherever it happened. The text is `aiChangeLine`; the Undo is the
  consumer's, and absent when the change has no exact inverse.
-->
<script lang="ts">
  import { aiChangeLine, type ProposalSource } from './ai_proposal';

  let {
    what,
    source,
    confirmed = true,
    onundo,
    undoing = false,
    undoBlocked = null,
    testid = 'ai-change',
  }: {
    /** What changed: "Linked to PD-2592", "Placed in Release 0.5". */
    what: string;
    source: ProposalSource;
    /** A person confirmed the proposal (the usual case: AI never decides). */
    confirmed?: boolean;
    onundo?: () => void;
    undoing?: boolean;
    /** Why Undo cannot run here (a hub client without the hub). */
    undoBlocked?: string | null;
    testid?: string;
  } = $props();
</script>

<div class="ai-change" data-testid={testid} data-source={source}>
  <span class="text">✓ {aiChangeLine(what, source, confirmed)}</span>
  {#if onundo}
    <span aria-hidden="true">·</span>
    <button
      type="button"
      class="link"
      data-testid="{testid}-undo"
      disabled={undoing || undoBlocked !== null}
      title={undoBlocked ?? ''}
      onclick={() => onundo?.()}>{undoing ? 'Undoing…' : 'Undo'}</button
    >
  {/if}
</div>

<style>
  .ai-change {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .link:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
</style>
