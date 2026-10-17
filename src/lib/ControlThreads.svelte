<!--
  Control's own left column (Thread room, 2026-10-14; board Main): Main chat
  at the top, then the threads Control handed out — the sessions its agent
  started and the sessions and missions its handoff receipts name — grouped
  Needs you / Working / Done today (and a folded Earlier), with filter chips
  All · Needs you · Missions, and today's spend against the routines' daily
  budget at the bottom. A session thread opens in the right panel's Session
  tab, a mission thread in its Missions view; Control stays.

  It takes the sessions list's place only while Control is open. "All
  sessions" at its foot (and the rail's Sessions) goes back to that list,
  which never went anywhere; « folds this column to an edge button.

  Each thread's bell mutes it (Control review F10): fleet stops telling
  Control about it until it is unmuted (`control_journal`).
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import Icon from './kit/Icon.svelte';
  import Meter from './kit/Meter.svelte';
  import OrbitMark from './kit/OrbitMark.svelte';
  import { sessions } from './sessions';
  import { handoffs, followHandoffs } from './handoffs';
  import { listMissions, type Mission } from './missions';
  import { onWorkChangedDebounced } from './work';
  import { operatorRow } from './operator';
  import { visibleFleet } from './control_views';
  import { buildThreads, filterThreads, groupThreads, threadCounts, type ControlThread, type ThreadFilter } from './control_threads';
  import { activeThread, openInControl, openMissionInControl, showMainChat } from './control_room';
  import { promptPreview } from './attention';
  import { shortAge } from './session_status';
  import { routineBudget, type FleetBudget } from './routines';
  import { money } from './automation';
  import { leave } from './destination';
  import { sidebarView } from './work_view';
  import { mutedThreads, refreshMutes, setThreadMuted } from './control_journal';

  let { oncollapse }: { oncollapse: () => void } = $props();

  onMount(() => followHandoffs());
  // A mute is a `handoff:changed`, which re-reads the receipts: re-read the
  // mutes with them.
  const stopMutes = handoffs.subscribe(() => void refreshMutes());
  onDestroy(stopMutes);

  let missions = $state<Mission[]>([]);
  let budget = $state<FleetBudget | null>(null);
  async function loadMissions() {
    const r = await listMissions();
    if (r.ok && Array.isArray(r.value)) missions = r.value;
  }
  async function loadBudget() {
    const r = await routineBudget();
    budget = r.ok && r.value ? r.value : null;
  }
  let stopWork: (() => void) | undefined;
  let nowSec = $state(Math.floor(Date.now() / 1000));
  let clock: ReturnType<typeof setInterval> | undefined;
  onMount(() => {
    void loadMissions();
    void loadBudget();
    stopWork = onWorkChangedDebounced(
      () => {
        void loadMissions();
        void loadBudget();
      },
      () => 500,
      () => 3000,
    );
    clock = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
  });
  onDestroy(() => {
    stopWork?.();
    clearInterval(clock);
  });

  let filter = $state<ThreadFilter>('all');
  const threads = $derived(
    buildThreads({
      rows: $sessions,
      handoffs: $handoffs,
      missions,
      operatorId: $operatorRow?.id ?? null,
      opts: $visibleFleet.opts,
      muted: $mutedThreads,
    }),
  );
  const counts = $derived(threadCounts(threads));
  const groups = $derived(groupThreads(filterThreads(threads, filter)));
  const CHIPS: readonly { id: ThreadFilter; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'needs', label: 'Needs you' },
    { id: 'missions', label: 'Missions' },
  ];

  const mainLine = $derived(promptPreview($operatorRow?.last_prompt ?? null, 48) || 'Talk to the fleet');

  function open(t: ControlThread) {
    if (t.kind === 'session') openInControl(t.id, t.title);
    else openMissionInControl(t.id);
  }

  let muteError = $state<string | null>(null);
  async function toggleMute(t: ControlThread) {
    const r = await setThreadMuted(t.kind, t.id, !t.muted);
    muteError = r.ok ? null : r.error.message;
  }

  function isActive(t: ControlThread): boolean {
    const a = $activeThread;
    return !!a && a.kind === t.kind && a.id === t.id;
  }

  function allSessions() {
    sidebarView.set('sessions');
    leave('control');
  }

  const spend = $derived(
    budget && budget.budget_micros && budget.budget_micros > 0
      ? { spent: budget.spent_micros, of: budget.budget_micros, pct: budget.spent_micros / budget.budget_micros }
      : null,
  );
</script>

