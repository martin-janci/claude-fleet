<script lang="ts">
  // This window's own fleet, as a header over its list — shown only beside
  // another fleet (multi-hub Phase 3a), so the Sessions list reads as one
  // group per fleet, each under its badge: this one first, the extra fleets
  // after (FleetSessionGroups). It folds, so a long list here never buries
  // another fleet's sessions; and when another fleet's Machine or
  // Organisation choice hides this list, it says so with the way back.
  import { fleets, fleetRows, connectionWord } from './fleets';
  import {
    anchor,
    anchorFacetWord,
    anchorText,
    clearMachineOrg,
    fleetScopesOf,
    primaryFleetOpen,
    primaryHiddenByAnchor,
    scopeLabelIn,
  } from './fleet_filters';

  let {
    count,
    hiddenCount,
    focused = false,
  }: {
    /** The rows this list shows now. */
    count: number;
    /** Its sessions that pass the filters every fleet shares, for the
     *  notice while another fleet's filter hides them. */
    hiddenCount: number;
    /** A session focus shows this list past the anchor and the fold. */
    focused?: boolean;
  } = $props();

  const primary = $derived($fleets.find((f) => f.primary) ?? null);
  const hidden = $derived(!focused && primaryHiddenByAnchor($anchor));
  // Whether the list under this header is on screen: a focus shows it past
  // the fold, so the button must not say "collapsed" over a visible list.
  const open = $derived(focused || ($primaryFleetOpen && !hidden));
  const words = $derived.by(() => {
    const a = $anchor;
    if (!a || !hidden) return '';
    const list = fleetScopesOf($fleetRows.get(a.fleet) ?? { sessions: new Map(), projects: [] });
    return anchorText(a, $fleets, (id) => scopeLabelIn(list, id));
  });
</script>

{#if primary}
  {@const cw = connectionWord(primary)}
  <section class="primary-fleet" style="--fleet: {primary.color}" data-testid="primary-fleet-header">
    <div class="head">
      <button
        type="button"
        class="fold"
        aria-expanded={open}
        aria-label="{open ? 'Hide' : 'Show'} the sessions of {primary.label}"
        data-testid="primary-fleet-fold"
        disabled={hidden || focused}
        onclick={() => primaryFleetOpen.update((v) => !v)}
        ><span class="caret" class:collapsed={!open}>▾</span></button
      >
      <span class="badge" data-testid="primary-fleet-badge">{primary.label}</span>
      <span class="state {cw.tone}">{cw.word}</span>
      {#if !hidden}<span class="count" data-testid="primary-fleet-count">{count}</span>{/if}
    </div>
    {#if hidden && $anchor}
      <p class="note" data-testid="primary-fleet-hidden">
        {hiddenCount} {hiddenCount === 1 ? 'session' : 'sessions'} on {primary.label} hidden: the
        {anchorFacetWord($anchor)} ({words}) applies to that fleet only.
        <button type="button" class="link" data-testid="primary-fleet-hidden-clear" onclick={clearMachineOrg}
          >Show every fleet</button
        >
      </p>
    {/if}
  </section>
{/if}

<style>
  .primary-fleet {
    border-left: 3px solid var(--fleet);
    padding: var(--space-1) var(--space-2);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-xs);
  }
  .fold {
    background: none;
    border: 0;
    padding: 0;
    color: var(--fg-muted);
    cursor: pointer;
    font: inherit;
  }
  .fold:disabled {
    cursor: default;
  }
  .caret {
    display: inline-block;
    transition: transform var(--dur-fast);
  }
  .caret.collapsed {
    transform: rotate(-90deg);
  }
  .badge {
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    background: var(--fleet);
    color: var(--accent-fg);
    font-weight: 600;
  }
  .state,
  .count {
    color: var(--fg-muted);
  }
  .state.bad {
    color: var(--status-failed);
  }
  .state.warn {
    color: var(--status-waiting);
  }
  .note {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    margin: var(--space-1) 0 0;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    text-decoration: underline;
  }
</style>
