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
    operatorSession,
    blockedCopy,
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

  const session = $derived($operatorSession);
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
      ? blockedCopy($operatorState as OperatorBlocked)
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
</script>

{#if $agentPanelOpen}
  <section class="agent-panel" aria-label="Agent">
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
  </section>
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
