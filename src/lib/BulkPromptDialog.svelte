<script module lang="ts">
  /** The board's presets: one click fills the text, which stays editable. */
  export const PRESETS: { label: string; text: string }[] = [
    { label: 'Status', text: 'Give a short status: what is done, what is left, and anything blocking you.' },
    { label: 'Continue', text: 'Continue.' },
    { label: 'Rebase on main', text: 'Rebase on main and re-run your tests. Report only failures.' },
  ];

</script>

<script lang="ts">
  // Bulk "send prompt" for the sidebar's multi-select (FE-4), in the one
  // dialog pattern (step 5.10). Each target gets the same text as a new turn
  // through `queue_prompt`: an idle session gets it at once, a busy one
  // when its turn ends (never into a dialog). Per-target results say which.
  import { queuePrompt, type SessionRow } from './sessions';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked } from './share';
  import DialogSheet from './DialogSheet.svelte';

  let {
    targets,
    onClose,
  }: {
    targets: SessionRow[];
    onClose: () => void;
  } = $props();

  let prompt = $state('');
  let sending = $state(false);
  let done = $state(false);
  let errors = $state<Record<number, string>>({});
  let outcome = $state<Record<number, 'sent' | 'queued'>>({});

  /**
   * Narrowed per target, then again at the send (multi-user M1, F2b). Sidebar
   * hands this dialog a selection it has already narrowed with `bulkTargets` —
   * but this dialog re-derives its own fan-out from `targets` and the dialog
   * stays open, so a grant narrowed while it is open would otherwise send a
   * prompt into a session this client may no longer drive. `queue_prompt` is
   * `drive` in `share.ts::SESSION_TIER`, like `send_prompt`; the narrowing is
   * asked here, over the rows this dialog is actually going to write to.
   */
  const writable = $derived(bulkTargets(targets, 'queue_prompt', $sessionBlocked));
  const sendable = $derived(
    writable.filter((t) => t.kind !== 'shell' && t.status === 'running'),
  );
  /** Rows the selection held that this client may not drive — counted so the
   *  dialog says so rather than quietly sending to fewer sessions. */
  const notMine = $derived(targets.length - writable.length);
  const hubBlocked = $derived(hubActionBlocked('queue_prompt', $hubStatus, $hubConnection));
  const canSend = $derived(
    prompt.trim().length > 0 && sendable.length > 0 && !sending && hubBlocked === null,
  );
  const n = $derived(sendable.length);

  async function send() {
    if (done) {
      onClose();
      return;
    }
    // Re-asked at the call: `canSend` reads the same two halves, and the
    // fan-out below walks `sendable`, which is the narrowed list.
    if (!canSend) return;
    sending = true;
    errors = {};
    outcome = {};
    await Promise.allSettled(
      sendable.map(async (t) => {
        const r = await queuePrompt(t.id, prompt);
        if (r.ok) outcome[t.id] = r.value.delivered ? 'sent' : 'queued';
        else errors[t.id] = r.error.message;
      }),
    );
    sending = false;
    const queued = Object.values(outcome).some((o) => o === 'queued');
    if (Object.keys(errors).length === 0 && !queued) setTimeout(() => onClose(), 600);
    else done = Object.keys(errors).length === 0;
  }
</script>

<DialogSheet
  title="Send prompt to {n} session{n === 1 ? '' : 's'}"
  lead="Each gets the same text as a new turn. Busy sessions get it when they are idle."
  verb={done ? 'Done' : `Send to ${n}`}
  busyVerb="Sending…"
  busy={sending}
  canConfirm={done || canSend}
  onconfirm={() => void send()}
  onclose={onClose}
  width="520px"
  testid="bulk-prompt-dialog"
  confirmTestid="bulk-prompt-send"
  confirmTitle={hubBlocked}
>
  <ul class="targets">
    {#each targets as t (t.id)}
      {@const skipped = !sendable.includes(t)}
      {@const notYours = !writable.includes(t)}
      <li class:skipped data-testid="bulk-target-{t.id}">
        <span class="tag tag--mono">{t.host_alias}</span>
        <span class="sess-name">{t.friendly_name ?? t.tmux_name}</span>
        {#if notYours}
          <span class="muted" data-testid="bulk-not-mine-{t.id}" title={$sessionBlocked(t, 'queue_prompt') ?? ''}
            >not yours</span
          >
        {:else if skipped}
          <span class="muted" title="shell or non-running sessions are skipped">skipped</span>
        {:else if outcome[t.id] === 'sent'}
          <span class="ok" data-testid="bulk-sent-{t.id}">✓ sent</span>
        {:else if outcome[t.id] === 'queued'}
          <span class="muted" data-testid="bulk-queued-{t.id}">queued · goes in when idle</span>
        {:else if errors[t.id]}
          <span class="err" data-testid="bulk-err-{t.id}">✗ {errors[t.id]}</span>
        {/if}
      </li>
    {/each}
  </ul>
  <textarea
    bind:value={prompt}
    rows="6"
    placeholder="Prompt to send to every selected session…"
    data-testid="bulk-prompt-textarea"
    disabled={sending || done}
  ></textarea>
  <div class="presets">
    <span class="field-label">Presets:</span>
    {#each PRESETS as p (p.label)}
      <button
        type="button"
        class="btn btn--chip"
        data-testid="bulk-preset-{p.label.toLowerCase().replace(/ /g, '-')}"
        disabled={sending || done}
        onclick={() => (prompt = p.text)}>{p.label}</button
      >
    {/each}
  </div>
  {#if hubBlocked}
    <p class="muted" data-testid="bulk-prompt-blocked">{hubBlocked}</p>
  {:else if notMine > 0}
    <p class="muted" data-testid="bulk-prompt-not-mine">
      {notMine} of the selected sessions {notMine === 1 ? 'is' : 'are'} not yours to drive and
      {notMine === 1 ? 'is' : 'are'} left out.
    </p>
  {/if}
</DialogSheet>

<style>
  .targets {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1) var(--space-3);
    max-height: 12rem;
    overflow: auto;
  }
  .targets li {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-sm);
  }
  .targets li.skipped { opacity: 0.55; }
  .sess-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .muted { color: var(--fg-muted); font-size: var(--text-xs); }
  .ok { color: var(--status-done); font-size: var(--text-xs); }
  .err { color: var(--danger); font-size: var(--text-xs); }
  textarea { width: 100%; box-sizing: border-box; min-height: 5rem; }
  .presets { display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap; }
</style>
