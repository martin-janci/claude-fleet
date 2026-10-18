/**
 * A strip of tabs that may hold more than its width: it scrolls sideways
 * instead of squeezing tabs on top of each other. The wheel scrolls it, the
 * selected tab is kept in view, and `data-more-left` / `data-more-right`
 * say which edge has tabs past it (the strip fades that edge in CSS).
 *
 *     <div role="tablist" use:tablistKeys use:tabStrip={active}>…</div>
 *
 * The tabs themselves must not shrink (`flex: none`): the app's 24 px target
 * floor sets `min-inline-size` on every tab, which takes the place of flex's
 * content-sized minimum, so a shrinking tab narrows to 24 px and its label
 * spills over its neighbours.
 */
export function tabStrip(node: HTMLElement, _active?: unknown) {
  function edges() {
    const max = node.scrollWidth - node.clientWidth;
    node.toggleAttribute('data-more-left', node.scrollLeft > 1);
    node.toggleAttribute('data-more-right', max - node.scrollLeft > 1);
  }

  function reveal() {
    const sel = node.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
    const max = Math.max(0, node.scrollWidth - node.clientWidth);
    if (!sel || max === 0) return edges();
    const left = sel.offsetLeft - node.offsetLeft;
    const right = left + sel.offsetWidth;
    if (left < node.scrollLeft) node.scrollLeft = Math.max(0, left - 16);
    else if (right > node.scrollLeft + node.clientWidth) node.scrollLeft = Math.min(max, right - node.clientWidth + 16);
    edges();
  }

  // A vertical wheel over a sideways strip scrolls it sideways.
  function onwheel(e: WheelEvent) {
    if (node.scrollWidth <= node.clientWidth || Math.abs(e.deltaX) >= Math.abs(e.deltaY)) return;
    e.preventDefault();
    node.scrollLeft += e.deltaY;
  }

  node.addEventListener('scroll', edges, { passive: true });
  node.addEventListener('wheel', onwheel, { passive: false });
  const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(edges) : null;
  ro?.observe(node);
  queueMicrotask(reveal);

  return {
    update() {
      queueMicrotask(reveal);
    },
    destroy() {
      node.removeEventListener('scroll', edges);
      node.removeEventListener('wheel', onwheel);
      ro?.disconnect();
    },
  };
}
