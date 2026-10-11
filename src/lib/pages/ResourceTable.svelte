<script lang="ts">
  // A master_detail page's records as one table (M15 G4.7, board
  // OrgDevices: People & devices): the page's columns, a filter per named
  // column over the values the records hold, and an optional grouping. A
  // row opens its record; the list beside it stays the keyboard path.
  import type { TableView } from './pages';
  import { cellText, idOf, type ResourceRecord, type ResourceType } from './resources';

  let {
    resource,
    table,
    records,
    selected = null,
    onselect,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    resource: ResourceType;
    table: TableView;
    records: ResourceRecord[];
    selected?: string | null;
    onselect: (id: string) => void;
    now?: () => number;
  } = $props();

  const cols = $derived(
    table.columns.map((c) => resource.fields.find((f) => f.id === c)).filter((f): f is NonNullable<typeof f> => !!f),
  );
  const colOf = (id: string) => cols.find((f) => f.id === id);
  const cell = (id: string, r: ResourceRecord) => {
    const f = colOf(id);
    return f ? cellText(f, r, now()) : '';
  };

  /** The line under a row's title (`table.subtitle`), or nothing. */
  const sub = (r: ResourceRecord) => {
    const f = table.subtitle ? resource.fields.find((x) => x.id === table.subtitle) : undefined;
    const v = f ? cellText(f, r, now()) : '';
    return v === '—' ? '' : v;
  };

  let picked = $state<Record<string, string>>({});
  let grouped = $state(false);

  const filters = $derived(
    (table.filters ?? []).flatMap((id) => {
      const f = colOf(id);
      return f ? [{ id, label: f.label, values: [...new Set(records.map((r) => cell(id, r)))].sort() }] : [];
    }),
  );
  const shown = $derived(records.filter((r) => Object.entries(picked).every(([id, v]) => !v || cell(id, r) === v)));
  const groups = $derived.by(() => {
    const by = table.group_by;
    if (!grouped || !by) return [{ key: null as string | null, rows: shown }];
    const out = new Map<string, ResourceRecord[]>();
    for (const r of shown) {
      const k = cell(by, r);
      out.set(k, [...(out.get(k) ?? []), r]);
    }
    // Rows with no value ("—") group last.
    const rank = (k: string) => (k === '—' ? 1 : 0);
    return [...out.entries()]
      .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
      .map(([key, rows]) => ({ key, rows }));
  });
  const groupLabel = $derived(table.group_by ? (colOf(table.group_by)?.label ?? table.group_by).toLowerCase() : '');
</script>

<section class="rtable" aria-label={table.title} data-testid="resource-table">
  <h3>{table.title}</h3>
  <div class="filters">
    {#each filters as f (f.id)}
      <select
        aria-label={f.label}
        data-testid={`table-filter-${f.id}`}
        value={picked[f.id] ?? ''}
        onchange={(e) => (picked = { ...picked, [f.id]: (e.currentTarget as HTMLSelectElement).value })}>
        <option value="">Every {f.label.toLowerCase()}</option>
        {#each f.values as v (v)}<option value={v}>{v}</option>{/each}
      </select>
    {/each}
    {#if table.group_by}
      <label class="group">
        <input type="checkbox" data-testid="table-group" bind:checked={grouped} />
        Group by {groupLabel}
      </label>
    {/if}
  </div>
  <table>
    <thead>
      <tr>{#each cols as f (f.id)}<th scope="col">{f.label}</th>{/each}</tr>
    </thead>
    {#each groups as g (g.key)}
      <tbody>
        {#if g.key !== null}
          <tr class="ghead" data-testid="table-group-head"><th scope="rowgroup" colspan={cols.length}>{g.key} · {g.rows.length}</th></tr>
        {/if}
        {#each g.rows as r (idOf(resource, r))}
          <tr
            class:sel={selected === idOf(resource, r)}
            data-testid="table-row"
            onclick={() => onselect(idOf(resource, r))}>
            {#each cols as f (f.id)}
              <td
                >{cell(f.id, r)}{#if table.subtitle && f.id === resource.title_field && sub(r)}<span
                    class="sub"
                    data-testid="table-subtitle">{sub(r)}</span
                  >{/if}</td>
            {/each}
          </tr>
        {/each}
      </tbody>
    {/each}
  </table>
  {#if shown.length === 0}<p class="none">Nothing matches the filters.</p>{/if}
</section>

<style>
  .rtable {
    margin-bottom: 0.8rem;
  }
  h3 {
    font-size: var(--text-xs);
    font-weight: 600;
    margin: 0 0 0.4rem;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    margin-bottom: 0.4rem;
  }
  .group {
    font-size: var(--text-2xs);
    display: flex;
    gap: 0.25rem;
    align-items: center;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .sub {
    display: block;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  td {
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  tr[data-testid='table-row'] {
    cursor: pointer;
  }
  tr.sel {
    background: var(--accent-soft);
  }
  .ghead th {
    color: var(--fg);
    font-weight: 600;
  }
  .none {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
