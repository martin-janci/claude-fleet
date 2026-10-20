<!--
  The status bar's host chip (Thread room, 2026-10-14; board Main's footer):
  CPU, memory and disk of the host the selected session runs on — in
  Control, of the session its right panel shows, else of the operator's
  host — and the other host in the worst trouble, if any ("· mercury mem
  94%"). A click opens every host's numbers above it; a host's name there
  opens it in the Hosts view. Sits beside Shortcuts. Open autofix findings
  on the focus host show as a count ("2 problems", src/lib/autofix.ts).
  With an extra fleet's session open (multi-hub) the chip follows that
  session's host on its own hub, and the popover lists every fleet the
  sidebar's fleet filter shows.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import HostHealthList from './HostHealthList.svelte';
  import { hosts } from './hosts';
  import { selectedSession } from './selection';
  import { destination, isFleetPage } from './destination';
  import { controlViews } from './control_views';
  import { operatorHost } from './operator';
  import { fleetSettings, settingInt, SETTING_KEYS } from './fleet_settings';
  import { requestHostsView } from './app_views';
  import { chipHost, fleetLoads, loadParts, otherTrouble, type LoadLimits } from './host_health';
  import { findingsByHost, watchFindings } from './autofix';
  import { fleetRows, multiFleet, selectedFleetSession, showPrimary } from './fleets';
  import FleetHostHealth from './FleetHostHealth.svelte';

  let nowSec = $state(Math.floor(Date.now() / 1000));
  let clock: ReturnType<typeof setInterval> | undefined;
  let release: (() => void) | undefined;
  onMount(() => {
    clock = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 20_000);
    release = watchFindings();
  });
  onDestroy(() => {
    clearInterval(clock);
    release?.();
  });

  const limits: LoadLimits = $derived({
    diskLowPct: settingInt($fleetSettings, SETTING_KEYS.healthDiskLowPct),
    memLowPct: settingInt($fleetSettings, SETTING_KEYS.healthMemLowPct),
    cpuBusyPct: settingInt($fleetSettings, SETTING_KEYS.healthCpuBusyPct),
  });
  const inControl = $derived($destination === 'control');
  const sessionShown = $derived($controlViews.open && $controlViews.active === 'session');
  const focus = $derived(
    chipHost({ inControl, sessionShown, selectedHost: $selectedSession?.host_alias ?? null, operatorHost: $operatorHost }),
  );
  const loads = $derived(fleetLoads($hosts, focus, nowSec, limits));
  // An extra fleet's open session: its host, read from that fleet's rows —
  // only while it is on screen. A fleet page (Control, Hosts, …) covers it
  // though it stays selected, and there the chip keeps Control's host and
  // its open findings, as it does for a primary session.
  const away = $derived(isFleetPage($destination) ? null : $selectedFleetSession);
  const awayLoads = $derived(
    away ? fleetLoads([...($fleetRows.get(away.fleet.id)?.hosts.values() ?? [])], away.row.host_alias, nowSec, limits) : [],
  );
  const awayFocus = $derived(away ? { fleet: away.fleet.id, alias: away.row.host_alias } : null);
  const mine = $derived(
    away ? (awayLoads.find((l) => l.alias === away.row.host_alias) ?? null) : (loads.find((l) => l.alias === focus) ?? null),
  );
  const other = $derived(otherTrouble(away ? awayLoads : loads, away ? away.row.host_alias : focus));
  const problems = $derived(away ? 0 : ($findingsByHost.get(focus) ?? 0));
  const shownHost = $derived(away ? `${away.fleet.label} · ${away.row.host_alias}` : focus);
  const why = $derived(
    away
      ? 'follows the selected session'
      : inControl && !(sessionShown && $selectedSession)
        ? "Control's host"
        : $selectedSession
          ? 'follows the selected session'
          : "Control's host",
  );

  let open = $state(false);
  let root: HTMLElement | undefined = $state();

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
  function openHost(alias: string) {
    open = false;
    requestHostsView(alias);
  }
</script>

<svelte:document onpointerdown={onDocDown} />

{#if loads.length > 0 || awayLoads.length > 0}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <span class="wrap" bind:this={root} onkeydown={onKey}>
    <button
      type="button"
      class="chip"
      class:alert={!!mine?.trouble}
      data-testid="host-health-chip"
      aria-expanded={open}
      aria-label="Host health for {shownHost}{problems ? `; ${problems} open problem${problems === 1 ? '' : 's'}` : ''}{other ? `; ${other.alias} ${other.trouble}` : ''}"
      title="CPU, memory and disk of {shownHost} ({why}). Click for every host."
      onclick={() => (open = !open)}
    >
      <span class="host">{shownHost}</span>
      {#if mine}
        {#each loadParts(mine) as p, i (i)}<span class="part">{p}</span>{/each}
      {:else}
        <span class="part">unknown host</span>
      {/if}
      {#if problems > 0}<span class="problems" data-testid="host-health-problems">{problems} problem{problems === 1 ? '' : 's'}</span>{/if}
      {#if other}<span class="other" data-testid="host-health-other">· {other.alias} {other.trouble}</span>{/if}
    </button>
    {#if open}
      <div class="pop" role="dialog" aria-label="Host health" data-testid="host-health-popover">
        {#if $showPrimary}
          <HostHealthList {loads} focus={away ? null : focus} now={nowSec} focusNote={why} onopen={openHost} />
        {/if}
        {#if $multiFleet}
          <FleetHostHealth now={nowSec} {limits} focus={awayFocus} focusNote={why} />
        {/if}
      </div>
    {/if}
  </span>
{/if}

<style>
  .wrap {
    position: relative;
    display: inline-flex;
  }
  .chip {
    height: 21px;
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    color: var(--fg-2);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
    white-space: nowrap;
  }
  .chip:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .chip.alert {
    background: var(--waiting-soft);
  }
  .host {
    color: var(--fg);
    font-weight: 500;
  }
  .part {
    font-variant-numeric: tabular-nums;
  }
  .problems {
    color: var(--status-waiting);
    font-weight: 500;
  }
  .other {
    color: var(--status-waiting);
    font-variant-numeric: tabular-nums;
  }
  .pop {
    position: absolute;
    right: 0;
    bottom: calc(100% + 6px);
    z-index: 30;
    width: 360px;
    max-height: 60vh;
    overflow: auto;
    padding: 12px 14px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
    box-shadow: var(--shadow-pop);
    color: var(--fg);
    white-space: normal;
  }
</style>
