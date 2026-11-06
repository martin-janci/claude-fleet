<!-- The inline confirm: "Delete it? [Delete] [Keep]" in place of the button
     that asked, for a step one stray click away (pause all, revoke, remove,
     delete). One row everywhere, so each site stops hand-writing its own.
     Keep takes focus as the row mounts (the button that opened it is often
     gone, and focus would drop to the page); Escape is Keep; and when the
     row closes with focus inside it, focus goes back to the opener — the
     `opener` given, else the element that had focus as the row mounted —
     if it is still on the page. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { focusNow } from '../focus_now';

  let {
    question = '',
    label,
    verb,
    danger = true,
    busy = false,
    disabled = false,
    confirmTitle = null,
    keepLabel = 'Keep',
    stacked = false,
    opener,
    onconfirm,
    onkeep,
    children,
    testid,
    confirmTestid,
    keepTestid,
  }: {
    /** "Pause 3 running missions?"; may be empty when the row speaks for itself. */
    question?: string;
    /** The row's accessible name; defaults to the question, then to the verb. */
    label?: string;
    /** The verb on the button: "Delete", "Revoke", "Pause all". */
    verb: string;
    /** Red verb (the default); false for a confirm that loses nothing, like Trust. */
    danger?: boolean;
    /** Turns both buttons off while the step runs. */
    busy?: boolean;
    /** Turns the verb off for another reason, said in `confirmTitle`. */
    disabled?: boolean;
    confirmTitle?: string | null;
    keepLabel?: string;
    /** The question on its own line with the buttons under it. */
    stacked?: boolean;
    /** Where focus goes back to when the row closes; else the element focused as it mounted. */
    opener?: () => HTMLElement | null | undefined;
    onconfirm: () => void;
    onkeep: () => void;
    /** Richer text than `question`, in its place. */
    children?: Snippet;
    testid?: string;
    confirmTestid?: string;
    keepTestid?: string;
  } = $props();

  let root: HTMLElement | undefined = $state();
  // Read as the row mounts, before Keep takes focus.
  const before = typeof document === 'undefined' ? null : (document.activeElement as HTMLElement | null);

  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    // Ours, not the dialog's: Escape answers the question, it does not close the sheet.
    e.preventDefault();
    e.stopPropagation();
    onkeep();
  }

  // Whether focus is in the row. Svelte removes the row's nodes before the
  // teardown below runs, so `contains(activeElement)` is too late there; a
  // focusout to nowhere (the row going, a click on the page) keeps it set.
  let inside = false;
  function onfocusin() {
    inside = true;
  }
  function onfocusout(e: FocusEvent) {
    const to = e.relatedTarget as Node | null;
    if (to && !root?.contains(to)) inside = false;
  }

  $effect(() => {
    return () => {
      if (!inside) return;
      // After the flush that removed the row, so an opener re-rendered in its place is on the page.
      queueMicrotask(() => {
        const active = document.activeElement;
        if (active && active !== document.body) return;
        const back = opener?.() ?? before;
        if (back?.isConnected) back.focus();
      });
    };
  });
</script>

<span
  class="inline-confirm"
  class:stacked
  role="alertdialog"
  tabindex="-1"
  aria-label={label ?? (question || `${verb}?`)}
  data-testid={testid}
  bind:this={root}
  {onkeydown}
  {onfocusin}
  {onfocusout}
>
  {#if children}<span class="q">{@render children()}</span>{:else if question}<span class="q">{question}</span>{/if}
  <span class="buttons">
    <button
      class="btn"
      class:btn--crit={danger}
      type="button"
      disabled={busy || disabled}
      title={confirmTitle ?? undefined}
      data-testid={confirmTestid}
      onclick={onconfirm}>{verb}</button
    >
    <button class="btn btn--quiet" type="button" disabled={busy} data-testid={keepTestid} use:focusNow onclick={onkeep}>{keepLabel}</button>
  </span>
</span>

<style>
  .inline-confirm {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
    font-size: var(--text-sm);
  }
  .inline-confirm.stacked {
    display: flex;
    flex-basis: 100%;
    flex-direction: column;
    align-items: flex-start;
  }
  .buttons {
    display: inline-flex;
    gap: 0.4rem;
  }
</style>
