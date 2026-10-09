<script lang="ts">
  // A session's work key as a chip (work graph M1), with its tracker item's
  // status when a tracker knows it (M3): a status-category dot, the status
  // name and "synced 4 min ago" in the tooltip, struck through grey when the
  // tracker no longer answers for it, and a small clock when the tracker's
  // last sync is older than twice the interval. A ticket-shaped key no
  // connected tracker owns says where to connect one.
  //
  // A ticket-shaped key with no connected tracker also says where to connect
  // one — and when the chip already draws a status dot of its own (local
  // work whose key happens to look like a ticket), the hint says whose
  // status the dot is rather than claiming there is none.
  //
  // Work graph M4: solid is a confirmed link; a small ring marks one that
  // detection made by itself (auto); `suggested` renders a dashed chip with
  // `?` — a guess nobody has decided. The tooltip always says why.
  //
  // Redesign 6.8: a suggestion the decision model made (J1, rule R12) is
  // `proposed`: the `?` becomes the AI mark ✦ in the accent, and the
  // tooltip leads with "Proposed by Jev".
  import { describeWorkKey, type WorkKey } from './work_keys';
  import { proposedByLabel } from './ai_proposal';
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
    proposed = false,
    onclick,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    workKey: WorkKey;
    testid?: string;
    /** A detected suggestion, not a link (dashed, `?`). */
    suggested?: boolean;
    /** The suggestion is the decision model's (J1, rule R12): ✦, not `?`. */
    proposed?: boolean;
    /** Click handler (opens the row's work popover). */
    onclick?: (e: MouseEvent) => void;
    /** Unix seconds; injectable for tests. */
    now?: () => number;
  } = $props();

  const tracker = $derived(trackerForKey(workKey.key, $trackers));
  const interval = $derived(settingInt($fleetSettings, SETTING_KEYS.workSyncIntervalSecs));
  // `trackerBacked`, not `status` (native item status, fix round 3): a
  // local item now has a `status` too (its own live status), so "has a
  // status" no longer means "a tracker owns it" — `trackerBacked` is the
  // field that still does.
  const stale = $derived(
    !!workKey.trackerBacked && !!tracker && trackerStale(tracker, now(), interval),
  );
  const ticketShaped = $derived(/^[A-Z][A-Z0-9_]{1,9}-\d{1,7}$/.test(workKey.key));
  /** The chip is drawing a status dot: exactly the condition the dot itself
   *  renders under, so the tooltip and the dot can never disagree. */
  const ownStatus = $derived(!!workKey.status && !workKey.status.unavailable);
  const unbound = $derived(!suggested && !workKey.trackerBacked && ticketShaped && !tracker);
  // Work graph M6: which tracker, once there is more than one kind.
  const prov = $derived(
    tracker && showProviderBadges($trackers) ? providerInfo(tracker.provider) : null,
  );
  const title = $derived.by(() => {
    let t = describeWorkKey(workKey);
    if (workKey.trackerBacked) t += ` · ${syncedAgo(tracker, now())}`;
    if (stale) t += ' (stale: the tracker has not synced lately)';
    if (prov) t = `${prov.label} · ${t}`;
    // A local item with a coincidentally ticket-shaped key is `unbound` AND
    // has a status of its own — the dot. Saying "connect its tracker to see
    // its status" next to a status dot claims two contradictory things about
    // the same chip (final review, item 6), so when the chip already shows
    // one, the hint says whose status the dot is and what a tracker would
    // add instead of pretending there is none.
    if (unbound) {
      t += ownStatus
        ? ` · the dot is fleet's own status; connect its tracker in Settings → Trackers to see ${workKey.key}'s too`
        : ` · connect its tracker in Settings → Trackers to see ${workKey.key}'s status`;
    }
    if (suggested && proposed) t = `${proposedByLabel('jev')} · ${t}`;
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
  data-proposed={suggested && proposed ? 'jev' : undefined}
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
  {displayKey(workKey.key)}{#if suggested && proposed}<span class="ai" data-testid="{testid}-proposed" aria-label={proposedByLabel('jev')}>&#x2726;</span>{:else if suggested}<span class="q" aria-label="suggested">?</span>{/if}
  {#if workKey.auto && !suggested}<span class="auto" data-testid="{testid}-auto" aria-label="linked automatically"></span>{/if}
  {#if stale}<span class="stale" data-testid="{testid}-stale" aria-label="stale">◷</span>{/if}
</span>

<style>
  .work-chip {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    padding: 0 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
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
  .ai {
    margin-left: -0.1rem;
    color: var(--accent);
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
    background: var(--accent);
  }
  .dot-done {
    background: var(--status-done);
  }
  .stale {
    font-size: var(--text-2xs);
    opacity: 0.8;
  }
  .prov {
    font-size: var(--text-2xs);
    font-weight: 600;
    opacity: 0.7;
    margin-right: 0.15rem;
  }
</style>
