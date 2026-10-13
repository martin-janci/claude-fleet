<script lang="ts">
  // Settings → Work & trackers (General → Work before step 7.1): what is left of the hand-written Work panel
  // since declarative pages P4. Trackers and organisations are generated
  // pages (Settings → Trackers, Settings → Organisations: `settings.trackers`
  // and `settings.orgs` in crates/fleet-core/pages/); this keeps the
  // introduction and a link to the work graph's usage counts (M13.2), now the
  // generated data page `usage.work`, which only the process that owns the
  // fleet can read.
  import { hubStatus, ownsTheFleet } from './hub';
  import WorkRules from './WorkRules.svelte';
  import SegmentedControl from './SegmentedControl.svelte';
  import { workRowClick, type WorkRowClick } from './prefs';

  // What a click on a Work row opens: a per-device pref in localStorage,
  // like Appearance's, not a fleet setting.
  const rowClicks = [
    { id: 'ticket', label: 'Ticket' },
    { id: 'session', label: 'Session' },
  ] as const satisfies readonly { id: WorkRowClick; label: string }[];

  let { onopen }: { onopen?: (pageId: string) => void } = $props();

  const owns = $derived(ownsTheFleet($hubStatus));
  // The placement rules the Work view's ⚙ opens, reachable from Settings too
  // (parity P4). The editor reads the rules itself and refuses what the hub
  // does not let this device change.
  let rulesOpen = $state(false);
</script>

<section class="block" data-testid="work-section">
  <div class="section-header"><h4>Work</h4></div>
  <p class="blurb">
    Trackers add a ticket's title and status to the sessions working on it, and list your tickets
    in ⌘K. Nothing needs one: keys in branch names group sessions without any tracker. Trackers and
    organisations have their own pages in the list on the left.
  </p>
  <div class="pref">
    <div class="pref-text">
      <span class="lbl">Clicking a work row opens</span>
      <p class="hook-desc">
        Ticket shows the task's detail. Session opens the session working on it, and the ticket when
        none is. The session chip on a row opens the session either way.
      </p>
    </div>
    <SegmentedControl
      options={rowClicks}
      value={$workRowClick}
      label="Clicking a work row opens"
      testidPrefix="work-row-click-"
      onchange={(id) => workRowClick.set(id)} />
  </div>
  <button type="button" class="link" data-testid="work-rules-open" onclick={() => (rulesOpen = true)}
    >Placement rules…</button
  >
  {#if owns && onopen}
    <button type="button" class="link" data-testid="work-usage-link" onclick={() => onopen('usage.work')}
      >Work graph usage →</button
    >
  {/if}
</section>

{#if rulesOpen}
  <WorkRules onclose={() => (rulesOpen = false)} />
{/if}

<style>
  .pref {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border);
    margin-bottom: var(--space-3);
  }
  .pref-text { flex: 1; min-width: 0; }
  .lbl { font-size: var(--text-sm); font-weight: 500; }
  .hook-desc {
    margin: 2px 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .pref :global(.seg-group) { flex: 0 0 auto; min-width: 180px; }
  .blurb {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .link {
    display: block;
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    font-size: var(--text-2xs);
    cursor: pointer;
  }
  .link:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
</style>
