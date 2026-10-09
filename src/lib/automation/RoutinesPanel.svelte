<script lang="ts">
  // The Automation screen's Routines tab (Orbit Fleet redesign 8.6, the
  // Automation board): the routines on the left, one routine on the right
  // with its switch, Run now, Skip next and Edit, then Runs, Definition and
  // Limits. A failed run carries Fix, Retry and Pause, as in the Inbox. New
  // starts from a template (the first is Morning PR sweep) or a blank one.
  // 8.4's Automation page mounts this as its tab; the built-in routines
  // (the fleet's own loops) are 8.4's.
  import { onDestroy, onMount } from 'svelte';
  import { hosts } from '../hosts';
  import { projects } from '../projects';
  import { errorText } from '../error_copy';
  import { push, pushError } from '../toasts';
  import {
    TEMPLATES,
    deleteRoutine,
    deviceOffsetMin,
    dollars,
    eventWords,
    fixRoutine,
    getRoutine,
    listRoutines,
    loadFailing,
    microsOf,
    morningPrSweep,
    routineStateWords,
    routinesRequest,
    runRoutineNow,
    runSourceHint,
    runWords,
    saveRoutine,
    setRoutineEnabled,
    skipNextRun,
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
  let menuOpen = $state(false);
  let confirmDelete = $state(false);
  /** The editor's draft: a new routine (`id` absent) or a change. */
  let draft = $state<(Draft & { id?: number }) | null>(null);

  interface Draft {
    name: string;
    trigger: RoutineTrigger;
    cron: string;
    event: string;
    host_alias: string;
    project_id: string;
    profile: string;
    prompt: string;
    budget_run: string;
    budget_day: string;
    overlap: 'skip' | 'parallel';
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
    await loadDetail();
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

  // ---- the editor -----------------------------------------------------------

  function dollarsField(m: number | undefined): string {
    return m === undefined ? '' : (m / 1_000_000).toFixed(2);
  }

  function fromInput(i: RoutineInput, id?: number): Draft & { id?: number } {
    return {
      id,
      name: i.name,
      trigger: i.trigger,
      cron: i.cron ?? '',
      event: i.event ?? 'stuck',
      host_alias: i.host_alias,
      project_id: i.project_id ? String(i.project_id) : '',
      profile: i.profile ?? '',
      prompt: i.prompt,
      budget_run: dollarsField(i.budget_run_micros),
      budget_day: dollarsField(i.budget_day_micros),
      overlap: i.overlap === 'parallel' ? 'parallel' : 'skip',
    };
  }

  function newFrom(template: string | null) {
    menuOpen = false;
    const host = $hosts.find((h) => !h.hidden)?.alias ?? '';
    const proj = pickable[0]?.project;
    if (template === 'morning-pr-sweep') {
      draft = fromInput(morningPrSweep(host, proj?.id ?? 0, proj ? `${proj.owner}/${proj.repo}` : undefined));
    } else {
      draft = fromInput({ name: '', trigger: 'cron', cron: '0 9 * * 1-5', host_alias: host, project_id: proj?.id ?? 0, prompt: '' });
    }
  }

  function edit(r: RoutineRow) {
    draft = fromInput(
      {
        name: r.name,
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
      },
      r.id,
    );
  }

  const draftReady = $derived(
    !!draft &&
      draft.name.trim() !== '' &&
      draft.prompt.trim() !== '' &&
      draft.host_alias !== '' &&
      draft.project_id !== '' &&
      (draft.trigger !== 'cron' || draft.cron.trim() !== ''),
  );

  async function saveDraft() {
    if (!draft || !draftReady) return;
    const d = draft;
    const was = d.id !== undefined ? list.find((x) => x.id === d.id) : undefined;
    const input: RoutineInput = {
      name: d.name.trim(),
      enabled: was ? was.enabled : true,
      trigger: d.trigger,
      cron: d.trigger === 'cron' ? d.cron.trim() : undefined,
      utc_offset_min: deviceOffsetMin(),
      event: d.trigger === 'event' ? d.event : undefined,
      host_alias: d.host_alias,
      project_id: Number(d.project_id),
      profile: d.profile.trim() || undefined,
      prompt: d.prompt.trim(),
      budget_run_micros: microsOf(d.budget_run),
      budget_day_micros: microsOf(d.budget_day),
      overlap: d.overlap,
    };
    busy = true;
    const r = await saveRoutine(input, d.id);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Saving the routine failed');
      return;
    }
    draft = null;
    selected = r.value.id;
    push({ kind: 'success', message: `${r.value.name} saved` });
    await reload();
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

  // ---- requests from elsewhere (the Inbox's Fix, the palette) -----------------

  const unsub = routinesRequest.subscribe((req) => {
    if (!req) return;
    if (req.template) newFrom(req.template);
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
</script>

<div class="routines" data-testid="routines-panel">
  <div class="list">
    <div class="list-head">
      <span class="title">Routines <span class="count">{list.length}</span></span>
      <div class="new">
        <button type="button" class="btn" data-testid="routine-new" aria-expanded={menuOpen} onclick={() => (menuOpen = !menuOpen)}
          >+ New…</button
        >
        {#if menuOpen}
          <div class="menu" role="menu" data-testid="routine-new-menu">
            {#each TEMPLATES as t (t.id)}
              <button type="button" role="menuitem" data-testid={`routine-template-${t.id}`} onclick={() => newFrom(t.id)}>
                <span>{t.label}</span><span class="muted">{t.description}</span>
              </button>
            {/each}
            <button type="button" role="menuitem" data-testid="routine-template-blank" onclick={() => newFrom(null)}>
              <span>Blank routine</span><span class="muted">Your own prompt and schedule</span>
            </button>
          </div>
        {/if}
      </div>
    </div>
    <ul role="listbox" aria-label="Routines">
      {#each list as r (r.id)}
        <li>
          <button
            type="button"
            role="option"
            aria-selected={selected === r.id}
            class="row"
            data-testid="routine-row"
            onclick={() => pick(r.id)}>
            <span class="name">{r.name}</span>
            <span class="muted">{triggerWords(r)}{r.enabled ? '' : ` · ${routineStateWords(r)}`}</span>
          </button>
        </li>
      {/each}
    </ul>
    {#if loaded && list.length === 0 && !error}
      <p class="empty" data-testid="routines-empty">
        No routines yet. A routine is a saved prompt that starts a session on a schedule. Start with Morning PR sweep.
      </p>
    {/if}
    {#if error}<p class="err" role="alert" data-testid="routines-error">
        {error}
        <button type="button" class="btn btn--quiet" data-testid="routines-retry" onclick={() => void reload()}>Retry</button>
      </p>{/if}
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
            <option value="event">When a session event happens</option>
            <option value="manual">Only with Run now</option>
          </select>
        </label>
        {#if draft.trigger === 'cron'}
          <label
            >Schedule <input data-testid="routine-cron" bind:value={draft.cron} placeholder="30 7 * * 1-5" spellcheck="false" />
            <span class="hint">{triggerWords({ trigger: 'cron', cron: draft.cron })} · minute hour day month weekday, your time</span>
          </label>
        {:else if draft.trigger === 'event'}
          <label
            >Event
            <select data-testid="routine-event" bind:value={draft.event}>
              {#each ['stuck', 'lost', 'turn_done'] as e (e)}<option value={e}>A session is {eventWords(e)}</option>{/each}
            </select>
          </label>
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
          >Account (login profile) <input data-testid="routine-profile" bind:value={draft.profile} placeholder="the host's own" />
        </label>
        <label>Prompt <textarea data-testid="routine-prompt" rows="4" bind:value={draft.prompt}></textarea></label>
        <div class="pair">
          <label>Per run, $ <input data-testid="routine-budget-run" bind:value={draft.budget_run} placeholder="no limit" /></label>
          <label>Per day, $ <input data-testid="routine-budget-day" bind:value={draft.budget_day} placeholder="no limit" /></label>
        </div>
        <label
          >Overlap
          <select data-testid="routine-overlap" bind:value={draft.overlap}>
            <option value="skip">Skip if the last run is still going</option>
            <option value="parallel">Run alongside it</option>
          </select>
        </label>
        <div class="actions">
          <button type="button" class="btn btn--quiet" onclick={() => (draft = null)}>Cancel</button>
          <button type="submit" class="btn btn--primary" data-testid="routine-save" disabled={!draftReady || busy}>Save</button>
        </div>
      </form>
    {:else if detail}
      {@const r = detail.routine}
      <header>
        <p class="kicker">
          Routine{detail.account ? ` · runs as ${detail.account.login?.profile ?? detail.account.login?.account_uuid ?? 'its account'} on ${r.host_alias}` : ` · on ${r.host_alias}`}
        </p>
        <h3 data-testid="routine-title">{r.name}</h3>
        <div class="bar">
          <span class="state" class:off={!r.enabled} data-testid="routine-state">{routineStateWords(r)}</span>
          {#if detail.may_change}
            <button
              type="button"
              class="btn"
              data-testid="routine-toggle"
              disabled={busy}
              onclick={() => act(r.enabled ? 'Pause' : 'Turn on', () => setRoutineEnabled(r.id, !r.enabled))}
              >{r.enabled ? 'Pause' : 'Turn on'}</button
            >
            <button type="button" class="btn" data-testid="routine-run-now" disabled={busy} onclick={() => act('Run now', () => runRoutineNow(r.id))}
              >Run now</button
            >
            {#if r.trigger === 'cron' && r.enabled}
              <button
                type="button"
                class="btn btn--quiet"
                data-testid="routine-skip-next"
                disabled={busy}
                onclick={() => act('Skip next', () => skipNextRun(r.id, !r.skip_next))}>{r.skip_next ? 'Run next' : 'Skip next'}</button
              >
            {/if}
            <button type="button" class="btn btn--quiet" data-testid="routine-edit" onclick={() => edit(r)}>Edit…</button>
          {/if}
        </div>
        {#if r.paused_reason}<p class="err" data-testid="routine-paused-reason">{r.paused_reason}</p>{/if}
      </header>

      <div class="tabs" role="tablist">
        {#each [['runs', 'Runs'], ['definition', 'Definition'], ['limits', 'Limits']] as [id, label] (id)}
          <button
            type="button"
            role="tab"
            aria-selected={tab === id}
            data-testid={`routine-tab-${id}`}
            onclick={() => (tab = id as RoutineTab)}>{label}</button
          >
        {/each}
      </div>

      {#if tab === 'runs'}
        <dl class="facts">
          <dt>Schedule</dt><dd>{triggerWords(r)}{r.skip_next ? ' · next one skipped' : ''}</dd>
          <dt>Last {detail.runs.length} runs</dt><dd>{okCount(detail.runs)} OK · {failedCount(detail.runs)} failed</dd>
        </dl>
        <ul class="runs" data-testid="routine-runs">
          {#each detail.runs as run (run.id)}
            <li class:failed={failed(run)} data-testid="routine-run" data-state={run.state}>
              <span class="when">{clock(run.started_at)}</span>
              <span class="what"
                >{runWords(run)}{#if runSourceHint(run)}<span class="by-jev" data-testid="routine-run-jev" title={runSourceHint(run)}
                    >· Jev</span
                  >{/if}</span
              >
              <span class="muted">{took(run)}</span>
              <span class="muted">{dollars(run.cost_micros)}</span>
              {#if failed(run) && detail.may_change}
                <span class="fix">
                  <button
                    type="button"
                    class="btn btn--chip"
                    data-testid="routine-run-fix"
                    onclick={() => {
                      if (fixRoutine({ routine: r, run, may_change: true }) === 'definition') tab = 'definition';
                    }}>Fix</button
                  >
                  <button type="button" class="btn btn--chip" data-testid="routine-run-retry" disabled={busy} onclick={() => act('Retry', () => runRoutineNow(r.id))}
                    >Retry</button
                  >
                  {#if r.enabled}
                    <button
                      type="button"
                      class="btn btn--chip"
                      data-testid="routine-run-pause"
                      disabled={busy}
                      onclick={() => act('Pause', () => setRoutineEnabled(r.id, false))}>Pause routine</button
                    >
                  {/if}
                </span>
              {/if}
            </li>
          {:else}
            <li class="muted">No runs yet.</li>
          {/each}
        </ul>
      {:else if tab === 'definition'}
        <dl class="facts" data-testid="routine-definition">
          <dt>When</dt><dd>{triggerWords(r)}</dd>
          <dt>Host</dt><dd>{r.host_alias}</dd>
          <dt>Project</dt><dd>{projectName(r.project_id)}</dd>
          <dt>Account</dt><dd>{r.profile ?? "the host's own"}</dd>
        </dl>
        <p class="prompt">{r.prompt}</p>
        {#if detail.may_change}
          {#if confirmDelete}
            <p class="confirm">
              Delete {r.name}? Its runs go with it.
              <button
                type="button"
                class="btn btn--crit"
                data-testid="routine-delete-confirm"
                onclick={async () => {
                  confirmDelete = false;
                  selected = null;
                  await act('Delete', () => deleteRoutine(r.id));
                }}>Delete</button
              >
              <button type="button" class="btn btn--quiet" onclick={() => (confirmDelete = false)}>Keep</button>
            </p>
          {:else}
            <button type="button" class="btn btn--quiet" data-testid="routine-delete" onclick={() => (confirmDelete = true)}
              >Delete routine…</button
            >
          {/if}
        {/if}
      {:else}
        <dl class="facts" data-testid="routine-limits">
          <dt>Per run</dt><dd>{r.budget_run_micros !== undefined ? dollars(r.budget_run_micros) : 'No limit'}</dd>
          <dt>Per day</dt><dd>{r.budget_day_micros !== undefined ? dollars(r.budget_day_micros) : 'No limit'}</dd>
          <dt>Overlap</dt><dd>{r.overlap === 'parallel' ? 'Runs alongside the last run' : 'Skip if the last run is still going'}</dd>
          <dt>Over budget</dt><dd>The run fails, the routine pauses, and it lands in the Inbox</dd>
        </dl>
      {/if}
    {:else if loaded && list.length > 0}
      <p class="muted">Pick a routine.</p>
    {/if}
  </div>
</div>

<style>
  .routines { display: grid; grid-template-columns: minmax(200px, 260px) 1fr; gap: var(--space-4, 16px); min-height: 360px; }
  .list { display: flex; flex-direction: column; gap: var(--space-2, 8px); border-right: 1px solid var(--border); padding-right: var(--space-3, 12px); }
  .list-head { display: flex; align-items: center; justify-content: space-between; }
  .title { font-weight: 600; }
  .count { color: var(--fg-muted); font-weight: 400; }
  .new { position: relative; }
  .menu { position: absolute; right: 0; top: 100%; z-index: 5; min-width: 260px; display: flex; flex-direction: column; background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md, 8px); padding: 4px; }
  .menu button { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; text-align: left; padding: 6px 8px; border: 0; background: none; color: var(--fg); border-radius: 6px; cursor: pointer; }
  .menu button:hover { background: var(--bg-hover); }
  ul { list-style: none; margin: 0; padding: 0; }
  .row { width: 100%; display: flex; flex-direction: column; align-items: flex-start; gap: 2px; padding: 6px 8px; border: 0; border-radius: 6px; background: none; color: var(--fg); text-align: left; cursor: pointer; }
  .row[aria-selected='true'] { background: var(--accent-soft); }
  .row:hover { background: var(--bg-hover); }
  .name { font-weight: 500; }
  .muted { color: var(--fg-muted); font-size: var(--text-xs, 11.5px); }
  .empty { color: var(--fg-muted); font-size: var(--text-sm, 12.5px); }
  .err { color: var(--danger); font-size: var(--text-xs, 11.5px); margin: 0; }
  .detail { display: flex; flex-direction: column; gap: var(--space-3, 12px); min-width: 0; }
  .kicker { margin: 0; color: var(--fg-muted); font-size: var(--text-xs, 11.5px); }
  h3 { margin: 0; font-size: var(--text-lg, 15px); }
  .bar { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2, 8px); margin-top: var(--space-2, 8px); }
  .state { font-size: var(--text-xs, 11.5px); padding: 2px 8px; border-radius: 999px; background: var(--done-soft); color: var(--status-done); }
  .state.off { background: var(--bg-raise); color: var(--fg-muted); }
  .tabs { display: flex; gap: var(--space-2, 8px); border-bottom: 1px solid var(--border); }
  .tabs button { border: 0; background: none; padding: 6px 2px; color: var(--fg-muted); cursor: pointer; border-bottom: 2px solid transparent; }
  .tabs button[aria-selected='true'] { color: var(--fg); border-bottom-color: var(--accent); }
  .facts { display: grid; grid-template-columns: max-content 1fr; gap: 4px 12px; margin: 0; font-size: var(--text-sm, 12.5px); }
  .facts dt { color: var(--fg-muted); }
  .facts dd { margin: 0; }
  .runs li { display: grid; grid-template-columns: 90px 1fr auto auto; gap: 4px 12px; align-items: baseline; padding: 6px 0; border-bottom: 1px solid var(--border); font-size: var(--text-sm, 12.5px); }
  .runs li.failed .what { color: var(--status-failed); }
  .by-jev { margin-left: var(--space-1, 4px); color: var(--fg-muted); font-size: var(--text-xs, 11.5px); }
  .fix { grid-column: 2 / -1; display: flex; gap: var(--space-2, 8px); }
  .prompt { white-space: pre-wrap; margin: 0; padding: 8px; background: var(--bg-sunk); border-radius: 6px; font-size: var(--text-sm, 12.5px); }
  .confirm { display: flex; align-items: center; gap: var(--space-2, 8px); margin: 0; font-size: var(--text-sm, 12.5px); }
  .editor { display: flex; flex-direction: column; gap: var(--space-2, 8px); }
  .editor label { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-sm, 12.5px); }
  .editor .hint { color: var(--fg-muted); font-size: var(--text-xs, 11.5px); }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-2, 8px); }
  .actions { display: flex; justify-content: flex-end; gap: var(--space-2, 8px); }
</style>
