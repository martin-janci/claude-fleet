<script lang="ts">
  // The agent's sheet: a compact conversation over the operator session, a
  // composer, and the removable context chip. Everything that renders turns
  // is ConversationPanel's job; what lives here is the frame, the chip, and
  // the three states where the agent cannot simply be talked to.
  //
  // Only two of the four blocked states get a button (`blockedCopy`'s own
  // rule): `absent` -> openAgent() wakes it, `lost` -> restartOperator()
  // brings the session back. `no_mcp` / `token_revoked` are explanatory
  // only — their fixes live outside this panel (Settings, the sidebar), so
  // there is nothing here to wire a click to.
  //
  // ConversationPanel carries its own composer, but it does not know about
  // the context chip's prefix — so it is mounted with `showComposer={false}`
  // and this panel is the only thing that ever calls `sendPrompt` for the
  // operator session. Two composers sending independently into one tmux
  // REPL is exactly the interleaved-paste failure `showComposer` exists to
  // prevent.
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
  import { sendPrompt } from './sessions';
  import { composerStatus } from './conversation';

  let { contextInput = null }: { contextInput?: AgentContextInput | null } = $props();

  let draft = $state('');
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  // Which context's chip the person dismissed, by label rather than a bare
  // boolean: "not this context" outlives the click, but only for as long as
  // it stays the SAME context. The moment `agentContext(...)` describes
  // something else, the labels no longer match and the chip is back — pure
  // derived state, no effect required to bring it back.
  let droppedLabel = $state<string | null>(null);

  // The LIVE row, not the snapshot `ensure_operator` returned: `busy`, the
  // `stuck_kind` line and ConversationPanel's own `$effect` on rowStatus all
  // hang off this, and all three are worthless if it cannot change. See
  // `operatorRow` in operator.ts.
  const session = $derived($operatorRow);
  // Same "is the agent busy" signal ConversationPanel's own composer reads
  // (stuck_kind first, then claude_status === 'working') — one shared
  // definition so the two composers can never disagree about it.
  const statusNote = $derived(
    session ? composerStatus({ claude_status: session.claude_status, stuck_kind: session.stuck_kind }) : null,
  );
  const busy = $derived(statusNote !== null);
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

  async function send() {
    if (!session || busy || sending || !draft.trim()) return;
    const s = session;
    const id = s.id;
    sending = true;
    sendError = null;
    const body = ctx ? `${ctx.prefix}\n\n${draft}` : draft;
    const r = await sendPrompt(s.host_alias, s.tmux_name, body);
    sending = false;
    // The operator session moved on while the send was on the wire (or the
    // panel was reopened onto a different one) — the prompt landed wherever
    // it landed, but none of this component's state belongs to it any more.
    if (session?.id !== id) return;
    if (!r.ok) {
      sendError = r.error.message;
      return;
    }
    draft = '';
  }

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
    {:else}
      {#if session}
        <ConversationPanel {session} visible={true} showComposer={false} />
      {/if}
      {#if ctx}
        <button
          class="chip"
          data-testid="agent-context-chip"
          onclick={() => (droppedLabel = ctx.chipLabel)}
          title="Send without this context"
        >
          {ctx.chipLabel} ✕
        </button>
      {/if}
      <div class="composer">
        <textarea bind:value={draft} placeholder="Ask the agent…" rows="2"></textarea>
        <button onclick={() => void send()} disabled={busy || sending || !session}>Send</button>
      </div>
      {#if sendError}
        <p class="error" data-testid="agent-composer-error">{sendError}</p>
      {/if}
      {#if statusNote}
        <p class="busy" class:stuck={!!session?.stuck_kind}>{statusNote}</p>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .agent-panel {
    position: fixed;
    right: 20px;
    bottom: 80px;
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
  .busy {
    margin: 0;
    color: var(--fg-muted);
    font-size: 0.8rem;
  }
  .busy.stuck {
    color: var(--usage-crit);
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
    font-size: 0.75rem;
    padding: 0.15rem 0.6rem;
    cursor: pointer;
  }
  .chip:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
  .composer {
    display: flex;
    gap: 0.5rem;
    align-items: flex-end;
  }
  .composer textarea {
    flex: 1;
    resize: vertical;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 4px;
    font: inherit;
    padding: 0.4rem;
  }
</style>
