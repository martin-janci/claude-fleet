<script lang="ts" module>
  import type { ChatFormEnded } from './ChatForm.svelte';
  // How each card ended, by its key, so a card drawn again (the transcript
  // re-read, the panel re-mounted) stays the one line it shrank to. Per
  // window, never saved: what the button did lives in the fleet's rows.
  const endedByKey = new Map<string, ChatFormEnded>();
</script>

<script lang="ts">
  // One of the app's wizards as a form in the conversation (redesign 10.12,
  // the ChatWizards board): opened by an agent's `wizard` block or by the app
  // (Control's Add project). Building while it reads the choices only known
  // now (your hosts, your SSH config), then the wizard, with its last button
  // running what the wizard's own screen runs. Nothing runs before that
  // button; never a modal or an overlay.
  import { onMount } from 'svelte';
  import ChatForm, { type ChatFormOutcome } from './ChatForm.svelte';
  import { WIZARDS } from './wizards';
  import { CHAT_WIZARD_READS, chatWizard, type ChatWizard } from './chat_wizard_runs';
  import type { ChatWizardId } from './chat_wizard_ids';
  import type { Values } from './forms';

  let {
    id,
    from,
    why = null,
    stateKey = null,
    report,
    onended,
  }: {
    id: ChatWizardId;
    /** Who opened it ("Control", the session's agent). */
    from: string;
    why?: string | null;
    /** Keeps how it ended across re-draws. */
    stateKey?: string | null;
    /** Tell the opener in words (an agent's block: fill its composer). */
    report?: (text: string) => void;
    onended?: (ended: ChatFormEnded) => void;
  } = $props();

  let ready = $state<ChatWizard | null>(null);
  let failed = $state<string | null>(null);
  let sending = $state<string | null>(null);
  const title = $derived(WIZARDS[id].spec.title);

  onMount(() => {
    let live = true;
    chatWizard(id).then(
      (w) => {
        if (live) ready = w;
      },
      (e) => {
        if (live) failed = e instanceof Error ? e.message : String(e);
      },
    );
    return () => {
      live = false;
    };
  });

  async function submit(values: Values): Promise<ChatFormOutcome> {
    if (!ready) return { ok: false, error: 'The form is not ready yet.' };
    sending = ready.wizard.sending;
    try {
      return await ready.run(values, (text) => (sending = text));
    } finally {
      sending = null;
    }
  }

  function ended(e: ChatFormEnded) {
    if (stateKey) endedByKey.set(stateKey, e);
    if (e.state === 'answered') report?.(`Done in the "${title}" form: ${e.summary}`);
    else report?.(`I declined the "${title}" form${e.note ? `: ${e.note}` : '.'}`);
    onended?.(e);
  }
</script>

<div data-testid="wizard-chat-card" data-wizard={id}>
  {#if failed}
    <p class="err" role="alert">{title} could not open: {failed}</p>
  {:else if ready}
    <ChatForm
      spec={ready.wizard.spec}
      {from}
      {why}
      sending={sending ?? ready.wizard.sending}
      outcome={stateKey ? (endedByKey.get(stateKey) ?? null) : null}
      onsubmit={submit}
      ondecline={() => undefined}
      onended={ended} />
  {:else}
    <ChatForm
      draft={JSON.stringify({ title })}
      building
      {from}
      {why}
      reading={CHAT_WIZARD_READS[id]}
      onsubmit={submit} />
  {/if}
</div>

<style>
  .err { margin: 0; font-size: var(--text-2xs); color: var(--usage-crit); }
</style>
