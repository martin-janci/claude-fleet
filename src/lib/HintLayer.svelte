<script lang="ts">
  import { activeHintId, anchorEl, hintDef, markSeen } from './hints';
  import { computeBubblePosition, type Pos } from './hintPosition';

  // Measured bubble size (read after render); start with sane defaults.
  let bubbleEl = $state<HTMLDivElement | null>(null);
  let pos = $state<Pos | null>(null);

  const def = $derived($activeHintId ? hintDef($activeHintId) : undefined);

  function reposition() {
    const id = $activeHintId;
    if (!id || !def) {
      pos = null;
      return;
    }
    const el = anchorEl(id);
    if (!el) {
      pos = null;
      return;
    }
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) {
      pos = null; // anchor not laid out / hidden
      return;
    }
    const bw = bubbleEl?.offsetWidth ?? 208;
    const bh = bubbleEl?.offsetHeight ?? 90;
    pos = computeBubblePosition(
      { top: r.top, left: r.left, width: r.width, height: r.height },
      def.placement,
      bw,
      bh,
      window.innerWidth,
      window.innerHeight,
    );
  }

  // Recompute when the active hint changes, and on resize/scroll while shown.
  $effect(() => {
    void $activeHintId;
    void def;
    reposition();
    if (!$activeHintId) return;
    const handler = () => reposition();
    window.addEventListener('resize', handler);
    window.addEventListener('scroll', handler, true); // capture: catch inner scrollers
    return () => {
      window.removeEventListener('resize', handler);
      window.removeEventListener('scroll', handler, true);
    };
  });

  function dismiss() {
    if ($activeHintId) markSeen($activeHintId);
  }
</script>

{#if $activeHintId && def && pos}
  <div
    class="hint"
    bind:this={bubbleEl}
    style="top:{pos.top}px; left:{pos.left}px;"
    role="status"
    data-testid="hint-bubble"
    data-hint-id={$activeHintId}
  >
    <div class="htext">{def.text}</div>
    <div class="hactions">
      <button class="gotit" onclick={dismiss}>Got it</button>
      <button class="x" onclick={dismiss} aria-label="Dismiss hint">✕</button>
    </div>
  </div>
{/if}

<style>
  .hint {
    position: fixed;
    z-index: 15; /* above general UI, below modals (z-index 20) so an open modal hides hints */
    width: 208px;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 10px 11px;
    box-shadow: 0 6px 22px rgba(0, 0, 0, 0.16);
    font-size: 0.75rem;
    line-height: 1.45;
  }
  .htext {
    color: var(--fg);
    margin-bottom: 8px;
  }
  .hactions {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .gotit {
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 0.75rem;
    cursor: pointer;
  }
  .x {
    background: none;
    border: none;
    color: var(--fg-muted, #777);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .gotit:hover {
    filter: brightness(1.08);
  }
  .x:hover {
    color: var(--fg);
  }
</style>
