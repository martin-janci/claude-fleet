<script lang="ts">
  import Badge from './Badge.svelte';
  import RowName from './RowName.svelte';
  import { isOpenCard, type ChangesetSummary, type ChangesetView, type ItemView } from './assets_workspace';
  import { cardNote, heldWords, isUndoBanner, NEEDS_A_LOOK, primaryVerb } from './assets_cards';

  /** One card (spec, Changesets; Rulings R12, R13; mockups screen 1): its
   *  sentence; a group table (Bootstrap, New, Layer) or a host table
   *  (Rollout); "needs a look" chips; one verb, Dismiss, and a note on what
   *  applying does. The verb is the region's `.btn--primary` only when the
   *  card is the selected one (`primary`, R-C: one primary per region). An
   *  applied, undoable card is a one-line banner with Undo. State is in
   *  words and glyphs, never colour alone. */
  let {
    card,
    view,
    selected,
    readOnly,
    busy,
    primary = false,
    onselect,
    onapply,
    ondismiss,
    onundo,
    onsynchost,
  }: {
    card: ChangesetSummary;
    view: ChangesetView | null;
    selected: boolean;
    readOnly: boolean;
    busy: boolean;
    /** This card's verb is the main column's one `.btn--primary`. */
    primary?: boolean;
    onselect: () => void;
    onapply: (positions?: number[] | null) => void;
    ondismiss: () => void;
    onundo: () => void;
    onsynchost?: (host: string) => void;
  } = $props();

  const key = $derived(`card:${card.id}`);
  const verb = $derived(primaryVerb(card, view));
  const failed = $derived(card.state === 'failed');
  const banner = $derived(isUndoBanner(card));
  const items = $derived(view?.items ?? []);
  const looks = $derived(items.filter((i) => i.grp === NEEDS_A_LOOK && i.state === 'pending'));

  /** Group → {count, deciders, catalogs}, in first-seen order, looks excluded. */
  const groups = $derived.by(() => {
    const out = new Map<string, { n: number; deciders: Set<string>; catalogs: Set<string> }>();
    if (!view) {
      for (const [g, n] of Object.entries(card.groups ?? {})) if (g !== NEEDS_A_LOOK) out.set(g, { n, deciders: new Set(), catalogs: new Set() });
      return out;
    }
    for (const i of items) {
      if (i.grp === NEEDS_A_LOOK || i.action === 'assign_layer') continue;
      const g = out.get(i.grp) ?? { n: 0, deciders: new Set<string>(), catalogs: new Set<string>() };
      g.n += 1;
      g.deciders.add(i.decider);
      if (i.catalog) g.catalogs.add(i.catalog);
      out.set(i.grp, g);
    }
    return out;
  });
  const hostRows = $derived(card.kind === 'rollout' ? items : ([] as ItemView[]));
  const short = (sha: string) => sha.slice(0, 7);
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<section
  class="card"
  class:selected
  class:failed
  class:banner
  aria-label={card.summary}
  aria-current={selected ? 'true' : undefined}
  data-row-key={key}
  data-testid={`card-${card.id}`}
  tabindex="0"
  onclick={(e) => { if (!(e.target as HTMLElement).closest('button')) onselect(); }}
  onkeydown={(e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) { e.preventDefault(); onselect(); } }}
>
  {#if banner}
    <div class="line">
      <span class="tick" aria-hidden="true">✓</span>
      <RowName name={`Applied: ${card.summary}`} strong />
      <span class="commits">
        {#each Object.entries(view?.commits ?? {}) as [cat, sha] (cat)}<Badge mono label={`${cat} ${short(sha)}`} />{/each}
      </span>
      {#if !readOnly}
        <button type="button" class="btn" data-testid={`card-undo-${card.id}`} disabled={busy} onclick={onundo}>Undo</button>
      {/if}
    </div>
  {:else}
    <header class="line">
      <Badge tone={failed ? 'crit' : 'accent'} glyph={failed ? '✗' : undefined} label={failed ? `${card.kind} · failed` : card.kind} />
      <RowName name={card.summary} strong />
    </header>
    {#if failed}
      <p class="err" role="alert">Failed: {card.error ?? 'unknown error'}</p>
    {/if}
    {#if card.kind === 'rollout'}
      <ul class="table" aria-label="Hosts">
        {#each hostRows as h (h.position)}
          <li class="row" data-testid={`card-host-${card.id}-${h.name}`}>
            <b>{h.name}</b>
            <Badge label={h.grp} />
            <Badge tone={h.state === 'applied' ? 'ok' : h.state === 'skipped' ? 'warn' : 'neutral'} glyph={h.state === 'applied' ? '✓' : h.state === 'skipped' ? '◐' : undefined} label={h.state} />
            {#each h.outcome?.held ?? [] as l (`${l.kind}/${l.name}`)}
              <span class="held">{l.kind}/{l.name} — {heldWords(l.why)} · sync it yourself</span>
            {/each}
            {#if h.outcome?.note}<span class="held">{h.outcome.note}</span>{/if}
            {#if (h.outcome?.held?.length ?? 0) > 0 && onsynchost && !readOnly}
              <button type="button" class="btn btn--quiet" disabled={busy} onclick={() => onsynchost(h.name)}>Sync {h.name}</button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if card.kind !== 'drift'}
      <ul class="table" aria-label="Groups">
        {#each [...groups] as [g, info] (g)}
          <li class="row">
            <b>{g}</b>
            <span class="num">{info.n}</span>
            {#if info.catalogs.size}<Badge label={[...info.catalogs].join(', ')} />{/if}
            {#if info.deciders.size}<Badge tone="muted" label={[...info.deciders].join(' + ')} />{/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if looks.length}
      <div class="looks" data-testid={`card-look-${card.id}`}>
        <span class="lbl">{looks.length} need a look</span>
        {#each looks as l (l.position)}<Badge title={l.params.reason ?? ''} label={`${l.name} · ${l.params.reason ?? 'needs a person'}`} />{/each}
      </div>
    {/if}
    {#if !readOnly && isOpenCard(card)}
      <footer class="line">
        {#if verb}
          <button
            type="button"
            class="btn"
            class:btn--primary={primary}
            data-testid={`card-primary-${card.id}`}
            disabled={busy}
            onclick={() => (verb.apply ? onapply(null) : onselect())}
          >{verb.label}</button>
        {/if}
        <button type="button" class="btn btn--quiet" data-testid={`card-dismiss-${card.id}`} disabled={busy} onclick={ondismiss}>Dismiss</button>
        {#if view}<span class="note">{cardNote(view)}</span>{/if}
      </footer>
    {/if}
  {/if}
</section>

<style>
  .card { display: grid; gap: 8px; margin: 8px 12px; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); }
  .card.selected { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
  .card:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .card.banner { padding: 6px 12px; }
  .line { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; min-width: 0; }
  .table { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
  .row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: var(--text-xs); min-height: 22px; }
  .num { font-variant-numeric: tabular-nums; color: var(--fg-muted); }
  .looks { display: flex; flex-wrap: wrap; gap: 4px; align-items: center; }
  .lbl { font-size: var(--text-xs); font-weight: 600; }
  .held { font-size: var(--text-xs); color: var(--usage-warn); }
  .err { margin: 0; color: var(--usage-crit); font-size: var(--text-xs); }
  .note { font-size: var(--text-xs); color: var(--fg-muted); }
  .tick { color: var(--usage-ok); }
  .commits { display: inline-flex; gap: 4px; }
</style>
