/**
 * The ARIA radio-group keyboard for a hand-built `role="radiogroup"` of
 * `role="radio"` buttons (review w19-8), as a native radio group has it:
 * one Tab stop (the checked radio, else the first), ←/↑ and →/↓ move to the
 * previous/next radio and wrap, Home/End to the first and last. The radio
 * reached is checked through its own click handler, unless `select: false`:
 * a group whose click commits something heavier than a choice (Control's
 * Autonomy, which closes its popover and rewrites settings) only moves focus,
 * and Enter or Space picks.
 *
 *     <div role="radiogroup" aria-label="…" use:radiogroupKeys>…</div>
 */
export function radiogroupKeys(node: HTMLElement, opts: { select?: boolean } = {}) {
  let select = opts.select !== false;
  const radios = () =>
    Array.from(node.querySelectorAll<HTMLElement>('[role="radio"]')).filter(
      (r) => r.closest('[role="radiogroup"]') === node && !r.hasAttribute('disabled') && r.getAttribute('aria-disabled') !== 'true',
    );

  /** One Tab stop: the checked radio, else the first. */
  function rove(focused?: HTMLElement) {
    const all = Array.from(node.querySelectorAll<HTMLElement>('[role="radio"]')).filter((r) => r.closest('[role="radiogroup"]') === node);
    const live = radios();
    const stop = focused ?? live.find((r) => r.getAttribute('aria-checked') === 'true') ?? live[0];
    for (const r of all) {
      const want = r === stop ? '0' : '-1';
      if (r.getAttribute('tabindex') !== want) r.setAttribute('tabindex', want);
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    const back = e.key === 'ArrowLeft' || e.key === 'ArrowUp';
    const fwd = e.key === 'ArrowRight' || e.key === 'ArrowDown';
    if (!back && !fwd && e.key !== 'Home' && e.key !== 'End') return;
    const list = radios();
    const at = list.indexOf(e.target as HTMLElement);
    if (at < 0) return;
    const last = list.length - 1;
    const to = fwd ? (at === last ? 0 : at + 1) : back ? (at === 0 ? last : at - 1) : e.key === 'Home' ? 0 : last;
    e.preventDefault();
    const next = list[to];
    next.focus();
    if (select) {
      if (next.getAttribute('aria-checked') !== 'true') next.click();
    } else {
      rove(next);
    }
  }

  rove();
  // Re-rove when the checked radio or the set of radios changes.
  const watch = new MutationObserver(() => rove(select ? undefined : (radios().find((r) => r === document.activeElement) ?? undefined)));
  watch.observe(node, { subtree: true, childList: true, attributes: true, attributeFilter: ['aria-checked', 'disabled'] });
  node.addEventListener('keydown', onkeydown);
  return {
    update(o: { select?: boolean } = {}) {
      select = o.select !== false;
    },
    destroy() {
      watch.disconnect();
      node.removeEventListener('keydown', onkeydown);
    },
  };
}
