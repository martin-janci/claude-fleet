<!--
  Gap plan G3.9: "Suggested from your fleet" above Control's composer
  (`control_suggestions.ts`). A failing PR opens its session or asks
  Control to finish CI; a mass loss opens its host, where "Restore n lost
  sessions" runs the dry run and the confirm it always has; finished
  sessions open the tidy sheet or ask Control to /tidy. "Ask Control" only
  fills the box. ✕ hides a card for a week; with the escalation scope on
  "Control's threads" only Control's own sessions raise one (w04-14).
-->
<script lang="ts">
  import { sessions } from './sessions';
  import { insertIntoComposer } from './conversation';
  import { openInControl } from './control_room';
  import { requestHostsView } from './app_views';
  import { dismissSuggestion, dismissedSuggestions, fleetSuggestions, type Suggestion } from './control_suggestions';
  import { controlSessionIds, escalationScope } from './control_escalations';
  import { handoffs } from './handoffs';
  import { threadFollows } from './control_journal';
  import { refreshTidy, requestTidy, tidyReport } from './tidy';
  import { onMount } from 'svelte';

  // `count`: Control's activity row (ControlActivity) shows it.
  let { sessionId, count = $bindable(0) }: { sessionId: number; count?: number } = $props();

  onMount(() => void refreshTidy());

  const scope = $derived($escalationScope === 'fleet' ? null : controlSessionIds($sessions, $handoffs, $threadFollows));
  const cards = $derived(fleetSuggestions($sessions, $dismissedSuggestions, { scope, tidy: $tidyReport.candidates }));

  $effect(() => {
    count = cards.length;
  });

  function dismiss(s: Suggestion) {
    dismissSuggestion(s.id);
  }
</script>

{#if cards.length > 0}
  <section class="suggestions" aria-label="Suggested from your fleet" data-testid="control-suggestions">
    <h3 class="head">Suggested from your fleet</h3>
    {#each cards as s (s.id)}
      <div class="card" data-testid="control-suggestion" data-kind={s.kind}>
        <div class="text">
          <span class="title" title={s.title}>{s.title}</span>
          <span class="why" title={s.why}>{s.why}</span>
        </div>
        <div class="acts">
          {#if s.kind === 'ci'}
            <button type="button" class="btn btn--quiet" data-testid="control-suggestion-open" onclick={() => openInControl(s.sessionId, s.label)}
              >Open session</button
            >
          {:else if s.kind === 'tidy'}
            {@const ids = s.sessionIds}
            <button type="button" class="btn btn--quiet" data-testid="control-suggestion-open" onclick={() => requestTidy(ids)}>Review</button>
          {:else}
            <button type="button" class="btn btn--quiet" data-testid="control-suggestion-open" onclick={() => requestHostsView(s.host)}
              >Review on {s.host}</button
            >
          {/if}
          <button
            type="button"
            class="btn btn--quiet"
            data-testid="control-suggestion-ask"
            title="Put this request in the box; nothing is sent until you press Enter"
            onclick={() => insertIntoComposer(sessionId, s.ask)}>Ask Control ↵</button
          >
          <button type="button" class="btn btn--icon btn--quiet" aria-label="Dismiss {s.title}" data-testid="control-suggestion-dismiss" onclick={() => dismiss(s)}
            >✕</button
          >
        </div>
      </div>
    {/each}
  </section>
{/if}

<style>
  .suggestions {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .head {
    margin: 0;
    font-size: var(--text-2xs);
    font-weight: 500;
    color: var(--fg-muted);
  }
  /* One compact row per suggestion (Control chat UX, 2026-10-10): the
     tray above the composer is shared with the task group. */
  .card {
    display: flex;
    flex-wrap: nowrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
  }
  .text {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .title,
  .why {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title {
    color: var(--fg);
    font-size: var(--text-xs);
  }
  .why {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .acts {
    flex-shrink: 0;
    display: flex;
    gap: var(--space-1);
    align-items: center;
  }
</style>
