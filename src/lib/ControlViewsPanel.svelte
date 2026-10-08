<!--
  Control's Views panel (Orbit Fleet redesign step 9.4, boards MissionControl
  and MCViews): the column beside the chat. A strip of views (Needs you, the
  session in focus, Pull requests, Library once 9.7 lands, Today briefing),
  "+" to turn them on and off and reorder them, ⤢ to open the view where it
  lives, ✕ to close the column. Tasks, Missions, Hosts and usage are links to
  where they live, not copies.
-->
<script lang="ts">
  import { selectSessionExplicitly, selectedSession } from './selection';
  import { sidebarView } from './work_view';
  import { leave } from './destination';
  import { controlTab } from './control';
  import { claudeStatusLabel, displayName, promptPreview } from './attention';
  import { timeAgo } from './session_status';
  import type { SessionRow } from './sessions';
  import WorkPrs from './WorkPrs.svelte';
  import TodayView from './TodayView.svelte';
  import {
    CONTROL_VIEWS,
    ELSEWHERE,
    controlViews,
    moveView,
    needsYouList,
    openElsewhere,
    selectView,
    setViewsOpen,
    shownViews,
    toggleView,
    type ControlViewId,
  } from './control_views';

  const shown = $derived(shownViews($controlViews));
  const active = $derived($controlViews.active);
  const activeDef = $derived(CONTROL_VIEWS.find((v) => v.id === active)!);
  let menuOpen = $state(false);

  const landed = CONTROL_VIEWS.filter((v) => v.landed);

  /** ⤢: the view's own place. */
  function expand(id: ControlViewId) {
    if (id === 'needs-you') {
      sidebarView.set('inbox');
      leave('control');
    } else if (id === 'session' || id === 'prs') {
      if (id === 'prs') sidebarView.set('work');
      leave('control');
    } else if (id === 'today') {
      controlTab.set('today');
    }
  }

  /** A Needs you row puts that session in focus, here in the panel. */
  function openRow(s: SessionRow) {
    selectSessionExplicitly(s);
    selectView('session');
  }
</script>

