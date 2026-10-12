<script lang="ts">
  // A session row's ⋯ menu and right-click menu (redesign step 3.10): every
  // action Details offers on the session, run by Details itself
  // (`session_actions.ts`). A `role=menu` at the pointer or under the ⋯
  // button; focus moves in, arrows move, Esc or a click outside closes it.
  import { onMount } from 'svelte';
  import type { SessionRow } from './sessions';
  import { requestSessionAction, sessionMenuItems, type SessionActionId } from './session_actions';

  let {
    session,
    x,
    y,
    onclose,
  }: { session: SessionRow; x: number; y: number; onclose: () => void } = $props();

  let root: HTMLElement | undefined = $state();
  const items = $derived($sessionMenuItems(session));

  // Kept inside the window: a row near the bottom opens the menu upwards.
  let pos = $state({ left: 0, top: 0 });
  function place() {
    const w = root?.offsetWidth ?? 0;
    const h = root?.offsetHeight ?? 0;
    pos = {
      left: Math.max(4, Math.min(x, window.innerWidth - w - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - h - 4)),
    };
  }

  onMount(() => {
    place();
    root?.querySelector<HTMLElement>('[role=menuitem]:not(:disabled)')?.focus();
    const away = (e: PointerEvent) => {
      if (root && !root.contains(e.target as Node)) onclose();
    };
    // Next tick: the click that opened the menu must not close it.
    const t = setTimeout(() => window.addEventListener('pointerdown', away, true));
    return () => {
      clearTimeout(t);
      window.removeEventListener('pointerdown', away, true);
    };
  });

  function run(action: SessionActionId | 'details') {
    onclose();
    requestSessionAction(session, action);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const all = Array.from(root?.querySelectorAll<HTMLElement>('[role=menuitem]:not(:disabled)') ?? []);
    if (!all.length) return;
    e.preventDefault();
    e.stopPropagation();
    const i = all.indexOf(document.activeElement as HTMLElement);
    const n =
      e.key === 'Home'
        ? 0
        : e.key === 'End'
          ? all.length - 1
          : e.key === 'ArrowDown'
            ? (i + 1) % all.length
            : (i - 1 + all.length) % all.length;
    all[n].focus();
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div
  class="menu"
  role="menu"
  aria-label="Session actions"
  style:left="{pos.left}px"
  style:top="{pos.top}px"
  bind:this={root}
  onkeydown={onKey}
  data-testid="session-row-menu"
>
  {#each items as a (a.id)}
    {#if a.danger}<div class="sep" role="separator"></div>{/if}
    <button
      type="button"
      role="menuitem"
      class="mi"
      class:danger={a.danger}
      disabled={a.blocked !== null}
      title={a.blocked ?? ''}
      data-testid="row-menu-{a.id}"
      onclick={() => run(a.id)}>{a.label}</button
    >
  {/each}
  <div class="sep" role="separator"></div>
  <button type="button" role="menuitem" class="mi" data-testid="row-menu-details" onclick={() => run('details')}>Details…</button>
</div>

<style>
  .menu {
    position: fixed;
    z-index: 30;
    min-width: 12rem;
    padding: 0.25rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-pop);
  }
  .mi {
    display: flex;
    align-items: center;
    width: 100%;
    height: var(--control-h-lg);
    padding: 0 0.5rem;
    border: none;
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-2xs);
    text-align: left;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .mi:hover:not(:disabled),
  .mi:focus-visible {
    background: var(--accent-soft);
  }
  /* The tint alone is ~1.1:1 on the menu: keyboard focus keeps the ring,
     inset so the menu's edge does not clip it (review r11). */
  .mi:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .mi:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
  .mi.danger:not(:disabled) {
    color: var(--danger);
  }
  .sep {
    height: 1px;
    margin: 0.25rem 0.25rem;
    background: var(--border);
  }
</style>
