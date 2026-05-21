<script lang="ts">
  let { id, onresize }: { id: string; onresize: (delta: number) => void } = $props();

  let lastX = 0;
  let dragging = false;
  let el: HTMLDivElement;

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    lastX = e.clientX;
    // Pointer capture routes every subsequent move/up/cancel for this
    // pointer to the resizer itself — even when the cursor leaves the
    // element — and is auto-released on up/cancel. No window listeners to
    // leak if a pointerup is missed (drag into native chrome, etc.).
    el.setPointerCapture?.(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const delta = e.clientX - lastX;
    lastX = e.clientX;
    onresize(delta);
  }

  function endDrag() {
    dragging = false;
  }
</script>

<div
  bind:this={el}
  data-testid="resizer-{id}"
  class="resizer"
  role="separator"
  aria-orientation="vertical"
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={endDrag}
  onpointercancel={endDrag}
  onlostpointercapture={endDrag}
></div>

<style>
  .resizer {
    width: 4px;
    cursor: col-resize;
    background: var(--border);
  }
  .resizer:hover { background: var(--accent); }
</style>
