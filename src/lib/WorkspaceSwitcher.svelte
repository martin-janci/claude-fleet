<!--
  The workspace switcher (persona design 2026-10-11): a tile at the head of
  the rail, the thing a workspace reshapes, that opens a menu of the
  workspaces. A workspace is named by its two letters and its word, never
  by a colour (amber is Needs you, the org colours are organisations).
  Switching is `switchWorkspace` (workspace.ts); the menu only picks.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import { WORKSPACES, switchWorkspace, workspace, workspacePreset, type WorkspaceId } from './workspace';

  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  let button: HTMLButtonElement | undefined = $state();

  const current = $derived(workspacePreset($workspace));

  async function toggle() {
    open = !open;
    if (open) {
      await tick();
      root?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
    }
  }

  function close(refocus = true) {
    open = false;
    if (refocus) button?.focus();
  }

  function pick(id: WorkspaceId) {
    switchWorkspace(id);
    close();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = Array.from(root?.querySelectorAll<HTMLElement>('[role=menuitemradio]') ?? []);
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    const n = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
    items[n].focus();
  }

  function onOutside(e: MouseEvent) {
    if (open && root && !root.contains(e.target as Node) && !button?.contains(e.target as Node)) close(false);
  }
</script>

<svelte:window onmousedown={onOutside} />

<div class="ws">
  <button
    bind:this={button}
    type="button"
    class="tile"
    class:open
    aria-haspopup="menu"
    aria-expanded={open}
    title="Workspace: {current.name}"
    data-testid="workspace-tile"
    onclick={toggle}
  >
    <span class="mono" aria-hidden="true">{current.mono}</span>
    <span class="short">{current.short}</span>
    <span class="sr">Workspace: {current.name}. Switch workspace</span>
  </button>
  {#if open}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div class="menu" role="menu" aria-label="Workspaces" bind:this={root} onkeydown={onKey} data-testid="workspace-menu">
      <div class="title" role="presentation">Workspace</div>
      {#each WORKSPACES as w (w.id)}
        <button
          type="button"
          role="menuitemradio"
          aria-checked={w.id === $workspace}
          class="mi"
          data-testid="workspace-{w.id}"
          onclick={() => pick(w.id)}
        >
          <span class="mono small" aria-hidden="true">{w.mono}</span>
          <span class="text"><span class="name">{w.name}</span><span class="need">{w.need}</span></span>
        </button>
      {/each}
      <div class="foot" role="presentation">Switching keeps your sessions, Needs you and access as they are.</div>
    </div>
  {/if}
</div>

<style>
  .ws {
    position: relative;
    width: var(--rail-w);
    box-sizing: border-box;
    padding: 8px 6px 4px;
    background: var(--bg-pane);
    border-right: 1px solid var(--border);
  }
  .tile {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 6px 0;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    color: var(--fg);
    font: inherit;
    font-size: var(--text-2xs);
    line-height: var(--text-2xs-lh);
    font-weight: 600;
    cursor: pointer;
  }
  .tile:hover {
    background: var(--bg-hover);
  }
  .tile.open {
    background: var(--accent-soft);
  }
  .tile:focus-visible,
  .mi:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
  .mono {
    width: 26px;
    height: 26px;
    border-radius: var(--radius-md);
    background: var(--fg);
    color: var(--bg-pane);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: var(--text-2xs);
    font-weight: 700;
    flex: 0 0 auto;
  }
  .mono.small {
    width: 24px;
    height: 24px;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  .menu {
    position: absolute;
    left: calc(var(--rail-w) - 4px);
    top: 6px;
    z-index: 20;
    width: 17rem;
    padding: 0.35rem;
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-pop);
  }
  .title {
    font-size: var(--text-2xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-muted);
    padding: 0.3rem 0.5rem 0.4rem;
  }
  .mi {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0.5rem;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .mi:hover,
  .mi[aria-checked='true'] {
    background: var(--bg-hover);
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .need {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .foot {
    border-top: 1px solid var(--border);
    margin-top: 0.3rem;
    padding: 0.45rem 0.5rem 0.2rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
