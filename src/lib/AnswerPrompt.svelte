<script lang="ts">
  // The answer card for a blocked session: Claude's question, its numbered
  // choices as buttons, and the keystroke that picks one.
  //
  // A choice is delivered as a KEY (`sendPrompt`'s `keys`), never as text:
  // the text path pastes, the REPL has bracketed paste on, and the first
  // byte a select dialog would then see is ESC — cancelling the dialog
  // instead of answering it.
  //
  // Rendered in two places (the Conversation panel and the sidebar row, the
  // latter `compact`), which is why the send lives HERE rather than in each
  // caller: the freshness check below is the whole safety of the feature and
  // must not be something a second caller can forget.
  //
  // Answering re-reads the pane first and refuses unless the dialog it is
  // about to answer is still the dialog on screen. The row is written by the
  // 20 s reconcile tick, so without that check a click on a stale card could
  // approve a permission dialog the user never saw — or press a digit into
  // the REPL of a session that has already moved on.
  import { sessionActivity } from './conversation';
  import { sendPrompt, type SessionRow } from './sessions';
  import { answerFingerprint, pendingInputFor, type AnswerOption, type AnswerView } from './pending_input';

  interface Props {
    session: SessionRow;
    view: AnswerView;
    /** Sidebar density: choices only, no Escape / Open terminal. */
    compact?: boolean;
    onOpenTerminal?: () => void;
  }

  const { session, view, compact = false, onOpenTerminal }: Props = $props();

  let busy = $state(false);
  let errorMsg = $state<string | null>(null);
  let staleMsg = $state<string | null>(null);
  /** What this card has already answered, so it stops offering choices for a
   *  question that is answered. The sidebar row has no probe of its own, so
   *  without this its buttons would stay live until the next 20 s tick. */
  let sent = $state<string | null>(null);

  // A new question is a new card: whatever the last one answered is spent.
  const identity = $derived(answerFingerprint(view));
  $effect(() => {
    void identity;
    sent = null;
    errorMsg = null;
    staleMsg = null;
  });

  /** An option that changes what Claude asks NEXT time, not just this time.
   *  Claude writes these labels, so this reads them rather than guessing
   *  from the ordinal (which differs between dialog kinds). */
  function isSticky(o: AnswerOption): boolean {
    return /don'?t ask again|auto[- ]?accept/i.test(o.label);
  }

  const optionTitle = (o: AnswerOption) =>
    o.key === null
      ? `Option ${o.n} has no single keystroke — answer it in the terminal`
      : `Press ${o.key}${isSticky(o) ? ' — this also stops Claude asking again' : ''}`;

  /** The pane as it is right now, or `null` when it cannot be read.
   *
   *  The reading has to come from the PROBE — `pendingInputFor` falls back to
   *  the row when handed no probe, and the row is precisely the stale value
   *  this check exists to distrust. So an absent probe is refused here rather
   *  than quietly answered from the row. */
  async function reread(): Promise<{ view: AnswerView | null } | { error: string }> {
    const r = await sessionActivity(session.id);
    if (!r.ok) return { error: r.error.message };
    if (!r.value) return { view: null };
    const fresh = pendingInputFor({
      rowStatus: session.claude_status,
      rowStuck: session.stuck_kind,
      rowPending: session.pending_input,
      probe: r.value,
    });
    return { view: fresh?.live ? fresh : null };
  }

  async function press(key: string, label: string) {
    if (busy) return;
    busy = true;
    errorMsg = null;
    staleMsg = null;
    const fresh = await reread();
    if ('error' in fresh) {
      // Not knowing what is on the pane is not the same as knowing it is
      // unchanged: without proof, a keystroke is a guess.
      errorMsg = fresh.error;
      busy = false;
      return;
    }
    if (fresh.view === null || answerFingerprint(fresh.view) !== answerFingerprint(view)) {
      staleMsg = fresh.view === null
        ? 'That dialog is gone — nothing was sent.'
        : 'The dialog changed — nothing was sent.';
      busy = false;
      return;
    }
    const r = await sendPrompt(session.host_alias, session.tmux_name, '', { keys: key });
    if (!r.ok) errorMsg = r.error.message;
    else sent = label;
    busy = false;
  }

  function choose(o: AnswerOption) {
    if (o.key === null) return;
    void press(o.key, o.label);
  }

  /** In the sidebar this card sits inside a row that is itself a button:
   *  answering a question must not also select the session, or tick its
   *  bulk-select checkbox. The card keeps every click it handles. */
  function mine(e: Event, run: () => void) {
    e.stopPropagation();
    run();
  }
