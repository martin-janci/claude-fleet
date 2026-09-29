<script lang="ts">
  // Settings → General → Work: what is left of the hand-written Work panel
  // since declarative pages P4. Trackers and organisations are generated
  // pages (Settings → Trackers, Settings → Organisations: `settings.trackers`
  // and `settings.orgs` in crates/fleet-core/pages/); this keeps the
  // introduction and the work graph's usage counts (M13.2), which only the
  // process that owns the fleet can read.
  import WorkUsage from './WorkUsage.svelte';
  import { hubStatus, ownsTheFleet } from './hub';

  const owns = $derived(ownsTheFleet($hubStatus));
</script>

<section class="block" data-testid="work-section">
  <div class="section-header"><h4>Work</h4></div>
  <p class="blurb">
    Trackers add a ticket's title and status to the sessions working on it, and list your tickets
    in ⌘K. Nothing needs one: keys in branch names group sessions without any tracker. Trackers and
    organisations have their own pages in the list on the left.
  </p>
  {#if owns}<WorkUsage />{/if}
</section>

<style>
  .blurb {
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
</style>
