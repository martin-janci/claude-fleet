<script lang="ts">
  import type { Snippet } from 'svelte';

  /** A pane, not a modal (spec, Inspector): a title, tabs, and what the
   *  active tab shows. Left/Right (and Home/End) move between tabs; only the
   *  active tab is in the tab order. Shared by the asset Inspector and, in
   *  M6, Layers and Hosts. */
  let {
    eyebrow,
    title,
    tabs,
    active,
    onchange,
    children,
    testid = 'inspector',
  }: {
    eyebrow?: string;
    title: string;
    tabs: readonly { id: string; label: string }[];
    active: string;
    onchange: (id: string) => void;
    children: Snippet;
    testid?: string;
  } = $props();

  let tablist: HTMLElement | undefined = $state();

  function onkeydown(e: KeyboardEvent) {
    if (tabs.length === 0) return;
    const i = Math.max(0, tabs.findIndex((t) => t.id === active));
    let next: number;
    if (e.key === 'ArrowRight') next = (i + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (i + tabs.length - 1) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    else return;
    e.preventDefault();
    const id = tabs[next].id;
    onchange(id);
    tablist?.querySelector<HTMLElement>(`[data-tab="${id}"]`)?.focus();
  }
</script>

<section class="insp" aria-label={title} data-testid={testid}>
  <header class="ih">
    {#if eyebrow}<span class="eyebrow">{eyebrow}</span>{/if}
    <h2 class="ititle">{title}</h2>
  </header>
  <div class="itabs" role="tablist" aria-label={`${title} views`} bind:this={tablist}>
    {#each tabs as t (t.id)}
      <button
        type="button"
        role="tab"
        id={`${testid}-tab-${t.id}`}
        data-tab={t.id}
        aria-selected={t.id === active}
        aria-controls={`${testid}-panel`}
        tabindex={t.id === active ? 0 : -1}
        class:on={t.id === active}
        onclick={() => onchange(t.id)}
        {onkeydown}
        data-testid={`inspector-tab-${t.id}`}
      >{t.label}</button>
    {/each}
  </div>
  <div class="ib" role="tabpanel" id={`${testid}-panel`} aria-labelledby={`${testid}-tab-${active}`}>
    {@render children()}
  </div>
</section>

<style>
  .insp { display: flex; flex-direction: column; min-height: 0; height: 100%; background: var(--bg); }
  .ih { display: grid; gap: 4px; padding: 12px 14px 0; }
  .eyebrow { font-size: 10.5px; font-weight: 600; letter-spacing: 0.07em; text-transform: uppercase; color: var(--fg-muted); }
  .ititle { margin: 0; font-size: 15px; font-weight: 650; letter-spacing: -0.01em; overflow-wrap: anywhere; }
  .itabs { display: flex; gap: 14px; margin-top: 8px; padding: 0 14px; border-bottom: 1px solid var(--border); }
  .itabs button {
    margin-bottom: -1px; padding: 7px 0; border: 0; border-bottom: 2px solid transparent; background: none;
    color: var(--fg-muted); font: inherit; font-size: 12px; cursor: pointer;
  }
  .itabs button.on { color: var(--fg); border-bottom-color: var(--accent); font-weight: 600; }
  .itabs button:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .ib { flex: 1; min-height: 0; overflow: auto; }
</style>
