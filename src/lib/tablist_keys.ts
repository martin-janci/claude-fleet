/**
 * Arrow keys for a hand-built `role="tablist"` (review r11), as kit Tabs has
 * them: ←/→ move to the previous/next tab and wrap, Home/End to the first
 * and last, and the tab reached is selected (automatic activation, through
 * its own click handler). Disabled tabs are skipped. A strip that renders
 * kit `<Tabs>` needs none of this.
 *
 *     <div role="tablist" aria-label="…" use:tablistKeys>…</div>
 */
export function tablistKeys(node: HTMLElement) {
  function onkeydown(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) return;
    const tabs = Array.from(node.querySelectorAll<HTMLElement>('[role="tab"]')).filter(
      (t) => t.closest('[role="tablist"]') === node && !t.hasAttribute('disabled') && t.getAttribute('aria-disabled') !== 'true',
    );
    const at = tabs.indexOf(e.target as HTMLElement);
    if (at < 0 || tabs.length === 0) return;
    const last = tabs.length - 1;
    const to = e.key === 'ArrowRight' ? (at === last ? 0 : at + 1) : e.key === 'ArrowLeft' ? (at === 0 ? last : at - 1) : e.key === 'Home' ? 0 : last;
    e.preventDefault();
    tabs[to].focus();
    if (tabs[to].getAttribute('aria-selected') !== 'true') tabs[to].click();
  }
  node.addEventListener('keydown', onkeydown);
  return { destroy: () => node.removeEventListener('keydown', onkeydown) };
}
