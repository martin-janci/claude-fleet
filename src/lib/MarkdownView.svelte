<script lang="ts">
  // Renders Markdown from untrusted transcript text. markdown.ts parses it
  // into a data tree; this component maps that tree onto Svelte elements, so
  // no string is ever interpreted as HTML.
  import { parseMarkdown, fenceLang, type Block } from './markdown';
  import { highlight } from './highlight';
  import { copyText } from './clipboard';
  import MarkdownInline from './MarkdownInline.svelte';
  import Self from './MarkdownView.svelte';

  let { source = '', blocks: given }: { source?: string; blocks?: Block[] } = $props();

  const blocks = $derived(given ?? parseMarkdown(source));

  // Code block index → "Copied" flash.
  let copied = $state<Set<number>>(new Set());

  async function copy(i: number, v: string) {
    if (!(await copyText(v))) return;
    copied = new Set(copied).add(i);
    setTimeout(() => {
      const next = new Set(copied);
      next.delete(i);
      copied = next;
    }, 1500);
  }
</script>

{#each blocks as b, i (i)}
  {#if b.t === 'para'}
    <p><MarkdownInline nodes={b.c} /></p>
  {:else if b.t === 'heading'}
    <svelte:element this={`h${Math.min(b.level + 2, 6)}`} class="md-h md-h{b.level}"
      ><MarkdownInline nodes={b.c} /></svelte:element
    >
  {:else if b.t === 'code'}
    <div class="md-pre-wrap">
      <div class="md-pre-bar">
        <span class="md-lang">{b.lang}</span>
        <button type="button" class="md-copy" data-testid="md-copy" onclick={() => void copy(i, b.v)}
          >{copied.has(i) ? 'Copied' : 'Copy'}</button
        >
      </div>
      <pre class="md-pre"><code
          >{#each highlight(b.v, fenceLang(b.lang)) as toks, li (li)}{#if li > 0}{'\n'}{/if}{#each toks as t, ti (ti)}{#if t.cls === 'txt'}{t.text}{:else}<span class="tok-{t.cls}">{t.text}</span>{/if}{/each}{/each}</code
        ></pre>
    </div>
  {:else if b.t === 'quote'}
    <blockquote class="md-quote"><Self blocks={b.c} /></blockquote>
  {:else if b.t === 'list'}
    {#if b.ordered}
      <ol class="md-list" start={b.start}>
        {#each b.items as item, k (k)}
          <li><Self blocks={item.c} /></li>
        {/each}
      </ol>
    {:else}
      <ul class="md-list" class:md-tasks={b.items.some((it) => it.task !== null)}>
        {#each b.items as item, k (k)}
          <li class:md-task={item.task !== null}>
            {#if item.task !== null}<input type="checkbox" checked={item.task} disabled aria-label={item.task ? 'done' : 'not done'} />{/if}<Self
              blocks={item.c}
            />
          </li>
        {/each}
      </ul>
    {/if}
  {:else if b.t === 'table'}
    <div class="md-table-wrap">
      <table class="md-table">
        <thead>
          <tr>
            {#each b.head as cell, k (k)}
              <th style:text-align={b.align[k] ?? undefined}><MarkdownInline nodes={cell} /></th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each b.rows as row, r (r)}
            <tr>
              {#each row as cell, k (k)}
                <td style:text-align={b.align[k] ?? undefined}><MarkdownInline nodes={cell} /></td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if b.t === 'hr'}
    <hr class="md-hr" />
  {/if}
{/each}

<style>
  p {
    margin: 0 0 0.6em;
  }
  p:last-child {
    margin-bottom: 0;
  }
  .md-h {
    margin: 0.9em 0 0.4em;
    line-height: 1.3;
    font-weight: 650;
    color: var(--fg);
  }
  .md-h:first-child {
    margin-top: 0;
  }
  .md-h1 { font-size: 1.25em; }
  .md-h2 { font-size: 1.12em; }
  .md-h3 { font-size: 1.02em; }
  .md-h4,
  .md-h5,
  .md-h6 { font-size: 0.95em; color: var(--fg-muted); }
  .md-pre-wrap {
    margin: 0.5em 0 0.7em;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-pane);
    overflow: hidden;
  }
  .md-pre-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.15rem 0.4rem 0.15rem 0.6rem;
    border-bottom: 1px solid var(--border);
    font-size: 0.7rem;
    color: var(--fg-muted);
  }
  .md-lang {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
  }
  .md-copy {
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--fg-muted);
    font-size: 0.7rem;
    padding: 0.05rem 0.45rem;
    cursor: pointer;
  }
  .md-copy:hover {
    color: var(--fg);
    border-color: var(--border);
  }
  .md-pre {
    margin: 0;
    padding: 0.55rem 0.7rem;
    overflow-x: auto;
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.78rem;
    line-height: 1.5;
    white-space: pre;
    tab-size: 4;
  }
  .md-pre :global(.tok-kw) { color: var(--syn-kw); }
  .md-pre :global(.tok-str) { color: var(--syn-str); }
  .md-pre :global(.tok-num) { color: var(--syn-num); }
  .md-pre :global(.tok-com) { color: var(--fg-muted); font-style: italic; }
  .md-pre :global(.tok-head) { color: var(--accent); font-weight: 700; }
  .md-pre :global(.tok-code) { color: var(--syn-code); }
  .md-quote {
    margin: 0.5em 0;
    padding: 0.1em 0 0.1em 0.8em;
    border-left: 3px solid var(--border);
    color: var(--fg-muted);
  }
  .md-list {
    margin: 0.3em 0 0.6em;
    padding-left: 1.4em;
  }
  .md-list li {
    margin: 0.15em 0;
  }
  .md-list :global(.md-list) {
    margin: 0.1em 0 0.2em;
  }
  .md-list li > :global(p) {
    margin: 0;
  }
  .md-tasks {
    list-style: none;
    padding-left: 0.3em;
  }
  .md-task {
    display: flex;
    gap: 0.45em;
    align-items: baseline;
  }
  .md-task input {
    margin: 0;
    flex: 0 0 auto;
  }
  .md-table-wrap {
    margin: 0.5em 0 0.7em;
    overflow-x: auto;
  }
  .md-table {
    border-collapse: collapse;
    font-size: 0.92em;
  }
  .md-table th,
  .md-table td {
    border: 1px solid var(--border);
    padding: 0.3em 0.6em;
    vertical-align: top;
  }
  .md-table th {
    background: var(--bg-pane);
    font-weight: 600;
    text-align: left;
  }
  .md-hr {
    border: none;
    border-top: 1px solid var(--border);
    margin: 0.9em 0;
  }
</style>
