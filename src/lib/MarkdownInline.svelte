<script lang="ts">
  // Renders parsed inline Markdown (markdown.ts) as Svelte elements — never
  // as HTML — so transcript text cannot inject markup.
  import type { Inline } from './markdown';
  import { openExternal } from './open_external';
  import Self from './MarkdownInline.svelte';

  let { nodes }: { nodes: Inline[] } = $props();

  function onLinkClick(e: MouseEvent, href: string) {
    e.preventDefault();
    void openExternal(href);
  }
</script>

{#each nodes as n, i (i)}{#if n.t === 'text'}{n.v}{:else if n.t === 'code'}<code class="md-code">{n.v}</code>{:else if n.t === 'strong'}<strong><Self nodes={n.c} /></strong>{:else if n.t === 'em'}<em><Self nodes={n.c} /></em>{:else if n.t === 'del'}<del><Self nodes={n.c} /></del>{:else if n.t === 'br'}<br />{:else if n.t === 'link'}{#if n.href}<a
        class="md-link"
        href={n.href}
        title={n.href}
        rel="noreferrer noopener"
        onclick={(e) => onLinkClick(e, n.href!)}
        onauxclick={(e) => e.preventDefault()}><Self nodes={n.c} /></a
      >{:else}<span class="md-link-inert" title="Link target not allowed"><Self nodes={n.c} /></span>{/if}{/if}{/each}

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
</style>
