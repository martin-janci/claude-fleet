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

  // A ticking clock, so an abandoned draft leaves on time: `Date.now()` in
  // the derived alone re-ran only when the row changed, and a quiet row kept
  // the skeleton up for good.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  /** The draft a person dismissed, by session and last write. */
  let dismissedDraft = $state<string | null>(null);
  const draftKey = $derived(session.form_draft ? `${session.id}:${session.form_draft.updated_at}` : null);
  const draft = $derived(
    live &&
      !session.pending_form &&
      session.form_draft &&
      session.form_draft.updated_at >= nowSec - DRAFT_TTL_SECS &&
      draftKey !== dismissedDraft
      ? session.form_draft
      : null,
  );
  const opened = $derived(live ? ($chatWizards.get(session.id) ?? []) : []);
</script>

{#if draft}
  <div class="opened" data-testid="chat-form-draft">
    <ChatForm
      draft={draft.draft}
      building
      from={agentName}
      reading={draft.why}
      onsubmit={async () => ({ ok: false, error: 'The form is still being written.' })} />
    <button type="button" class="dismiss" data-testid="chat-form-draft-dismiss" onclick={() => (dismissedDraft = draftKey)}>Dismiss</button>
  </div>
{/if}
{#each opened as w (w.key)}
  <div class="opened">
    <WizardChatCard id={w.id} from={w.from} why={w.why} stateKey={`app:${session.id}:${w.key}`} onended={() => endChatWizard(session.id, w.key)} />
    <!-- Closable at any point, not only once it ended: a wizard that failed
         to open, or that the person changed their mind about, must not stay. -->
    <button type="button" class="dismiss" data-testid="chat-wizard-dismiss" onclick={() => closeChatWizard(session.id, w.key)}
      >{w.ended ? 'Dismiss' : 'Close'}</button
    >
  </div>
{/each}

<style>
  .opened { display: flex; flex-direction: column; gap: 0.25rem; }
  .dismiss { align-self: flex-end; background: none; border: none; padding: 0; color: var(--fg-muted); text-decoration: underline; cursor: pointer; font-size: var(--text-2xs); min-block-size: var(--control-h); }
</style>
