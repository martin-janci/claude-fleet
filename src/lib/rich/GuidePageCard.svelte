<script lang="ts">
  // A `guide` block with `page` (docs/chat-blocks.md, step 10.4): a guide
  // fleet already has, compiled in or approved, drawn in the chat with the
  // same PageView Settings › Guides uses, so the steps, the live fields and
  // their guards are the same in both places. A field written here is the
  // person's own write, as it is in Settings.
  import { onMount } from 'svelte';
  import PageView from '../pages/PageView.svelte';
  import { openSettingsAt } from '../app_views';
  import { loadFleetSettings } from '../fleet_settings';
  import { hubStatus, ownsTheFleet } from '../hub';
  import { allPages, loadGuides } from '../pages/guides';
  import { descriptors, loadDescriptors, loadPages, pagesBundle, settingValues } from '../pages/pages';
  import { settingsWritable } from '../pages/review';

  let { pageId }: { pageId: string } = $props();

  let loaded = $state(false);
  let finished = $state(false);

  onMount(() => {
    const waits: Promise<unknown>[] = [loadGuides(), loadFleetSettings()];
    if ($descriptors.size === 0) waits.push(loadDescriptors());
    if ($pagesBundle.pages.length === 0) waits.push(loadPages());
    void Promise.all(waits).then(() => (loaded = true));
  });

  const page = $derived($allPages.find((p) => p.id === pageId && p.layout === 'guide') ?? null);
  const ownsFleet = $derived(ownsTheFleet($hubStatus));

  /** A link inside the guide opens Settings there; Done (the guide's own
   *  parent) folds the card. */
  function navigate(id: string) {
    if (page && id === (page.parent ?? 'guides')) finished = true;
    else openSettingsAt(id);
  }
</script>

<section class="card" data-testid="rich-guide-page" data-page={pageId}>
  {#if page && finished}
    <p class="done" data-testid="rich-guide-page-done">
      Done: {page.title}.
      <button type="button" class="link" onclick={() => (finished = false)}>Show it again</button>
    </p>
  {:else if page}
    <div class="open">
      <button type="button" class="link" data-testid="rich-guide-page-open" onclick={() => openSettingsAt(pageId)}>Open in Settings</button>
    </div>
    <PageView
      {page}
      pages={$allPages}
      descs={$descriptors}
      values={$settingValues}
      sources={$pagesBundle.sources}
      resources={$pagesBundle.resources}
      actions={$pagesBundle.actions}
      readonly={!ownsFleet && !$settingsWritable}
      remote={!ownsFleet}
      onnavigate={navigate}
      onopen={(id, key) => openSettingsAt(id, key)} />
  {:else if loaded}
    <p class="muted" data-testid="rich-guide-page-missing">
      This fleet has no guide <code>{pageId}</code>.
      <button type="button" class="link" onclick={() => openSettingsAt('guides')}>See the guides in Settings</button>
    </p>
  {:else}
    <p class="muted">Reading the guide…</p>
  {/if}
</section>

<style>
  .card {
    position: relative;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-pane);
  }
  .open {
    position: absolute;
    top: 0.65rem;
    right: 0.8rem;
    font-size: 0.75rem;
  }
  .done, .muted {
    margin: 0;
    font-size: 0.85rem;
  }
  .muted { color: var(--fg-muted); }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
  }
</style>
