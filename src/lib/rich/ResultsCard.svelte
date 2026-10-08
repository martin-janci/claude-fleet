<script lang="ts">
  // A `results` block (docs/chat-blocks.md): numbers, a chart and a table,
  // drawn with the page widgets (Chart.svelte, formatCell) from the data in
  // the block itself rather than a data source.
  import Chart from '../pages/Chart.svelte';
  import { formatCell, type Column } from '../pages/pages';
  import Markdown from '../MarkdownView.svelte';
  import type { ResultAxis, ResultCell, ResultItem, UiBlock } from '../rich_blocks';

  let { block }: { block: Extract<UiBlock, { kind: 'results' }> } = $props();

  const stats = $derived(block.items.filter((i): i is Extract<ResultItem, { type: 'stat' }> => i.type === 'stat'));
  const others = $derived(block.items.filter((i) => i.type !== 'stat'));

  const statText = (s: Extract<ResultItem, { type: 'stat' }>) =>
    typeof s.value === 'string' ? s.value : formatCell(s.ty ?? 'int', s.value);
  const column = (id: string, a: ResultAxis, fallback: Column['ty']): Column => ({ id, label: a.label, ty: a.ty ?? fallback });
  const cellText = (c: ResultCell, a: ResultAxis) =>
    typeof c === 'boolean' ? (c ? 'yes' : 'no') : formatCell(a.ty ?? (typeof c === 'number' ? 'int' : 'text'), c);
  const numeric = (a: ResultAxis, rows: ResultCell[][], k: number) =>
    a.ty ? a.ty !== 'text' && a.ty !== 'day' : rows.some((r) => typeof r[k] === 'number');
</script>

<section class="card" data-testid="rich-results" aria-label={block.title ?? 'Results'}>
  {#if block.title}<strong>{block.title}</strong>{/if}
  {#if block.summary}<Markdown source={block.summary} />{/if}
  {#if stats.length}
    <dl class="stats">
      {#each stats as s, k (k)}
        <div class="stat" data-testid="rich-results-stat">
          <dt>{s.label}</dt>
          <dd><span class="value">{statText(s)}</span>{#if s.hint}<span class="hint">{s.hint}</span>{/if}</dd>
        </div>
      {/each}
    </dl>
  {/if}
  {#each others as item, k (k)}
    {#if item.type === 'chart'}
      <Chart
        points={item.points.map(([x, y]) => ({ x, y }))}
        x={column('x', item.x, 'text')}
        y={column('y', item.y, 'int')}
        kind={item.chart}
        title={item.title}
        testid="rich-results-chart" />
    {:else if item.type === 'table'}
      <div class="table-wrap" data-testid="rich-results-table">
        {#if item.title}<span class="table-title">{item.title}</span>{/if}
        <table>
          <thead>
            <tr>
              {#each item.columns as c, j (j)}<th class:num={numeric(c, item.rows, j)}>{c.label}</th>{/each}
            </tr>
          </thead>
          <tbody>
            {#each item.rows as row, r (r)}
              <tr>
                {#each item.columns as c, j (j)}<td class:num={numeric(c, item.rows, j)}>{cellText(row[j] ?? null, c)}</td>{/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/each}
</section>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: 6px;
    background: var(--bg-pane);
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1.4rem;
    margin: 0;
  }
  .stat dt {
    color: var(--fg-muted);
    font-size: 11px;
  }
  .stat dd { margin: 0; }
  .value {
    font-size: 1.25rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .hint {
    margin-left: 0.3rem;
    color: var(--fg-muted);
    font-size: 11px;
  }
  .table-wrap { overflow-x: auto; }
  .table-title {
    display: block;
    margin-bottom: 0.25rem;
    font-weight: 600;
    font-size: 0.85em;
  }
  table {
    border-collapse: collapse;
    font-size: 0.85em;
    width: 100%;
  }
  th,
  td {
    padding: 0.2rem 0.6rem 0.2rem 0;
    text-align: left;
    border-bottom: 1px solid var(--border);
  }
  th { color: var(--fg-muted); font-weight: 500; }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
</style>
