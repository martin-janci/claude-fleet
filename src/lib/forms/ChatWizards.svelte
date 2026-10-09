<script lang="ts">
  // The app's forms at the end of a conversation (redesign 10.12), kept out
  // of ConversationPanel: the form this session's agent is still writing
  // (`ask { draft }`, drawn in as skeleton fields until its `ask { form }`
  // opens the form card), and the wizards the app opened here (Control's
  // Add project, chat_wizards.ts). Only on the live conversation.
  import ChatForm from './ChatForm.svelte';
  import WizardChatCard from './WizardChatCard.svelte';
  import { chatWizards, closeChatWizard, endChatWizard } from './chat_wizards';
  import type { SessionRow } from '../sessions';

  /** A draft not written to for this long has been abandoned (the tick
   *  drops it after DRAFT_TTL_SECS; this covers the time until it does). */
  const DRAFT_TTL_SECS = 10 * 60;

  let {
    session,
    agentName,
    live = true,
  }: { session: Pick<SessionRow, 'id' | 'pending_form' | 'form_draft'>; agentName: string; live?: boolean } = $props();

  const draft = $derived(
    live && !session.pending_form && session.form_draft && session.form_draft.updated_at >= Date.now() / 1000 - DRAFT_TTL_SECS
      ? session.form_draft
      : null,
  );
  const opened = $derived(live ? ($chatWizards.get(session.id) ?? []) : []);
</script>

{#if draft}
  <div data-testid="chat-form-draft">
    <ChatForm
      draft={draft.draft}
      building
      from={agentName}
      reading={draft.why}
      onsubmit={async () => ({ ok: false, error: 'The form is still being written.' })} />
  </div>
{/if}
{#each opened as w (w.key)}
  <div class="opened">
    <WizardChatCard id={w.id} from={w.from} why={w.why} stateKey={`app:${session.id}:${w.key}`} onended={() => endChatWizard(session.id, w.key)} />
    {#if w.ended}
      <button type="button" class="dismiss" data-testid="chat-wizard-dismiss" onclick={() => closeChatWizard(session.id, w.key)}>Dismiss</button>
    {/if}
  </div>
{/each}

<style>
  .opened { display: flex; flex-direction: column; gap: 0.25rem; }
  .dismiss { align-self: flex-end; background: none; border: none; padding: 0; color: var(--fg-muted); text-decoration: underline; cursor: pointer; font-size: var(--text-2xs); min-block-size: var(--control-h); }
</style>
