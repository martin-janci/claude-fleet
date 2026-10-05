<script lang="ts">
  import Inspector from './Inspector.svelte';
  import { PERSONAL, hostProvenance, type ResolutionView } from './assets_workspace';
  import { olderHubWords } from './assets_cards';

  /** A host in the Inspector (spec, Workspace shell; R18): what it receives,
   *  and for every asset the layer that brought it and the catalog it came
   *  from ("skill/w — via layer core from personal"), then what the hub
   *  refused and which catalogs it held back. It loads its own provenance. */
  let { alias }: { alias: string } = $props();

  const TABS = [{ id: 'effective', label: 'Effective' }] as const;
  let tab = $state<string>('effective');
  let view = $state<ResolutionView | null>(null);
  let problem = $state<string | null>(null);
  let loading = $state(true);

  $effect(() => {
    const host = alias;
    let stale = false;
    view = null;
    problem = null;
    loading = true;
    void hostProvenance(host).then((r) => {
      if (stale) return;
      loading = false;
      if (r.ok) view = r.value;
      else problem = olderHubWords(r.error, 'show what a host receives') ?? r.error.message;
    });
    return () => (stale = true);
  });

  const groups = $derived.by(() => {
    const by = new Map<string, { key: string; by: string; over: string[] }[]>();
    for (const [key, p] of Object.entries(view?.provenance ?? {})) {
      const list = by.get(p.catalog) ?? [];
      list.push({ key, by: p.introduced_by, over: p.overridden_by ?? [] });
      by.set(p.catalog, list);
    }
    return [...by.entries()]
      .sort(([a], [b]) => (a === PERSONAL ? -1 : b === PERSONAL ? 1 : a.localeCompare(b)))
      .map(([catalog, lines]) => ({ catalog, lines: lines.sort((a, b) => a.key.localeCompare(b.key)) }));
  });
  const refused = $derived(view?.refused ?? []);
  const heldBack = $derived(Object.entries(view?.held_back ?? {}).sort(([a], [b]) => a.localeCompare(b)));
</script>

<Inspector eyebrow="Host" title={alias} tabs={TABS} active={tab} onchange={(id) => (tab = id)} testid="host-inspector">
  <div class="pad">
    {#if loading}
      <p class="muted">Loading…</p>
    {:else if problem}
      <p class="muted" data-testid="host-prov-error">{problem}</p>
    {:else if view}
      {#if groups.length === 0}
        <p class="muted">Nothing resolves for this host yet.</p>
      {/if}
      {#each groups as g (g.catalog)}
        <section data-testid={`host-prov-${g.catalog}`}>
          <h3 class="grp">{g.catalog} <span class="n">{g.lines.length}</span></h3>
          <ul class="list">
            {#each g.lines as l (l.key)}
              <li data-testid={`host-prov-line-${l.key}`}>
                <code>{l.key}</code> — via layer {l.by} from {g.catalog}{l.over.length ? ` (overridden by ${l.over.join(', ')})` : ''}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
      {#if refused.length}
        <section data-testid="host-refused">
          <h3 class="grp">Refused</h3>
          <ul class="list">
            {#each refused as r (`${r.catalog ?? ''}:${r.kind}/${r.name}`)}
              <li><code>{r.kind}/{r.name}</code> — {r.reason}</li>
            {/each}
          </ul>
        </section>
      {/if}
      {#if heldBack.length}
        <section data-testid="host-held-back">
          <h3 class="grp">Held back</h3>
          <ul class="list">
            {#each heldBack as [cat, why] (cat)}
              <li>{cat}: {why} — its assets are left as they are</li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}
  </div>
</Inspector>

<style>
  .pad { display: grid; gap: 10px; padding: 12px 14px; align-content: start; }
  .muted { margin: 0; color: var(--fg-muted); font-size: 12px; }
  .grp { margin: 0 0 4px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  .n { font-variant-numeric: tabular-nums; }
  code { font-family: var(--mono); font-size: 11.5px; overflow-wrap: anywhere; }
  .list { display: grid; gap: 4px; margin: 0; padding: 0; list-style: none; font-size: 12px; }
</style>
