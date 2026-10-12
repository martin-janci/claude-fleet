<script lang="ts">
  // A master_detail page's records as a graph (`Page::graph`, the Federation
  // board): the centre node ("This hub") joined to each record, the line
  // solid while the record's state is up and dashed while it is not. The
  // picture repeats what the list beside it says, so the list stays the
  // keyboard path; the figure's label and its caption say it in words, and
  // the legend names the line style, never the colour alone.
  import { ago, choiceLabel, idOf, titleOf, type FieldSpec, type ResourceRecord, type ResourceType } from './resources';
  import type { GraphView } from './pages';

  let {
    graph,
    resource,
    records,
    selected = null,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    graph: GraphView;
    resource: ResourceType;
    records: ResourceRecord[];
    /** The record the list has selected: its node is ringed. */
    selected?: string | null;
    now?: () => number;
  } = $props();

  const W = 560;
  const H = 220;
  const CX = W / 2;
  const CY = H / 2 - 6;
  const R = 82;

  const stateField = $derived(resource.fields.find((f) => f.id === graph.state));
  const factFields = $derived(
    graph.facts.map((id) => resource.fields.find((f) => f.id === id)).filter((f): f is FieldSpec => f !== undefined),
  );

  /** One fact in words: "42 ms", "7 messages today", "last exchange 5 min ago". */
  function fact(f: FieldSpec, r: ResourceRecord): string {
    const v = r[f.id];
    if (v === null || v === undefined || v === '') return '';
    if (f.type === 'count') return `${typeof v === 'number' ? v.toLocaleString('en-US') : v} ${f.label.toLowerCase()}`;
    if (f.type === 'time') return `${f.label.toLowerCase()} ${ago(v, now())}`;
    return String(v);
  }

  const nodes = $derived(
    records.map((r, i) => {
      const angle = -Math.PI / 2 + (2 * Math.PI * i) / Math.max(1, records.length);
      const state = String(r[graph.state] ?? '');
      return {
        id: idOf(resource, r),
        title: titleOf(resource, r),
        up: graph.up.includes(state),
        state: stateField ? choiceLabel(stateField, state) : state,
        facts: factFields.map((f) => fact(f, r)).filter(Boolean).join(' · '),
        x: CX + Math.cos(angle) * R * 2.4,
        y: CY + Math.sin(angle) * R,
      };
    }),
  );

  const summary = $derived(
    nodes.length === 0
      ? `${graph.center}, linked to nothing yet.`
      : `${graph.center}, linked to ${nodes.length}: ${nodes.map((n) => `${n.title} ${n.up ? 'up' : `down (${n.state})`}`).join(', ')}.`,
  );

  /** Keep a label inside the picture: anchor it toward the centre near an edge. */
  const anchor = (x: number) => (x < W * 0.2 ? 'start' : x > W * 0.8 ? 'end' : 'middle');
</script>

{#if records.length > 0}
  <figure class="graph" data-testid="resource-graph">
    <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={summary}>
      {#each nodes as n (n.id)}
        <line
          class="link"
          class:up={n.up}
          class:down={!n.up}
          x1={CX}
          y1={CY}
          x2={n.x}
          y2={n.y}
          data-testid="graph-link"
          data-up={n.up ? 'true' : 'false'} />
      {/each}
      <circle class="center" cx={CX} cy={CY} r="9" />
      <text class="name" x={CX} y={CY + 24} text-anchor="middle">{graph.center}</text>
      {#each nodes as n (n.id)}
        <g class="node" class:sel={selected === n.id}>
          <circle cx={n.x} cy={n.y} r="7" class:down={!n.up} />
          <text class="name" x={n.x} y={n.y + 20} text-anchor={anchor(n.x)}>{n.title}</text>
          <text class="fact" x={n.x} y={n.y + 34} text-anchor={anchor(n.x)}
            >{n.up ? n.facts || n.state : n.state}</text
          >
        </g>
      {/each}
    </svg>
    <figcaption data-testid="resource-graph-legend">Solid line: link up. Dashed line: link down; its state says why.</figcaption>
  </figure>
{/if}

<style>
  .graph {
    margin: 0 0 0.75rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
  }
  svg {
    width: 100%;
    height: auto;
    display: block;
    overflow: visible;
  }
  .link {
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
  }
  .link.up {
    stroke: var(--status-done);
  }
  .link.down {
    stroke: var(--status-failed);
    stroke-dasharray: 6 4;
  }
  .center {
    fill: var(--accent);
  }
  .node circle {
    fill: var(--bg);
    stroke: var(--status-done);
    stroke-width: 2;
  }
  .node circle.down {
    stroke: var(--status-failed);
  }
  .node.sel circle {
    stroke-width: 3;
    fill: var(--accent-soft);
  }
  .name {
    fill: var(--fg);
    font-size: var(--text-2xs);
    font-weight: 500;
  }
  .fact {
    fill: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  figcaption {
    margin-top: 0.25rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
