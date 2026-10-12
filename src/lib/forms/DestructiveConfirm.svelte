<script lang="ts">
  // The destructive confirm (G1.4, FormsAnatomy "Destructive confirm"):
  // delete and remove ask through this one sheet. The verb is red, a safer
  // way out sits on the left ("Pause it instead"), and when the loss is
  // large (`needsTypedName`) the person types the name before the verb
  // turns on. It is a DialogSheet, so the failure banner, the reason under
  // an off verb and the submit keys come with it.
  import DialogSheet from '../DialogSheet.svelte';
  import type { IpcError } from '../result';
  import { needsTypedName, typedNameMatches, typedNameWhy } from './destructive';

  let {
    title,
    lead,
    verb,
    busyVerb,
    name,
    noun,
    loss = 0,
    safer = null,
    busy = false,
    error = null,
    onconfirm,
    onclose,
    testid = 'destructive-confirm',
    confirmTestid = 'destructive-confirm-go',
  }: {
    /** 'Delete routine "Morning PR sweep"?' */
    title: string;
    /** What goes and what stays, ending "This can't be undone." */
    lead: string;
    /** The red verb: "Delete routine", "Remove organisation". */
    verb: string;
    busyVerb?: string;
    /** The thing's name, typed to confirm when the loss is large. */
    name: string;
    /** What it is, for "Type the routine name to confirm". */
    noun: string;
    /** How many things go with it (runs, rules, hosts, …). */
    loss?: number;
    /** The safer way out, on the left. Runs instead of the verb. */
    safer?: { label: string; run: () => void; testid?: string } | null;
    busy?: boolean;
    error?: string | IpcError | null;
    onconfirm: () => void;
    onclose: () => void;
    testid?: string;
    confirmTestid?: string;
  } = $props();

  let typed = $state('');
  const typedAsk = $derived(needsTypedName(loss));
  const ready = $derived(!typedAsk || typedNameMatches(typed, name));
</script>

<DialogSheet
  {title}
  {lead}
  {verb}
  {busyVerb}
  danger
  {busy}
  {error}
  {testid}
  {confirmTestid}
  errorTestid={`${testid}-error`}
  canConfirm={ready}
  confirmTitle={ready ? null : typedNameWhy(noun)}
  onconfirm={() => {
    if (ready) onconfirm();
  }}
  {onclose}
>
  {#snippet secondary()}
    {#if safer}
      <button
        type="button"
        class="btn btn--quiet"
        disabled={busy}
        data-testid={safer.testid ?? 'destructive-safer'}
        onclick={safer.run}>{safer.label}</button
      >
    {/if}
  {/snippet}
  {#if typedAsk}
    <label class="field">
      <span class="field-label">Type the {noun} name to confirm</span>
      <input
        type="text"
        bind:value={typed}
        placeholder={name}
        autocomplete="off"
        spellcheck="false"
        aria-label={`${noun} name`}
        data-testid="destructive-typed-name"
      />
    </label>
  {/if}
</DialogSheet>
