<script lang="ts">
  // The New session picker's per-project actions (project picker spec v2):
  // ⇧F10 / right-click / ⌘G on the highlighted project. A `role=menu`;
  // focus moves in, and Esc hands it back to the switcher's input.
  import { onMount, tick, untrack } from 'svelte';

  let {
    title, pinned, hidden, groups, counts = {}, currentGroup, manualGroup, startIn,
    onpin, onhide, ongroup, onclose,
  }: {
    title: string;
    pinned: boolean;
    hidden: boolean;
    groups: readonly string[];
    /** Projects per group name (M15 G7.12: "3 projects" beside each). */
    counts?: Readonly<Record<string, number>>;
    currentGroup: string | null;
    manualGroup: boolean;
    startIn: 'main' | 'groups';
    onpin: () => void;
    onhide: () => void;
    ongroup: (name: string | null) => void;
    onclose: () => void;
  } = $props();

  let mode = $state<'main' | 'groups'>(untrack(() => startIn)); // the opening mode only
  let draft = $state('');
  let root: HTMLElement | undefined = $state();

  // Exact name first, then prefix matches, then other substring matches, so
  // an exact name is never cut by the six-row cap.
  function rank(g: string, d: string): number {
    const l = g.toLowerCase();
    const q = d.toLowerCase();
    return l === q ? 0 : l.startsWith(q) ? 1 : 2;
  }

  const shown = $derived.by(() => {
    const d = draft.trim();
    const list = groups
      .filter((g) => !d || g.toLowerCase().includes(d.toLowerCase()))
      .map((g, i) => ({ g, i, r: d ? rank(g, d) : 0 }))
      .sort((a, b) => a.r - b.r || a.i - b.i)
      .slice(0, 6)
      .map((x) => x.g);
    const out: { label: string; value: string | null; current: boolean; count?: number }[] = list.map((g) => ({
      label: g,
      value: g,
      current: g === currentGroup,
      count: counts[g],
    }));
    if (d && !groups.some((g) => g.toLowerCase() === d.toLowerCase())) out.push({ label: `New group “${d}”`, value: d, current: false });
    if (!d && manualGroup) out.push({ label: 'Back to automatic', value: null, current: false });
    return out;
  });

  // Enter: an empty draft does nothing; otherwise the existing group whose
  // name equals the draft (any case), else a new group named by the draft.
  function commitDraft() {
    const d = draft.trim();
    if (!d) return;
    const exact = groups.find((g) => g.toLowerCase() === d.toLowerCase());
    ongroup(exact ?? d);
  }

  function focusNow() {
    const el =
      mode === 'groups'
        ? root?.querySelector<HTMLElement>('input')
        : root?.querySelector<HTMLElement>('[role=menuitem]');
    el?.focus();
  }
  async function focusFirst() {
    await tick();
    focusNow();
  }
  onMount(focusNow);

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = Array.from(root?.querySelectorAll<HTMLElement>('input, [role^=menuitem]') ?? []);
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    const n = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
    items[n].focus();
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="menu" role="menu" aria-label={title} bind:this={root} onkeydown={onKey} data-testid="project-actions">
  <div class="title" role="presentation">{title}</div>
  {#if mode === 'main'}
    <button type="button" role="menuitem" class="mi" onclick={onpin}>{pinned ? 'Unpin' : 'Pin to top'}<kbd>⌘P</kbd></button>
    <button type="button" role="menuitem" class="mi" onclick={() => { mode = 'groups'; void focusFirst(); }}>Move to group…<kbd>⌘G</kbd></button>
    <button type="button" role="menuitem" class="mi" onclick={onhide}>{hidden ? 'Unhide' : 'Hide'}<kbd>⌘⌫</kbd></button>
  {:else}
    <input
      class="gi"
      type="text"
      aria-label="Group name"
      placeholder="Group name…"
      bind:value={draft}
      autocomplete="off"
      spellcheck="false"
      maxlength={40}
      onkeydown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          commitDraft();
        }
      }}
    />
    {#each shown as g (g.label)}
      <button type="button" role="menuitemradio" aria-checked={g.current} class="mi" onclick={() => ongroup(g.value)}
        >{g.label}{#if g.count}<span class="count" data-testid="project-group-count">{g.count} project{g.count === 1 ? '' : 's'}</span>{/if}</button
      >
    {/each}
    <button type="button" role="menuitem" class="mi cancel" data-testid="project-group-cancel" onclick={onclose}>Cancel<kbd>Esc</kbd></button>
  {/if}
</div>

<style>
  .menu {
    position: absolute;
    right: 1rem;
    z-index: 3;
    width: 16rem;
    padding: 0.35rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-pop);
  }
  .title { padding: 0.3rem 0.5rem; font-size: var(--text-2xs); color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mi {
    display: flex; align-items: center; width: 100%; gap: 0.5rem;
    height: var(--control-h-lg); padding: 0 0.5rem;
    border: none; background: transparent; color: var(--fg);
    font: inherit; font-size: var(--text-2xs); text-align: left; border-radius: var(--radius-sm); cursor: pointer;
  }
  .mi:hover, .mi:focus-visible { background: var(--accent-soft); }
  /* Keyboard focus keeps the ring; the tint alone is ~1.1:1 (review r11). */
  .mi:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .mi[aria-checked='true'] { font-weight: 600; }
  .count { margin-left: auto; color: var(--fg-muted); font-weight: 400; }
  .cancel { color: var(--fg-muted); }
  kbd { margin-left: auto; font: inherit; font-size: var(--text-2xs); color: var(--fg-muted); }
  .gi { width: 100%; box-sizing: border-box; margin-bottom: 0.3rem; padding: 0.3rem 0.5rem; font: inherit; font-size: var(--text-2xs); }
</style>
