<script lang="ts">
  // A work handover (handover.ts) as a card: the key and title, a strip of
  // counts for what is still open, then the sections in a fixed order, the
  // next steps as a numbered track and the gotchas as warnings. It is what
  // the session wrote, not a checked record, so every section is its own
  // Markdown and the source stays one click away. "Ask" only fills the
  // composer; nothing is sent from here.
  import { HANDOVER_ORDER, type Handover, type HandoverKind, type HandoverSection } from '../handover';
  import { insertIntoComposer } from '../conversation';
  import Badge from '../Badge.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Markdown from '../MarkdownView.svelte';

  let {
    handover,
    raw,
    nonce,
    sessionId = null,
  }: { handover: Handover; raw: string; nonce: string; sessionId?: number | null } = $props();

  const LABEL: Record<HandoverKind, string> = {
    work: 'What the work is',
    done: 'Done',
    left: 'Left',
    decisions: 'Decisions',
    where: 'Where things are',
    blockers: 'Blockers',
    next: 'Next steps',
    gotchas: 'Gotchas',
  };
  /** Sections that read on half the card; the rest span it. */
  const HALF: HandoverKind[] = ['work', 'decisions', 'done', 'left'];
  /** The counts the strip shows: what is still open for the next session. */
  const COUNTED: HandoverKind[] = ['left', 'blockers', 'next', 'gotchas'];

  const byKind = $derived(new Map(handover.sections.map((s) => [s.kind, s])));
  const sections = $derived(HANDOVER_ORDER.flatMap((k) => byKind.get(k) ?? []));
  const count = (s: HandoverSection) => (s.items.length > 0 ? s.items.length : s.body ? 1 : 0);
  const tiles = $derived(
    COUNTED.flatMap((k) => {
      const s = byKind.get(k);
      return s && count(s) > 0 ? [{ kind: k, n: count(s) }] : [];
    }),
  );
  const blocked = $derived((byKind.get('blockers')?.body ?? '') !== '');
  const half = (s: HandoverSection) => HALF.includes(s.kind) && halfPartner(s);
  // A half-width section with no partner on its row spans it, so no cell
  // sits beside empty space.
  function halfPartner(s: HandoverSection): boolean {
    const halves = sections.filter((x) => HALF.includes(x.kind));
    return halves.length % 2 === 0 || halves.indexOf(s) < halves.length - 1;
  }
</script>

