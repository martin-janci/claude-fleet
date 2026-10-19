<script lang="ts">
  // The Agents tab's fleet agents (configurable agents, step 5): the
  // catalog's `fleet_agent` definitions, each with its instances (the
  // `agent = fleet` rules naming it) under it. New instance and Edit open
  // the definition's grant form, the fleet.form/1 spec the hub builds
  // (`agents_fleet { action: form }`), in the same WizardDialog every other
  // wizard uses; sending it grants (`grant` with the answers), which starts
  // each MCP binding on the host and asks it for its tools first, so a store
  // that is not there is the dialog's error, not the first job's. Revoke
  // ends an instance; Run now opens a small panel under the instance: what
  // to look into (the request) and where (the scope: a window, a host, a
  // session, a part of the fleet), and Run queues the manual agent's job.
  // The gallery lists the definitions claude-fleet ships that the catalog
  // does not have yet; Add to catalog copies one in, listed above from then.
  import { onMount, tick } from 'svelte';
  import WizardDialog from '../forms/WizardDialog.svelte';
  import StatusDot from '../kit/StatusDot.svelte';
  import type { Values } from '../forms/forms';
  import type { Wizard } from '../forms/wizards';
  import {
    fleetDefinitions,
    fleetForm,
    fleetGallery,
    fleetWizard,
    grantFleet,
    installFleet,
    loadAgents,
    revokeAgentRule,
    ruleLine,
    runFleetAgent,
    SCOPE_COMPONENTS,
    SCOPE_WINDOWS,
    type AgentRule,
    type FleetDefinition,
    type GalleryEntry,
    type RunScope,
  } from '../agents';

  let { rules }: { rules: AgentRule[] } = $props();

  let defs = $state<FleetDefinition[] | null>(null);
  let gallery = $state<GalleryEntry[]>([]);
  let loadError = $state<string | null>(null);
  let open = $state<{ wizard: Wizard; definition: string } | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);
  /** The instance whose Run panel is open, and what it holds. */
  let running = $state<{ id: number; request: string; scope: RunScope } | null>(null);
  let queuing = $state(false);
  /** The gallery entry being added (one at a time: a second click would be refused as taken). */
  let installing = $state<string | null>(null);
  let noteEl = $state<HTMLElement | null>(null);

  async function load() {
    const [r, g] = await Promise.all([fleetDefinitions(), fleetGallery()]);
    if (r.ok) {
      defs = r.value ?? [];
      loadError = null;
    } else {
      loadError = r.error?.message ?? 'Could not read the catalog';
    }
    gallery = g.ok && Array.isArray(g.value) ? g.value.filter((e) => !e.installed) : [];
  }

  async function install(name: string) {
    if (installing) return;
    installing = name;
    note = null;
    try {
      const r = await installFleet(name);
      note = r.ok ? `${name} added to the catalog: start an instance of it above.` : (r.error?.message ?? 'Could not add it');
      await load();
    } finally {
      installing = null;
    }
    // The installed row (and the button that had focus) leaves the gallery:
    // focus moves to the result instead of dropping to the page.
    await tick();
    noteEl?.focus();
  }

  onMount(() => void load());

  const instancesOf = (name: string) => rules.filter((r) => r.agent === 'fleet' && r.definition === name);

  async function openForm(definition: string, id?: number) {
    error = null;
    note = null;
    const r = await fleetForm(definition, id);
    if (r.ok) open = { wizard: fleetWizard(r.value), definition };
    else note = r.error?.message ?? 'Could not build the form';
  }

  async function run(values: Values) {
    if (!open) return;
    busy = true;
    error = null;
    const r = await grantFleet(open.definition, values);
    busy = false;
    if (!r.ok) {
      error = r.error?.message ?? 'The grant failed';
      return;
    }
    open = null;
    void loadAgents();
  }

  async function revoke(id: number) {
    const r = await revokeAgentRule(id);
    note = r.ok ? `Instance ${id} revoked.` : (r.error?.message ?? 'Revoke failed');
    void loadAgents();
  }

  function openRun(id: number) {
    running = running?.id === id ? null : { id, request: '', scope: {} };
  }

  async function runNow() {
    if (!running) return;
    queuing = true;
    const r = await runFleetAgent(running.id, running.request, running.scope);
    queuing = false;
    note = r.ok ? 'Queued: it runs on the next pass.' : (r.error?.message ?? 'Run failed');
    if (r.ok) running = null;
    void loadAgents();
  }
</script>

