<script lang="ts">
  // Settings → General → Work: what is left of the hand-written Work panel
  // since declarative pages P4. Trackers and organisations are generated
  // pages (Settings → Trackers, Settings → Organisations: `settings.trackers`
  // and `settings.orgs` in crates/fleet-core/pages/); this keeps the
  // introduction and a link to the work graph's usage counts (M13.2), now the
  // generated data page `usage.work`, which only the process that owns the
  // fleet can read.
  import { hubStatus, ownsTheFleet } from './hub';

  let { onopen }: { onopen?: (pageId: string) => void } = $props();

  const owns = $derived(ownsTheFleet($hubStatus));
</script>

<section class="block" data-testid="work-section">
  <div class="section-header"><h4>Work</h4></div>
  <p class="blurb">
    Trackers add a ticket's title and status to the sessions working on it, and list your tickets
    in ⌘K. Nothing needs one: keys in branch names group sessions without any tracker. Trackers and
    organisations have their own pages in the list on the left.
  </p>
  {#if owns && onopen}
    <button type="button" class="link" data-testid="work-usage-link" onclick={() => onopen('usage.work')}
      >Work graph usage →</button
    >
  {/if}
</section>

<style>
  .blurb {
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .link:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
</style>
