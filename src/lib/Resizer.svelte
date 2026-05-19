<script lang="ts">
  import { onDestroy } from 'svelte';

  let { id, onresize }: { id: string; onresize: (delta: number) => void } = $props();

  let lastX = 0;
  let dragging = false;

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    lastX = e.clientX;
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp, { once: true });
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const delta = e.clientX - lastX;
    lastX = e.clientX;
    onresize(delta);
  }

  function onPointerUp() {
    dragging = false;
    window.removeEventListener('pointermove', onPointerMove);
  }

  onDestroy(() => {
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
  });
</script>

<div
  data-testid="resizer-{id}"
  class="resizer"
  role="separator"
  aria-orientation="vertical"
  onpointerdown={onPointerDown}
></div>

<style>
  .resizer {
    width: 4px;
    cursor: col-resize;
    background: var(--border);
  }
  .resizer:hover { background: var(--accent); }
</style>
