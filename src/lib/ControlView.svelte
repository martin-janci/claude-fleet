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
  import { controlViews, setViewsOpen } from './control_views';
  import { controlTab, type ControlTab } from './control';
  import { shortcutLabel } from './shortcuts';
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
    <div class="btn-group" role="tablist" aria-label="Control" data-testid="control-tabs" use:tablistKeys>
      {#each TABS as t (t.id)}
        <button
          type="button"
          class="btn btn--chip btn--toggle"
          role="tab"
          aria-selected={$controlTab === t.id}
          class:is-active={$controlTab === t.id}
          data-testid="control-tab-{t.id}"
          title="{t.label} ({shortcutLabel(t.chord, isMac)})"
          onclick={() => controlTab.set(t.id)}>{t.label}</button
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
  </header>
  <div class="split" class:with-views={$controlViews.open}>
    <div class="body" role="tabpanel">
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
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .title {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .views-open {
    margin-left: auto;
    flex: none;
  }
  .split {
    flex: 1 1 auto;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }
  .split.with-views {
    grid-template-columns: minmax(0, 1fr) minmax(260px, 340px);
  }
  .body {
    min-height: 0;
    overflow: auto;
  }
</style>
