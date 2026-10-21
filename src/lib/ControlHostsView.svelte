<!--
  Control's Hosts view (Thread room, boards A4Events and A5Menus): every
  host's CPU, memory and disk, the operator's host on top, the ones in
  trouble next. The same list the status bar's host chip opens; a host's
  name opens it in the Hosts view. With more than one hub, each extra fleet
  the sidebar's fleet filter shows follows under its badge, and this
  window's own hosts drop out when the filter hides them.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import HostHealthList from './HostHealthList.svelte';
  import { hosts, hostsLoad, loadHosts } from './hosts';
  import ListGate from './states/ListGate.svelte';
  import { operatorHost } from './operator';
  import { fleetSettings, settingInt, SETTING_KEYS } from './fleet_settings';
  import { requestHostsView } from './app_views';
  import { fleetLoads, type LoadLimits } from './host_health';
  import { multiFleet, showPrimary } from './fleets';
  import FleetHostHealth from './FleetHostHealth.svelte';

  let nowSec = $state(Math.floor(Date.now() / 1000));
  let clock: ReturnType<typeof setInterval> | undefined;
  onMount(() => (clock = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 20_000)));
  onDestroy(() => clearInterval(clock));

  const limits: LoadLimits = $derived({
    diskLowPct: settingInt($fleetSettings, SETTING_KEYS.healthDiskLowPct),
    memLowPct: settingInt($fleetSettings, SETTING_KEYS.healthMemLowPct),
    cpuBusyPct: settingInt($fleetSettings, SETTING_KEYS.healthCpuBusyPct),
  });
  const loads = $derived($showPrimary ? fleetLoads($hosts, $operatorHost, nowSec, limits) : []);
</script>

<div class="hosts" data-testid="control-hosts-view">
  {#if loads.length > 0}
    <HostHealthList {loads} focus={$operatorHost} now={nowSec} focusNote="Control's host" onopen={(a) => requestHostsView(a)} />
  {:else if !$showPrimary}
    <p class="empty" data-testid="control-hosts-primary-filtered">This window's own hosts are hidden by the fleet filter in the sidebar.</p>
  {:else if !$multiFleet}
    <ListGate load={$hostsLoad} what="hosts" onretry={() => void loadHosts()} testid="control-hosts">
      <p class="empty" data-testid="control-hosts-empty">No hosts yet.</p>
    </ListGate>
  {/if}
  {#if $multiFleet}
    <FleetHostHealth now={nowSec} {limits} />
  {/if}
</div>

<style>
  .hosts {
    padding: var(--space-3);
  }
  .empty {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
</style>
