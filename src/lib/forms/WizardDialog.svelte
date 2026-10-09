<script lang="ts">
  // A wizard as a dialog (redesign step 10.12): its fleet.form/1 spec from
  // wizards.ts, the same one ChatForm shows in the conversation. The title
  // and intro are the spec's; the fields, Back / Next and the last button are
  // FormWizard's, with Cancel at the row's start. While the last step runs
  // the button says what it is doing and the wizard's own loader sits in the
  // body: one loader per screen, so no Comet in the button (review r12);
  // the screen that opened it decides what the button does (`run`) and closes
  // it on success. `extra` is that screen's own line under the fields (a
  // follow-up the backend asked for, such as the hub's plaintext opt-in).
  import type { Snippet } from 'svelte';
  import Modal from '../Modal.svelte';
  import Loader from '../Loader.svelte';
  import FormWizard from './FormWizard.svelte';
  import type { FieldProblem, Values } from './forms';
  import type { Wizard } from './wizards';

  let {
    wizard,
    initial = {},
    busy = false,
    error = null,
    errorTestid = 'wizard-error',
    problems = [],
    run,
    onclose,
    extra,
  }: {
    wizard: Wizard;
    initial?: Values;
    busy?: boolean;
    error?: string | null;
    errorTestid?: string;
    problems?: FieldProblem[];
    run: (values: Values) => void;
    onclose: () => void;
    extra?: Snippet;
  } = $props();
</script>

<Modal label={wizard.spec.title} onclose={busy ? undefined : onclose} width="480px" testid={`wizard-${wizard.id}`}>
  <div class="sheet">
    <header>
      <h3>{wizard.spec.title}</h3>
      {#if wizard.spec.intro}<p class="lead">{wizard.spec.intro}</p>{/if}
    </header>
    <FormWizard
      spec={wizard.spec}
      {busy}
      {initial}
      sending={wizard.sending}
      buttonLoader={false}
      ownDefaults
      serverProblems={problems}
      onsubmit={run}
      oncancel={onclose} />
    {#if busy}
      <div class="running" data-testid="wizard-running">
        <Loader name={wizard.loader} size={32} label={wizard.sending} />
      </div>
    {/if}
    {#if error}<p class="err" role="alert" data-testid={errorTestid}>{error}</p>{/if}
    {#if extra}{@render extra()}{/if}
  </div>
</Modal>

<style>
  .sheet { display: flex; flex-direction: column; gap: var(--space-3); }
  header { display: flex; flex-direction: column; gap: var(--space-1); }
  h3 { margin: 0; font-size: var(--text-lg); font-weight: var(--text-lg-weight); }
  .lead { margin: 0; color: var(--fg-muted); font-size: var(--text-sm); line-height: var(--text-sm-lh); }
  .running { display: flex; justify-content: center; }
  .err { margin: 0; font-size: var(--text-xs); color: var(--danger); }
</style>
