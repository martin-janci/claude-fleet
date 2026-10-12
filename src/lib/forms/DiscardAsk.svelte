<!-- "Discard changes?" in a form's footer, in place of Cancel and the verb
     (FormsAnatomy, Saving: "Changed — Discard · Save"). Asked once: the
     CloseGuard closes on the next close request. -->
<script lang="ts">
  let { onkeep, ondiscard }: { onkeep: () => void; ondiscard: () => void } = $props();
  // The footer button that had focus is gone: hold it on the safe answer,
  // so Enter keeps editing and the dialog still hears the keyboard.
  let keepBtn: HTMLButtonElement | undefined = $state();
  $effect(() => keepBtn?.focus());
</script>

<div class="discard-ask" role="group" aria-label="Discard changes?" data-testid="form-discard-ask">
  <span class="q">Discard changes?</span>
  <button type="button" class="btn" data-testid="form-keep-editing" bind:this={keepBtn} onclick={onkeep}>Keep editing</button>
  <button type="button" class="btn btn--crit" data-testid="form-discard" onclick={ondiscard}
    >Discard</button
  >
</div>

<style>
  .discard-ask {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex: 1;
    justify-content: flex-end;
  }
  .q {
    margin-right: auto;
    font-size: var(--text-sm);
    color: var(--fg);
  }
</style>
