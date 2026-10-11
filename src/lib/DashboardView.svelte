<!--
  Dashboards (persona design 2026-10-11, steps 4 and 5): the active
  workspace's built-in dashboard, or one the person pinned from an agent's
  proposal, its widgets drawn from `work_stats` (DashboardBody.svelte).
  Every number is the app's own count of the Work view's tasks under the
  person's access; the footer says where they come from and when.

  "Change" (the page's, or a widget's) hands the dashboard to the agent:
  it lands in Control's composer, the person says what to change, and the
  agent answers with a new block of the same id to pin over this one.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Button from './kit/Button.svelte';
  import DashboardBody from './DashboardBody.svelte';
  import { workspace, WORKSPACES, workspacePreset } from './workspace';
  import { askAgent } from './operator';
  import { pushError } from './toasts';
  import {
    BUILTIN_DASHBOARDS,
    changePrompt,
    pinnedDashboards,
    shownDashboard,
    loadPinned,
    unpinDashboard,
    type Dashboard,
    type PinnedDashboard,
  } from './dashboards';

  // The workspace's own dashboard, until the person picks another: a pick
  // holds while the workspace it was made in is the active one.
  const builtin = $derived(BUILTIN_DASHBOARDS[$workspace]);
  const pick_ = $derived($shownDashboard?.workspace === $workspace ? $shownDashboard.id : null);
  const pinned = $derived<PinnedDashboard | null>($pinnedDashboards.find((p) => p.id === pick_) ?? null);
  const pickedBuiltin = $derived(Object.values(BUILTIN_DASHBOARDS).find((d) => d.id === pick_) ?? null);
  const dash = $derived<Dashboard>(pinned ?? pickedBuiltin ?? builtin);
  const owner = $derived(WORKSPACES.find((w) => BUILTIN_DASHBOARDS[w.id].id === dash.id)?.id ?? $workspace);

  let body = $state<ReturnType<typeof DashboardBody> | null>(null);
  let asOf = $state<number | null>(null);
  let loading = $state(true);

  function pick(id: string) {
    shownDashboard.set({ id, workspace: $workspace });
  }

  function change(widget?: string) {
    void askAgent(changePrompt(dash, widget));
  }

  // Pinned on another device since this window last read them: re-read on
  // every visit.
  onMount(() => void loadPinned());

  async function unpin() {
    if (!pinned) return;
    const r = await unpinDashboard(pinned.id);
    if (r.ok) shownDashboard.set(null);
    else pushError(r.error, 'Unpin the dashboard');
  }

  function clock(unix: number | null): string {
    if (unix == null) return '—';
    return new Date(unix * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }

  function day(unix: number): string {
    return new Date(unix * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  }
</script>

<div class="dashboard" data-testid="dashboard-view">
  <header class="head">
    <div class="titles">
      <div class="kicker">
        {#if pinned}Pinned · proposed by the agent{:else}{workspacePreset(owner).name} · dashboard{/if}
      </div>
      <h1>{dash.title}</h1>
      {#if dash.intro}<p class="intro">{dash.intro}</p>{/if}
    </div>
    <div class="tools">
      <label class="pick">
        <span class="sr">Dashboard</span>
        <select bind:value={() => dash.id, pick} data-testid="dashboard-pick">
          <optgroup label="Built in">
            {#each WORKSPACES as w (w.id)}
              <option value={BUILTIN_DASHBOARDS[w.id].id}>{BUILTIN_DASHBOARDS[w.id].title} · {w.short}</option>
            {/each}
          </optgroup>
          {#if $pinnedDashboards.length}
            <optgroup label="Pinned">
              {#each $pinnedDashboards as p (p.id)}
                <option value={p.id}>{p.title}</option>
              {/each}
            </optgroup>
          {/if}
        </select>
      </label>
      <Button size="sm" onclick={() => change()} testid="dashboard-change">Change with the agent</Button>
      {#if pinned}
        <Button size="sm" onclick={unpin} testid="dashboard-unpin">Unpin</Button>
      {/if}
      <Button size="sm" onclick={() => void body?.load()}>Refresh</Button>
    </div>
  </header>

  {#key dash}
    <DashboardBody bind:this={body} {dash} bind:asOf bind:loading onchange={change} />
  {/key}

  <footer class="prov">
    {#if pinned}Pinned on {day(pinned.pinned_at)} from the agent's proposal{:else}Built-in dashboard{/if} · counted by
    fleet from your Work view (work stats), only what you can open · as of {clock(asOf)}{#if loading} · reading…{/if}
  </footer>
</div>

<style>
  .dashboard { display: flex; flex-direction: column; height: 100%; min-height: 0; overflow: auto; background: var(--bg); }
  .head { display: flex; align-items: flex-end; gap: var(--space-3); flex-wrap: wrap; padding: var(--space-4) var(--space-6) var(--space-2); }
  .titles { flex: 1 1 320px; min-width: 0; }
  .kicker { font-size: var(--text-xs); color: var(--fg-muted); }
  h1 { margin: 2px 0 0; font-size: var(--text-xl); line-height: var(--text-xl-lh); font-weight: 600; }
  .intro { margin: 4px 0 0; font-size: var(--text-sm); color: var(--fg-2); }
  .tools { display: flex; gap: var(--space-2); align-items: center; flex-wrap: wrap; }
  .pick select { font: inherit; font-size: var(--text-sm); height: 28px; border: 1px solid var(--control-border); border-radius: var(--radius-sm); background: var(--bg-pane); color: var(--fg); padding: 0 6px; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .prov { margin-top: auto; padding: var(--space-2) var(--space-6) var(--space-3); border-top: 1px solid var(--border); font-size: var(--text-xs); color: var(--fg-muted); }
</style>
