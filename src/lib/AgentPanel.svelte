<script lang="ts">
  // Control's Chat tab (redesign step 9.1): a conversation over the operator
  // session, the removable context chip, and nothing else. Everything that renders turns
  // — and everything that sends — is ConversationPanel's job; what lives
  // here is the frame, the chip, and the four states where the agent cannot
  // simply be talked to.
  //
  // Every blocked state gets a next step (redesign step 9.1, `blockedCopy`'s
  // `next`): wake, restart and move run here; Control API opens Settings,
  // add/open host open the Hosts view; replace kills the agent's session
  // after an inline confirm (a kill is always confirmed) and starts a new one.
  //
  // It is drawn inline in the right column. Until step 13.1 it was also
  // Classic's floating sheet, with a grip, maximize and close.
  //
  // The sheet used to own a composer of its own, because the chip's prefix
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
    operatorState,
    operatorHost,
    operatorFallback,
    operatorRow,
    operatorError,
    blockedCopy,
    ensureAgent,
    restartOperator,
    replaceOperator,
    type OperatorBlocked,
  } from './operator';
  import { openSettingsAt, requestHostsView } from './app_views';
  import { agentContext, type AgentContextInput } from './agent_context';
  import { OPERATOR_COMMANDS } from './operator';
  import { insertIntoComposer } from './conversation';
  import ConfirmCards from './ConfirmCards.svelte';
  let { contextInput = null }: { contextInput?: AgentContextInput | null } = $props();

  // Control shows the agent whenever it is open, so it makes sure there is
  // one.
  $effect(() => {
    void ensureAgent();
  });

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
      ? blockedCopy($operatorState as OperatorBlocked, $operatorHost, $operatorFallback)
      : null,
  );
  // Which function a blocked-state button runs, keyed on `blocked.next`
  // rather than the copy string.
  function runNext() {
    if (!blocked) return;
    switch (blocked.next) {
      case 'wake':
      case 'move':
        void ensureAgent();
        return;
      case 'restart':
        void busyWhile(restartOperator);
        return;
      case 'control_api':
        openSettingsAt('control-api');
        return;
      case 'replace':
        confirmingReplace = true;
        return;
      case 'add_host':
        requestHostsView();
        return;
      case 'open_host':
        requestHostsView($operatorHost);
        return;
    }
  }
  // A restart or a replace is an SSH round trip or two (and after a host
  // reboot, a new tmux session); the button says so and takes no second
  // press meanwhile.
  let restarting = $state(false);
  let confirmingReplace = $state(false);
  async function busyWhile(fn: () => Promise<void>) {
    if (restarting) return;
    restarting = true;
    try {
      await fn();
    } finally {
      restarting = false;
    }
  }
  async function replace() {
    confirmingReplace = false;
    await busyWhile(replaceOperator);
  }
  // A confirm left open belongs to the state that asked it.
  $effect(() => {
    if ($operatorState !== 'token_revoked') confirmingReplace = false;
  });

</script>

{#snippet chip()}
  <!-- Step 9.2: the agent's starts and kills wait here as cards, in the
       transcript. Mounted only while this row renders, so
       a request is never parked on a card nobody can see (confirms.ts). -->
    <ConfirmCards />
  {#if ctx}
    <button
      class="chip"
      data-testid="agent-context-chip"
      onclick={() => (droppedLabel = ctx!.chipLabel)}
      title="Send without this context"
    >
      {ctx!.chipLabel} ✕
    </button>
  {/if}
  <!-- Operator commands (work graph M9): they fill the composer, never send. -->
  {#each OPERATOR_COMMANDS as c (c.label)}
    <button
      class="chip command"
      data-testid="agent-command"
      title="Put this request in the box; nothing is sent until you press Enter"
      onclick={() => session && insertIntoComposer(session.id, c.text)}>{c.label}</button
    >
  {/each}
{/snippet}

<div class="agent-panel" role="region" aria-label="Agent" data-testid="control-agent">
  {#if blocked}
    <p class="blocked">{blocked.title}</p>
    {#if confirmingReplace}
      <p class="confirm" data-testid="agent-replace-confirm">
        Kill the agent's session on {$operatorHost} and start a new one? Its conversation so far stays in that session's
        transcript.
      </p>
      <div class="confirm-row">
        <button onclick={() => void replace()} data-testid="agent-replace-yes">Kill and start a new agent</button>
        <button onclick={() => (confirmingReplace = false)}>Cancel</button>
      </div>
    {:else}
      <button onclick={runNext} disabled={restarting} data-testid="agent-next-step"
        >{restarting ? (blocked.next === 'replace' ? 'Replacing…' : 'Restarting…') : blocked.action}</button
      >
    {/if}
    {#if $operatorError}
      <p class="error" role="alert" data-testid="agent-panel-error">{$operatorError}</p>
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
      composerAbove={chip}
    />
  {/if}
</div>

<style>
  /* Fills Control's right column. */
  .agent-panel {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    height: 100%;
    min-height: 0;
    box-sizing: border-box;
    padding: 0.75rem;
    background: var(--bg-pane);
    color: var(--fg);
  }
  .blocked {
    margin: 0;
    color: var(--fg-muted);
  }
  .confirm {
    margin: 0;
  }
  .confirm-row {
    display: flex;
    gap: 0.5rem;
  }
  .error {
    margin: 0;
    color: var(--usage-crit);
    font-size: 0.8rem;
  }
  .chip {
    align-self: flex-start;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg);
    color: var(--fg-muted);
    font-size: 11px;
    padding: 0.15rem 0.6rem;
    cursor: pointer;
  }
  .chip.command {
    border-style: dashed;
  }
  .chip:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
</style>