<aside class="views" aria-label="Views" data-testid="control-views">
  <div class="strip">
    <div class="tabs" role="tablist" aria-label="Views">
      {#each shown as v (v.id)}
        <button
          type="button"
          role="tab"
          class="tab"
          aria-selected={v.id === active}
          title={v.label}
          data-testid="control-view-tab-{v.id}"
          onclick={() => selectView(v.id)}
        >
          <span aria-hidden="true">{v.glyph}</span>
          {#if v.id === active}<span class="name">{v.label}</span>{:else}<span class="sr">{v.label}</span>{/if}
          {#if v.id === 'needs-you' && $needsYouList.length > 0}<span class="count">{$needsYouList.length}</span>{/if}
        </button>
      {/each}
    </div>
    <span class="grow"></span>
    <button
      type="button"
      class="icon"
      title="Views"
      aria-label="Choose views"
      aria-expanded={menuOpen}
      data-testid="control-views-add"
      onclick={() => (menuOpen = !menuOpen)}>+</button
    >
    <button
      type="button"
      class="icon"
      title="Open {activeDef.label} where it lives"
      aria-label="Open {activeDef.label} where it lives"
      data-testid="control-views-expand"
      onclick={() => expand(active)}>⤢</button
    >
    <button
      type="button"
      class="icon"
      title="Close the views"
      aria-label="Close the views"
      data-testid="control-views-close"
      onclick={() => setViewsOpen(false)}>✕</button
    >
  </div>

  {#if menuOpen}
    <div class="menu" data-testid="control-views-menu">
      <p class="menu-head">From this chat</p>
      <ul>
        {#each $controlViews.order.map((id) => landed.find((v) => v.id === id)).filter((v) => !!v) as v, i (v.id)}
          <li>
            <label>
              <input
                type="checkbox"
                checked={!$controlViews.hidden.includes(v.id)}
                disabled={!$controlViews.hidden.includes(v.id) && shown.length <= 1}
                data-testid="control-views-toggle-{v.id}"
                onchange={() => toggleView(v.id)}
              />
              {v.label}
            </label>
            <span class="moves">
              <button type="button" class="icon" aria-label="Move {v.label} up" disabled={i === 0} onclick={() => moveView(v.id, -1)}>↑</button>
              <button
                type="button"
                class="icon"
                aria-label="Move {v.label} down"
                disabled={i === $controlViews.order.length - 1}
                onclick={() => moveView(v.id, 1)}>↓</button
              >
            </span>
          </li>
        {/each}
      </ul>
      <p class="menu-head">Open elsewhere</p>
      <ul>
        {#each ELSEWHERE as l (l.id)}
          <li>
            <button type="button" class="link" data-testid="control-views-elsewhere-{l.id}" onclick={() => openElsewhere(l.id)}
              >{l.label} ↗</button
            >
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  <div class="body" role="tabpanel" aria-label={activeDef.label} data-testid="control-view-{active}">
    {#if active === 'needs-you'}
      {#if $needsYouList.length === 0}
        <p class="empty">Nothing needs you.</p>
      {:else}
        <ul class="rows">
          {#each $needsYouList as s (s.id)}
            <li>
              <button type="button" class="row" data-testid="control-needs-you-row" onclick={() => openRow(s)}>
                <span class="row-name">{displayName(s, true)}</span>
                <span class="row-why">{claudeStatusLabel(s.claude_status)}</span>
                {#if s.last_activity_at}<span class="row-age">{timeAgo(s.last_activity_at)}</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if active === 'session'}
      {#if $selectedSession}
        <div class="focus" data-testid="control-session-focus">
          <p class="row-name">{displayName($selectedSession, true)}</p>
          <p class="row-why">{claudeStatusLabel($selectedSession.claude_status)} · {$selectedSession.host_alias}</p>
          {#if $selectedSession.last_prompt}<p class="prompt">{promptPreview($selectedSession.last_prompt, 160)}</p>{/if}
        </div>
      {:else}
        <p class="empty">No session in focus. Pick one in Sessions or the Inbox.</p>
      {/if}
    {:else if active === 'prs'}
      <WorkPrs />
    {:else if active === 'today'}
      <TodayView />
    {/if}
  </div>
</aside>

<style>
  .views {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    border-left: 1px solid var(--border);
    background: var(--bg-pane);
    position: relative;
  }
  .strip {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    gap: 2px;
    min-width: 0;
  }
  .tab,
  .icon {
    border: 0;
    background: transparent;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-sm);
    padding: 4px 6px;
    border-radius: var(--radius-md);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .tab:hover,
  .icon:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  .tab[aria-selected='true'] {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .tab:focus-visible,
  .icon:focus-visible,
  .row:focus-visible,
  .link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .icon:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .count {
    font-size: var(--text-xs);
    color: var(--status-waiting);
  }
  .grow {
    flex: 1 1 auto;
  }
  .menu {
    position: absolute;
    top: 36px;
    right: var(--space-2);
    z-index: 2;
    min-width: 240px;
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.25);
    font-size: var(--text-sm);
  }
  .menu-head {
    margin: var(--space-1) 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .menu ul,
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .menu li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .link {
    border: 0;
    background: transparent;
    color: var(--fg);
    font: inherit;
    padding: 2px 0;
    cursor: pointer;
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 2px var(--space-2);
    width: 100%;
    text-align: left;
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-sm);
    padding: var(--space-2) var(--space-3);
    cursor: pointer;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row-name {
    margin: 0;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-why,
  .row-age,
  .prompt,
  .empty {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .row-why {
    grid-column: 1;
  }
  .row-age {
    grid-column: 2;
    grid-row: 1;
  }
  .focus {
    padding: var(--space-3);
  }
  .focus p {
    margin: 0 0 var(--space-1);
  }
  .empty {
    padding: var(--space-3);
    margin: 0;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
