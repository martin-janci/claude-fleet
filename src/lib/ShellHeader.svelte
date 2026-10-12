<!-- The 44 px header (redesign step 3.17, Main board): the Orbit mark and
     name, the ⌘K command field, the account pills (health dot, name, both
     windows), and Automation with Pause all. Automation (step 8.4) says the
     active missions and today's spend and opens the Automation screen; Pause
     all is `automation.paused`, and Resume clears it. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import AppHeader from './kit/AppHeader.svelte';
  import Icon from './kit/Icon.svelte';
  import { accounts } from './accounts';
  import { accountUsage } from './account_usage_store';
  import { openAccount } from './account_pill';
  import { goTo } from './destination';
  import { HEADER_ACCOUNTS_MAX, headerAccounts } from './header_accounts';
  import { listMissions } from './missions';
  import { loadFleetSettings } from './fleet_settings';
  import { automationPaused, money, runsToday, setAutomationPaused, spendMicros } from './automation';
  import { workChanged } from './work';
  import { openSwitcher } from './switcher_request';
  import { pushError } from './toasts';

  let { mac }: { mac?: boolean } = $props();

  let nowSec = $state(Math.floor(Date.now() / 1000));
  let active = $state<number | null>(null);
  let spend = $state<number | null>(null);
  let pausing = $state(false);

  onMount(() => {
    void loadFleetSettings();
    // Usage readings age out; the pills re-read them on the same cadence as
    // the status bar's mark.
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });

  async function loadActive() {
    const r = await listMissions();
    active = r.ok && Array.isArray(r.value) ? r.value.filter((m) => m.state === 'active').length : null;
    const today = await runsToday();
    spend = today.ok ? spendMicros(today.value) : null;
  }
  $effect(() => {
    void $workChanged;
    void loadActive();
  });

  const pills = $derived(headerAccounts($accounts, $accountUsage, nowSec));
  const shown = $derived(pills.slice(0, HEADER_ACCOUNTS_MAX));
  const more = $derived(pills.length - shown.length);

  async function togglePause() {
    pausing = true;
    const r = await setAutomationPaused(!$automationPaused);
    pausing = false;
    if (!r.ok) pushError(r.error, $automationPaused ? 'Resume failed' : 'Pause all failed');
  }
</script>

<AppHeader oncommand={openSwitcher} {mac} testid="shell-header">
  {#each shown as a (a.uuid)}
    <button
      type="button"
      class="of-btn quiet pill"
      data-testid="header-account"
      aria-label={a.aria}
      title={a.aria}
      onclick={() => openAccount(a.uuid)}
    >
      <span class="hdot {a.health}" aria-hidden="true"></span>
      <span>{a.label}</span>
      {#if a.meta}<span class="meta tnum" class:limited={a.limited}>{a.meta}</span>{/if}
    </button>
  {/each}
  {#if more > 0}
    <button
      type="button"
      class="of-btn quiet"
      data-testid="header-accounts-more"
      aria-label="{more} more account{more === 1 ? '' : 's'}"
      onclick={() => goTo('accounts')}>+{more}</button
    >
  {/if}
  {#if shown.length > 0}<span class="sep" aria-hidden="true"></span>{/if}
  <button
    type="button"
    class="of-btn quiet automation"
    data-testid="header-automation"
    title="Open Automation"
    onclick={() => goTo('automation')}
  >
    <Icon name="clock" />
    <span
      >Automation{#if $automationPaused}&nbsp;paused{:else if active !== null}&nbsp;{active} active{/if}</span
    >
    {#if spend !== null}<span class="meta tnum" data-testid="header-spend">{money(spend)} today</span>{/if}
  </button>
  <button
    type="button"
    class="of-btn quiet sm"
    data-testid="header-pause-all"
    aria-label={$automationPaused ? 'Resume automation' : 'Pause all automation'}
    disabled={pausing}
    onclick={togglePause}>{$automationPaused ? 'Resume' : 'Pause all'}</button
  >
</AppHeader>

<style>
  .pill {
    gap: 8px;
  }
  .hdot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-pill);
    flex: none;
    background: var(--status-idle);
  }
  /* Colour never alone: the meta says the level in numbers or words. */
  .hdot.ok {
    background: var(--status-done);
  }
  .hdot.caution {
    background: var(--status-waiting);
  }
  .hdot.limit {
    background: var(--status-failed);
  }
  .meta {
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .meta.limited {
    color: var(--status-waiting);
  }
  .automation {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: var(--text-sm);
    color: var(--fg-2);
  }
</style>
