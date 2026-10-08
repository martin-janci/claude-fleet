<script lang="ts">
  import type { Snippet } from 'svelte';
  import { KIND_LETTER } from './assets_visual';

  /** The lead of a selectable Assets list row — the kind mark and the name
   *  line (name, then badges, then a muted why) — shared by the Inbox's
   *  asset, card and identity rows so the look lives in one place. */
  let {
    kind,
    name,
    why = '',
    whyTitle,
    strong = false,
    children,
  }: { kind?: string; name: string; why?: string; whyTitle?: string; strong?: boolean; children?: Snippet } = $props();
</script>

{#if kind !== undefined}
  <span class="kico" title={kind} aria-hidden="true">{KIND_LETTER[kind] ?? '?'}</span>
{/if}
<span class="nm">
  <b class:strong>{name}</b>
  {@render children?.()}
  {#if why}<span class="why" title={whyTitle}>{why}</span>{/if}
</span>

<style>
  .nm { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .nm b { font-weight: 560; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .nm b.strong { font-weight: 600; }
  .why { color: var(--fg-muted); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .kico {
    display: grid; place-items: center; width: 16px; height: 16px; border-radius: var(--radius-sm);
    background: var(--control-bg-active); color: var(--control-fg-quiet); font-family: var(--mono); font-size: 11px; font-weight: 700;
  }
</style>
