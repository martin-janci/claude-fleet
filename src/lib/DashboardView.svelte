<!--
  Dashboards (persona design 2026-10-11, step 4): the active workspace's
  dashboard, its widgets drawn from `work_stats` (dashboards.ts). Every
  number is the app's own count of the Work view's tasks under the
  person's access; the footer says where they come from and when. The
  page re-reads when work changes (debounced) and on Refresh.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Button from './kit/Button.svelte';
  import Loader from './Loader.svelte';
  import { workspace, WORKSPACES, workspacePreset, type WorkspaceId } from './workspace';
  import { workChanged } from './work';
  import { errorText } from './error_copy';
  import {
    BUILTIN_DASHBOARDS,
    PACE_LABELS,
    STAGE_KEYS,
    STAGE_LABELS,
    bucketsShown,
    dashboardQueries,
    groupsShown,
    paceOf,
    queryKey,
    segmentsOf,
    shortDate,
    statOf,
    workStats,
    type WorkStats,
  } from './dashboards';

  // Which built-in is shown: the workspace's own, until the person picks
  // another here (kept for this mount only).
  let picked = $state<WorkspaceId | null>(null);
  const shownId = $derived(picked ?? $workspace);
  const dash = $derived(BUILTIN_DASHBOARDS[shownId]);

  let answers = $state<Record<string, WorkStats>>({});
  let errors = $state<Record<string, string>>({});
  let loading = $state(true);
  let asOf = $state<number | null>(null);
  let seq = 0;

  async function load() {
    const run = ++seq;
    const queries = dashboardQueries(dash);
    const got: Record<string, WorkStats> = {};
    const failed: Record<string, string> = {};
    await Promise.all(
      [...queries].map(async ([key, q]) => {
        const r = await workStats(q);
        if (r.ok) got[key] = r.value;
        else failed[key] = errorText(r.error);
      }),
    );
    if (run !== seq) return;
    answers = got;
    errors = failed;
    loading = false;
    const times = Object.values(got).map((s) => s.generated_at);
    asOf = times.length ? Math.min(...times) : null;
  }

  // A new dashboard reads at once; a change in work re-reads after a pause,
  // so a burst of row events is one read.
  $effect(() => {
    void dash;
    loading = true;
    void load();
  });
  let timer: ReturnType<typeof setTimeout> | null = null;
  let first = true;
  const unsub = workChanged.subscribe(() => {
    if (first) {
      first = false;
      return;
    }
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void load(), 1500);
  });
  onMount(() => () => {
    if (timer) clearTimeout(timer);
  });
  onDestroy(unsub);

  const answerOf = (q: object) => answers[queryKey(q as never)];
  const errorOf = (q: object) => errors[queryKey(q as never)];

  function clock(unix: number | null): string {
    if (unix == null) return '—';
    return new Date(unix * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }
</script>

<div class="dashboard" data-testid="dashboard-view">
  <header class="head">
    <div class="titles">
      <div class="kicker">{workspacePreset(shownId).name} · dashboard</div>
      <h1>{dash.title}</h1>
      <p class="intro">{dash.intro}</p>
    </div>
    <div class="tools">
      <label class="pick">
        <span class="sr">Dashboard</span>
        <select bind:value={() => shownId, (v) => (picked = v)} data-testid="dashboard-pick">
          {#each WORKSPACES as w (w.id)}
            <option value={w.id}>{BUILTIN_DASHBOARDS[w.id].title} · {w.short}</option>
          {/each}
        </select>
      </label>
      <Button size="sm" onclick={() => void load()}>Refresh</Button>
    </div>
  </header>

  {#if loading && Object.keys(answers).length === 0}
    <div class="loading"><Loader /></div>
  {:else}
    <div class="grid">
      {#each dash.widgets as w, i (i)}
        {@const s = answerOf(w.query)}
        {@const err = errorOf(w.query)}
        <section
          class="card"
          class:wide={w.kind !== 'stat'}
          aria-label={w.title}
          data-testid="widget-{w.kind}"
        >
          {#if err}
            <h2>{w.title}</h2>
            <p class="err">Could not read: {err}</p>
          {:else if !s}
            <h2>{w.title}</h2>
            <p class="none">—</p>
          {:else if w.kind === 'stat'}
            {@const v = statOf(s, w.value)}
            <div class="stat-label">{w.title}</div>
            <div class="stat tnum" class:warn={w.warn && v.raw > 0}>{v.value}</div>
            <div class="stat-note">{v.note}</div>
          {:else if w.kind === 'groups'}
            {@const rows = groupsShown(s, w)}
            <h2>{w.title}</h2>
            {#if rows.length === 0}
              <p class="none">Nothing here yet.</p>
            {:else}
              <ul class="rows">
                {#each rows as g (g.group.id + (g.org_id ?? ''))}
                  <li>
                    <span class="label" title={g.group.label}>{g.group.label}</span>
                    {#if w.show === 'stages'}
                      <span class="bar" role="img" aria-label={segmentsOf(g).map((x) => `${x.count} ${STAGE_LABELS[x.stage]}`).join(', ')}>
                        {#each segmentsOf(g) as seg (seg.stage)}
                          <span class="seg st-{seg.stage}" style:width="{seg.pct}%" title="{seg.count} {STAGE_LABELS[seg.stage]}"></span>
                        {/each}
                      </span>
                      <span class="num tnum">{g.total}</span>
                    {:else}
                      <span class="bar" role="img" aria-label="{g.pct_done}% done">
                        <span class="seg st-done" style:width="{g.pct_done}%"></span>
                      </span>
                      <span class="num tnum">{g.stages.done}/{g.total}</span>
                    {/if}
                  </li>
                {/each}
              </ul>
              {#if w.show === 'stages'}
                <div class="legend" aria-hidden="true">
                  {#each STAGE_KEYS as k (k)}<span><i class="sw st-{k}"></i>{STAGE_LABELS[k]}</span>{/each}
                </div>
              {/if}
            {/if}
          {:else if w.kind === 'buckets'}
            {@const rows = bucketsShown(s, w)}
            <h2>{w.title}</h2>
            {#if rows.length === 0}
              <p class="none">None {w.states ? `${w.states.join(' or ')} ` : ''}right now.</p>
            {:else}
              <ul class="buckets">
                {#each rows as g (g.group.id)}
                  {@const b = g.bucket!}
                  {@const pace = paceOf(g)}
                  <li>
                    <div class="b-head">
                      <span class="b-name">{g.group.label}</span>
                      <span class="pace pace-{pace}">{PACE_LABELS[pace]}</span>
                    </div>
                    {#if b.goal}<div class="b-goal">{b.goal}</div>{/if}
                    <div class="b-bars">
                      <span class="b-row"><span class="b-k">Done</span>
                        <span class="bar"><span class="seg st-done" style:width="{g.pct_done}%"></span></span>
                        <span class="num tnum">{g.pct_done}%</span></span>
                      {#if b.pct_time_elapsed != null}
                        <span class="b-row"><span class="b-k">Time</span>
                          <span class="bar"><span class="seg time" style:width="{b.pct_time_elapsed}%"></span></span>
                          <span class="num tnum">{b.pct_time_elapsed}%</span></span>
                      {/if}
                    </div>
                    <div class="b-meta">
                      {g.stages.done}/{g.total} done · {g.stages.blocked} blocked · {shortDate(b.starts_at)} → {shortDate(b.ends_at)}
                      {#if b.added_after_start || b.removed_after_start}
                        · scope +{b.added_after_start}/−{b.removed_after_start} since start
                      {/if}
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </section>
      {/each}
    </div>
  {/if}

  <footer class="prov">
    Built-in dashboard · counted by fleet from your Work view (work stats), only what you can open ·
    as of {clock(asOf)}{#if loading} · reading…{/if}
  </footer>
</div>

<style>
  .dashboard { display: flex; flex-direction: column; height: 100%; min-height: 0; overflow: auto; background: var(--bg); }
  .head { display: flex; align-items: flex-end; gap: var(--space-3); flex-wrap: wrap; padding: var(--space-4) var(--space-6) var(--space-2); }
  .titles { flex: 1 1 320px; min-width: 0; }
  .kicker { font-size: var(--text-xs); color: var(--fg-muted); }
  h1 { margin: 2px 0 0; font-size: var(--text-xl); line-height: var(--text-xl-lh); font-weight: 600; }
  .intro { margin: 4px 0 0; font-size: var(--text-sm); color: var(--fg-2); }
  .tools { display: flex; gap: var(--space-2); align-items: center; }
  .pick select { font: inherit; font-size: var(--text-sm); height: 28px; border: 1px solid var(--control-border); border-radius: var(--radius-sm); background: var(--bg-pane); color: var(--fg); padding: 0 6px; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .loading { display: flex; justify-content: center; padding: var(--space-6); }
  .grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: var(--space-3); padding: var(--space-2) var(--space-6) var(--space-4); }
  @media (max-width: 900px) { .grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
  @media (max-width: 560px) { .grid { grid-template-columns: minmax(0, 1fr); } }
  .card { background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); padding: var(--space-3) var(--space-4); min-width: 0; }
  .card.wide { grid-column: span 2; }
  @media (max-width: 560px) { .card.wide { grid-column: span 1; } }
  h2 { margin: 0 0 var(--space-2); font-size: var(--text-sm); font-weight: 600; }
  .stat-label { font-size: var(--text-xs); color: var(--fg-muted); }
  .stat { font-size: var(--text-xl); line-height: var(--text-xl-lh); font-weight: 600; margin-top: 2px; }
  .stat.warn { color: var(--status-waiting); }
  .stat-note { font-size: var(--text-xs); color: var(--fg-2); }
  .tnum { font-variant-numeric: tabular-nums; }
  .none, .err { margin: 0; font-size: var(--text-sm); color: var(--fg-muted); }
  .err { color: var(--status-failed); }
  .rows, .buckets { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .rows li { display: grid; grid-template-columns: minmax(0, 9rem) minmax(0, 1fr) 3.5rem; gap: 10px; align-items: center; font-size: var(--text-sm); }
  .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg-2); }
  .num { text-align: right; font-size: var(--text-xs); color: var(--fg); }
  .bar { display: flex; height: 10px; border-radius: var(--radius-pill); overflow: hidden; background: var(--track); }
  .seg { display: block; height: 100%; }
  .st-backlog { background: var(--fg-muted); }
  .st-in_progress { background: var(--status-working); }
  .st-in_review { background: var(--code); }
  .st-blocked { background: var(--status-waiting); }
  .st-done { background: var(--status-done); }
  .seg.time { background: var(--fg-muted); }
  .legend { display: flex; flex-wrap: wrap; gap: 4px 12px; margin-top: var(--space-2); font-size: var(--text-2xs); color: var(--fg-muted); }
  .legend span { display: inline-flex; align-items: center; gap: 4px; }
  .sw { display: inline-block; width: 8px; height: 8px; border-radius: var(--radius-xs); }
  .buckets li { display: flex; flex-direction: column; gap: 4px; padding-top: 6px; }
  .buckets li + li { border-top: 1px solid var(--border); padding-top: 10px; }
  .b-head { display: flex; justify-content: space-between; gap: 8px; align-items: baseline; }
  .b-name { font-size: var(--text-sm); font-weight: 600; }
  .b-goal { font-size: var(--text-xs); color: var(--fg-2); }
  .b-bars { display: flex; flex-direction: column; gap: 4px; }
  .b-row { display: grid; grid-template-columns: 2.5rem minmax(0, 1fr) 3rem; gap: 8px; align-items: center; }
  .b-k { font-size: var(--text-xs); color: var(--fg-muted); }
  .b-meta { font-size: var(--text-xs); color: var(--fg-muted); }
  .pace { font-size: var(--text-2xs); font-weight: 600; padding: 1px 8px; border-radius: var(--radius-sm); background: var(--chip-bg); color: var(--fg-2); }
  .pace-behind { background: var(--failed-soft); color: var(--status-failed); }
  .pace-ahead, .pace-on_track { background: var(--done-soft); color: var(--status-done); }
  .prov { margin-top: auto; padding: var(--space-2) var(--space-6) var(--space-3); border-top: 1px solid var(--border); font-size: var(--text-xs); color: var(--fg-muted); }
</style>
