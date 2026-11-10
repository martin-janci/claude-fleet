<!--
  A dashboard's summary widget (owner report 2026-11-06): the person's
  request answered by a model from the widget's counts. It never writes on
  open: it shows the text written last on this device, with when, by which
  model on which host and what it cost; Write (the first time) and Refresh
  ask the hub for a new one (`work_link dashboard_summary`, booked as
  `dashboard_summary`). A text written for another request is not shown.
-->
<script lang="ts">
  import Loader from './Loader.svelte';
  import { errorText } from './error_copy';
  import { readPref, writePref } from './prefs';
  import { dashboardSummary, type Draft } from './drafts';
  import { exactTime, timeAgo } from './session_status';
  import { dashboardFeeds, summaryKey, type Widget } from './dashboards';
  import { summaryContext, type FleetData } from './dashboard_sources';

  let {
    dashId,
    widget,
    fleet,
    feedKeys = [],
  }: {
    dashId: string;
    widget: Extract<Widget, { kind: 'summary' }>;
    /** What the page holds of the fleet, for `sources` other than work. */
    fleet?: FleetData;
    /** The feeds the dashboard's `feed` widgets name: the ones sent. */
    feedKeys?: readonly string[];
  } = $props();

  /** What the text answered: the request, the counts' filters and the
   *  sources. A text kept for another ask is not shown. */
  type Kept = { ask: string; draft: Draft };
  const isKept = (v: unknown): v is Kept =>
    typeof v === 'object' && v !== null && typeof (v as Kept).ask === 'string' && typeof (v as Kept).draft?.text === 'string';
  const askOf = (w: typeof widget) => JSON.stringify([w.prompt, w.query ?? {}, w.sources ?? []]);

  const key = $derived(summaryKey(dashId, widget.title));
  let kept = $state<Kept | null>(null);
  $effect(() => {
    const k = readPref<Kept | null>(key, null, (v): v is Kept | null => v === null || isKept(v));
    kept = k && k.ask === askOf(widget) ? k : null;
  });

  /** A run costs cents or less: show what it cost, not $0.00. */
  const cost = (micros: number) => '$' + (micros / 1_000_000).toFixed(micros < 10_000 ? 4 : 2);

  let busy = $state(false);
  let error = $state<string | null>(null);

  async function write() {
    busy = true;
    error = null;
    const ask = askOf(widget);
    const context = fleet ? summaryContext(widget.sources, fleet, $dashboardFeeds, feedKeys) : {};
    const r = await dashboardSummary(widget.prompt, { archived: true, ...widget.query }, context);
    busy = false;
    if (!r.ok) {
      error = errorText(r.error);
      return;
    }
    kept = { ask, draft: r.value };
    writePref(key, kept);
  }
</script>

<h2 class="title">{widget.title}</h2>
<p class="ask" data-testid="summary-prompt">{widget.prompt}</p>
{#if widget.sources?.some((x) => x !== 'work')}
  <p class="reads" data-testid="summary-sources">Reads {widget.sources.join(', ')}</p>
{/if}
{#if kept}
  <p class="text" data-testid="summary-text">{kept.draft.text}</p>
  <p class="meta" data-testid="summary-meta">
    AI · <span title={exactTime(kept.draft.at)}>written {timeAgo(kept.draft.at)}</span> · {kept.draft.model} on {kept.draft.host_alias} · from {kept.draft.from}{#if kept.draft.cost_micros != null}
      · {cost(kept.draft.cost_micros)}{/if}{#if kept.draft.truncated} · cut short{/if}
  </p>
{:else if !busy}
  <p class="none">Not written yet. Write runs a model over these counts.</p>
{/if}
{#if error}<p class="err" role="alert" data-testid="summary-error">Not written: {error}</p>{/if}
<div class="act">
  {#if busy}<Loader />{/if}
  <button type="button" class="link" onclick={write} disabled={busy} data-testid="summary-write">
    {kept ? 'Refresh' : 'Write'}
  </button>
</div>

<style>
  .title { margin: 0 0 var(--space-2); font-size: var(--text-sm); font-weight: 600; }
  .ask { margin: 0; font-size: var(--text-xs); color: var(--fg-muted); font-style: italic; }
  .reads { margin: 2px 0 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .text { margin: 6px 0 0; font-size: var(--text-sm); white-space: pre-wrap; color: var(--fg); }
  .meta { margin: 6px 0 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .none { margin: 6px 0 0; font-size: var(--text-sm); color: var(--fg-muted); }
  .err { margin: 6px 0 0; font-size: var(--text-sm); color: var(--status-failed); }
  .act { display: flex; align-items: center; gap: 6px; margin-top: 6px; }
  .link { font: inherit; font-size: var(--text-sm); background: none; border: none; padding: 0; color: var(--accent, var(--fg)); cursor: pointer; }
  .link:disabled { opacity: 0.5; cursor: default; }
</style>
