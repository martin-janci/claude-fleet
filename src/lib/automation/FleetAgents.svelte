<script lang="ts">
  // The Agents tab's fleet agents (configurable agents, step 5): the
  // catalog's `fleet_agent` definitions, each with its instances (the
  // `agent = fleet` rules naming it) under it. New instance and Edit open
  // the definition's grant form, the fleet.form/1 spec the hub builds
  // (`agents_fleet { action: form }`), in the same WizardDialog every other
  // wizard uses; sending it grants (`grant` with the answers), which starts
  // each MCP binding on the host and asks it for its tools first, so a store
  // that is not there is the dialog's error, not the first job's. Revoke
  // ends an instance; Run now queues a manual agent's job.
  import { onMount } from 'svelte';
  import WizardDialog from '../forms/WizardDialog.svelte';
  import StatusDot from '../kit/StatusDot.svelte';
  import type { Values } from '../forms/forms';
  import type { Wizard } from '../forms/wizards';
  import {
    fleetDefinitions,
    fleetForm,
    fleetWizard,
    grantFleet,
    loadAgents,
    revokeAgentRule,
    ruleLine,
    runFleetAgent,
    type AgentRule,
    type FleetDefinition,
  } from '../agents';

  let { rules }: { rules: AgentRule[] } = $props();

  let defs = $state<FleetDefinition[] | null>(null);
  let loadError = $state<string | null>(null);
  let open = $state<{ wizard: Wizard; definition: string } | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);

  async function load() {
    const r = await fleetDefinitions();
    if (r.ok) {
      defs = r.value ?? [];
      loadError = null;
    } else {
      loadError = r.error?.message ?? 'Could not read the catalog';
    }
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

  async function runNow(id: number) {
    const r = await runFleetAgent(id);
    note = r.ok ? 'Queued: it runs on the next pass.' : (r.error?.message ?? 'Run failed');
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
              <button class="act" type="button" data-testid="fleet-instance-run" onclick={() => void runNow(r.id)}>Run now</button>
            {/if}
            <button class="act" type="button" data-testid="fleet-instance-edit" onclick={() => void openForm(d.name, r.id)}>Edit</button>
            <button class="act" type="button" data-testid="fleet-instance-revoke" onclick={() => void revoke(r.id)}>Revoke</button>
          </span>
        </li>
      {/each}
    {/each}
  </ul>
{/if}
{#if note}<p class="none" data-testid="fleet-agents-note">{note}</p>{/if}

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
</style>
