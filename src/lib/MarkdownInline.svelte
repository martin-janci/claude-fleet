<script lang="ts">
  // Renders parsed inline Markdown (markdown.ts) as Svelte elements — never
  // as HTML — so transcript text cannot inject markup.
  import { getContext } from 'svelte';
  import type { Inline } from './markdown';
  import { openExternal } from './open_external';
  import { splitPaths } from './paths';
  import { OPEN_PATH_CONTEXT, type OpenPathFn } from './app_views';
  import Self from './MarkdownInline.svelte';

  // `inLink`: a link's label must stay plain text, or a path-shaped label
  // would nest a button inside the anchor and one click would do both.
  let { nodes, inLink = false }: { nodes: Inline[]; inLink?: boolean } = $props();

  // A host that can open files (the Conversation tab) sets this; elsewhere
  // paths render as plain text.
  const openPath = getContext<OpenPathFn | undefined>(OPEN_PATH_CONTEXT);

  function onLinkClick(e: MouseEvent, href: string) {
    e.preventDefault();
    void openExternal(href);
  }
</script>

{#snippet withPaths(v: string)}{#if openPath && !inLink}{#each splitPaths(v) as piece, k (k)}{#if piece.t === 'path'}<button
        type="button"
        class="md-path"
        data-testid="md-path"
        title="Open {piece.path} in Files"
        onclick={() => openPath(piece.path, piece.line)}>{piece.v}</button
      >{:else}{piece.v}{/if}{/each}{:else}{v}{/if}{/snippet}
{#each nodes as n, i (i)}{#if n.t === 'text'}{@render withPaths(n.v)}{:else if n.t === 'code'}<code class="md-code">{@render withPaths(n.v)}</code>{:else if n.t === 'strong'}<strong><Self nodes={n.c} {inLink} /></strong>{:else if n.t === 'em'}<em><Self nodes={n.c} {inLink} /></em>{:else if n.t === 'del'}<del><Self nodes={n.c} {inLink} /></del>{:else if n.t === 'br'}<br />{:else if n.t === 'link'}{#if n.href}<a
        class="md-link"
        href={n.href}
        title={n.href}
        rel="noreferrer noopener"
        onclick={(e) => onLinkClick(e, n.href!)}
        onauxclick={(e) => e.preventDefault()}><Self nodes={n.c} inLink={true} /></a
      >{:else}<span class="md-link-inert" title="Link target not allowed"><Self nodes={n.c} inLink={true} /></span>{/if}{/if}{/each}

<style>
  .md-code {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.88em;
    padding: 0.08em 0.35em;
    border-radius: 4px;
    background: color-mix(in srgb, var(--fg) 8%, transparent);
    color: var(--syn-code);
    overflow-wrap: anywhere;
  }
  .md-link {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    overflow-wrap: anywhere;
    cursor: pointer;
  }
  .md-link-inert {
    text-decoration: underline dotted;
  }
  .md-path {
    display: inline;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    text-decoration: underline dotted;
    text-underline-offset: 2px;
    cursor: pointer;
    overflow-wrap: anywhere;
  }
  .md-path:hover {
    text-decoration-style: solid;
  }
</style>
