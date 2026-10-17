<!--
  Thread room escalations (control_escalations.ts): what in the fleet needs
  the person, as amber cards above Control's composer. Open shows the
  session beside the chat, where its question can be answered; Later puts
  the card off until that session needs you again. More than a few: the
  rest are counted, and the count opens the Needs you tab.
-->
<script lang="ts">
  import { ESCALATIONS_SHOWN, escalations, laterEscalations, putOff } from './control_escalations';
  import { visibleFleet, selectView } from './control_views';
  import { operatorSession } from './operator';
  import { openInControl } from './control_room';
  import { displayName } from './attention';
  import { hosts } from './hosts';

  const all = $derived(escalations($visibleFleet.rows, $visibleFleet.opts, $operatorSession?.id ?? null, $laterEscalations, $hosts));
  const shown = $derived(all.slice(0, ESCALATIONS_SHOWN));
  const more = $derived(all.length - shown.length);
</script>

{#if shown.length > 0}
  <ul class="escalations" data-testid="control-escalations" aria-label="Needs you" aria-live="polite">
    {#each shown as e (e.key)}
      <li class="card" data-testid="control-escalation" data-bucket={e.bucket}>
        <span class="mark" aria-hidden="true">!</span>
        <div class="text">
          <span class="line">{e.line}</span>
          {#if e.detail}<span class="detail" data-testid="control-escalation-detail">{e.detail}</span>{/if}
        </div>
        {#if e.session}
          {@const s = e.session}
          <button
            type="button"
            class="btn btn--chip"
            data-testid="control-escalation-open"
            onclick={() => openInControl(s.id, displayName(s, true))}>Open</button
          >
        {/if}
        <button type="button" class="btn btn--quiet" data-testid="control-escalation-later" onclick={() => putOff(e.key)}>Later</button>
      </li>
    {/each}
    {#if more > 0}
      <li>
        <button type="button" class="more" data-testid="control-escalations-more" onclick={() => selectView('needs-you')}
          >{more} more in Needs you</button
        >
      </li>
    {/if}
  </ul>
{/if}

<style>
  .escalations {
    flex-basis: 100%;
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .card {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0.4rem var(--space-3);
    border: 1px solid var(--border);
    border-left: 3px solid var(--usage-warn);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--usage-warn) 8%, var(--bg-pane));
    font-size: var(--text-xs);
  }
  .mark {
    color: var(--usage-warn);
    font-weight: 700;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .line {
    color: var(--fg);
  }
  .detail {
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .more {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: var(--text-xs);
  }
</style>
