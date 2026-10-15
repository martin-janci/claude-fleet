<!--
  Control (Orbit Fleet redesign step 9.1, the MissionControl board), as a
  Thread room (direction A, chosen 2026-10-14; parity map
  control-chat-first/2026-10-14/control-parity.md):

  - left, Control's own column (ControlThreads): Main chat and the threads
    Control handed out, grouped Needs you / Working / Done today, with
    filter chips. It takes the sessions list's place only while Control is
    open, folds to an edge button, and the rail's Sessions (or its own
    "All sessions") brings the list back;
  - centre, the chat: the same AgentPanel over the same ConversationPanel
    (one sender), drawn as a chat in a 720 px column;
  - right, the views panel (ControlViewsPanel): tabs Needs you · Session ·
    Pull requests and "+" for the rest, resizable 320–640 px, its width
    remembered; any session the chat names opens in its Session tab.

  ⌘E opens Control and ⌘⇧T puts Today on the right (control.ts). The old
  header's Chat and Today tabs are gone: the chat is the page, Today is a
  view. Overview and the Control settings stay in the chat header.
-->
<script lang="ts">
  import AgentPanel from './AgentPanel.svelte';
  import ControlViewsPanel from './ControlViewsPanel.svelte';
  import ControlThreads from './ControlThreads.svelte';
  import Resizer from './Resizer.svelte';
  import { controlViews, selectView, setViewsOpen } from './control_views';
  import { controlLeftCollapsed, controlLeftPx, controlRightPx, dragLeft, dragRight } from './control_room';
  import { shortcutLabel } from './shortcuts';
  import { openSettingsAt } from './app_views';
  import { operatorHost } from './operator';
  import type { AgentContextInput } from './agent_context';

  let { isMac, contextInput = null }: { isMac: boolean; contextInput?: AgentContextInput | null } = $props();

  // The right handle's running width during one drag, unclamped, so a
  // drag past the minimum can hide the panel (control_room.dragRight).
  let rawRight = $state<number | null>(null);
  function onResizeRight(delta: number) {
    rawRight = dragRight(rawRight ?? $controlRightPx, delta);
  }
  function endRight() {
    rawRight = null;
  }

  const leftCol = $derived($controlLeftCollapsed ? '0px' : `${$controlLeftPx}px`);
  const rightCol = $derived($controlViews.open ? `${$controlRightPx}px` : '0px');
  const grid = $derived(
    `${leftCol} ${$controlLeftCollapsed ? '0px' : '4px'} minmax(0, 1fr) ${$controlViews.open ? '4px' : '0px'} ${rightCol}`,
  );
</script>

<section class="control" aria-label="Control" data-testid="control-view" style:grid-template-columns={grid}>
  {#if $controlLeftCollapsed}
    <div></div>
    <div></div>
  {:else}
    <ControlThreads oncollapse={() => controlLeftCollapsed.set(true)} />
    <Resizer id="control-left" onresize={dragLeft} />
  {/if}
  <div class="centre">
    <header class="head">
      {#if $controlLeftCollapsed}
        <button
          type="button"
          class="btn btn--quiet btn--icon edge"
          data-testid="control-threads-expand"
          title="Show Control's threads"
          aria-label="Show Control's threads"
          onclick={() => controlLeftCollapsed.set(false)}>›</button
        >
      {/if}
      <h2 class="title" data-testid="control-chat-title">Main chat</h2>
      <span class="sub" title="The agent runs as a session on {$operatorHost}">Control on {$operatorHost}</span>
      <span class="grow"></span>
      <!-- UX audit C3 (board MissionControl): Overview opens the fleet's
           summary, the Today briefing view on the right. -->
      <button
        type="button"
        class="btn btn--quiet overview"
        data-testid="control-overview"
        title="The fleet at a glance: the Today briefing on the right ({shortcutLabel('today', isMac)})"
        aria-pressed={$controlViews.open && $controlViews.active === 'today'}
        onclick={() => selectView('today')}>Overview</button
      >
      <button
        type="button"
        class="btn btn--quiet btn--icon settings"
        title="Control settings: the Control API and the agent"
        aria-label="Control settings"
        data-testid="control-settings"
        onclick={() => openSettingsAt('control-api')}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"
          ><circle cx="8" cy="8" r="2.2" /><path
            d="M8 1.8v1.6M8 12.6v1.6M1.8 8h1.6M12.6 8h1.6M3.6 3.6l1.1 1.1M11.3 11.3l1.1 1.1M3.6 12.4l1.1-1.1M11.3 4.7l1.1-1.1"
          /></svg
        >
      </button>
      {#if !$controlViews.open}
        <!-- The panel's way back: it hid with its own edge button or a drag
             to zero. -->
        <button
          type="button"
          class="btn btn--quiet btn--icon edge"
          data-testid="control-views-open"
          title="Show the panel: Needs you, Session, Pull requests"
          aria-label="Show the panel"
          onclick={() => setViewsOpen(true)}>‹</button
        >
      {/if}
    </header>
    <div class="body">
      <AgentPanel {contextInput} />
    </div>
  </div>
  {#if $controlViews.open}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="right-handle" onpointerup={endRight} onpointercancel={endRight}>
      <Resizer id="control-right" onresize={onResizeRight} />
    </div>
    <ControlViewsPanel />
  {:else}
    <div></div>
    <div></div>
  {/if}
</section>

<style>
  .control {
    display: grid;
    height: 100%;
    min-height: 0;
    background: var(--bg);
  }
  .control > :global(*) {
    min-width: 0;
    min-height: 0;
  }
  .centre {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    background: var(--bg);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-4);
    min-height: var(--header-h);
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .title {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: 600;
    white-space: nowrap;
  }
  .sub {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .grow {
    flex: 1 1 auto;
  }
  .settings,
  .overview,
  .edge {
    flex: none;
  }
  .settings svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.3;
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .right-handle {
    display: flex;
  }
  .right-handle > :global(.resizer) {
    flex: 1;
  }
</style>