<section class="card rich-card" class:blocked data-testid="rich-handover" aria-label="Work handover">
  <header>
    <div class="eyebrow">
      <span>Work handover</span>
      {#if handover.key}<Badge label={handover.key} tone="accent" mono testid="rich-handover-key" />{/if}
    </div>
    <div class="title-row">
      <strong class="title" data-testid="rich-handover-title">{handover.title ?? 'Hand-off for the next session'}</strong>
      {#if blocked}<Badge label="Needs you" tone="crit" glyph="■" testid="rich-handover-blocked" />{/if}
    </div>
  </header>

  {#if tiles.length > 1}
    <div class="glance" data-testid="rich-handover-glance">
      {#each tiles as t (t.kind)}
        <div class="tile {t.kind}">
          <span class="n">{t.n}</span>
          <span class="lbl">{LABEL[t.kind]}</span>
        </div>
      {/each}
    </div>
  {/if}

  {#if handover.intro}
    <div class="intro"><Markdown source={handover.intro} /></div>
  {/if}

  {#if sections.length}
    <div class="grid">
      {#each sections as s (s.kind)}
        <div class="sec {s.kind}" class:half={half(s)} data-testid="rich-handover-{s.kind}">
          <h6><span class="dot" aria-hidden="true"></span>{s.heading}</h6>
          {#if s.kind === 'next' && s.items.length}
            <ol class="steps">
              {#each s.items as item, k (k)}
                <li>
                  <span class="num" aria-hidden="true">{k + 1}</span>
                  <div class="step">
                    <Markdown source={item} />
                    {#if sessionId !== null}
                      <button
                        type="button"
                        class="ghost"
                        data-testid="rich-handover-ask"
                        title="Put this step in the composer"
                        onclick={() => sessionId !== null && insertIntoComposer(sessionId, item)}>Ask</button
                      >
                    {/if}
                  </div>
                </li>
              {/each}
            </ol>
          {:else if s.kind === 'gotchas' && s.items.length}
            <ul class="gotchas">
              {#each s.items as item, k (k)}
                <li><span class="bang" aria-hidden="true">!</span><Markdown source={item} /></li>
              {/each}
            </ul>
          {:else if s.body}
            <Markdown source={s.body} />
          {:else}
            <p class="empty">Nothing written.</p>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  <details class="raw">
    <summary>Source · {handover.words} words · {nonce}</summary>
    <div class="raw-bar"><CopyButton text={raw} label="Copy handover" /></div>
    <pre>{raw}</pre>
  </details>
</section>

<style>
  .card {
    --tone: var(--accent);
    display: flex;
    flex-direction: column;
    margin: 0.4em 0 0.7em;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    overflow: hidden;
  }
  .card > * { padding: var(--space-3) var(--space-3); }
  .card > * + * { border-top: 1px solid var(--border); }
  header { display: flex; flex-direction: column; gap: var(--space-1); }
  .eyebrow {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-2xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .title-row { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .title { font-size: var(--text-lg); line-height: var(--text-lg-lh); text-wrap: balance; }

  .glance {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(7rem, 1fr));
    padding: 0 !important;
  }
  .tile {
    display: flex;
    flex-direction: column;
    padding: var(--space-2) var(--space-3);
    border-right: 1px solid var(--border);
  }
  .tile:last-child { border-right: 0; }
  .tile .n {
    font-size: var(--text-xl);
    line-height: var(--text-xl-lh);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .tile .lbl { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .tile.blockers .n { color: var(--usage-crit); }
  .tile.gotchas .n,
  .tile.left .n { color: var(--usage-warn); }
  .tile.next .n { color: var(--accent); }

  .grid { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); padding: 0 !important; }
  .sec {
    --dot: var(--fg-muted);
    grid-column: 1 / -1;
    min-width: 0;
    padding: var(--space-3);
    border-bottom: 1px solid var(--border);
    font-size: 0.92em;
  }
  .sec:last-child { border-bottom: 0; }
  .sec.half { grid-column: auto; }
  .sec.half:nth-child(odd) { border-right: 1px solid var(--border); }
  .sec.work, .sec.left, .sec.gotchas { --dot: var(--usage-warn); }
  .sec.done { --dot: var(--usage-ok); }
  .sec.blockers { --dot: var(--usage-crit); background: color-mix(in srgb, var(--usage-crit) 5%, var(--bg-pane)); }
  .sec.decisions, .sec.next { --dot: var(--accent); }
  h6 {
    display: flex;
    align-items: center;
    gap: 0.45em;
    margin: 0 0 var(--space-1);
    font-size: var(--text-2xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--dot); flex: none; }
  .sec :global(p) { margin: 0.15em 0; }
  .empty { color: var(--fg-muted); font-style: italic; }

  .steps, .gotchas { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .steps li { position: relative; display: flex; gap: var(--space-2); padding-bottom: var(--space-3); }
  .steps li:last-child { padding-bottom: 0; }
  .steps li:not(:last-child)::after {
    content: '';
    position: absolute;
    left: 10px;
    top: 22px;
    bottom: 2px;
    width: 1.5px;
    background: var(--border);
  }
  .num {
    flex: none;
    width: 21px;
    height: 21px;
    border-radius: 50%;
    border: 1.5px solid var(--accent);
    color: var(--accent);
    background: var(--bg-pane);
    display: grid;
    place-items: center;
    font-size: var(--text-2xs);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .step { min-width: 0; flex: 1; display: flex; flex-direction: column; align-items: flex-start; gap: 2px; }
  .gotchas { gap: var(--space-1); }
  .gotchas li {
    display: flex;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--waiting-soft);
  }
  .bang { color: var(--usage-warn); font-weight: 700; flex: none; }
  .gotchas li > :global(:last-child) { min-width: 0; }

  .ghost {
    background: none;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--control-fg-quiet, var(--fg-muted));
    font-size: var(--text-2xs);
    padding: 0 0.4rem;
    cursor: pointer;
  }
  .ghost:hover { color: var(--fg); }

  .raw { background: var(--bg-raise); padding-block: var(--space-2) !important; }
  .raw summary {
    cursor: pointer;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    font-family: var(--mono, ui-monospace, monospace);
  }
  .raw-bar { display: flex; justify-content: flex-end; }
  .raw pre {
    margin: 0.2rem 0 0;
    max-height: 18rem;
    overflow: auto;
    font-size: var(--text-2xs);
    white-space: pre-wrap;
    word-break: break-word;
  }

  @media (max-width: 560px) {
    .grid { grid-template-columns: minmax(0, 1fr); }
    .sec.half:nth-child(odd) { border-right: 0; }
  }
</style>
