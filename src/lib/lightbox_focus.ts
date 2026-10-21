// Focus for a hand-rolled `aria-modal` overlay such as an image lightbox
// (review w19-5): on mount it remembers what had focus and moves focus into
// the layer (its `[data-autofocus]`, else the first control, else the layer
// itself); Tab and Shift+Tab cycle inside it; on destroy focus goes back to
// the opener, so closing a screenshot in a long transcript keeps your place.
// `Modal` does all of this for a real dialog; this is for a layer that is
// not one.

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function lightboxFocus(node: HTMLElement) {
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const first = node.querySelector<HTMLElement>('[data-autofocus]') ?? node.querySelector<HTMLElement>(FOCUSABLE) ?? node;
  first.focus();

  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Tab') return;
    const items = Array.from(node.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (items.length === 0) {
      e.preventDefault();
      node.focus();
      return;
    }
    const at = items.indexOf(document.activeElement as HTMLElement);
    const next = e.shiftKey ? (at <= 0 ? items.length - 1 : at - 1) : at === -1 || at === items.length - 1 ? 0 : at + 1;
    e.preventDefault();
    items[next].focus();
  }
  node.addEventListener('keydown', onKeydown);

  return {
    destroy() {
      node.removeEventListener('keydown', onKeydown);
      // After the layer leaves the DOM, so the opener's focus is not stolen
      // back by anything the teardown does.
      queueMicrotask(() => {
        if (opener?.isConnected) opener.focus();
      });
    },
  };
}
