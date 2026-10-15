<!--
  Control's Hosts view (Thread room, boards A4Events and A5Menus): every
  host's CPU, memory and disk, the operator's host on top, the ones in
  trouble next. The same list the status bar's host chip opens; a host's
  name opens it in the Hosts view.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import HostHealthList from './HostHealthList.svelte';
  import { hosts } from './hosts';
  import { operatorHost } from './operator';
  import { fleetSettings, settingInt, SETTING_KEYS } from './fleet_settings';
  import { requestHostsView } from './app_views';
  import { fleetLoads } from './host_health';

  let nowSec = $state(Math.floor(Date.now() / 1000));
  let clock: ReturnType<typeof setInterval> | undefined;
  onMount(() => (clock = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 20_000)));
  onDestroy(() => clearInterval(clock));

  const loads = $derived(
    fleetLoads($hosts, $operatorHost, nowSec, {
      diskLowPct: settingInt($fleetSettings, SETTING_KEYS.healthDiskLowPct),
      memLowPct: settingInt($fleetSettings, SETTING_KEYS.healthMemLowPct),
      cpuBusyPct: settingInt($fleetSettings, SETTING_KEYS.healthCpuBusyPct),
    }),
  );
</script>

<div class="hosts" data-testid="control-hosts-view">
  {#if loads.length === 0}
    <p class="empty">No hosts yet.</p>
  {:else}
    <HostHealthList {loads} focus={$operatorHost} now={nowSec} focusNote="Control's host" onopen={(a) => requestHostsView(a)} />
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
