<script lang="ts">
  // A `guide` block with `page` (docs/chat-blocks.md, step 10.4): a guide
  // fleet already has, compiled in or approved, drawn in the chat with the
  // same PageView Settings › Guides uses, so the steps, the live fields and
  // their guards are the same in both places. A field written here is the
  // person's own write, as it is in Settings.
  //
  // G7.15 (Guide board, "In the chat too"): the card opens as a summary,
  // "guide · <title> · 4 steps · changes 2 settings", with Start (the steps
  // inline) and Open in Settings; nothing is written until Start.
  import { onMount } from 'svelte';
  import PageView from '../pages/PageView.svelte';
  import { openSettingsAt } from '../app_views';
  import { loadFleetSettings } from '../fleet_settings';
  import { hubStatus, ownsTheFleet } from '../hub';
  import { allPages, loadGuides } from '../pages/guides';
  import { guideSummary } from '../pages/guide_changes';
  import { descriptors, loadDescriptors, loadPages, pagesBundle, settingValues } from '../pages/pages';
  import { settingsWritable } from '../pages/review';

  let { pageId }: { pageId: string } = $props();

  let loaded = $state(false);
  let finished = $state(false);
  let started = $state(false);

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
    if (page && id === (page.parent ?? 'guides')) {
      finished = true;
      started = false;
    }
    else openSettingsAt(id);
  }
</script>

<section class="card" data-testid="rich-guide-page" data-page={pageId}>
  {#if page && finished}
    <p class="done" data-testid="rich-guide-page-done">
      Done: {page.title}.
      <button type="button" class="link" onclick={() => (finished = false)}>Show it again</button>
    </p>
  {:else if page && !started}
    <div class="summary" data-testid="rich-guide-page-summary">
      <span class="tag">guide</span>
      <strong class="title">{page.title}</strong>
      <span class="muted" data-testid="rich-guide-page-count">{guideSummary(page, $descriptors)}</span>
      <div class="acts">
        <button type="button" class="btn btn--primary" data-testid="rich-guide-page-start" onclick={() => (started = true)}>Start</button>
        <button type="button" class="link" data-testid="rich-guide-page-open" onclick={() => openSettingsAt(pageId)}>Open in Settings</button>
      </div>
    </div>
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
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .open {
    position: absolute;
    top: 0.65rem;
    right: 0.8rem;
    font-size: var(--text-2xs);
  }
  .summary {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .summary .tag {
    align-self: flex-start;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    text-transform: lowercase;
  }
  .summary .title {
    font-size: var(--text-sm);
  }
  .summary .muted {
    font-size: var(--text-2xs);
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    margin-top: 0.3rem;
    font-size: var(--text-xs);
  }
  .done, .muted {
    margin: 0;
    font-size: var(--text-xs);
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
