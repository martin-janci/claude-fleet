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
  import { OPERATOR_COMMANDS } from './operator';
  import { insertIntoComposer } from './conversation';
  import {
    agentPanelSize,
    agentPanelMaximized,
    clampAgentPanelSize,
    dragResize,
    AGENT_PANEL_DEFAULT_W,
    type AgentPanelSize,
  } from './agent_panel_size';

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

  // Resizing. The sheet is pinned bottom-right, so its top-left corner is
  // the one that moves: dragging it left / up grows the sheet. The grip
  // also answers the arrow keys (Home returns to the default size), and a
  // double-click resets it, the way a split-pane divider does.
  let panelEl: HTMLDivElement | undefined = $state();
  let drag: { x: number; y: number; start: AgentPanelSize } | null = null;
  const RESIZE_STEP = 20;
  // How far the sheet may grow: up to 20px from the window's top and left
  // edges. Its right and bottom edges do not move.
  function growRoom(): AgentPanelSize | undefined {
    if (!panelEl) return undefined;
    const r = panelEl.getBoundingClientRect();
    return r.right > 0 && r.bottom > 0 ? { w: r.right - 20, h: r.bottom - 20 } : undefined;
  }
  function currentSize(): AgentPanelSize {
    const r = panelEl?.getBoundingClientRect();
    return r && r.width > 0 ? { w: r.width, h: r.height } : { w: AGENT_PANEL_DEFAULT_W, h: 0 };
  }
  function onGripDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    drag = { x: e.clientX, y: e.clientY, start: currentSize() };
    agentPanelMaximized.set(false);
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  }
  function onGripMove(e: PointerEvent) {
    if (!drag) return;
    agentPanelSize.set(dragResize(drag.start, e.clientX - drag.x, e.clientY - drag.y, growRoom()));
  }
  function endGrip() {
    drag = null;
  }
  function resetSize() {
    agentPanelMaximized.set(false);
    agentPanelSize.set(null);
  }
  function onGripKey(e: KeyboardEvent) {
    const d: Record<string, [number, number]> = {
      ArrowLeft: [RESIZE_STEP, 0],
      ArrowRight: [-RESIZE_STEP, 0],
      ArrowUp: [0, RESIZE_STEP],
      ArrowDown: [0, -RESIZE_STEP],
    };
    if (e.key === 'Home') {
      e.preventDefault();
      resetSize();
      return;
    }
    const step = d[e.key];
    if (!step) return;
    e.preventDefault();
    const s = currentSize();
    agentPanelMaximized.set(false);
    agentPanelSize.set(clampAgentPanelSize({ w: s.w + step[0], h: s.h + step[1] }, growRoom()));
  }
</script>

{#snippet chip()}
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

{#if $agentPanelOpen}
  <!-- A non-modal dialog: `role="dialog"` on a div (a <section> is a
       landmark and may not take the role), `tabindex="-1"` so the sheet
       itself can hold focus and Escape reaches this handler even when no
       control inside it is focused. Not <dialog>/Modal.svelte: showModal()
       would dim and focus-trap the whole app, and the point of this sheet
       is that the app stays usable underneath it. -->
  <div
    class="agent-panel"
    class:sized={$agentPanelSize !== null && !$agentPanelMaximized}
    class:maximized={$agentPanelMaximized}
    style:--agent-w={$agentPanelSize ? `${$agentPanelSize.w}px` : undefined}
    style:--agent-h={$agentPanelSize ? `${$agentPanelSize.h}px` : undefined}
    role="dialog"
    tabindex="-1"
    aria-label="Agent"
    data-testid="agent-panel"
    bind:this={panelEl}
    onkeydown={onPanelKeydown}
  >
    <!-- A focusable separator is a widget (it takes the arrow keys), which
         the a11y rules do not model; the same exception Resizer.svelte is. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="grip"
      data-testid="agent-panel-grip"
      role="separator"
      aria-label="Resize the agent"
      aria-orientation="horizontal"
      tabindex="0"
      title="Drag to resize · double-click to reset"
      onpointerdown={onGripDown}
      onpointermove={onGripMove}
      onpointerup={endGrip}
      onpointercancel={endGrip}
      onlostpointercapture={endGrip}
      ondblclick={resetSize}
      onkeydown={onGripKey}
    ></div>
    <header class="head">
      <span class="who">Agent</span>
      <button
        class="close"
        data-testid="agent-panel-maximize"
        aria-label={$agentPanelMaximized ? 'Restore the agent' : 'Maximize the agent'}
        aria-pressed={$agentPanelMaximized}
        title={$agentPanelMaximized ? 'Restore' : 'Maximize'}
        onclick={() => agentPanelMaximized.update((m) => !m)}>{$agentPanelMaximized ? '⤡' : '⤢'}</button
      >
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
        composerAbove={chip}
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
    --agent-bottom: calc(var(--status-h) + var(--fab-size) + var(--layer-gap) * 2);
    bottom: var(--agent-bottom);
    width: min(360px, calc(100vw - 40px));
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
  /* A size the person chose (drag / arrow keys), capped by the window so a
     size saved on a large screen never pushes the sheet off a small one. */
  .agent-panel.sized {
    width: min(var(--agent-w), calc(100vw - 40px));
    height: min(var(--agent-h), calc(100vh - var(--agent-bottom) - 20px));
    max-height: none;
  }
  .agent-panel.maximized {
    width: calc(100vw - 40px);
    height: calc(100vh - var(--agent-bottom) - 20px);
    max-height: none;
  }
  .grip {
    position: absolute;
    top: 0;
    left: 0;
    width: 14px;
    height: 14px;
    cursor: nwse-resize;
    border-top-left-radius: 8px;
    /* Two short diagonal strokes: the corner reads as a handle. */
    background: linear-gradient(
      135deg,
      transparent 0 30%,
      var(--fg-muted) 30% 38%,
      transparent 38% 52%,
      var(--fg-muted) 52% 60%,
      transparent 60%
    );
    opacity: 0.45;
    touch-action: none;
  }
  .grip:hover,
  .grip:focus-visible {
    opacity: 1;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .who {
    margin-right: auto;
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
  .chip.command {
    border-style: dashed;
  }
  .chip:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
</style>