</script>

<div class="answer" class:compact data-testid="answer-card" data-kind={view.kind} role="group"
  aria-label={view.kind === 'permission' ? 'Permission request' : 'Question from Claude'}>
  {#if view.question}
    <p class="question" data-testid="answer-question">{view.question}</p>
  {:else}
    <p class="question muted" data-testid="answer-question">
      Claude is waiting for one of these{view.kind === 'permission' ? ' (permission)' : ''}.
    </p>
  {/if}
  {#if sent !== null}
    <p class="sent" role="status" data-testid="answer-sent">✓ Sent: {sent} · <button
        type="button"
        class="linkish"
        data-testid="answer-again"
        title="The dialog is still on screen — the key may not have registered"
        onclick={(e) => mine(e, () => (sent = null))}>Choose again</button></p>
  {:else}
  <div class="options">
    {#each view.options as o (o.n)}
      <button
        type="button"
        class="btn btn--chip option"
        class:sticky={isSticky(o)}
        data-testid="answer-option"
        data-n={o.n}
        data-selected={o.selected || undefined}
        data-sticky={isSticky(o) || undefined}
        disabled={busy || o.key === null}
        title={optionTitle(o)}
        onclick={(e) => mine(e, () => choose(o))}
      ><span class="ordinal" aria-hidden="true">{o.n}</span><span class="label">{o.label}</span></button>
    {/each}
  </div>
  {#if !compact}
    <div class="secondary">
      <button
        type="button"
        class="btn btn--chip btn--quiet"
        data-testid="answer-esc"
        disabled={busy}
        title="Dismiss the dialog (Escape)"
        onclick={(e) => mine(e, () => void press('Escape', 'Dismissed'))}>Esc — dismiss</button>
      {#if onOpenTerminal}
        <button
          type="button"
          class="btn btn--chip btn--quiet"
          data-testid="answer-open-terminal"
          onclick={(e) => mine(e, () => onOpenTerminal?.())}>Open terminal</button>
      {/if}
    </div>
  {/if}
  {/if}
  {#if staleMsg}<p class="stale" role="status" data-testid="answer-stale">{staleMsg}</p>{/if}
  {#if errorMsg}<p class="err" role="status" data-testid="answer-error">{errorMsg}</p>{/if}
</div>

<style>
  .answer {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin: 0.35rem 0 0.6rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--usage-warn);
    border-left-width: 3px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--usage-warn) 10%, var(--bg-pane));
    font-size: 0.8rem;
  }
  .question {
    margin: 0;
    font-weight: 600;
    line-height: 1.35;
  }
  .question.muted {
    font-weight: 500;
    opacity: 0.8;
  }
  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .option {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    max-width: 100%;
  }
  .option .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ordinal {
    flex: none;
    min-width: 1.15em;
    padding: 0 0.15em;
    border-radius: 3px;
    background: color-mix(in srgb, currentColor 16%, transparent);
    font-variant-numeric: tabular-nums;
    text-align: center;
  }
  .option[data-selected] {
    border-color: var(--usage-warn);
  }
  /* The one choice that changes what Claude asks next time reads differently
     from the ones that only answer today. */
  .option.sticky .ordinal {
    background: color-mix(in srgb, var(--usage-warn) 45%, transparent);
  }
  .sent {
    margin: 0;
    line-height: 1.35;
    opacity: 0.85;
  }
  .linkish {
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }
  .secondary {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .stale,
  .err {
    margin: 0;
    line-height: 1.35;
  }
  .err {
    color: var(--danger, #e64a4a);
  }

  /* Sidebar density: the choices are the whole point there. */
  .answer.compact {
    gap: 0.3rem;
    margin: 0.2rem 0 0;
    padding: 0.3rem 0.4rem;
    font-size: 0.72rem;
  }
  .answer.compact .question {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .answer.compact .option .label {
    max-width: 12rem;
  }
</style>
