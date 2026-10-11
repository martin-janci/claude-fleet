// Marks a scroll container that holds more than it shows, so CSS can fade
// its bottom edge (`data-overflow`) until it is scrolled to the end
// (`data-at-end`). Used by the tray above Control's composer.

export function markOverflow(node: HTMLElement): void {
  const over = node.scrollHeight > node.clientHeight + 1;
  node.dataset.overflow = String(over);
  node.dataset.atEnd = String(!over || node.scrollTop + node.clientHeight >= node.scrollHeight - 1);
}

export function overflowMark(node: HTMLElement): { destroy(): void } {
  const update = () => markOverflow(node);
  update();
  node.addEventListener('scroll', update, { passive: true });
  const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update);
  ro?.observe(node);
  // Cards come and go inside without resizing the capped box itself.
  const mo = typeof MutationObserver === 'undefined' ? null : new MutationObserver(update);
  mo?.observe(node, { childList: true, subtree: true });
  return {
    destroy() {
      node.removeEventListener('scroll', update);
      ro?.disconnect();
      mo?.disconnect();
    },
  };
}
