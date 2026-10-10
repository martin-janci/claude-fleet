<script lang="ts">
  import type { Snippet } from 'svelte';
  import { tablistKeys } from '../tablist_keys';
  // The Automation screen's Routines tab (Orbit Fleet redesign 8.6, the
  // Automation board): the routines on the left, yours then the built-in
  // ones (the fleet's own loops, 8.4), with Filters and Group; one routine
  // in the middle with its switch, Run now, Skip next and Edit, then Runs,
  // Definition and Limits; its limits and kill switches on the right. A
  // failed run carries Fix, Retry and Pause, as in the Inbox. New starts
  // from a template (the first is Morning PR sweep) or a blank one.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { hosts } from '../hosts';
  import { sessions } from '../sessions';
  import { selectSessionExplicitly } from '../selection';
  import { goTo } from '../destination';
  import { fleetSettings, SETTING_KEYS, settingInt } from '../fleet_settings';
  import { loopEvery, loopLine, startOfToday } from '../automation';
  import { listRuns, type RunRow } from '../runs';
  import type { LoopHealth } from '../ipc';
  import Button from '../kit/Button.svelte';
  import Count from '../kit/Count.svelte';
  import Icon from '../kit/Icon.svelte';
  import KeyValue from '../kit/KeyValue.svelte';
  import Meter from '../kit/Meter.svelte';
  import StatusChip from '../kit/StatusChip.svelte';
  import StatusDot from '../kit/StatusDot.svelte';
  import type { OfState } from '../kit/status';
  import NewRoutineMenu from './NewRoutineMenu.svelte';
  import RoutineEventTrigger from './RoutineEventTrigger.svelte';
  import DestructiveConfirm from '../forms/DestructiveConfirm.svelte';
  import type { IpcError } from '../result';
  import { accountByUuid } from '../accounts';
  import { projects } from '../projects';
  import { errorText } from '../error_copy';
  import { push, pushError } from '../toasts';
  import {
    averageRun,
    AUTONOMY_CHOICES,
    autonomyWords,
    capSecsOf,
    capWords,
    fixLabel,
    hostWords,
    onFailureWords,
    OUTCOME_ROUTING,
    startedByWords,
    clockChange,
    cronParts,
    deviceZone,
    dryRunLine,
    nextRunLabel,
    offsetWords,
    previewRoutine,
    SCHEDULE_DAYS,
    scheduleCron,
    schedulePick,
    type RoutineAccount,
    type RoutinePreview,
    type ScheduleDays,
    deleteRoutine,
    deviceOffsetMin,
    dollars,
    eventFilterInput,
    eventFilterOf,
    eventFilterWords,
    type EventFilter,
    fixRoutine,
    getRoutine,
    lastRunByRoutine,
    listRoutines,
    loadFailing,
    microsOf,
    morningPrSweep,
    nextRunWords,
    routineAccountLabel,
    routineDeleteLoss,
    routineDot,
    routineLine,
    routineStateWords,
    routinesRequest,
    runRoutineNow,
    runSourceHint,
    runDot,
    runWords,
    saveRoutine,
    setRoutineEnabled,
    skipNextRun,
    spentSince,
    triggerWords,
    type RoutineDetail,
    type RoutineInput,
    type RoutineRow,
    type RoutineRunRow,
    type RoutineTab,
    type RoutineTrigger,
  } from '../routines';

  let list = $state<RoutineRow[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<number | null>(null);
  let detail = $state<RoutineDetail | null>(null);
  let tab = $state<RoutineTab>('runs');
  let busy = $state(false);
  let confirmDelete = $state(false);
  /** The delete confirm's failure, shown as its banner (input kept). */
  let deleteError = $state<IpcError | null>(null);
  /** A built-in routine (a fleet loop) picked in the list, by name. */
  let loopSel = $state<string | null>(null);
  /** Each routine's newest run, for the list's dot and line. */
  let lastRuns = $state<Map<number, RunRow>>(new Map());
  /** The editor's draft: a new routine (`id` absent) or a change. */
  let draft = $state<(Draft & { id?: number }) | null>(null);

  interface Draft {
    name: string;
    trigger: RoutineTrigger;
    /** The picker's days and time; `custom` edits `cron` as text. */
    days: ScheduleDays;
    time: string;
    cron: string;
    event: string;
    host_alias: string;
    project_id: string;
    profile: string;
    prompt: string;
    budget_run: string;
    budget_day: string;
    overlap: 'skip' | 'parallel';
    filter: EventFilter;
    /** Time cap in minutes; empty = none (G3.8). */
    cap_min: string;
    fallback_host: string;
    retry_once: boolean;
    autonomy: number;
  }

  const pickable = $derived($projects.filter((p) => !p.project.system));
  const projectName = (id: number) => {
    const p = $projects.find((x) => x.project.id === id)?.project;
    return p ? `${p.owner}/${p.repo}` : `project ${id}`;
  };

  async function reload() {
    const r = await listRoutines();
    loaded = true;
    if (!r.ok) {
      error = `Couldn't load routines: ${errorText(r.error)}.`;
      return;
    }
    error = null;
    list = Array.isArray(r.value) ? r.value : [];
    if (selected === null || !list.some((x) => x.id === selected)) selected = list[0]?.id ?? null;
    await Promise.all([loadDetail(), loadLastRuns()]);
  }

  /** The routines' newest runs; a failed read leaves the lines without them. */
  async function loadLastRuns() {
    const r = await listRuns({ kind: 'routine', limit: 200 });
    if (r.ok && Array.isArray(r.value?.runs)) lastRuns = lastRunByRoutine(r.value.runs);
  }

  async function loadDetail() {
    if (selected === null) {
      detail = null;
      return;
    }
    const r = await getRoutine(selected);
    detail = r.ok ? r.value : null;
    if (!r.ok) error = `Couldn't load the routine: ${errorText(r.error)}.`;
  }

  function pick(id: number) {
    selected = id;
    loopSel = null;
    draft = null;
    confirmDelete = false;
    void loadDetail();
  }

  /** Run one action, then re-read the list, the routine and the Inbox. */
  async function act<T>(label: string, f: () => Promise<{ ok: true; value: T } | { ok: false; error: { code: string; message: string } }>) {
    busy = true;
    const r = await f();
    busy = false;
    if (!r.ok) pushError(r.error, `${label} failed`);
    await reload();
    await loadFailing();
    return r.ok;
  }

  // ---- delete (G1.4, the destructive confirm) --------------------------------

  function askDelete() {
    deleteError = null;
    confirmDelete = true;
  }

  /** Delete the routine; a failure stays in the confirm as its banner. */
  async function doDelete(id: number) {
    busy = true;
    const r = await deleteRoutine(id);
    busy = false;
    if (!r.ok) {
      deleteError = r.error;
      return;
    }
    confirmDelete = false;
    selected = null;
    await reload();
    await loadFailing();
  }

  /** The safer way out: pause it, keep it and its runs. */
  async function pauseInstead(id: number) {
    confirmDelete = false;
    await act('Pause', () => setRoutineEnabled(id, false));
  }

  // ---- the editor -----------------------------------------------------------

  function dollarsField(m: number | undefined): string {
    return m === undefined ? '' : (m / 1_000_000).toFixed(2);
  }

  function fromInput(i: RoutineInput, id?: number): Draft & { id?: number } {
    const pick = schedulePick(i.cron);
    return {
      id,
      name: i.name,
      trigger: i.trigger,
      days: pick.days,
      time: pick.time,
      cron: i.cron ?? '',
      event: i.event ?? 'stuck',
      host_alias: i.host_alias,
      project_id: i.project_id ? String(i.project_id) : '',
      profile: i.profile ?? '',
      prompt: i.prompt,
      budget_run: dollarsField(i.budget_run_micros),
      budget_day: dollarsField(i.budget_day_micros),
      overlap: i.overlap === 'parallel' ? 'parallel' : 'skip',
      filter: eventFilterOf(i),
      cap_min: i.run_max_secs ? String(Math.round(i.run_max_secs / 60)) : '',
      fallback_host: i.fallback_host ?? '',
      retry_once: i.retry_once ?? false,
      autonomy: i.autonomy ?? 2,
    };
  }

  /** Open the editor on `d`, remembering it to tell an unsaved change. */
  function open(d: Draft & { id?: number }) {
    draft = d;
    draftBase = JSON.stringify(d);
    preview = null;
  }

  function newFrom(template: string | null) {
    loopSel = null;
    const host = $hosts.find((h) => !h.hidden)?.alias ?? '';
    const proj = pickable[0]?.project;
    if (template === 'morning-pr-sweep') {
      open(fromInput(morningPrSweep(host, proj?.id ?? 0, proj ? `${proj.owner}/${proj.repo}` : undefined)));
    } else {
      open(fromInput({ name: '', trigger: 'cron', cron: '0 9 * * 1-5', host_alias: host, project_id: proj?.id ?? 0, prompt: '' }));
    }
  }

  function edit(r: RoutineRow, copy = false) {
    open(fromInput(
      {
        name: copy ? `${r.name} copy` : r.name,
        trigger: (r.trigger as RoutineTrigger) ?? 'cron',
        cron: r.cron,
        event: r.event,
        host_alias: r.host_alias,
        project_id: r.project_id,
        profile: r.profile,
        prompt: r.prompt,
        budget_run_micros: r.budget_run_micros,
        budget_day_micros: r.budget_day_micros,
        overlap: r.overlap === 'parallel' ? 'parallel' : 'skip',
        event_repo: r.event_repo,
        event_author: r.event_author === 'anyone' ? 'anyone' : undefined,
        event_rate_secs: r.event_rate_secs,
        run_max_secs: r.run_max_secs,
        fallback_host: r.fallback_host,
        retry_once: r.retry_once,
        autonomy: r.autonomy,
      },
      copy ? undefined : r.id,
    ));
  }

  /** The cron line the draft means: the picker's, or the text of Custom. */
  const cronOf = (d: Draft) => (d.days === 'custom' ? d.cron.trim() : (scheduleCron(d.days, d.time) ?? ''));

  const draftReady = $derived(
    !!draft &&
      draft.name.trim() !== '' &&
      draft.prompt.trim() !== '' &&
      draft.host_alias !== '' &&
      draft.project_id !== '' &&
      (draft.trigger !== 'cron' || cronOf(draft) !== ''),
  );

  /** The routine as `save` and `preview` read it. */
  function inputOf(d: Draft & { id?: number }): RoutineInput {
    const was = d.id !== undefined ? list.find((x) => x.id === d.id) : undefined;
    return {
      name: d.name.trim(),
      enabled: was ? was.enabled : true,
      trigger: d.trigger,
      cron: d.trigger === 'cron' ? cronOf(d) : undefined,
      utc_offset_min: deviceOffsetMin(),
      event: d.trigger === 'event' ? d.event : undefined,
      host_alias: d.host_alias,
      project_id: Number(d.project_id),
      profile: d.profile.trim() || undefined,
      prompt: d.prompt.trim(),
      budget_run_micros: microsOf(d.budget_run),
      budget_day_micros: microsOf(d.budget_day),
      overlap: d.overlap,
      ...eventFilterInput(d.trigger, d.event, d.filter),
      ...(zone ? { time_zone: zone } : {}),
      run_max_secs: capSecsOf(d.cap_min),
      fallback_host: d.fallback_host || undefined,
      retry_once: d.retry_once,
      autonomy: d.autonomy < 2 ? d.autonomy : undefined,
    };
  }

  /** Save the draft; answers the saved routine, or null on a refusal. */
  async function saveDraft(): Promise<RoutineRow | null> {
    if (!draft || !draftReady) return null;
    const d = draft;
    busy = true;
    const r = await saveRoutine(inputOf(d), d.id);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Saving the routine failed');
      return null;
    }
    draft = null;
    selected = r.value.id;
    push({ kind: 'success', message: `${r.value.name} saved` });
    await reload();
    return r.value;
  }

  // ---- the dry run, next run and Run once now (gap plan G2.3) ------------------

  /** The editor's draft as it was opened, to tell an unsaved change. */
  let draftBase = $state('');
  const dirty = $derived(!!draft && JSON.stringify(draft) !== draftBase);
  /** The backend's dry run of the draft (`routines { preview }`). */
  let preview = $state<RoutinePreview | null>(null);
  let previewSeq = 0;
  const zone = deviceZone();

  $effect(() => {
    if (!draft) return;
    const input = inputOf(draft);
    const id = draft.id;
    const seq = ++previewSeq;
    const t = setTimeout(async () => {
      const r = await previewRoutine(input, id);
      if (seq === previewSeq) preview = r.ok ? r.value : null;
    }, 250);
    return () => clearTimeout(t);
  });

  /** The Account picker's value: the login the profile names, else `?`. */
  const accountPick = $derived(
    !draft || !preview ? '' : preview.logins.some((l) => (l.profile ?? '') === draft!.profile.trim()) ? draft.profile.trim() : '?',
  );
  /** The account a login bills, by its nickname or email, not the profile. */
  const accountName = (l: RoutineAccount) => routineAccountLabel({ ...l, profile: null }, $accountByUuid.get(l.account_uuid));
  const loginLabel = (l: RoutineAccount) => `${accountName(l)} · ${l.profile ? `profile ${l.profile}` : "the host's own login"}`;

  const moved = $derived(preview && draft?.trigger === 'cron' ? clockChange(preview.next_runs, preview.utc_offset_min) : null);

  /** Run once now: the saved routine as it is; a new or changed one is
   *  saved first, since a run is a saved routine's. */
  async function runOnce() {
    if (!draft) return;
    let id = draft.id;
    if (id === undefined || dirty) {
      const saved = await saveDraft();
      if (!saved) return;
      id = saved.id;
    } else {
      draft = null;
    }
    await act('Run once now', () => runRoutineNow(id!));
  }

  // ---- runs -------------------------------------------------------------------

  const clock = (s: number) => {
    const d = new Date(s * 1000);
    const today = new Date();
    const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
    return d.toDateString() === today.toDateString() ? `Today ${hm}` : `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${hm}`;
  };
  const took = (run: RoutineRunRow) => {
    if (run.finished_at === undefined) return '';
    const s = Math.max(0, run.finished_at - run.started_at);
    return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`;
  };
  const failed = (run: RoutineRunRow) => run.state === 'failed' || run.outcome === 'failed';
  const okCount = (runs: RoutineRunRow[]) => runs.filter((r) => r.state === 'done' && r.outcome !== 'failed').length;
  const failedCount = (runs: RoutineRunRow[]) => runs.filter(failed).length;

  /** A run's session, when this desktop still has it. */
  const runSession = (run: RoutineRunRow) => (run.session_id === undefined ? undefined : $sessions.find((x) => x.id === run.session_id));

  function openSession(run: RoutineRunRow) {
    const s = get(sessions).find((x) => x.id === run.session_id);
    if (!s) return;
    selectSessionExplicitly(s);
    goTo('session');
  }


  // ---- requests from elsewhere (the Inbox's Fix, the palette) -----------------

  const unsub = routinesRequest.subscribe((req) => {
    if (!req) return;
    if (req.template) newFrom(req.template === 'blank' ? null : req.template);
    if (req.select !== undefined) {
      selected = req.select;
      draft = null;
      void loadDetail();
    }
    if (req.tab) tab = req.tab;
    routinesRequest.set(null);
  });
  onDestroy(unsub);

  onMount(() => void reload());

  // Automation's own column (UX audit 2026-10-09): with `fill`, the list is
  // the page's left column and carries Automation's head and foot. `loops`
  // are the built-in routines, listed under yours.
  let {
    listHead,
    listFoot,
    fill = false,
    loops = [],
    paused = false,
    nowSec = Math.floor(Date.now() / 1000),
  }: {
    listHead?: Snippet;
    listFoot?: Snippet;
    fill?: boolean;
    loops?: LoopHealth[];
    paused?: boolean;
    nowSec?: number;
  } = $props();

  // ---- the list: filters, grouping, rows ---------------------------------------

  type Filter = 'failed' | 'working' | 'idle' | 'yours';
  const FILTERS: { id: Filter; label: string }[] = [
    { id: 'failed', label: 'Failed' },
    { id: 'working', label: 'Working' },
    { id: 'idle', label: 'Paused' },
    { id: 'yours', label: 'Yours only' },
  ];
  let filters = $state<Filter[]>([]);
  let filtersOpen = $state(false);
  let grouping = $state<'owner' | 'state'>('owner');

  interface Item {
    key: string;
    kind: 'routine' | 'loop';
    id?: number;
    name?: string;
    title: string;
    meta: string;
    line: string;
    /** Why a loop keeps running on Pause all (redesign step 8.1). */
    why?: string;
    state: OfState;
  }

  const loopState = (l: LoopHealth): OfState => (l.result === 'error' ? 'failed' : paused && l.pausable ? 'idle' : 'done');

  const routineItems = $derived(
    list.map((r): Item => {
      const last = lastRuns.get(r.id);
      return {
        key: `r${r.id}`,
        kind: 'routine',
        id: r.id,
        title: r.name,
        meta: r.enabled ? (r.trigger === 'cron' ? (cronParts(r.cron).at ?? '') : '') : 'Paused',
        line: routineLine(r, last, nowSec),
        state: routineDot(r, last),
      };
    }),
  );
  const loopItems = $derived(
    loops.map(
      (l): Item => ({
        key: `l${l.name}`,
        kind: 'loop',
        name: l.name,
        title: l.label,
        meta: paused && l.pausable ? 'Paused' : (loopEvery(l) ?? ''),
        line: `System · ${l.pausable ? 'acts on its own' : 'keeps running on Pause all'} · ${loopLine(l, nowSec, paused)}`,
        why: l.pausable ? undefined : (l.keeps_running ?? undefined),
        state: loopState(l),
      }),
    ),
  );

  const keep = (it: Item) => {
    const states: string[] = filters.filter((f) => f !== 'yours');
    if (filters.includes('yours') && it.kind === 'loop') return false;
    return states.length === 0 || states.includes(it.state);
  };

  const STATE_SECTIONS: { state: OfState; label: string }[] = [
    { state: 'failed', label: 'Failed' },
    { state: 'waiting', label: 'Needs you' },
    { state: 'working', label: 'Working' },
    { state: 'done', label: 'On' },
    { state: 'idle', label: 'Paused' },
  ];

  const sections = $derived.by(() => {
    const yours = routineItems.filter(keep);
    const builtIn = loopItems.filter(keep);
    if (grouping === 'owner') {
      return [
        { id: 'yours', label: 'Yours', items: yours },
        { id: 'built-in', label: 'Built in', items: builtIn },
      ].filter((x) => x.items.length > 0 || (x.id === 'yours' && filters.length === 0 && !loaded));
    }
    const all = [...yours, ...builtIn];
    return STATE_SECTIONS.map((x) => ({ id: x.state, label: x.label, items: all.filter((i) => i.state === x.state) })).filter(
      (x) => x.items.length > 0,
    );
  });

  function toggleFilter(f: Filter) {
    filters = filters.includes(f) ? filters.filter((x) => x !== f) : [...filters, f];
  }

  function pickLoop(name: string) {
    loopSel = name;
    selected = null;
    draft = null;
    confirmDelete = false;
    detail = null;
  }

  const loop = $derived(loopSel === null ? undefined : loops.find((l) => l.name === loopSel));

  const pauseAt = $derived(settingInt($fleetSettings, SETTING_KEYS.accountsPauseAt));
</script>

{#snippet row(it: Item)}
  <li>
    <button
      type="button"
      role="option"
      aria-selected={it.kind === 'routine' ? selected === it.id && loopSel === null : loopSel === it.name}
      class="of-row"
      title={it.line}
      data-testid={it.kind === 'routine' ? 'routine-row' : 'automation-loop'}
      data-loop={it.name}
      data-state={it.state}
      onclick={() => (it.kind === 'routine' ? pick(it.id!) : pickLoop(it.name!))}>
      <StatusDot state={it.state} />
      <span class="body">
        <span class="l1"><span class="title">{it.title}</span>{#if it.meta}<span class="meta tnum">{it.meta}</span>{/if}</span>
        <span class="line" class:f={it.state === 'failed'}>{it.line}</span>
        {#if it.why}<span class="line" data-testid="automation-loop-why">{it.why}</span>{/if}
      </span>
    </button>
  </li>
{/snippet}

<div class="routines" class:fill data-testid="routines-panel">
  <div class="list">
    {#if listHead}
      {@render listHead()}
    {:else}
      <div class="list-head">
        <span class="title">Routines <span class="count">{list.length}</span></span>
        <NewRoutineMenu onpick={(t) => newFrom(t === 'blank' ? null : t)} />
      </div>
    {/if}
    <div class="filters">
      <div class="filter-menu">
        <Button testid="routine-filters" onclick={() => (filtersOpen = !filtersOpen)}
          ><Icon name="filter" size={12} />Filters{#if filters.length}{' '}<Count n={filters.length} />{/if}</Button
        >
        {#if filtersOpen}
          <div class="menu" role="menu" data-testid="routine-filters-menu">
            {#each FILTERS as f (f.id)}
              <button type="button" role="menuitemcheckbox" aria-checked={filters.includes(f.id)} data-testid={`routine-filter-${f.id}`} onclick={() => toggleFilter(f.id)}
                ><span class="tick">{filters.includes(f.id) ? '✓' : ''}</span>{f.label}</button
              >
            {/each}
          </div>
        {/if}
      </div>
      {#each filters as id (id)}
        <span class="of-chip"
          >{FILTERS.find((f) => f.id === id)?.label}
          <button class="remove" aria-label={`Remove filter ${FILTERS.find((f) => f.id === id)?.label}`} onclick={() => toggleFilter(id)}>×</button></span
        >
      {/each}
      <span class="grow"></span>
      <Button variant="quiet" testid="routine-group" onclick={() => (grouping = grouping === 'owner' ? 'state' : 'owner')}
        >Group: {grouping} ▾</Button
      >
    </div>
    <div class="rows">
      <ul role="listbox" aria-label="Routines" data-testid="automation-routines">
        {#each sections as sec (sec.id)}
          <li class="of-sec" role="presentation" data-testid={`routine-section-${sec.id}`}>{sec.label} <Count n={sec.items.length} /></li>
          {#each sec.items as it (it.key)}{@render row(it)}{/each}
        {/each}
      </ul>
      {#if loaded && list.length === 0 && !error}
        <div class="empty" data-testid="routines-empty">
          <p>No routines yet. A routine is a saved prompt that starts a session on a schedule.</p>
          <!-- Gap plan G3.13 (board Finish, "Automation · no routines"). -->
          <div class="empty-actions">
            <Button size="sm" testid="routines-empty-new" onclick={() => newFrom(null)}>+ New routine</Button>
            <Button variant="quiet" size="sm" testid="routines-empty-template" onclick={() => newFrom('morning-pr-sweep')}
              >Use a template: Morning PR sweep</Button
            >
          </div>
        </div>
      {:else if loaded && filters.length > 0 && sections.length === 0}
        <p class="empty">Nothing matches these filters.</p>
      {/if}
      {#if error}<p class="err" role="alert" data-testid="routines-error">
          {error}
          <Button variant="quiet" size="sm" testid="routines-retry" onclick={() => void reload()}>Retry</Button>
        </p>{/if}
    </div>
    {@render listFoot?.()}
  </div>

  <div class="detail">
    {#if draft}
      <form class="editor" data-testid="routine-editor" onsubmit={(e) => (e.preventDefault(), void saveDraft())}>
        <h3>{draft.id === undefined ? 'New routine' : `Edit ${draft.name}`}</h3>
        <label>Name <input data-testid="routine-name" bind:value={draft.name} maxlength="80" /></label>
        <label
          >When
          <select data-testid="routine-trigger" bind:value={draft.trigger}>
            <option value="cron">On a schedule</option>
            <option value="event">On an event</option>
            <option value="manual">Only with Run now</option>
          </select>
        </label>
        {#if draft.trigger === 'cron'}
          <div class="pair">
            <label
              >Schedule
              <select data-testid="routine-days" bind:value={draft.days}>
                {#each SCHEDULE_DAYS as d (d.id)}<option value={d.id}>{d.label}</option>{/each}
              </select>
            </label>
            {#if draft.days === 'custom'}
              <label
                >Cron line <input data-testid="routine-cron" bind:value={draft.cron} placeholder="30 7 * * 1-5" spellcheck="false" />
              </label>
            {:else if draft.days === 'hourly'}
              <label>At minute <input data-testid="routine-time" bind:value={draft.time} placeholder="00" inputmode="numeric" /></label>
            {:else}
              <label>At <input data-testid="routine-time" type="time" bind:value={draft.time} /></label>
            {/if}
          </div>
          <p class="hint" data-testid="routine-next-run">
            {triggerWords({ trigger: 'cron', cron: cronOf(draft) })}{#if preview?.next_runs.length}{' · next run '}{nextRunLabel(preview.next_runs[0])}{:else if preview && cronOf(draft)}{' · never runs'}{/if}{#if draft.days === 'custom'}{' · minute hour day month weekday'}{/if}
          </p>
          {#if moved}
            <p class="hint warn" data-testid="routine-clock-change">
              From {nextRunLabel(moved.at)} it runs {Math.abs(moved.shiftMin) === 60 ? 'an hour' : `${Math.abs(moved.shiftMin)} minutes`}
              {moved.shiftMin > 0 ? 'later' : 'earlier'} on your clock: the clocks change and a routine keeps the UTC offset it was saved at. Save it
              again after the change.
            </p>
          {/if}
          <p class="hint" data-testid="routine-zone">
            Time zone: {zone ?? 'this device'}{zone ? ' (this device)' : ''} · saved as {offsetWords(deviceOffsetMin())}
          </p>
        {:else if draft.trigger === 'event'}
          <RoutineEventTrigger bind:event={draft.event} bind:filter={draft.filter} repos={pickable.map((p) => `${p.project.owner}/${p.project.repo}`)} />
        {/if}
        <label
          >Host
          <select data-testid="routine-host" bind:value={draft.host_alias}>
            {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}<option value={h.alias}>{h.alias}</option>{/each}
          </select>
        </label>
        <label
          >Project
          <select data-testid="routine-project" bind:value={draft.project_id}>
            {#each pickable as p (p.project.id)}<option value={String(p.project.id)}>{p.project.owner}/{p.project.repo}</option>{/each}
          </select>
        </label>
        <label
          >Account
          <select
            data-testid="routine-account"
            value={accountPick}
            onchange={(e) => {
              const v = (e.currentTarget as HTMLSelectElement).value;
              if (draft && v !== '?') draft.profile = v;
            }}>
            {#each preview?.logins ?? [] as l (l.profile ?? '')}<option value={l.profile ?? ''}>{loginLabel(l)}</option>{/each}
            {#if accountPick === '?' || !preview}<option value="?">{draft.profile.trim() ? `Profile ${draft.profile.trim()} · no known account yet` : "The host's own login · no known account yet"}</option>{/if}
          </select>
        </label>
        <label>Prompt <textarea data-testid="routine-prompt" rows="4" bind:value={draft.prompt}></textarea></label>
        <div class="pair">
          <label>Per run, $ <input data-testid="routine-budget-run" bind:value={draft.budget_run} placeholder="no limit" /></label>
          <label>Per day, $ <input data-testid="routine-budget-day" bind:value={draft.budget_day} placeholder="no limit" /></label>
        </div>
        <div class="pair">
          <label
            >Time cap, min <input data-testid="routine-cap" bind:value={draft.cap_min} placeholder="no cap" inputmode="numeric" />
          </label>
          <label
            >Fallback host
            <select data-testid="routine-fallback" bind:value={draft.fallback_host}>
              <option value="">None</option>
              {#each $hosts.filter((h) => !h.hidden && h.alias !== draft!.host_alias) as h (h.alias)}<option value={h.alias}>{h.alias}</option>{/each}
            </select>
          </label>
        </div>
        <p class="hint">A run past its time cap is stopped and fails. The fallback host takes a run when its host is unreachable, its login is past the line, or its start fails.</p>
        <label class="check"
          ><input type="checkbox" data-testid="routine-retry-once" bind:checked={draft.retry_once} /> On failure, retry once before it goes to the Inbox</label
        >
        <label
          >Autonomy
          <select data-testid="routine-autonomy" bind:value={draft.autonomy}>
            {#each AUTONOMY_CHOICES as c (c.level)}<option value={c.level}>{c.label}</option>{/each}
          </select>
          <span class="hint">Told to the agent under the prompt. Sessions run without permission prompts, so this is an instruction, not a sandbox.</span>
        </label>
        <label
          >Overlap
          <select data-testid="routine-overlap" bind:value={draft.overlap}>
            <option value="skip">Skip if the last run is still going</option>
            <option value="parallel">Run alongside it</option>
          </select>
        </label>
        <label
          >Profile <span class="hint">optional</span>
          <input data-testid="routine-profile" bind:value={draft.profile} placeholder="the host's own" spellcheck="false" />
          <span class="hint">A claude profile on the host (~/.claude-profiles); picking an Account fills it.</span>
        </label>
        {#if preview}
          {#if preview.problem}
            <p class="dry bad" data-testid="routine-dry-run" role="status">Dry run: it would not save: {preview.problem}.</p>
          {:else}
            {@const inp = inputOf(draft)}
            <p class="dry" data-testid="routine-dry-run" role="status">
              {dryRunLine(
                inp,
                projectName(inp.project_id),
                preview.account ? accountName(preview.account) : null,
              )}
            </p>
          {/if}
        {/if}
        <div class="actions">
          <Button testid="routine-run-once" disabled={!draftReady || busy || !!preview?.problem} onclick={() => void runOnce()}
            >{draft.id === undefined || dirty ? 'Save and run once' : 'Run once now'}</Button
          >
          <span class="grow"></span>
          <button type="button" class="btn btn--quiet" onclick={() => (draft = null)}>Cancel</button>
          <button type="submit" class="btn btn--primary" data-testid="routine-save" disabled={!draftReady || busy}
            >{draft.id === undefined ? 'Create routine' : 'Save'}</button
          >
        </div>
      </form>
    {:else if loop}
      {@const stands = paused && loop.pausable}
      <div class="main" data-testid="automation-loop-detail">
        <header class="head">
          <p class="kicker">Routine · built in · {loop.pausable ? 'acts on its own' : 'keeps running on Pause all'}</p>
          <div class="title-bar">
            <h3>{loop.label}</h3>
            {#if loop.result === 'error'}<StatusChip state="failed" />{:else if stands}<StatusChip state="idle" label="Paused" />{:else}<span class="state">On</span>{/if}
          </div>
        </header>
        <section class="stats">
          <div class="stat"><span class="meta">Runs</span><span>{loopEvery(loop) ?? 'On its own beat'}</span></div>
          <div class="stat"><span class="meta">Since start</span><span class="tnum">{loop.runs} runs{#if loop.failures > 0}{' · '}<span class="bad">{loop.failures} failed</span>{/if}</span></div>
          <div class="stat"><span class="meta">Now</span><span>{loopLine(loop, nowSec, paused)}</span></div>
        </section>
        {#if loop.last_error}
          <p class="prompt mono" data-testid="automation-loop-error">{loop.last_error}</p>
        {/if}
        <p class="note">
          {loop.pausable
            ? 'Pause all, at the foot of the list, stops it until you resume.'
            : `It keeps running under Pause all: ${loop.keeps_running ?? 'it only reads the fleet, it changes nothing.'}`}
        </p>
      </div>
    {:else if detail}
      {@const r = detail.routine}
      {@const today = spentSince(detail.runs, startOfToday(nowSec * 1000))}
      {@const avg = averageRun(detail.runs)}
      <div class="main">
        <header class="head">
          <p class="kicker">
            Routine{detail.may_change ? ' · yours' : ''}{detail.account ? ` · runs as ${routineAccountLabel(detail.account, $accountByUuid.get(detail.account.account_uuid))} on ${r.host_alias}` : ` · on ${r.host_alias}`}
          </p>
          <div class="title-bar">
            <h3 data-testid="routine-title">{r.name}</h3>
            <span class="state" class:off={!r.enabled} data-testid="routine-state">{routineStateWords(r)}</span>
            <span class="grow"></span>
            {#if detail.may_change}
              <Button testid="routine-edit" onclick={() => edit(r)}>Edit</Button>
              {#if r.trigger === 'cron' && r.enabled}
                <Button variant="quiet" testid="routine-skip-next" disabled={busy} onclick={() => act('Skip next', () => skipNextRun(r.id, !r.skip_next))}
                  >{r.skip_next ? 'Run next' : 'Skip next'}</Button
                >
              {/if}
              <Button testid="routine-toggle" disabled={busy} onclick={() => act(r.enabled ? 'Pause' : 'Turn on', () => setRoutineEnabled(r.id, !r.enabled))}
                >{r.enabled ? 'Pause' : 'Turn on'}</Button
              >
              <Button variant="primary" testid="routine-run-now" disabled={busy} onclick={() => act('Run now', () => runRoutineNow(r.id))}>Run now</Button>
            {/if}
          </div>
          {#if r.paused_reason}<p class="err" data-testid="routine-paused-reason">{r.paused_reason}</p>{/if}
        </header>

        <div class="tabs" role="tablist" aria-label="Routine" use:tablistKeys>
          {#each [['runs', 'Runs'], ['definition', 'Definition'], ['limits', 'Limits']] as [id, label] (id)}
            <button type="button" role="tab" aria-selected={tab === id} data-testid={`routine-tab-${id}`} onclick={() => (tab = id as RoutineTab)}
              >{label}</button
            >
          {/each}
        </div>

        {#if tab === 'runs'}
          <section class="stats" data-testid="routine-stats">
            <div class="stat"><span class="meta">Schedule</span><span>{triggerWords(r)}</span></div>
            <div class="stat">
              <span class="meta">Last {detail.runs.length} runs</span>
              <span class="tnum">{okCount(detail.runs)} OK{#if failedCount(detail.runs) > 0}{' · '}<span class="bad">{failedCount(detail.runs)} failed</span>{/if}</span>
            </div>
            <div class="stat"><span class="meta">Average cost</span><span class="tnum">{avg ?? 'No runs yet'}</span></div>
            <div class="stat"><span class="meta">Next run</span><span class="tnum">{nextRunWords(r, nowSec)}</span></div>
            <div class="stat"><span class="meta">Autonomy</span><span data-testid="routine-autonomy-stat">{autonomyWords(r.autonomy)}</span></div>
          </section>

          <section>
            <h4 class="sub">Runs <span class="meta">each run opens as a session</span></h4>
            <ul class="runs" data-testid="routine-runs">
              {#each detail.runs as run (run.id)}
                {@const s = runSession(run)}
                {@const st = runDot(run)}
                <li class:failed={failed(run)} data-testid="routine-run" data-state={run.state}>
                  <StatusDot state={st} label={runWords(run)} />
                  <span class="tnum">{clock(run.started_at)}</span>
                  <span class="what">
                    {#if failed(run)}<span class="bad">{runWords(run)}</span>
                    {:else if run.outcome === 'needs_person'}<StatusChip state="waiting" />
                    {:else if run.outcome === 'nothing'}<span class="of-chip">Nothing to do</span>
                    {:else}{runWords(run)}{/if}{#if s && !failed(run)}<span class="meta">{' · '}{s.friendly_name ?? s.tmux_name}</span>{/if}
                  </span>
                  <span class="meta tnum">{took(run)}</span>
                  <span class="meta tnum">{dollars(run.cost_micros)}</span>
                  {#if s}
                    <Button variant="quiet" size="sm" testid="routine-run-session" onclick={() => openSession(run)}>Session</Button>
                  {:else}<span></span>{/if}
                  {#if runSourceHint(run)}
                    <span class="sub-line">
                      <span class="ai" data-testid="routine-run-jev" title={runSourceHint(run)}>Read by Jev</span>
                      <span class="meta">from the run's last screen; open its session to check</span>
                    </span>
                  {:else if run.outcome === 'needs_person' && !failed(run)}
                    <span class="sub-line meta">Its session waits for you in the Inbox.</span>
                  {/if}
                  {#if failed(run) && detail.may_change}
                    {@const named = (detail.fixes ?? []).find((x) => x.run_id === run.id)}
                    <span class="sub-line">
                      <Button
                        size="sm"
                        testid="routine-run-fix"
                        onclick={() => {
                          if (named?.action === 'edit') return edit(r);
                          if (named?.action === 'retry') return void act('Retry', () => runRoutineNow(r.id));
                          if (fixRoutine({ routine: r, run, may_change: true, fix: named }) === 'definition') tab = 'definition';
                        }}>{fixLabel(named)}</Button
                      >
                      <Button size="sm" testid="routine-run-retry" disabled={busy} onclick={() => act('Retry', () => runRoutineNow(r.id))}>Retry</Button>
                      {#if r.enabled}
                        <Button variant="quiet" size="sm" testid="routine-run-pause" disabled={busy} onclick={() => act('Pause', () => setRoutineEnabled(r.id, false))}
                          >Pause routine</Button
                        >
                      {/if}
                      <details class="why">
                        <summary class="meta">Details</summary>
                        {#if run.error_code}<span class="meta mono" data-testid="routine-run-code">{run.error_code}{run.host_alias ? ` · on ${run.host_alias}` : ''}</span>{/if}
                        <span class="meta">Started by {startedByWords(run)}{run.reason ? `: ${run.reason}` : ''}</span>
                      </details>
                    </span>
                  {/if}
                </li>
              {:else}
                <li class="none meta">No runs yet. Run now starts one.</li>
              {/each}
            </ul>
          </section>

          <section>
            <h4 class="sub">Prompt</h4>
            <p class="prompt mono" data-testid="routine-prompt-text">{r.prompt}</p>
          </section>
        {:else if tab === 'definition'}
          <dl class="of-kv" data-testid="routine-definition">
            <dt>When</dt><dd>{[triggerWords(r), eventFilterWords(r)].filter(Boolean).join(' · ')}{r.trigger === 'cron' && r.time_zone ? ` · ${r.time_zone}` : ''}</dd>
            <dt>Host</dt><dd>{hostWords(r)}</dd>
            <dt>Project</dt><dd>{projectName(r.project_id)}</dd>
            <dt>Account</dt><dd>{r.profile ?? "the host's own"}</dd>
          </dl>
          <p class="prompt mono">{r.prompt}</p>
        {:else}
          <dl class="of-kv" data-testid="routine-limits">
            <dt>Per run</dt><dd>{r.budget_run_micros !== undefined ? dollars(r.budget_run_micros) : 'No limit'}</dd>
            <dt>Per day</dt><dd>{r.budget_day_micros !== undefined ? dollars(r.budget_day_micros) : 'No limit'}</dd>
            <dt>Time cap</dt><dd>{r.run_max_secs ? `${capWords(r.run_max_secs)} · then its turn is stopped and it fails` : 'No cap'}</dd>
            <dt>Overlap</dt><dd>{r.overlap === 'parallel' ? 'Runs alongside the last run' : 'Skip if the last run is still going'}</dd>
            <dt>On failure</dt><dd>{onFailureWords(r)}</dd>
            <dt>Autonomy</dt><dd>{autonomyWords(r.autonomy)}</dd>
            <dt>Over budget</dt><dd>The run fails, the routine pauses, and it lands in the Inbox</dd>
          </dl>
        {/if}
      </div>

      {#snippet perDay()}
        {#if r.budget_day_micros}
          <span class="tnum">{dollars(today)} of {dollars(r.budget_day_micros)}</span>
          <Meter
            value={today / r.budget_day_micros}
            level={today >= r.budget_day_micros ? 'crit' : today >= r.budget_day_micros * 0.8 ? 'warn' : 'ok'}
            label={`${dollars(today)} of ${dollars(r.budget_day_micros)} today`}
          />
        {:else}
          <span class="tnum">No limit · {dollars(today)} today</span>
        {/if}
      {/snippet}
      <aside class="inspector" aria-label="Limits and kill switches" data-testid="routine-inspector">
        <strong>Limits and kill switches</strong>
        <KeyValue
          items={[
            {
              label: 'Per run',
              value: [r.budget_run_micros !== undefined ? `${dollars(r.budget_run_micros)}` : 'No limit', r.run_max_secs ? capWords(r.run_max_secs) : null]
                .filter(Boolean)
                .join(' · '),
              tnum: true,
            },
            { label: 'Per day', content: perDay },
            {
              label: 'Account',
              value: `${detail.account ? routineAccountLabel(detail.account, $accountByUuid.get(detail.account.account_uuid)) : (r.profile ?? "the host's own")} · skips a run past ${pauseAt}% used`,
            },
            { label: 'Host', value: hostWords(r) },
            { label: 'Overlap', value: r.overlap === 'parallel' ? 'Runs alongside the last run' : 'Skip if the last run is still going' },
            { label: 'On failure', value: onFailureWords(r) },
            { label: 'Autonomy', value: autonomyWords(r.autonomy) },
            { label: 'Outcomes', value: OUTCOME_ROUTING },
          ]}
        />
        <div class="usage">
          <span class="of-sec flat">Counted in usage</span>
          <span class="meta">Each run is a Claude session on the account and counts against it like any other.</span>
        </div>
        {#if detail.may_change}
          <footer class="inspector-foot">
            <Button variant="quiet" testid="routine-duplicate" onclick={() => edit(r, true)}>Duplicate</Button>
            <Button variant="danger" testid="routine-delete" onclick={askDelete}>Delete routine…</Button>
            {#if confirmDelete}
              {@const lost = routineDeleteLoss(detail)}
              <DestructiveConfirm
                title={`Delete routine "${r.name}"?`}
                lead={lost.lead}
                verb="Delete routine"
                busyVerb="Deleting"
                name={r.name}
                noun="routine"
                loss={lost.loss}
                safer={r.enabled ? { label: 'Pause it instead', testid: 'routine-delete-pause', run: () => void pauseInstead(r.id) } : null}
                {busy}
                error={deleteError}
                confirmTestid="routine-delete-confirm"
                onconfirm={() => void doDelete(r.id)}
                onclose={() => (confirmDelete = false)}
              />
            {/if}
          </footer>
        {/if}
      </aside>
    {:else if loaded && list.length > 0}
      <p class="muted pick">Pick a routine.</p>
    {/if}
  </div>
</div>

<style>
  .empty-actions { display: flex; gap: var(--space-2); flex-wrap: wrap; margin-top: var(--space-2); }
  .empty p { margin: 0; }
  .routines { display: grid; grid-template-columns: minmax(220px, 300px) minmax(0, 1fr); min-height: 360px; }
  .routines.fill { grid-template-columns: var(--list-w) minmax(0, 1fr); height: 100%; min-height: 0; }
  .list { display: flex; flex-direction: column; min-height: 0; border-right: 1px solid var(--border); background: var(--bg-pane); }
  .fill .list { overflow: hidden; }
  .list-head { display: flex; align-items: center; justify-content: space-between; padding: var(--space-3) var(--space-3) var(--space-2); }
  .title { font-weight: 600; }
  .count { color: var(--fg-muted); font-weight: 400; }
  .filters { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; padding: 0 var(--space-3) var(--space-1); }
  .filter-menu { position: relative; }
  .remove { border: 0; padding: 0; background: none; color: inherit; font: inherit; cursor: pointer; }
  .menu { position: absolute; left: 0; top: calc(100% + 4px); z-index: 5; min-width: 180px; display: flex; flex-direction: column; background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); padding: 4px; box-shadow: var(--shadow-pop, 0 8px 24px rgb(0 0 0 / 0.25)); }
  .menu button { display: flex; align-items: center; gap: 6px; text-align: left; padding: 5px 8px; border: 0; background: none; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; font: inherit; font-size: var(--text-xs); }
  .menu button:hover { background: var(--bg-hover); }
  .tick { width: 12px; color: var(--accent); }
  .grow { flex: 1 1 auto; }
  .rows { flex: 1 1 auto; min-height: 0; overflow: auto; padding-bottom: var(--space-2); }
  ul { list-style: none; margin: 0; padding: 0; }
  .rows .of-row { width: calc(100% - 12px); border: 0; background: none; color: var(--fg); font: inherit; text-align: left; cursor: pointer; }
  .rows .of-row:hover { background: var(--bg-hover); }
  .rows .of-row[aria-selected='true'] { background: var(--accent-soft); }
  .of-row .body { display: flex; flex-direction: column; }
  .meta { font-size: var(--text-xs); line-height: 16px; color: var(--fg-muted); }
  .tnum { font-variant-numeric: tabular-nums; }
  .mono { font-family: var(--font-mono); font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); font-size: var(--text-xs); }
  .empty { color: var(--fg-muted); font-size: var(--text-sm); margin: var(--space-2) var(--space-3); }
  .err { color: var(--danger); font-size: var(--text-xs); margin: var(--space-2) var(--space-3); }
  .head .err { margin: var(--space-2) 0 0; }
  .detail { display: flex; min-width: 0; min-height: 0; }
  .fill .detail { overflow: hidden; }
  .main { flex: 1 1 520px; min-width: 0; overflow: auto; padding: var(--space-4) var(--space-6); display: flex; flex-direction: column; gap: var(--space-4); }
  .main > * { max-width: 780px; }
  .pick { padding: var(--space-4) var(--space-6); }
  .kicker { margin: 0; color: var(--fg-muted); font-size: var(--text-xs); }
  .title-bar { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); margin-top: 4px; }
  h3 { margin: 0; font-size: var(--text-xl); line-height: 24px; font-weight: 600; }
  .state { font-size: var(--text-2xs); font-weight: 500; padding: 0 6px; line-height: 20px; border-radius: var(--radius-sm); background: var(--done-soft); color: var(--status-done); }
  .state.off { background: var(--chip-bg); color: var(--fg-muted); }
  .tabs { display: flex; gap: 20px; border-bottom: 1px solid var(--border); margin-top: calc(-1 * var(--space-1)); }
  .tabs button { border: 0; background: none; padding: 10px 0; color: var(--fg-muted); cursor: pointer; border-bottom: 2px solid transparent; font: inherit; font-size: var(--text-sm); }
  .tabs button[aria-selected='true'] { color: var(--fg); border-bottom-color: var(--accent); font-weight: 500; }
  .stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 10px; }
  .stat { display: flex; flex-direction: column; gap: 2px; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); }
  .bad { color: var(--status-failed); }
  .sub { margin: 0 0 8px; font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); display: flex; gap: 6px; align-items: baseline; }
  .sub .meta { font-weight: 400; }
  .runs { border: 1px solid var(--border); border-radius: var(--radius-md); overflow: hidden; }
  .runs li { display: grid; grid-template-columns: 16px 110px minmax(0, 1fr) 64px 52px 64px; gap: 6px 10px; align-items: center; padding: 9px 12px; font-size: var(--text-sm); }
  .runs li + li { border-top: 1px solid var(--border); }
  .runs li:first-child { background: var(--bg-pane); }
  .runs li.none { display: block; }
  .what { min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--fg-2); }
  .what :global(.of-chip) { height: 18px; }
  .sub-line { grid-column: 2 / -1; display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
  .why { margin-left: auto; }
  .why summary { cursor: pointer; }
  .ai { display: inline-flex; align-items: center; gap: 4px; font-size: var(--text-2xs); line-height: 16px; font-weight: 500; color: var(--accent); padding: 0 6px; border-radius: var(--radius-sm); background: var(--accent-soft); }
  .ai::before { content: '✦'; font-size: var(--text-2xs); }
  .prompt { white-space: pre-wrap; margin: 0; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); color: var(--fg-2); line-height: 18px; }
  .note { margin: 0; font-size: var(--text-xs); color: var(--fg-muted); }
  .inspector { flex: 0 1 300px; min-width: 260px; max-width: var(--inspector-max, 320px); border-left: 1px solid var(--border); background: var(--bg-pane); padding: 14px 16px; display: flex; flex-direction: column; gap: 18px; overflow: auto; }
  .inspector :global(.of-kv) { grid-template-columns: 84px minmax(0, 1fr); }
  .inspector :global(.of-meter), .inspector :global([role='meter']) { margin-top: 4px; }
  .usage { display: flex; flex-direction: column; gap: 6px; }
  .of-sec.flat { padding: 0; }
  .inspector-foot { margin-top: auto; padding-top: 12px; border-top: 1px solid var(--border); display: flex; gap: 6px; flex-wrap: wrap; }
  .editor { flex: 1; min-width: 0; overflow: auto; padding: var(--space-4) var(--space-6); max-width: 720px; display: flex; flex-direction: column; gap: var(--space-2); }
  .editor h3 { font-size: var(--text-lg); }
  .editor label { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-sm); }
  .editor .hint { color: var(--fg-muted); font-size: var(--text-xs); margin: 0; }
  .editor .hint.warn { color: var(--status-waiting, var(--fg-2)); }
  .dry { margin: 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); font-size: var(--text-xs); color: var(--fg-2); }
  .dry.bad { color: var(--status-failed); }
  .editor label.check { flex-direction: row; align-items: center; gap: 6px; }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-2); }
  .actions { display: flex; justify-content: flex-end; gap: var(--space-2); }
  @media (max-width: 1180px) {
    .detail { flex-direction: column; overflow: auto; }
    .main { overflow: visible; flex: none; }
    .inspector { max-width: none; border-left: 0; border-top: 1px solid var(--border); flex: none; }
  }
</style>
