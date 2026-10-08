<script lang="ts">
  // Layout L6 `review_apply` (declarative pages P5, design §4–5): what an
  // agent proposed, grouped by the page each setting lives on, as now →
  // proposed with who and why. A person ticks rows and applies or rejects
  // them; nothing is written before that. A row is left unticked when its
  // value moved since it was proposed, or when the setting needs
  // confirming — then Apply asks first.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { push, pushError } from '../toasts';
  import { ago } from './resources';
  import { homeOf, type Descriptor, type Page } from './pages';
  import { decideProposals, valueInWords, whoWords, type SettingProposal } from './review';

  let {
    proposals,
    pages,
    descs,
    readonly = false,
    now = () => Math.floor(Date.now() / 1000),
    onopen,
  }: {
    proposals: SettingProposal[];
    pages: Page[];
    descs: Map<string, Descriptor>;
    readonly?: boolean;
    now?: () => number;
    /** Open a setting where it lives. */
    onopen?: (pageId: string, key: string) => void;
  } = $props();

  const moved = (p: SettingProposal) => p.current !== p.before;
  const confirmed = (p: SettingProposal) => descs.get(p.key)?.danger.level === 'confirm';

  /** Ticked by default unless it moved or needs confirming; the person's
   *  own ticks override. */
  let ticks = $state<Record<number, boolean>>({});
  const ticked = (p: SettingProposal) => ticks[p.id] ?? !(moved(p) || confirmed(p));
  const selected = $derived(proposals.filter(ticked));

  const groups = $derived.by(() => {
    const out: { title: string; pageId: string | null; rows: SettingProposal[] }[] = [];
    for (const p of proposals) {
      const home = homeOf(pages, p.key);
      const title = home ? (pages.find((pg) => pg.id === home.page)?.title ?? home.page) : 'Other';
      let g = out.find((x) => x.title === title);
      if (!g) out.push((g = { title, pageId: home?.page ?? null, rows: [] }));
      g.rows.push(p);
    }
    return out;
  });

  let busy = $state(false);
  let confirming = $state<string[] | null>(null);

  async function run(accept: number[], reject: number[]) {
    busy = true;
    const r = await decideProposals(accept, reject);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Proposed changes');
      return;
    }
    const { applied, rejected, failed } = r.value;
    const parts = [];
    if (applied.length) parts.push(`applied ${applied.length}`);
    if (rejected.length) parts.push(`rejected ${rejected.length}`);
    if (parts.length) push({ kind: 'success', message: `Proposed changes: ${parts.join(', ')}` });
    for (const f of failed) {
      const p = proposals.find((x) => x.id === f.id);
      push({ kind: 'error', message: `${p ? (descs.get(p.key)?.label ?? p.key) : `#${f.id}`}: ${f.error}` });
    }
    ticks = {};
  }

  function apply() {
    const messages = selected
      .map((p) => descs.get(p.key)?.danger)
      .flatMap((d) => (d?.level === 'confirm' ? [d.message] : []));
    if (messages.length) {
      confirming = messages;
      return;
    }
    void run(
      selected.map((p) => p.id),
      [],
    );
  }
</script>

<div class="review" data-testid="review-apply">
  {#if proposals.length === 0}
    <p class="empty" data-testid="review-empty">No proposed changes. When an agent proposes one, it waits here.</p>
  {:else}
    {#each groups as g (g.title)}
      <section class="group" data-testid={`review-group-${g.title}`}>
        <h5>{g.title}</h5>
        <ul>
          {#each g.rows as p (p.id)}
            {@const d = descs.get(p.key)}
            <li class="row" class:moved={moved(p)} data-testid={`review-row-${p.key}`}>
              {#if !readonly}
                <input
                  type="checkbox"
                  aria-label={`Select ${d?.label ?? p.key}`}
                  checked={ticked(p)}
                  disabled={busy}
                  data-testid={`review-tick-${p.key}`}
                  onchange={(e) => (ticks = { ...ticks, [p.id]: (e.currentTarget as HTMLInputElement).checked })} />
              {/if}
              <div class="body">
                <div class="line">
                  {#if g.pageId && onopen}
                    <button type="button" class="name" onclick={() => onopen?.(g.pageId!, p.key)}>{d?.label ?? p.key}</button>
                  {:else}
                    <span class="name">{d?.label ?? p.key}</span>
                  {/if}
                  <code class="key">{p.key}</code>
                </div>
                <div class="diff" data-testid={`review-diff-${p.key}`}>
                  <span class="before">{valueInWords(d, p.current)}</span>
                  <span aria-hidden="true">→</span>
                  <span class="after">{valueInWords(d, p.value)}</span>
                </div>
                {#if p.current === p.value}
                  <p class="note" data-testid={`review-moved-${p.key}`}>Already set to this since it was proposed: reject it to clear it.</p>
                {:else if moved(p)}
                  <p class="note warn" data-testid={`review-moved-${p.key}`}>
                    Changed since it was proposed (it was {valueInWords(d, p.before)}).
                  </p>
                {/if}
                {#if p.why}<p class="why">“{p.why}”</p>{/if}
                <p class="meta">✦ suggested by {whoWords(p.source, p.source_detail)} · {ago(p.at, now())}</p>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
    {#if !readonly}
      <div class="actions">
        <button class="btn btn--primary" type="button" disabled={busy || selected.length === 0} data-testid="review-apply-selected" onclick={apply}
          >Apply selected ({selected.length})</button
        >
        <button
          class="btn"
          type="button"
          disabled={busy || selected.length === 0}
          data-testid="review-reject-selected"
          onclick={() => void run([], selected.map((p) => p.id))}>Reject selected</button
        >
      </div>
    {/if}
  {/if}
</div>

{#if confirming}
  <ConfirmDialog
    title="Apply proposed changes"
    message={confirming.join(' ')}
    confirmLabel="Apply"
    danger
    confirmTestId="review-confirm"
    onconfirm={() => {
      confirming = null;
      void run(
        selected.map((p) => p.id),
        [],
      );
    }}
    oncancel={() => (confirming = null)} />
{/if}

<style>
  .empty {
    font-size: 0.82rem;
    color: var(--fg-muted);
  }
  .group {
    border-top: 1px solid var(--border);
    padding: 0.5rem 0;
  }
  h5 {
    margin: 0 0 0.35rem;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    align-items: flex-start;
    padding: 0.4rem 0.5rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--accent);
  }
  .row.moved {
    border-left-color: var(--usage-warn);
  }
  .body {
    flex: 1;
    min-width: 0;
  }
  .line {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .name {
    font-weight: 600;
    font-size: 0.85rem;
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font-family: inherit;
    text-align: left;
  }
  button.name {
    cursor: pointer;
    text-decoration: underline dotted;
  }
  .key {
    font-size: 11px;
    color: var(--fg-muted);
  }
  .diff {
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    font-size: 0.82rem;
    margin-top: 0.15rem;
    overflow-wrap: anywhere;
  }
  .before {
    text-decoration: line-through;
    color: var(--fg-muted);
  }
  .after {
    font-weight: 600;
  }
  .why,
  .meta,
  .note {
    margin: 0.2rem 0 0;
    font-size: 11px;
  }
  .meta {
    color: var(--fg-muted);
  }
  .note.warn {
    color: var(--usage-warn);
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
</style>
