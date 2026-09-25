<script lang="ts">
  // A session's work key as a chip (work graph M1), with its tracker item's
  // status when a tracker knows it (M3): a status-category dot, the status
  // name and "synced 4 min ago" in the tooltip, struck through grey when the
  // tracker no longer answers for it, and a small clock when the tracker's
  // last sync is older than twice the interval. A ticket-shaped key no
  // connected tracker owns says where to connect one.
  //
  // Work graph M4: solid is a confirmed link; a small ring marks one that
  // detection made by itself (auto); `suggested` renders a dashed chip with
  // `?` — a guess nobody has decided. The tooltip always says why.
  import { describeWorkKey, type WorkKey } from './work_keys';
  import {
    trackers,
    trackerForKey,
    trackerStale,
    syncedAgo,
    statusDotClass,
    displayKey,
    providerInfo,
    showProviderBadges,
  } from './trackers';
  import { fleetSettings, settingInt, SETTING_KEYS } from './fleet_settings';

  let {
    workKey,
    testid = 'work-chip',
    suggested = false,
    onclick,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    workKey: WorkKey;
    testid?: string;
    /** A detected suggestion, not a link (dashed, `?`). */
    suggested?: boolean;
    /** Click handler (opens the row's work popover). */
    onclick?: (e: MouseEvent) => void;
    /** Unix seconds; injectable for tests. */
    now?: () => number;
  } = $props();

  const tracker = $derived(trackerForKey(workKey.key, $trackers));
  const interval = $derived(settingInt($fleetSettings, SETTING_KEYS.workSyncIntervalSecs));
  const stale = $derived(
    !!workKey.status && !!tracker && trackerStale(tracker, now(), interval),
  );
  const ticketShaped = $derived(/^[A-Z][A-Z0-9_]{1,9}-\d{1,7}$/.test(workKey.key));
  const unbound = $derived(!suggested && !workKey.status && ticketShaped && !tracker);
  // Work graph M6: which tracker, once there is more than one kind.
  const prov = $derived(
    tracker && showProviderBadges($trackers) ? providerInfo(tracker.provider) : null,
  );
  const title = $derived.by(() => {
    let t = describeWorkKey(workKey);
    if (workKey.status) t += ` · ${syncedAgo(tracker, now())}`;
    if (stale) t += ' (stale: the tracker has not synced lately)';
    if (prov) t = `${prov.label} · ${t}`;
    if (unbound) t += ` · connect its tracker in Settings → Work to see ${workKey.key}'s status`;
    if (suggested) t += ' · suggestion: Confirm (y) or Not this (n)';
    return t;
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<span
  class="work-chip"
  class:unavailable={workKey.status?.unavailable}
  class:unbound
  class:suggested
  class:clickable={!!onclick}
  data-testid={testid}
  data-state={suggested ? 'suggested' : workKey.auto ? 'auto' : 'confirmed'}
  {title}
  {onclick}
>
  {#if workKey.status && !workKey.status.unavailable}
    <span
      class="dot {statusDotClass(workKey.status.category)}"
      data-testid="{testid}-dot"
      aria-label={workKey.status.name ?? workKey.status.category}
    ></span>
  {/if}
  {#if prov}<span class="prov" data-testid="{testid}-provider" aria-label={prov.label}>{prov.icon}</span>{/if}
  {displayKey(workKey.key)}{#if suggested}<span class="q" aria-label="suggested">?</span>{/if}
  {#if workKey.auto && !suggested}<span class="auto" data-testid="{testid}-auto" aria-label="linked automatically"></span>{/if}
  {#if stale}<span class="stale" data-testid="{testid}-stale" aria-label="stale">◷</span>{/if}
</span>

<style>
  .work-chip {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.65rem;
    font-family: var(--font-mono, ui-monospace, monospace);
    padding: 0 0.3rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .work-chip.unavailable {
    text-decoration: line-through;
    opacity: 0.6;
  }
  .work-chip.unbound {
    border-style: dashed;
  }
  .work-chip.suggested {
    border-style: dashed;
    opacity: 0.85;
  }
  .work-chip.clickable {
    cursor: pointer;
  }
  .q {
    margin-left: -0.15rem;
    font-weight: 600;
  }
  .auto {
    width: 0.3rem;
    height: 0.3rem;
    border-radius: 50%;
    border: 1px solid var(--fg-muted);
    display: inline-block;
  }
  .dot {
    width: 0.45rem;
    height: 0.45rem;
    border-radius: 50%;
    display: inline-block;
    background: var(--fg-muted);
  }
  .dot-todo {
    background: var(--fg-muted);
  }
  .dot-progress {
    background: var(--accent, #3b82f6);
  }
  .dot-done {
    background: var(--ok, #22c55e);
  }
  .stale {
    font-size: 0.6rem;
    opacity: 0.8;
  }
  .prov {
    font-size: 0.58rem;
    font-weight: 600;
    opacity: 0.7;
    margin-right: 0.15rem;
  }
</style>
