<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  export let id: string;

  const dispatch = createEventDispatcher<{ resize: number }>();

  let startX = 0;
  let dragging = false;
  let element: HTMLDivElement;

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    startX = e.clientX;
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp, { once: true });
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const delta = e.clientX - startX;
    dispatch('resize', delta);
    // Also dispatch a bubbling DOM event so test containers can listen
    element.dispatchEvent(new CustomEvent('resize', { detail: delta, bubbles: true }));
  }

  function onPointerUp() {
    dragging = false;
    window.removeEventListener('pointermove', onPointerMove);
  }
</script>

<div
  bind:this={element}
  data-testid="resizer-{id}"
  class="resizer"
  role="separator"
  aria-orientation="vertical"
  on:pointerdown={onPointerDown}
></div>

<style>
  .resizer {
    width: 4px;
    cursor: col-resize;
    background: var(--border);
  }
  .resizer:hover { background: var(--accent); }
</style>
