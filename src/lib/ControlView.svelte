<!--
  Control (Orbit Fleet redesign step 9.1, the MissionControl board): the New
  layout's rail item for the fleet agent. Chat is today's operator panel in
  the right column instead of a floating sheet; Today moved here from the
  Inbox and keeps ⌘⇧T. The Views panel beside the chat (step 9.4) shows Needs
  you, the session in focus, Pull requests and Today without leaving Control.
-->
<script lang="ts">
  import { tablistKeys } from './tablist_keys';
  import AgentPanel from './AgentPanel.svelte';
  import TodayView from './TodayView.svelte';
  import ControlViewsPanel from './ControlViewsPanel.svelte';
  import { controlViews, selectView, setViewsOpen } from './control_views';
  import { controlTab, type ControlTab } from './control';
  import { shortcutLabel } from './shortcuts';
  import { openSettingsAt } from './app_views';
  import type { AgentContextInput } from './agent_context';

  let { isMac, contextInput = null }: { isMac: boolean; contextInput?: AgentContextInput | null } = $props();

  const TABS: readonly { id: ControlTab; label: string; chord: string }[] = [
    { id: 'chat', label: 'Chat', chord: 'agent' },
    { id: 'today', label: 'Today', chord: 'today' },
  ];
</script>

<section class="control" aria-label="Control" data-testid="control-view">
  <header class="head">
    <h2 class="title">Control</h2>
    <!-- Underlined tabs, Today with its chord (UX audit 2026-10-09, C3). -->
    <div class="of of-tabs tabs" role="tablist" aria-label="Control" data-testid="control-tabs" use:tablistKeys>
      {#each TABS as t (t.id)}
        <button
          type="button"
          class="of-tab"
          role="tab"
          aria-selected={$controlTab === t.id}
          data-testid="control-tab-{t.id}"
          title="{t.label} ({shortcutLabel(t.chord, isMac)})"
          onclick={() => controlTab.set(t.id)}
          >{t.label}{#if t.id === 'today'}{' '}<span class="of-kbd" aria-hidden="true"
              >{shortcutLabel(t.chord, isMac)}</span
            >{/if}</button
        >
      {/each}
    </div>
    <span class="hint">talk to the fleet · it hands work to sessions and missions</span>
    {#if !$controlViews.open}
      <button
        type="button"
        class="btn btn--chip views-open"
        data-testid="control-views-open"
        title="Show the views beside the chat"
        onclick={() => setViewsOpen(true)}>Views</button
      >
    {/if}
    <!-- UX audit C3 (board MissionControl): Overview opens the fleet's
         summary, the Today briefing view beside the chat. -->
    <button
      type="button"
      class="btn btn--quiet overview"
      data-testid="control-overview"
      title="The fleet at a glance: the Today briefing beside the chat"
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
  </header>
  <div class="split" class:with-views={$controlViews.open}>
    <div class="body" class:today={$controlTab === 'today'} role="tabpanel">
      {#if $controlTab === 'today'}
        <TodayView />
      {:else}
        <AgentPanel {contextInput} />
      {/if}
    </div>
    {#if $controlViews.open}
      <ControlViewsPanel />
    {/if}
  </div>
</section>

<style>
  .control {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg-pane);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: 0 var(--space-4);
    min-height: 44px;
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .title {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: 600;
  }
  /* The strip sits on the header's own border. */
  .tabs { align-self: stretch; border-bottom: 0; flex: none; }
  .tabs .of-tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: none;
    border-top: 0;
    border-left: 0;
    border-right: 0;
    font: inherit;
    cursor: pointer;
  }
  .body.today { padding: var(--space-4) var(--space-6); }
  .settings { flex: none; }
  .settings svg { fill: none; stroke: currentColor; stroke-width: 1.3; }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .views-open {
    flex: none;
  }
  .hint { flex: 1 1 auto; min-width: 0; }
  .split {
    flex: 1 1 auto;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }
  .split.with-views {
    grid-template-columns: minmax(0, 1fr) minmax(var(--inspector-min), var(--list-w));
  }
  .body {
    min-height: 0;
    overflow: auto;
  }
</style>
