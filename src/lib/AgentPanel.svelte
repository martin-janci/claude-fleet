<script lang="ts">
  // The agent's sheet: a compact conversation over the operator session, the
  // removable context chip, and nothing else. Everything that renders turns
  // — and everything that sends — is ConversationPanel's job; what lives
  // here is the frame, the chip, and the four states where the agent cannot
  // simply be talked to.
  //
  // Only two of the four blocked states get a button (`blockedCopy`'s own
  // rule): `absent` -> openAgent() wakes it, `lost` -> restartOperator()
  // brings the session back. `no_mcp` / `token_revoked` are explanatory
  // only — their fixes live outside this panel (Settings, the sidebar), so
  // there is nothing here to wire a click to.
  //
  // This sheet used to own a composer of its own, because the chip's prefix
  // had to be glued onto the prompt and ConversationPanel knew nothing about
  // it. That bought a second sender and cost every live signal the panel
  // had: `pending`, `optimistic` (which is what keeps the transcript on the
  // 5 s cadence instead of the 15 s quiet one) and the refetch-on-send are
  // all set inside ConversationPanel's `send()`. Sending around it meant a
  // prompt that vanished, no working indicator, and a reply that appeared
  // up to fifteen seconds after it existed. The prefix is a prop now, and
  // there is exactly one composer again.
  import ConversationPanel from './ConversationPanel.svelte';
  import {
    agentPanelOpen,
    operatorState,
    operatorHost,
    operatorRow,
    blockedCopy,
    closeAgent,
    openAgent,
    restartOperator,
    type OperatorBlocked,
  } from './operator';
  import { agentContext, type AgentContextInput } from './agent_context';

  let { contextInput = null }: { contextInput?: AgentContextInput | null } = $props();

  // Which context's chip the person dismissed, by label rather than a bare
  // boolean: "not this context" outlives the click, but only for as long as
  // it stays the SAME context. The moment `agentContext(...)` describes
  // something else, the labels no longer match and the chip is back — pure
  // derived state, no effect required to bring it back.
  let droppedLabel = $state<string | null>(null);

  // The LIVE row, not the snapshot `ensure_operator` returned. This sheet
  // owns no composer any more, so what hangs off this row is what the sheet
  // hands DOWN to ConversationPanel's: the row it renders, and through it the
  // `claude_status` / `stuck_kind` that drive both the note under the box and
  // — because this sheet passes `blockWhileBusy` — the gate that refuses a
  // send while the agent is mid-turn. A frozen row would leave both stale.
  // See `operatorRow` in operator.ts.
  const session = $derived($operatorRow);
  const rawCtx = $derived(contextInput ? agentContext(contextInput) : null);
  const ctx = $derived(rawCtx && rawCtx.chipLabel !== droppedLabel ? rawCtx : null);

  const blocked = $derived(
    $operatorState !== 'ready' && $operatorState !== 'waking' && $operatorState !== 'unknown'
      ? blockedCopy($operatorState as OperatorBlocked, $operatorHost)
      : null,
  );
  // Which function a blocked-state button runs, keyed on the actual state
  // rather than matching the copy string — `absent` wakes, `lost` restarts,
  // everything else has no button at all (`blocked.action` is null there).
  const blockedAction = $derived(
    $operatorState === 'absent'
      ? () => void openAgent()
      : $operatorState === 'lost'
        ? () => void restartOperator()
        : null,
  );

  // Escape closes the sheet, INCLUDING from inside the composer. This is a
  // non-modal overlay, so it is not a <dialog> and gets no `cancel` event
  // from the browser (Modal.svelte's route); and App.svelte's window-level
  // Escape deliberately leaves an editable element alone, which would leave
  // the one field you are most likely to be in with no way out. Handled
  // here, where the panel owns the key, and marked handled so the same
  // press does not also leave Files or Hosts behind it.
  function onPanelKeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    e.stopPropagation();
    closeAgent();
  }
</script>

{#snippet chip()}
  <button
    class="chip"
    data-testid="agent-context-chip"
    onclick={() => (droppedLabel = ctx!.chipLabel)}
    title="Send without this context"
  >
    {ctx!.chipLabel} ✕
  </button>
{/snippet}

{#if $agentPanelOpen}
  <!-- A non-modal dialog: `role="dialog"` on a div (a <section> is a
       landmark and may not take the role), `tabindex="-1"` so the sheet
       itself can hold focus and Escape reaches this handler even when no
       control inside it is focused. Not <dialog>/Modal.svelte: showModal()
       would dim and focus-trap the whole app, and the point of this sheet
       is that the app stays usable underneath it. -->
  <div
    class="agent-panel"
    role="dialog"
    tabindex="-1"
    aria-label="Agent"
    data-testid="agent-panel"
    onkeydown={onPanelKeydown}
  >
    <header class="head">
      <span class="who">Agent</span>
      <button
        class="close"
        data-testid="agent-panel-close"
        aria-label="Close the agent"
        title="Close the agent (Esc)"
        onclick={closeAgent}>✕</button
      >
    </header>
    {#if blocked}
      <p class="blocked">{blocked.title}</p>
      {#if blocked.action && blockedAction}
        <button onclick={blockedAction}>{blocked.action}</button>
      {/if}
    {:else if session}
      <!-- `blockWhileBusy` is the gate the sheet's own composer had before it
           was deleted (`busy = statusNote !== null` at v0.2.35) and lost in
           the refactor. It is passed explicitly, and only here: the
           Conversation tab never had it, and queueing a prompt behind a
           running turn is a workflow there, not a mistake. -->
      <ConversationPanel
        {session}
        visible={true}
        promptPrefix={ctx?.prefix ?? null}
        blockWhileBusy={true}
        composerAbove={ctx ? chip : undefined}
      />
    {/if}
  </div>
{/if}

<style>
  .agent-panel {
    position: fixed;
    right: 20px;
    /* Same slot as the toast column: clear of the status bar AND the FAB.
       Reads the tokens rather than restating the sum, so a change to any
       one of them moves this too (the hardcoded 80px was already 9px off). */
    bottom: calc(var(--status-h) + var(--fab-size) + var(--layer-gap) * 2);
    width: 360px;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-pane);
    color: var(--fg);
    box-shadow: 0 4px 20px rgb(0 0 0 / 35%);
    z-index: 39;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .who {
    color: var(--fg-muted);
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .close {
    border: none;
    background: none;
    color: var(--fg-muted);
    font-size: 0.9rem;
    line-height: 1;
    padding: 0.15rem 0.3rem;
    cursor: pointer;
    border-radius: 4px;
  }
  .close:hover {
    color: var(--fg);
    background: var(--bg);
  }
  .blocked {
    margin: 0;
    color: var(--fg-muted);
  }
  .chip {
    align-self: flex-start;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg);
    color: var(--fg-muted);
    font-size: 0.75rem;
    padding: 0.15rem 0.6rem;
    cursor: pointer;
  }
  .chip:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
</style>
