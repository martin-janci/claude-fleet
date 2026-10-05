<script lang="ts">
  import type { Snippet } from 'svelte';
  import Badge from './Badge.svelte';
  import type { ChangesetView, ItemView } from './assets_workspace';
  import { heldWords } from './assets_cards';

  /** A card in the Inspector (R12): every item by group, with per-item ✕
   *  (reject — a person's verdict: it sticks until the content changes) and
   *  "Skip this group"; an applied card's commits and Undo; a Drift card's
   *  diff (the `diff` snippet, Task 10). `onapply` and `onsynchost` are for
   *  Task 10's drift buttons and Hosts lines. */
  let {
    view,
    readOnly,
    busy,
    onreject,
    onundo,
    onreview,
    diff,
  }: {
    view: ChangesetView;
    readOnly: boolean;
    busy: boolean;
    onreject: (positions: number[]) => void;
    onapply?: (positions?: number[] | null) => void;
    onundo: () => void;
    onsynchost?: (host: string) => void;
    /** A Rollout card's Review plan: each pending host planned host-scoped, read-only. */
    onreview?: () => void;
    diff?: Snippet;
  } = $props();

  const open = $derived(view.state === 'proposed' || view.state === 'failed');
  const byGroup = $derived.by(() => {
    const m = new Map<string, ItemView[]>();
    for (const i of view.items) m.set(i.grp, [...(m.get(i.grp) ?? []), i]);
    return m;
  });
  const label = (i: ItemView) => (i.kind === 'host' ? i.name : `${i.kind}/${i.name}`);
</script>

<div class="detail">
  {#if view.kind === 'drift' && diff}{@render diff()}{/if}
  {#each [...byGroup] as [g, items] (g)}
    {@const pending = items.filter((i) => i.state === 'pending').map((i) => i.position)}
    <section class="grp">
      <header>
        <span class="sec-t">{g}</span>
        {#if open && !readOnly && pending.length && view.kind !== 'drift'}
          <button type="button" class="btn btn--quiet" data-testid={`card-skip-${view.id}-${g}`} disabled={busy} onclick={() => onreject(pending)}>Skip this group</button>
        {/if}
      </header>
      <ul>
        {#each items as i (i.position)}
          <li data-testid={`card-item-${view.id}-${i.position}`}>
            <span class="nm">{label(i)}</span>
            <Badge label={i.action.replace('_', ' ')} />
            <Badge tone="muted" label={i.decider} />
            <Badge tone={i.state === 'applied' ? 'ok' : i.state === 'rejected' ? 'muted' : i.state === 'skipped' ? 'warn' : 'neutral'} label={i.state} />
            {#if i.params.reason}<span class="why">{i.params.reason}</span>{/if}
            {#each i.outcome?.held ?? [] as l (`${l.kind}/${l.name}`)}<span class="why">{l.kind}/{l.name} — {heldWords(l.why)} · sync it yourself</span>{/each}
            {#if i.outcome?.note}<span class="why">{i.outcome.note}</span>{/if}
            {#if open && !readOnly && i.state === 'pending' && view.kind !== 'drift'}
              <button type="button" class="btn btn--quiet x" aria-label={`Reject ${label(i)}`} data-testid={`card-reject-${view.id}-${i.position}`} disabled={busy} onclick={() => onreject([i.position])}>✕</button>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/each}
  {#if open && view.kind === 'rollout' && onreview && !readOnly}
    <button type="button" class="btn" data-testid={`card-review-${view.id}`} disabled={busy} onclick={onreview}>Review plan</button>
  {/if}
  {#if Object.keys(view.commits).length}
    <p class="commits" data-testid={`card-commits-${view.id}`}>
      {#each Object.entries(view.commits) as [c, sha] (c)}<Badge mono label={`${c} ${sha.slice(0, 7)}`} />{/each}
    </p>
  {/if}
  {#if view.state === 'applied' && view.undoable && !readOnly}
    <button type="button" class="btn" disabled={busy} onclick={onundo}>Undo</button>
  {/if}
</div>

<style>
  .detail { display: grid; gap: 12px; }
  .grp header { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .sec-t { font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  ul { list-style: none; margin: 4px 0 0; padding: 0; display: grid; gap: 2px; }
  li { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 12.5px; min-height: 22px; }
  .nm { font-weight: 560; }
  .why { color: var(--fg-muted); font-size: 12px; }
  .x { margin-left: auto; }
  .commits { display: flex; gap: 4px; margin: 0; }
</style>
