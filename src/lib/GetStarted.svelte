<!--
  Get started (Orbit Fleet redesign step 10.5, board Tour): the six things
  that make a working fleet, floating in the bottom-right corner (it
  replaced the sidebar's onboarding card, deleted with Classic in 13.1). "–" folds it to its
  title line; ✕ hides it until Settings → Setup guide replays it.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Button from './kit/Button.svelte';
  import Loader from './Loader.svelte';
  import { creatingStart, startingSessions } from './session_starting';
  import { hosts } from './hosts';
  import { accounts } from './accounts';
  import { sessions, hasNoPane } from './sessions';
  import { trackers, loadTrackers } from './trackers';
  import { devices, loadDevices } from './devices';
  import { goTo } from './destination';
  import { openSettingsAt, requestHostsView } from './app_views';
  import { projects } from './projects';
  import { selectSessionExplicitly } from './selection';
  import WizardDialog from './forms/WizardDialog.svelte';
  import { buildFirstFleet, getStartedWizard } from './forms/get_started_wizard';
  import type { FieldProblem, Values } from './forms/forms';
  import { onboardingDismissed } from './onboarding';
  import { openRoutines } from './routines';
  import {
    buildingFirstFleet,
    doneCount,
    enabledRoutineCount,
    getStartedFolded,
    getStartedItems,
    type GetStartedId,
  } from './get_started';

  let routines = $state<number | null>(null);

  onMount(() => {
    if ($trackers.length === 0) void loadTrackers();
    if ($devices.length === 0) void loadDevices();
    void enabledRoutineCount().then((n) => (routines = n));
  });

  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));

  let wizardOpen = $state(false);
  let wizardBusy = $state(false);
  let wizardError = $state<string | null>(null);
  let wizardProblems = $state<FieldProblem[]>([]);
  // Step 10.12: Get started runs on its own form spec
  // (`forms/wizards/get_started.json`): a host, a project and the first
  // session, built in one go when its last button is pressed.
  const wizard = $derived(getStartedWizard({ projects: $projects.map((p) => p.project), hosts: visibleHosts }));

  async function startFromWizard(values: Values) {
    wizardBusy = true;
    wizardError = null;
    wizardProblems = [];
    const r = await buildFirstFleet(values);
    wizardBusy = false;
    if (!r.ok) {
      wizardError = r.error ?? null;
      wizardProblems = r.problems ?? [];
      return;
    }
    wizardOpen = false;
    selectSessionExplicitly(r.row);
  }

  const items = $derived(
    getStartedItems({
      visibleHostCount: visibleHosts.length,
      accountCount: $accounts.length,
      workSessionCount: $sessions.filter((s) => !hasNoPane(s)).length,
      githubConnected: $trackers.some((t) => t.provider === 'github' && t.state === 'ok'),
      otherDeviceCount: $devices.filter((d) => !d.this_device).length,
      enabledRoutineCount: routines,
    }),
  );
  const done = $derived(doneCount(items));
  // Step 10.10: the Galaxy while the first session of the fleet starts.
  const building = $derived(
    buildingFirstFleet({
      workSessionIds: $sessions.filter((s) => !hasNoPane(s)).map((s) => s.id),
      starting: $startingSessions,
      creating: $creatingStart?.kind === 'work',
    }),
  );
  const all = $derived(done === items.length);

  function open(id: GetStartedId) {
    switch (id) {
      case 'host':
        return requestHostsView();
      case 'account':
        return goTo('accounts');
      case 'session':
        // The Get started wizard: host, project and agent in one place (10.12).
        wizardOpen = true;
        return;
      case 'github':
        return openSettingsAt('trackers');
      case 'phone':
        return openSettingsAt('devices');
      case 'routine':
        // The Routines open in Automation, on the first template (8.6).
        return openRoutines({ template: 'morning-pr-sweep' });
    }
  }
</script>

<section class="panel" aria-label="Get started" data-testid="get-started">
  <div class="top">
    <strong class="grow">Get started</strong>
    <span class="meta tnum" data-testid="get-started-count">{done} of {items.length}</span>
    <Button
      variant="quiet"
      size="sm"
      label={$getStartedFolded ? 'Unfold Get started' : 'Fold Get started'}
      onclick={() => getStartedFolded.update((f) => !f)}
      testid="get-started-fold">{$getStartedFolded ? '+' : '–'}</Button
    >
    <Button
      variant="quiet"
      size="sm"
      label="Hide Get started"
      title="Hide it; Settings → Appearance → Setup guide brings it back"
      onclick={() => onboardingDismissed.set(true)}
      testid="get-started-hide">✕</Button
    >
  </div>
  {#if !$getStartedFolded}
    <div class="bar" aria-hidden="true"><span style="width:{(done / items.length) * 100}%"></span></div>
    <!-- One loader per screen: while the wizard builds, its own Galaxy shows. -->
    {#if building && !wizardBusy}
      <div class="building" data-testid="get-started-building">
        <Loader name="galaxy" size={120} label="Building your fleet" testid="get-started-galaxy" />
        <span class="meta">Building your fleet · starting your first session</span>
      </div>
    {/if}
    {#if all}
      <p class="all" data-testid="get-started-all">Your fleet is set up.</p>
    {:else}
      <ul class="rows">
        {#each items as item (item.id)}
          <li>
            <button
              type="button"
              class="row"
              class:next={item.next}
              class:done={item.done}
              data-testid="get-started-{item.id}"
              onclick={() => open(item.id)}
            >
              <span class="mark" aria-hidden="true">{item.done ? '✓' : '○'}</span>
              <span class="label grow">{item.label}</span>
              {#if item.next}<span class="meta">{item.minutes} min ›</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

{#if wizardOpen}
  <WizardDialog
    {wizard}
    busy={wizardBusy}
    error={wizardError}
    problems={wizardProblems}
    run={(v) => void startFromWizard(v)}
    onclose={() => (wizardOpen = false)} />
{/if}

<style>
  .panel {
    position: fixed;
    right: 20px;
    bottom: 40px;
    z-index: 800;
    width: 300px;
    background: var(--bg-raise);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-pop);
    overflow: hidden;
    color: var(--fg);
    font-size: var(--text-sm);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 14px;
  }
  .grow {
    flex: 1 1 auto;
  }
  .meta {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .bar {
    margin: 0 14px;
    height: 4px;
    border-radius: var(--radius-xs);
    background: var(--border);
    overflow: hidden;
  }
  .bar > span {
    display: block;
    height: 100%;
    background: var(--status-done);
    transition: width var(--dur-base);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 8px 6px;
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    width: 100%;
    padding: 5px 8px;
    border: none;
    border-radius: var(--radius-md);
    background: none;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row .mark {
    color: var(--fg-muted);
  }
  .row.done .mark {
    color: var(--status-done);
  }
  .row.done .label {
    color: var(--fg-muted);
    text-decoration: line-through;
  }
  .row.next {
    background: var(--accent-soft);
  }
  .row.next .mark {
    color: var(--accent);
  }
  .building {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 12px 14px 4px;
  }
  .all {
    margin: 10px 14px 14px;
  }
</style>
