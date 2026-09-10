<script lang="ts">
  import { onMount, onDestroy, type Snippet } from 'svelte';

  // The one modal primitive. Built on the native <dialog> element opened with
  // showModal(), which gives us for free what the old div-backdrop copies
  // lacked: a focus trap, Escape handling (the `cancel` event), correct
  // stacking of nested dialogs (top layer — a picker opened from Settings sits
  // above Settings and Escape closes only the topmost one), and aria-modal.
  //
  // Visibility is owned by the parent via `{#if}`; `onclose` is the only way
  // out of here. The dialog never closes itself without telling the parent.
  let {
    title,
    label,
    onclose,
    closeOnBackdrop = true,
    width,
    testid,
    children,
  }: {
    /** Rendered as the heading and used as the accessible name. */
    title?: string;
    /** Accessible name only (for dialogs that render their own header). */
    label?: string;
    /** Called on Escape, backdrop click, or a native close. Omit for a
     *  dialog that can only be closed through its own buttons. */
    onclose?: () => void;
    /** Clicking the dimmed backdrop calls `onclose`. */
    closeOnBackdrop?: boolean;
    /** CSS width of the dialog box (e.g. "480px"). */
    width?: string;
    testid?: string;
    children: Snippet;
  } = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  let restoreTo: Element | null = null;
  let tearingDown = false;

  const FOCUSABLE =
    'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  onMount(() => {
    if (!dialog) return;
    restoreTo = document.activeElement;
    if (typeof dialog.showModal === 'function') {
      try {
        dialog.showModal();
      } catch {
        // Already open (HMR / double-mount) — fall through to the attribute.
        dialog.setAttribute('open', '');
      }
    } else {
      dialog.setAttribute('open', '');
    }
    // Initial focus: an explicit [data-autofocus] wins, else the first
    // focusable control, else the dialog itself so Escape still reaches it.
    const target =
      dialog.querySelector<HTMLElement>('[data-autofocus]') ??
      dialog.querySelector<HTMLElement>(FOCUSABLE) ??
      dialog;
    target.focus();
  });

  onDestroy(() => {
    tearingDown = true;
    if (dialog && dialog.open && typeof dialog.close === 'function') {
      try {
        dialog.close();
      } catch {
        /* nothing to undo */
      }
    }
    // Restore focus to whatever opened us (a toolbar button, a row) — the
    // browser only does this for a *native* close, not for an unmount.
    const el = restoreTo as HTMLElement | null;
    if (el && typeof el.focus === 'function' && el.isConnected) el.focus();
  });

  // Escape → the browser fires `cancel` before closing. Prevent the native
  // close so the parent's `{#if}` stays the single source of truth, and hand
  // the intent to the parent.
  function onCancel(e: Event) {
    e.preventDefault();
    onclose?.();
  }

  // A native close that did slip through (some engines close on a second
  // Escape without user activation) still ends up telling the parent.
  function onNativeClose() {
    if (tearingDown) return;
    onclose?.();
  }

  // Clicks on the ::backdrop are delivered to the <dialog> element itself.
  // But so are clicks on the dialog's own border — and, if it ever scrolled,
  // its scrollbar — so the target check alone is not enough: a click only
  // counts as "outside" when its point lies beyond the dialog's box. (The
  // box never scrolls itself: overflow lives on .body, see the styles.)
  function onClick(e: MouseEvent) {
    if (!closeOnBackdrop || !dialog) return;
    if (e.target !== dialog) return;
    if (isInside(dialog.getBoundingClientRect(), e.clientX, e.clientY)) return;
    onclose?.();
  }

  function isInside(r: DOMRect, x: number, y: number): boolean {
    // Strict on all edges: a zero-size rect (jsdom) classifies every click
    // as outside, matching the old behaviour there.
    return x > r.left && x < r.right && y > r.top && y < r.bottom;
  }
</script>

<dialog
  bind:this={dialog}
  class="modal"
  style:width={width}
  aria-label={title ?? label}
  aria-modal="true"
  oncancel={onCancel}
  onclose={onNativeClose}
  onclick={onClick}
  data-testid={testid}
>
  <div class="body">
    {#if title}
      <h3 class="title">{title}</h3>
    {/if}
    {@render children()}
  </div>
</dialog>

<style>
  .modal {
    /* Reset the UA dialog box; the visible chrome lives on .body. */
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--fg);
    max-width: min(90vw, 720px);
    /* No overflow here: a scrollbar on the <dialog> would be part of the
       element, and grabbing it would read as a backdrop click. */
    overflow: hidden;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.3);
  }
  .modal::backdrop {
    background: rgba(0, 0, 0, 0.4);
  }
  /* jsdom / engines without top-layer support: emulate the overlay so the
     box is still centered over a dimmed page. */
  .modal[open]:not(:modal) {
    position: fixed;
    inset: 0;
    margin: auto;
    z-index: 20;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 1rem;
    box-sizing: border-box;
    max-height: 85vh;
    overflow: auto;
  }
  .title {
    margin: 0;
    font-size: 0.95rem;
  }
</style>
