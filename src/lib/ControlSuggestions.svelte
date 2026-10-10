<!--
  Gap plan G3.9: "Suggested from your fleet" above Control's composer
  (`control_suggestions.ts`). A failing PR opens its session or asks
  Control to finish CI; a mass loss opens its host, where "Restore n lost
  sessions" runs the dry run and the confirm it always has. "Ask Control"
  only fills the box. ✕ hides a card for this run of the app.
-->
<script lang="ts">
  import { sessions } from './sessions';
  import { insertIntoComposer } from './conversation';
  import { focusSession } from './session_focus';
  import { requestHostsView } from './app_views';
  import { dismissedSuggestions, fleetSuggestions, type Suggestion } from './control_suggestions';

  let { sessionId }: { sessionId: number } = $props();

  const cards = $derived(fleetSuggestions($sessions, $dismissedSuggestions));

  function dismiss(s: Suggestion) {
    dismissedSuggestions.update((d) => new Set([...d, s.id]));
  }
</script>

{#if cards.length > 0}
  <section class="suggestions" aria-label="Suggested from your fleet" data-testid="control-suggestions">
    <h3 class="head">Suggested from your fleet</h3>
    {#each cards as s (s.id)}
      <div class="card" data-testid="control-suggestion" data-kind={s.kind}>
        <div class="text">
          <span class="title">{s.title}</span>
          <span class="why">{s.why}</span>
        </div>
        <div class="acts">
          {#if s.kind === 'ci'}
            <button type="button" class="btn btn--quiet" data-testid="control-suggestion-open" onclick={() => focusSession(s.sessionId, s.label)}
              >Open session</button
            >
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
  .card {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .title {
    color: var(--fg);
    font-size: var(--text-sm);
  }
  .why {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .acts {
    display: flex;
    gap: var(--space-1);
    align-items: center;
  }
</style>
