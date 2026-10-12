<!-- Underlined tabs for the views of one object (manual: Tabs), with
     optional counts and shortcut hints. Arrow keys, Home and End move
     between tabs; the selected one is marked by the accent bar and weight,
     not colour alone. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Count from './Count.svelte';
  import Kbd from './Kbd.svelte';

  interface TabEntry {
    id: string;
    label: string;
    count?: number;
    /** Mac chord shown after the label. */
    kbd?: string;
  }

  let {
    tabs,
    selected,
    onselect,
    label,
    mac,
    icon,
    testid,
  }: {
    tabs: TabEntry[];
    selected: string;
    onselect: (id: string) => void;
    label: string;
    mac?: boolean;
    /** A leading icon per tab (the agent's own mark on the agent tab). */
    icon?: Snippet<[string]>;
    testid?: string;
  } = $props();

  let strip = $state<HTMLElement | null>(null);

  function onkeydown(e: KeyboardEvent, i: number) {
    const last = tabs.length - 1;
    const to =
      e.key === 'ArrowRight' ? (i === last ? 0 : i + 1) : e.key === 'ArrowLeft' ? (i === 0 ? last : i - 1) : e.key === 'Home' ? 0 : e.key === 'End' ? last : -1;
    if (to < 0) return;
    e.preventDefault();
    onselect(tabs[to].id);
    strip?.querySelectorAll<HTMLElement>('[role="tab"]')[to]?.focus();
  }
</script>

<div class="of of-tabs" role="tablist" aria-label={label} bind:this={strip} data-testid={testid}>
  {#each tabs as t, i (t.id)}
    <button
      class="of-tab"
      role="tab"
      aria-selected={t.id === selected ? 'true' : 'false'}
      tabindex={t.id === selected ? 0 : -1}
      onclick={() => onselect(t.id)}
      onkeydown={(e) => onkeydown(e, i)}
      data-tab={t.id}
      >{@render icon?.(t.id)}{t.label}{#if t.count !== undefined}{' '}<Count n={t.count} />{/if}{#if t.kbd}{' '}<Kbd
          chord={t.kbd}
          {mac}
        />{/if}</button
    >
  {/each}
</div>

<style>
  .of-tab {
    background: none;
    border-top: 0;
    border-left: 0;
    border-right: 0;
    font: inherit;
    cursor: pointer;
  }
</style>
