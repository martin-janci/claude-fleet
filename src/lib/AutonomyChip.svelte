<!--
  The autonomy chip in Control's composer (Thread room, board Autonomy):
  Ask first · Within your brief · Autopilot. A position sets the mission
  level, auto-continue (and its wait) and Control's event feed together
  (autonomy.ts); "Detailed settings" opens Settings → Control API, where each
  stays on its own. Today's routine spend against the daily budget shows
  when a budget is set, and Pause all stops every loop that acts on its own.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { fleetSettings } from './fleet_settings';
  import { openSettingsAt } from './app_views';
  import { routineBudget, type FleetBudget } from './routines';
  import { automationPaused, money, setAutomationPaused } from './automation';
  import { AUTONOMY, autonomyLabel, autonomyOf, setAutonomy, type AutonomyId } from './autonomy';
  import Meter from './kit/Meter.svelte';

  const current = $derived(autonomyOf($fleetSettings));
  let open = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let budget = $state<FleetBudget | null>(null);
  let root: HTMLElement | undefined = $state();

  async function loadBudget() {
    const r = await routineBudget();
    budget = r.ok && r.value ? r.value : null;
  }
  onMount(() => void loadBudget());

  async function pick(id: AutonomyId) {
    if (busy) return;
    busy = true;
    error = await setAutonomy(id, $fleetSettings);
    busy = false;
    if (!error) open = false;
  }

  function toggle() {
    open = !open;
    if (open) {
      error = null;
      void loadBudget();
    }
  }

  function onDocDown(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
  function onKey(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      open = false;
    }
  }

  const spend = $derived(
    budget && budget.budget_micros && budget.budget_micros > 0
      ? { spent: budget.spent_micros, of: budget.budget_micros, pct: budget.spent_micros / budget.budget_micros }
      : null,
  );
</script>

<svelte:document onpointerdown={onDocDown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<span class="wrap" bind:this={root} onkeydown={onKey}>
  <button
    type="button"
    class="btn btn--chip chip"
    data-testid="autonomy-chip"
    data-level={current}
    aria-expanded={open}
    aria-haspopup="dialog"
    title="How much Control does on its own"
    onclick={toggle}><span class="dot" aria-hidden="true"></span>{autonomyLabel(current)} ▾</button
  >
  {#if open}
    <div class="pop" role="dialog" aria-label="Autonomy" data-testid="autonomy-popover">
      <div class="options" role="radiogroup" aria-label="Autonomy">
        {#each AUTONOMY as p (p.id)}
          <button
            type="button"
            role="radio"
            class="option"
            aria-checked={current === p.id}
            disabled={busy}
            data-testid="autonomy-option-{p.id}"
            onclick={() => void pick(p.id)}
          >
            <span class="opt-label">{p.label}</span>
            <span class="opt-blurb">{p.blurb}</span>
          </button>
        {/each}
      </div>
      {#if current === 'custom'}
        <p class="note" data-testid="autonomy-custom">Your settings mix these. Pick one to set the mission level, auto-continue and the event feed together.</p>
      {/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      {#if spend}
        <div class="spend" data-testid="autonomy-spend">
          <span class="spend-line"><span>Spend today</span><span class="tnum">{money(spend.spent)} of {money(spend.of)}</span></span>
          <Meter value={spend.pct} level={spend.pct >= 1 ? 'crit' : spend.pct >= 0.8 ? 'warn' : 'ok'} label="{money(spend.spent)} of {money(spend.of)} spent today" />
        </div>
      {/if}
      <div class="foot">
        <button
          type="button"
          class="btn btn--quiet"
          data-testid="autonomy-settings"
          onclick={() => {
            open = false;
            openSettingsAt('control-api');
          }}>Detailed settings</button
        >
        <span class="grow"></span>
        <button
          type="button"
          class="btn btn--quiet"
          data-testid="autonomy-pause"
          aria-pressed={$automationPaused}
          onclick={() => void setAutomationPaused(!$automationPaused)}>{$automationPaused ? 'Resume all' : 'Pause all'}</button
        >
      </div>
    </div>
  {/if}
</span>

<style>
  .wrap {
    position: relative;
    display: inline-flex;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
    background: var(--accent);
  }
  .chip[data-level='ask'] .dot {
    background: var(--status-idle);
  }
  .chip[data-level='autopilot'] .dot {
    background: var(--status-done);
  }
  .chip[data-level='custom'] .dot {
    background: var(--control-border-strong);
  }
  .pop {
    position: absolute;
    left: 0;
    bottom: calc(100% + 6px);
    z-index: 20;
    width: 340px;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
    box-shadow: var(--shadow-pop);
    color: var(--fg);
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .option {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2);
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .option:hover {
    background: var(--bg-hover);
  }
  .option[aria-checked='true'] {
    background: var(--accent-soft);
  }
  .option:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: -2px;
  }
  .opt-label {
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .opt-blurb,
  .note {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
    margin: 0;
  }
  .error {
    margin: 0;
    color: var(--status-failed);
    font-size: var(--text-xs);
  }
  .spend {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: var(--space-2);
    border-top: 1px solid var(--border);
  }
  .spend-line {
    display: flex;
    justify-content: space-between;
    font-size: var(--text-xs);
  }
  .tnum {
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .foot {
    display: flex;
    align-items: center;
    border-top: 1px solid var(--border);
    padding-top: var(--space-1);
  }
  .grow {
    flex: 1 1 auto;
  }
</style>