<aside class="threads" aria-label="Control's threads" data-testid="control-threads">
  <div class="head">
    <span class="title">Control</span>
    <button
      type="button"
      class="btn btn--quiet btn--icon"
      data-testid="control-threads-collapse"
      title="Hide Control's threads"
      aria-label="Hide Control's threads"
      onclick={oncollapse}>‹</button
    >
  </div>
  <div class="chips" role="group" aria-label="Show threads">
    {#each CHIPS as c (c.id)}
      <button
        type="button"
        class="chip"
        aria-pressed={filter === c.id}
        data-testid="control-threads-filter-{c.id}"
        onclick={() => (filter = c.id)}>{c.label}{#if counts[c.id] > 0}{' '}<span class="n">{counts[c.id]}</span>{/if}</button
      >
    {/each}
  </div>
  <div class="list">
    <button
      type="button"
      class="row main"
      class:on={$activeThread === null}
      aria-current={$activeThread === null ? 'true' : undefined}
      data-testid="control-thread-main"
      onclick={showMainChat}
    >
      <span class="mark"><OrbitMark size={20} label={null} /></span>
      <span class="text"><span class="ttl">Main chat</span><span class="ln">{mainLine}</span></span>
    </button>
    {#if threads.length === 0}
      <p class="empty" data-testid="control-threads-empty">
        No threads yet. Whatever Control hands to a session or a mission shows up here, grouped by what needs you.
      </p>
    {:else if groups.length === 0}
      <p class="empty">Nothing here for this filter.</p>
    {/if}
    {#if muteError}<p class="empty" role="alert" data-testid="control-thread-mute-error">{muteError}</p>{/if}
    {#each groups as g (g.id)}
      {#snippet rows()}
        {#each g.threads as t (t.key)}
          <div class="trow" class:muted={t.muted}>
            <button
              type="button"
              class="row"
              class:on={isActive(t)}
              aria-current={isActive(t) ? 'true' : undefined}
              data-testid="control-thread"
              data-kind={t.kind}
              data-group={t.group}
              data-muted={t.muted ? 'true' : undefined}
              onclick={() => open(t)}
            >
              <span class="dot"><StatusDot state={t.state} /></span>
              <span class="text">
                <span class="ttl">{t.title}</span>
                <span class="ln" class:waiting={t.group === 'needs'}>{t.muted ? `Muted · ${t.line}` : t.line}</span>
              </span>
              {#if t.at > 0}<span class="age">{shortAge(t.at, nowSec)}</span>{/if}
            </button>
            <button
              type="button"
              class="mute"
              data-testid="control-thread-mute"
              aria-pressed={t.muted}
              title={t.muted ? 'Unmute: Control hears about it again' : 'Mute: Control stops hearing about it'}
              aria-label={t.muted ? `Unmute ${t.title}` : `Mute ${t.title}`}
              onclick={() => void toggleMute(t)}><Icon name={t.muted ? 'bell-off' : 'bell'} size={12} /></button
            >
          </div>
        {/each}
      {/snippet}
      {#if g.id === 'earlier'}
        <details class="group" data-testid="control-thread-group-{g.id}">
          <summary class="sec">{g.label} <span class="n">{g.threads.length}</span></summary>
          {@render rows()}
        </details>
      {:else}
        <section class="group" data-testid="control-thread-group-{g.id}" aria-label={g.label}>
          <p class="sec">{g.label} <span class="n">{g.threads.length}</span></p>
          {@render rows()}
        </section>
      {/if}
    {/each}
  </div>
  <div class="foot">
    {#if spend}
      <div class="spend" data-testid="control-threads-spend">
        <span class="spend-line"><span>Spend today</span><span class="tnum">{money(spend.spent)} of {money(spend.of)}</span></span>
        <Meter
          value={spend.pct}
          level={spend.pct >= 1 ? 'crit' : spend.pct >= 0.8 ? 'warn' : 'ok'}
          label="{money(spend.spent)} of {money(spend.of)} spent today"
        />
      </div>
    {/if}
    <button type="button" class="btn btn--quiet all" data-testid="control-threads-all-sessions" onclick={allSessions}
      >All sessions ›</button
    >
  </div>
</aside>

<style>
  .threads {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--bg-pane);
    border-right: 1px solid var(--border);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--header-h);
    padding: 0 var(--space-2) 0 var(--space-4);
  }
  .title {
    flex: 1 1 auto;
    font-size: var(--text-lg);
    font-weight: 600;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    padding: 0 var(--space-3) var(--space-2);
  }
  .chip {
    height: 20px;
    padding: 0 7px;
    border: 0;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    color: var(--fg-2);
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    cursor: pointer;
  }
  .chip[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .chip:focus-visible,
  .row:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: -2px;
  }
  .trow {
    position: relative;
  }
  .trow.muted .row {
    opacity: 0.6;
  }
  .mute {
    position: absolute;
    top: 50%;
    right: var(--space-2);
    transform: translateY(-50%);
    display: inline-flex;
    padding: 2px;
    border: 0;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    color: var(--fg-2);
    cursor: pointer;
    opacity: 0;
  }
  .trow:hover .mute,
  .mute:focus-visible,
  .mute[aria-pressed='true'] {
    opacity: 1;
  }
  .mute:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .list {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    padding-bottom: var(--space-2);
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    width: calc(100% - 12px);
    margin: 0 6px;
    padding: 8px 10px;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row.on {
    background: var(--accent-soft);
  }
  .row.main {
    margin-top: var(--space-1);
  }
  .mark {
    flex: none;
    line-height: 0;
  }
  .dot {
    flex: none;
    margin-top: 5px;
    line-height: 0;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1 1 auto;
  }
  .ttl {
    font-size: var(--text-sm);
    font-weight: 500;
    line-height: var(--text-sm-lh);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ln {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ln.waiting {
    color: var(--status-waiting);
  }
  .age {
    flex: none;
    color: var(--fg-muted);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }
  .sec {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    padding: 14px 14px 6px;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-weight: 500;
    letter-spacing: 0.02em;
  }
  summary.sec {
    cursor: pointer;
  }
  .n {
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--count-bg);
    color: var(--fg-2);
    font-size: var(--text-2xs);
    font-weight: 500;
    line-height: 16px;
  }
  .empty {
    margin: var(--space-3) var(--space-4);
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
  }
  .foot {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: 10px 14px;
    border-top: 1px solid var(--border);
  }
  .spend {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .spend-line {
    display: flex;
    justify-content: space-between;
    font-size: var(--text-xs);
    color: var(--fg-2);
  }
  .tnum {
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .all {
    align-self: flex-start;
  }
</style>
