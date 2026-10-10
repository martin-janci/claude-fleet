<script lang="ts">
  // The one dialog pattern (redesign step 5.10, the Dialogs board): a title,
  // one plain sentence, the fields, then Cancel and the verb. Fork, Resume,
  // Review and Send prompt all use it, so the four read alike and a fifth
  // starts from here instead of from a copy of one of them.
  //
  // The sheet owns the frame and the footer only; each dialog keeps its own
  // state, gates and calls. `busy` disables both buttons and swaps the verb
  // for `busyVerb`, with the comet loader in the button (step 3.13): a call
  // is running, never a wait on a person.
  //
  // The form kit's behaviour (G1.2, FormsAnatomy) lives here too, so every
  // sheet gets it: a failure is a banner at the top of the body (a hub
  // refusal keeps the input and asks an admin), a disabled verb says why
  // under it, Enter submits a one-field sheet and ⌘↵ / Ctrl+Enter any
  // sheet, and closing a `dirty` sheet asks "Discard changes?" once, in
  // the footer.
  import type { Snippet } from 'svelte';
  import Loader from './Loader.svelte';
  import Modal from './Modal.svelte';
  import FormBanner from './forms/FormBanner.svelte';
  import DiscardAsk from './forms/DiscardAsk.svelte';
  import { CloseGuard } from './forms/close_guard.svelte';
  import { fieldCount, submitKey } from './forms/form_frame';
  import type { IpcError } from './result';
  import { detectMac } from './terminal_keys';

  let {
    title,
    lead,
    verb,
    busyVerb,
    onconfirm,
    onclose,
    canConfirm = true,
    busy = false,
    error = null,
    width = '480px',
    testid,
    confirmTestid,
    errorTestid,
    confirmTitle = null,
    dirty = false,
    oninvalid,
    secondary,
    danger = false,
    children,
  }: {
    title: string;
    /** The one plain sentence under the title. */
    lead: string;
    /** The confirm button's label: the action, never "OK". */
    verb: string;
    busyVerb?: string;
    onconfirm: () => void;
    onclose: () => void;
    canConfirm?: boolean;
    busy?: boolean;
    /** A banner at the top of the body. An `IpcError` is read for people;
     *  `E_FORBIDDEN` reads "The hub refused this", input kept. */
    error?: string | IpcError | null;
    width?: string;
    testid?: string;
    confirmTestid?: string;
    errorTestid?: string;
    /** Why the verb is off (a gate's reason): said under the verb while it
     *  is off, and as its tooltip. */
    confirmTitle?: string | null;
    /** The person changed something: closing asks "Discard changes?" once. */
    dirty?: boolean;
    /** ⌘↵ / Enter pressed while the verb is off (show every field's problem). */
    oninvalid?: () => void;
    /** A quiet action at the footer's start (e.g. "Start fresh instead"). */
    secondary?: Snippet;
    /** A destructive verb (delete, remove): red instead of the accent
     *  (G1.4; `forms/DestructiveConfirm.svelte` sets it). */
    danger?: boolean;
    children: Snippet;
  } = $props();

  const guard = new CloseGuard(
    () => dirty,
    () => onclose(),
  );
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  let fieldsEl: HTMLDivElement | undefined = $state();
  const why = $derived(!canConfirm && !busy ? confirmTitle : null);

  function onkeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || busy || guard.asking) return;
    if (!submitKey(e, fieldCount(fieldsEl), isMac)) return;
    e.preventDefault();
    if (canConfirm) onconfirm();
    else oninvalid?.();
  }
</script>

<Modal label={title} onclose={busy ? undefined : guard.request} {width} {testid}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="sheet" {onkeydown}>
    <header>
      <h3 class="sheet-title">{title}</h3>
      <p class="sheet-lead">{lead}</p>
    </header>

    <FormBanner {error} testid={errorTestid} />

    <div class="sheet-fields" bind:this={fieldsEl}>
      {@render children()}
    </div>

    <footer>
      {#if guard.asking}
        <DiscardAsk onkeep={guard.keep} ondiscard={guard.discard} />
      {:else}
        <div class="secondary">{#if secondary}{@render secondary()}{/if}</div>
        <button type="button" class="btn" onclick={guard.request} disabled={busy} data-testid="sheet-cancel"
          >Cancel</button
        >
        <button
          type="button"
          class="btn btn--primary"
          class:btn--danger={danger}
          data-testid={confirmTestid}
          title={confirmTitle ?? ''}
          aria-describedby={why ? `${testid ?? 'sheet'}-why` : undefined}
          disabled={!canConfirm || busy}
          onclick={onconfirm}
          >{#if busy}<Loader name="comet" size={12} class="btn-loader" />{busyVerb ?? verb}{:else}{verb}{/if}</button
        >
      {/if}
    </footer>
    {#if why && !guard.asking}
      <p class="sheet-why" id={`${testid ?? 'sheet'}-why`} data-testid="sheet-why">{why}</p>
    {/if}
  </div>
</Modal>

<style>
  footer :global(.btn-loader) {
    margin-right: 0.35em;
    vertical-align: -1px;
  }
  .sheet {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  header {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .sheet-title {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--text-lg-weight);
  }
  .sheet-lead {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-sm);
    line-height: var(--text-sm-lh);
  }
  .sheet-fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  /* Field rows inside any sheet: a small label over its control. */
  .sheet-fields :global(.field) {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .sheet-fields :global(.field-label) {
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .sheet-fields :global(.field-note) {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .sheet-fields :global(select),
  .sheet-fields :global(textarea),
  .sheet-fields :global(input[type='text']) {
    font: inherit;
    font-size: var(--text-sm);
    color: var(--fg);
    background: var(--bg-sunk);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
  }
  .sheet-fields :global(textarea) {
    resize: vertical;
    font-family: var(--font-mono);
  }
  .sheet-why {
    margin: calc(-1 * var(--space-2)) 0 0;
    text-align: right;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  footer {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .secondary {
    flex: 1;
  }
</style>
