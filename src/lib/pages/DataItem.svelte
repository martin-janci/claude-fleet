<script lang="ts">
  // A `stat`, `record`, `table` or `chart` item: reads its data source when
  // shown, again when a page action changes it or a filter changes its
  // params, and formats each value by the column type the source declares
  // (`list_pages` carries the shapes).
  import { untrack } from 'svelte';
  import Chart from './Chart.svelte';
  import { copyText } from '../clipboard';
  import {
    fetchSource,
    formatCell,
    tableText,
    type Column,
    type Item,
    type SourceRef,
    type SourceSpec,
  } from './pages';

  let {
    item,
    spec,
    source = item.source,
    copyTitle,
    tick = 0,
  }: {
    item: Extract<Item, { type: 'stat' | 'record' | 'table' | 'chart' }>;
    spec: SourceSpec | undefined;
    /** What to read: the item's source with the page's filters bound. */
    source?: SourceRef;
    /** A copied table's first line; the source's label when absent. */
    copyTitle?: string;
    /** Bumped when the page's data changed (a page action ran): re-read. */
    tick?: number;
  } = $props();

  let data = $state<unknown>(null);
  let error = $state<string | null>(null);
  let loaded = $state(false);
  let copied = $state(false);
  /** Only the newest read lands: a slow answer for an older filter never
   *  overwrites a newer one. */
  let reads = 0;

  async function read(ref: SourceRef) {
    const mine = ++reads;
    copied = false;
    const r = await fetchSource(ref);
    if (mine !== reads) return;
    loaded = true;
    error = null;
    if (r.ok) data = r.value;
    else error = r.error.message;
  }

  // Once when shown, then on every new tick or filter value — by value, so
  // a re-bound ref with the same params reads nothing.
  const readKey = $derived(`${tick}|${JSON.stringify(source)}`);
  $effect(() => {
    void readKey;
    untrack(() => void read(source));
  });

  async function copy() {
    copied = await copyText(tableText(copyTitle ?? spec?.label ?? '', tableColumns, rows));
  }

  const testid = $derived(`data-${item.type}-${item.source.id}`);
  const rows = $derived(Array.isArray(data) ? (data as Record<string, unknown>[]) : []);
  const record = $derived(
    data && typeof data === 'object' && !Array.isArray(data) ? (data as Record<string, unknown>) : {},
  );

  const statColumn = $derived.by<Column | null>(() => {
    if (item.type !== 'stat' || !spec) return null;
    if (spec.shape === 'record') return spec.fields.find((c) => c.id === item.field) ?? null;
    if (spec.shape === 'scalar') return { id: '', label: spec.label, ty: spec.ty };
    return null;
  });
  const tableColumns = $derived.by<Column[]>(() => {
    if (item.type !== 'table' || spec?.shape !== 'rows') return [];
    const want = item.columns ?? [];
    return want.length ? spec.columns.filter((c) => want.includes(c.id)) : spec.columns;
  });
</script>

{#if !spec}
  <p class="err" data-testid={testid}>Unknown data source {item.source.id}.</p>
{:else if error}
  <p class="err" role="alert" data-testid={testid}>{spec.label}: {error}</p>
{:else if item.type === 'stat'}
  <div class="stat" data-testid={testid}>
    <span class="stat-label">{item.label ?? statColumn?.label ?? spec.label}</span>
    <span class="stat-value">
      {#if !loaded}…{:else if statColumn}{formatCell(
          statColumn.ty,
          spec.shape === 'scalar' ? data : record[statColumn.id],
        )}{/if}
    </span>
  </div>
{:else if item.type === 'record' && spec.shape === 'record'}
  <dl class="record" data-testid={testid}>
    {#each spec.fields as c (c.id)}
      <dt>{c.label}</dt>
      <dd>{loaded ? formatCell(c.ty, record[c.id]) : '…'}</dd>
    {/each}
  </dl>
{:else if item.type === 'table'}
  <div class="table-wrap" data-testid={testid}>
    <div class="caption">
      <p>{spec.label}</p>
      {#if item.type === 'table' && item.copy}
        <button type="button" class="copy" data-testid={`${testid}-copy`} disabled={!loaded || rows.length === 0} onclick={() => void copy()}
          >{copied ? 'Copied' : 'Copy as text'}</button
        >
      {/if}
    </div>
    {#if loaded && rows.length === 0}
      <p class="empty">Nothing yet.</p>
    {:else}
      <table>
        <thead>
          <tr>{#each tableColumns as c (c.id)}<th class:num={c.ty !== 'text' && c.ty !== 'day'}>{c.label}</th>{/each}</tr>
        </thead>
        <tbody>
          {#each rows as row, i (i)}
            <tr>
              {#each tableColumns as c (c.id)}
                <td class:num={c.ty !== 'text' && c.ty !== 'day'}>{formatCell(c.ty, row[c.id])}</td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
{:else if item.type === 'chart' && spec.shape === 'series'}
  {#if loaded}
    <Chart
      points={rows}
      x={spec.x}
      y={spec.y[0]}
      kind={item.chart}
      title={item.title ?? `${spec.y[0].label} by ${spec.x.label.toLowerCase()}`}
      {testid} />
  {:else}
    <p class="empty" data-testid={testid}>…</p>
  {/if}
{/if}

<style>
  .stat {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    min-width: 8rem;
  }
  .stat-label {
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .stat-value {
    font-size: 1.35rem;
    font-weight: 600;
    font-variant-numeric: proportional-nums;
  }
  .record {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.15rem 0.75rem;
    font-size: 0.8rem;
    margin: 0;
  }
  .record dt {
    color: var(--fg-muted);
  }
  .record dd {
    margin: 0;
  }
  .caption {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    margin: 0 0 0.25rem;
  }
  .caption p {
    margin: 0;
  }
  .copy {
    font: inherit;
    font-size: 0.72rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.78rem;
  }
  th,
  td {
    text-align: left;
    padding: 0.2rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  th {
    color: var(--fg-muted);
    font-weight: 500;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin: 0;
  }
  .err {
    font-size: 0.78rem;
    color: var(--usage-crit);
  }
</style>
