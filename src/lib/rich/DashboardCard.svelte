<script lang="ts">
  // A `dashboard` block (docs/chat-blocks.md, persona design step 5): a
  // dashboard the agent proposes, drawn live from `work_stats` under the
  // person's access, as the Dashboards page draws it. The block names only
  // filters; every number here is the app's. Pin keeps it on the hub for
  // the person (step 6), so every device of theirs shows it; a block with the id of a pinned dashboard is offered as its
  // replacement, with what it changes. Nothing is pinned without a click.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { errorText } from '../error_copy';
  import Button from '../kit/Button.svelte';
  import DashboardBody from '../DashboardBody.svelte';
  import { goTo } from '../destination';
  import { workspace } from '../workspace';
  import {
    dashboardChanges,
    followPinned,
    MAX_PINNED,
    pinDashboard,
    pinnedDashboards,
    pinStateOf,
    shownDashboard,
    type Dashboard,
  } from '../dashboards';
  import type { UiBlock } from '../rich_blocks';

  let { block }: { block: Extract<UiBlock, { kind: 'dashboard' }> } = $props();

  const dash = $derived<Dashboard>({ id: block.id, title: block.title, intro: block.intro, widgets: block.widgets });
  const pinState = $derived(pinStateOf(dash, $pinnedDashboards));
  const before = $derived($pinnedDashboards.find((p) => p.id === block.id) ?? null);
  const changes = $derived(before ? dashboardChanges(before, dash) : []);

  let body = $state<ReturnType<typeof DashboardBody> | null>(null);

  let busy = $state(false);
  let failure = $state<string | null>(null);

  // Whether it is pinned already decides the button: follow the pins, so a
  // pin made on another device turns it into Pinned or Replace at once.
  onMount(() => followPinned());

  async function pin() {
    busy = true;
    failure = null;
    const r = await pinDashboard(dash, block.why);
    busy = false;
    if (r.ok) open();
    else failure = errorText(r.error);
  }

  function open() {
    shownDashboard.set({ id: block.id, workspace: get(workspace) });
    goTo('dashboard');
  }
</script>

<section class="card" data-testid="rich-dashboard" aria-label="Proposed dashboard: {block.title}">
  <header>
    <span class="tag">Dashboard · proposed by the agent</span>
    <button type="button" class="link" onclick={() => void body?.load()}>Refresh</button>
  </header>
  <strong class="title">{block.title}</strong>
  {#if block.intro}<p class="intro">{block.intro}</p>{/if}
  {#if block.why}<p class="why">{block.why}</p>{/if}

  <DashboardBody bind:this={body} {dash} compact live={false} />

  <p class="prov">Counted by fleet from your Work view (work stats), only what you can open. The agent chose what to show, not the numbers.</p>

  <div class="actions" data-testid="rich-dashboard-state" data-state={pinState}>
    {#if pinState === 'new'}
      <Button size="sm" variant="primary" onclick={pin} disabled={busy} testid="rich-dashboard-pin">Pin to Dashboards</Button>
    {:else if pinState === 'replace'}
      <span class="note">
        Replaces your pinned “{before?.title}”{changes.length ? `: ${changes.join(', ')}` : ''}.
      </span>
      <Button size="sm" variant="primary" onclick={pin} disabled={busy} testid="rich-dashboard-pin">Replace pinned</Button>
    {:else if pinState === 'same'}
      <span class="note">Pinned.</span>
      <Button size="sm" onclick={open} testid="rich-dashboard-open">Open</Button>
    {:else if pinState === 'full'}
      <span class="note">{MAX_PINNED} dashboards are pinned. Unpin one on the Dashboards page first.</span>
    {:else}
      <span class="note">Its id is a built-in dashboard's, so it cannot be pinned. Ask the agent for an id of its own.</span>
    {/if}
  </div>
  {#if failure}<p class="err" role="alert" data-testid="rich-dashboard-error">Not pinned: {failure}</p>{/if}
</section>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.5rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .tag {
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .link {
    font: inherit;
    color: var(--fg-muted);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }
  .link:hover { color: var(--fg); text-decoration: underline; }
  .title { font-size: var(--text-sm); }
  .intro, .why, .prov, .note { margin: 0; font-size: var(--text-xs); color: var(--fg-2); }
  .why { color: var(--fg-muted); }
  .prov { color: var(--fg-muted); }
  .actions { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .note { flex: 1 1 12rem; }
  .err { margin: 0; font-size: var(--text-xs); color: var(--status-failed); }
</style>
