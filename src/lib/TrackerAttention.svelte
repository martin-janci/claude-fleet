<script lang="ts">
  // Work graph M12.4 / decision D22: a failing tracker (an expired token, a
  // refused credential) raises ONE Attention item per tracker in the
  // attention strip — "Reconnect Jira (acme)" — which opens Settings → Trackers.
  // One failing because it keeps skipping items it cannot store (M13.1)
  // reads "Sync skipping items — Jira (acme)" instead.
  // A degraded tracker (rate limited, briefly unreachable) does not: the
  // sync retries it by itself, and the footer's health line says so.
  //
  // The roll-up is `health_check`'s (the hub's `fleet_health` when paired):
  // cached sync state, never a live call to a tracker. App seeds it at
  // startup; this re-reads it on a slow interval.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { openSettingsAt } from './app_views';
  import { hubStatus } from './hub';
  import { refreshTrackersHealth, trackerAttentionItems, trackersHealth } from './tracker_health';
  import { decideAttentionItem, decideHealth } from './decide_health';
  import { orgBudgetItems, orgBudgets } from './org_budget';
  import { windowHidden } from './window_hidden';

  /** Tracker health moves on the sync's scale (minutes). */
  const REFRESH_MS = 60_000;

  const items = $derived(trackerAttentionItems($trackersHealth));
  // The Jev envelope's calls failing (test map §7) rides the same strip and
  // the same read: one item, to Settings → Decisions (Jev).
  const jev = $derived(decideAttentionItem($decideHealth));
  // An org over its budget (org administration phase C): one item each, to
  // Settings → Organisations.
  const budgets = $derived(orgBudgetItems($orgBudgets));

  onMount(() => {
    const t = setInterval(() => {
      if (get(hubStatus).unavailable || windowHidden()) return;
      void refreshTrackersHealth();
    }, REFRESH_MS);
    return () => clearInterval(t);
  });
</script>

{#if items.length > 0 || jev || budgets.length > 0}
  <div class="trackers" data-testid="tracker-attention">
    {#each budgets as b (b.key)}
      <button class="pill hot" data-testid="org-budget-item" title={b.detail} onclick={() => openSettingsAt(b.page)}>
        ⚠ {b.label} →
      </button>
    {/each}
    {#if jev}
      <button
        class="pill hot"
        data-testid="decide-attention-item"
        title={jev.detail}
        onclick={() => openSettingsAt(jev.section)}
      >
        ⚠ {jev.label} →
      </button>
    {/if}
    {#each items as it (it.key)}
      <button
        class="pill hot"
        data-testid="tracker-attention-item"
        data-tracker-id={it.tracker_id}
        title={it.detail}
        onclick={() => openSettingsAt(it.section)}
      >
        ⚠ {it.label} →
      </button>
    {/each}
  </div>
{/if}

<style>
  .trackers {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
  }
  .pill {
    font-size: var(--text-2xs);
    padding: 0.1rem 0.45rem;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .pill.hot {
    border-color: var(--usage-crit);
    color: var(--usage-crit);
  }
</style>