<h3 class="sub">Fleet agents</h3>
{#if loadError}
  <p class="none" data-testid="fleet-agents-error">{loadError}</p>
{:else if defs && defs.length === 0}
  <p class="none" data-testid="fleet-agents-none">
    The catalog has no fleet agent. Add one in Assets (Fleet agents), or check one with <code>fleet-hub agents define DIR</code>.
  </p>
{:else if defs}
  <ul class="table agents" data-testid="fleet-agents">
    {#each defs as d (d.name)}
      <li data-testid="fleet-agent">
        <StatusDot state={d.problems.length ? 'idle' : 'done'} label={null} />
        <span class="main">
          <strong>{d.name}</strong>
          <span class="summary">{d.description}</span>
          {#if d.problems.length}
            <span class="summary bad" data-testid="fleet-agent-problems">Not valid: {d.problems.join('; ')}</span>
          {:else}
            <span class="summary">{d.trigger} · {d.writes}</span>
          {/if}
        </span>
        <button
          class="act"
          type="button"
          disabled={d.problems.length > 0}
          data-testid="fleet-agent-new"
          onclick={() => void openForm(d.name)}>New instance</button
        >
      </li>
      {#each instancesOf(d.name) as r (r.id)}
        <li class="instance" data-testid="fleet-instance">
          <StatusDot state={r.active ? 'done' : 'idle'} label={null} />
          <span class="main"><span class="summary">{ruleLine(r)}</span></span>
          <span class="buttons">
            {#if d.trigger === 'manual' || d.trigger === 'schedule'}
              <button
                class="act"
                type="button"
                aria-expanded={running?.id === r.id}
                data-testid="fleet-instance-run"
                onclick={() => openRun(r.id)}>Run now</button
              >
            {/if}
            <button class="act" type="button" data-testid="fleet-instance-edit" onclick={() => void openForm(d.name, r.id)}>Edit</button>
            <button class="act" type="button" data-testid="fleet-instance-revoke" onclick={() => void revoke(r.id)}>Revoke</button>
          </span>
        </li>
        {#if running?.id === r.id}
          <li class="instance run" data-testid="fleet-run-panel">
            <label class="wide">
              <span>What to look into</span>
              <textarea
                rows="2"
                maxlength="4000"
                placeholder="Why did Control get stuck at 14:00?"
                data-testid="fleet-run-request"
                bind:value={running.request}
              ></textarea>
            </label>
            <label>
              <span>Window</span>
              <select data-testid="fleet-run-window" bind:value={running.scope.minutes}>
                <option value={undefined}>Any</option>
                {#each SCOPE_WINDOWS as w (w.minutes)}<option value={w.minutes}>{w.label}</option>{/each}
              </select>
            </label>
            <label>
              <span>Part</span>
              <select data-testid="fleet-run-component" bind:value={running.scope.component}>
                <option value={undefined}>Whole fleet</option>
                {#each SCOPE_COMPONENTS as c (c)}<option value={c}>{c}</option>{/each}
              </select>
            </label>
            <label>
              <span>Host</span>
              <input type="text" maxlength="100" placeholder="any" data-testid="fleet-run-host" bind:value={running.scope.host} />
            </label>
            <label>
              <span>Session</span>
              <input type="text" maxlength="100" placeholder="any" data-testid="fleet-run-session" bind:value={running.scope.session} />
            </label>
            <span class="buttons">
              <button class="act primary" type="button" disabled={queuing} data-testid="fleet-run-go" onclick={() => void runNow()}>Run</button>
              <button class="act" type="button" data-testid="fleet-run-cancel" onclick={() => (running = null)}>Cancel</button>
            </span>
          </li>
        {/if}
      {/each}
    {/each}
  </ul>
{/if}
{#if gallery.length}
  <h3 class="sub">Gallery</h3>
  <ul class="table agents" data-testid="fleet-gallery">
    {#each gallery as g (g.name)}
      <li data-testid="fleet-gallery-entry">
        <span class="main">
          <strong>{g.name}</strong>
          <span class="summary">{g.description}</span>
          <span class="summary">{g.trigger} · {g.writes}</span>
        </span>
        <button
          class="act"
          type="button"
          disabled={installing !== null}
          aria-busy={installing === g.name}
          aria-label={`Add ${g.name} to catalog`}
          data-testid="fleet-gallery-install"
          onclick={() => void install(g.name)}>Add to catalog</button
        >
      </li>
    {/each}
  </ul>
{/if}
{#if note}<p class="none" role="status" tabindex="-1" bind:this={noteEl} data-testid="fleet-agents-note">{note}</p>{/if}

{#if open}
  <WizardDialog
    wizard={open.wizard}
    {busy}
    {error}
    errorTestid="fleet-agent-error"
    {run}
    onclose={() => (open = null)}
  />
{/if}

<style>
  .instance { padding-left: 24px; }
  .bad { color: var(--danger); }
  .buttons { display: flex; gap: 6px; }
  .act {
    font: inherit;
    padding: 2px 8px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: transparent;
    cursor: pointer;
  }
  .act:disabled { opacity: 0.5; cursor: default; }
  .act.primary { border-color: var(--accent); color: var(--accent); }
  .run { display: flex; flex-wrap: wrap; gap: 8px 12px; align-items: flex-end; }
  .run label { display: flex; flex-direction: column; gap: 2px; font-size: var(--text-sm); }
  .run label.wide { flex-basis: 100%; }
  .run textarea, .run input, .run select {
    font: inherit;
    padding: 4px 6px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: transparent;
    color: inherit;
  }
  .run textarea { width: 100%; resize: vertical; }
</style>
