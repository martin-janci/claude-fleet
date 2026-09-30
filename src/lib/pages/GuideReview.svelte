<script lang="ts">
  // The Guides page (layout L6 `review_apply`, `review: guides`): guides an
  // agent proposed over the control API, waiting for a person, and the live
  // ones. A proposal is read step by step — every setting it lets a person
  // change is named with its registry label — before Approve; nothing is on
  // the pages until then.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { push, pushError } from '../toasts';
  import { ago } from './resources';
  import type { Descriptor, Item, Page, SourceSpec } from './pages';
  import {
    decideGuide,
    guideAuthor,
    guideProposals,
    guidesWritable,
    liveGuides,
    removeGuide,
    type GuideProposal,
  } from './guides';

  let {
    pages,
    descs,
    sources,
    now = () => Math.floor(Date.now() / 1000),
    onnavigate,
  }: {
    pages: Page[];
    descs: Map<string, Descriptor>;
    sources: SourceSpec[];
    now?: () => number;
    onnavigate: (pageId: string) => void;
  } = $props();

  let open = $state<Record<number, boolean>>({});
  let busy = $state(false);
  let removing = $state<Page | null>(null);

  const titleOf = (id: string) => pages.find((p) => p.id === id)?.title ?? id;

  /** One item of a step in plain words, as the reviewer reads it. */
  function words(item: Item): string {
    switch (item.type) {
      case 'field': {
        const d = descs.get(item.key);
        return `Setting: ${d?.label ?? item.key} (${item.key})${item.hint ? ` — “${item.hint}”` : ''}`;
      }
      case 'notice':
        return `Note: ${item.text}`;
      case 'link':
        return `Link: ${item.label ?? titleOf(item.page)} → ${titleOf(item.page)}`;
      case 'action':
        return `Button: ${item.action}`;
      case 'stat':
      case 'record':
        return `Shows: ${sources.find((s) => s.id === item.source.id)?.label ?? item.source.id}`;
      default:
        return item.type;
    }
  }

  async function decide(p: GuideProposal, approve: boolean) {
    busy = true;
    const r = await decideGuide(p.id, approve);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Guides');
      return;
    }
    push({ kind: 'success', message: approve ? `“${p.title}” is on the pages now.` : `Rejected “${p.title}”.` });
  }

  async function remove(page: Page) {
    removing = null;
    busy = true;
    const r = await removeGuide(page.id);
    busy = false;
    if (!r.ok) pushError(r.error, 'Guides');
  }
</script>

<div class="guides" data-testid="guide-review">
  {#if $liveGuides.length > 0}
    <section class="group">
      <h5>Live guides</h5>
      <ul>
        {#each $liveGuides as g (g.id)}
          <li class="live" data-testid={`guide-live-${g.id}`}>
            <button type="button" class="name" onclick={() => onnavigate(g.id)}>{g.title}</button>
            <span class="meta">{(g.sections ?? []).length} steps</span>
            {#if $guidesWritable}
              <button type="button" class="btn" disabled={busy} data-testid={`guide-remove-${g.id}`} onclick={() => (removing = g)}>Remove</button>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section class="group">
    <h5>Waiting for review</h5>
    {#if $guideProposals.length === 0}
      <p class="empty" data-testid="guide-review-empty">No guide is waiting. When a Claude session proposes one, it waits here.</p>
    {:else}
      {#if !$guidesWritable}
        <p class="note" data-testid="guide-review-readonly">This device reads the proposals. A person approves them on the hub, or on a device the hub’s operator trusts.</p>
      {/if}
      <ul>
        {#each $guideProposals as p (p.id)}
          <li class="row" data-testid={`guide-proposal-${p.id}`}>
            <div class="line">
              <span class="name">{p.title}</span>
              <code class="key">{p.page_id}</code>
              {#if p.replaces}<span class="tag">replaces the live one</span>{/if}
            </div>
            {#if p.page.intro}<p class="intro">{p.page.intro}</p>{/if}
            {#if p.why}<p class="why">“{p.why}”</p>{/if}
            <p class="meta">✦ proposed by {guideAuthor(p)} · {ago(p.at, now())} · {(p.page.sections ?? []).length} steps</p>
            <button type="button" class="link" data-testid={`guide-preview-${p.id}`} onclick={() => (open[p.id] = !open[p.id])}
              >{open[p.id] ? 'Hide the steps' : 'Read the steps'}</button
            >
            {#if open[p.id]}
              <ol class="preview" data-testid={`guide-steps-${p.id}`}>
                {#each p.page.sections ?? [] as s (s.title)}
                  <li>
                    <strong>{s.title}</strong>{#if s.when}<span class="meta"> (only when it applies)</span>{/if}
                    <ul>
                      {#each s.items as item, i (i)}<li>{words(item)}</li>{/each}
                    </ul>
                  </li>
                {/each}
              </ol>
            {/if}
            {#if $guidesWritable}
              <div class="actions">
                <button type="button" class="btn btn--primary" disabled={busy} data-testid={`guide-approve-${p.id}`} onclick={() => decide(p, true)}>Approve</button>
                <button type="button" class="btn" disabled={busy} data-testid={`guide-reject-${p.id}`} onclick={() => decide(p, false)}>Reject</button>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>

{#if removing}
  <ConfirmDialog
    title="Remove this guide?"
    message={`“${removing.title}” leaves the pages. The settings it changed keep their values.`}
    confirmLabel="Remove"
    danger
    confirmTestId="guide-remove-confirm"
    onconfirm={() => removing && remove(removing)}
    oncancel={() => (removing = null)} />
{/if}

<style>
  .guides {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  h5 {
    margin: 0 0 0.35rem;
    font-size: 0.75rem;
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
    gap: 0.4rem;
  }
  .live {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
  }
  .row {
    padding: 0.45rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--accent);
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
  .key,
  .meta {
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .tag {
    font-size: 0.7rem;
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    background: var(--bg);
    border: 1px solid var(--border);
  }
  .intro,
  .why,
  .meta,
  .note,
  .empty {
    margin: 0.2rem 0 0;
    font-size: 0.78rem;
  }
  .empty,
  .note {
    color: var(--fg-muted);
  }
  .link {
    background: none;
    border: none;
    padding: 0.2rem 0;
    color: var(--accent);
    font: inherit;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .link:focus-visible,
  button.name:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .preview {
    margin: 0.2rem 0 0.2rem 1.1rem;
    padding: 0;
    font-size: 0.78rem;
    line-height: 1.45;
  }
  .preview ul {
    gap: 0.1rem;
    margin-left: 0.2rem;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.4rem;
  }
</style>
