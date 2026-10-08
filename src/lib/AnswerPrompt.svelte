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
  // latter `compact`), which is why the send lives in this card (and, for
  // the ⌘K Approve command, in `answer_send.ts`, which this card uses too)
  // rather than in each caller: the freshness check is the whole safety of
  // the feature and must not be something a second caller can forget.
  //
  // A multi-select question is answered in two steps, because a digit only
  // TOGGLES a box there: each choice button toggles (and the card stays up),
  // then Continue presses `Tab`, which keeps the ticks and moves Claude Code
  // on — to the next question or to "Review your answers", whose
  // `1. Submit answers` arrives as an ordinary dialog on this same card.
  //
  // Answering re-reads the pane first (`sendAnswer`) and refuses unless the
  // dialog it is about to answer is still the dialog on screen. The row is written by the
  // 20 s reconcile tick, so without that check a click on a stale card could
  // approve a permission dialog the user never saw — or press a digit into
  // the REPL of a session that has already moved on.
  import { sendAnswer } from './answer_send';
  import { destination } from './destination';
  import { matchShortcut } from './shortcuts';
  import { detectMac, isEditable } from './terminal_keys';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import type { SessionRow } from './sessions';
  import {
    answerFingerprint,
    isFreeTextOption,
    MULTI_CONTINUE_KEY,
    type AnswerOption,
    type AnswerView,
  } from './pending_input';

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

  /**
   * Why this client may not answer (multi-user M1, F2b). Both of this card's
   * callers hide it when the client may not write to the row — but the card
   * itself asked neither half, and it is the thing that sends: a key into the
   * pane is `send_prompt`, `drive` in `share.ts::SESSION_TIER`. The gate
   * belongs here for the same reason the freshness re-read does (see the
   * header): this is the one place the send happens, and a second caller must
   * not be able to forget it. It also closes the window the re-read opens — a
   * revoke that lands during `await reread()` is seen by the check below it.
   */
  const writeBlocked = $derived(
    hubActionBlocked('send_prompt', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'send_prompt'),
  );

  /** Boxes this card toggled that no reading of the pane shows yet: a toggle
   *  does not change the question, and the row behind the sidebar's card is
   *  up to a tick old. Once a reading's ticks change, it is the truth again. */
  let toggled = $state<ReadonlySet<number>>(new Set());

  // A new question is a new card: whatever the last one answered is spent.
  const identity = $derived(answerFingerprint(view));
  $effect(() => {
    void identity;
    sent = null;
    errorMsg = null;
    staleMsg = null;
  });
  const ticks = $derived(view.options.filter((o) => o.checked).map((o) => o.n).join());
  $effect(() => {
    void identity;
    void ticks;
    toggled = new Set();
  });

  const isChecked = (o: AnswerOption) => (o.checked === true) !== toggled.has(o.n);

  /** Why a multi-select's boxes cannot be toggled from here right now: with
   *  the cursor in the free-text row, a digit is typed into that box. */
  const toggleBlocked = $derived(
    view.multi && view.options.some((o) => o.selected && isFreeTextOption(o))
      ? 'The cursor is in the “Type something” box, where a digit would be typed — move it in the terminal'
      : null,
  );
  /** An option that changes what Claude asks NEXT time, not just this time.
   *  Claude writes these labels, so this reads them rather than guessing
   *  from the ordinal (which differs between dialog kinds). */
  function isSticky(o: AnswerOption): boolean {
    return /don'?t ask again|auto[- ]?accept/i.test(o.label);
  }

  /** A multi-select's free-text row: a tick on it answers nothing without
   *  text, and text is typed in the terminal. */
  const terminalOnly = (o: AnswerOption) => view.multi && isFreeTextOption(o);

  const optionTitle = (o: AnswerOption) =>
    o.key === null
      ? `Option ${o.n} has no single keystroke — answer it in the terminal`
      : terminalOnly(o)
        ? 'Type your own answer in the terminal'
        : view.multi
          ? (toggleBlocked ?? `Press ${o.key} — ${isChecked(o) ? 'untick' : 'tick'} it`)
          : `Press ${o.key}${isSticky(o) ? ' — this also stops Claude asking again' : ''}`;

  /** Re-read the pane, then press `key` only if it still shows this card's
   *  dialog (`answer_send.ts`). `label` is what the card reports as sent; a
   *  `null` label (a multi-select toggle) leaves the choices up and runs
   *  `onSent` instead. */
  async function press(key: string, label: string | null, onSent?: () => void) {
    if (busy || writeBlocked !== null) return;
    busy = true;
    errorMsg = null;
    staleMsg = null;
    const out = await sendAnswer(session, view, key);
    if (out.ok) {
      if (label !== null) sent = label;
      else onSent?.();
    } else if ('stale' in out) staleMsg = out.stale;
    else if ('error' in out) errorMsg = out.error;
    busy = false;
  }

  function choose(o: AnswerOption) {
    if (o.key === null) return;
    if (!view.multi) {
      void press(o.key, o.label);
      return;
    }
    if (toggleBlocked !== null || terminalOnly(o)) return;
    void press(o.key, null, () => {
      const next = new Set(toggled);
      if (!next.delete(o.n)) next.add(o.n);
      toggled = next;
    });
  }

  /** What Continue reports: the ticked choices, so "Sent" says what went. */
  function continueLabel(): string {
    const ticked = view.options.filter(isChecked).map((o) => o.label);
    return ticked.length ? ticked.join(', ') : 'Nothing ticked';
  }

  // 1–9 answer (redesign step 3.8): only the full card in the Conversation,
  // only while the session view is showing (no overlay over it), and never
  // while a text field, the terminal or a dialog has the keyboard. The
  // sidebar's compact cards take no digits: there could be several.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  function onWindowKeydown(e: KeyboardEvent) {
    if (compact || sent !== null || busy || e.defaultPrevented || $destination !== 'session') return;
    if (matchShortcut('question-card', e, isMac) !== 'question-card.answer') return;
    const target = e.target as HTMLElement | null;
    if (isEditable(target) || target?.dataset?.imeProxy !== undefined || target?.closest?.('dialog')) return;
    const o = view.options.find((x) => x.n === Number(e.key));
    if (!o || o.key === null || writeBlocked !== null) return;
    e.preventDefault();
    choose(o);
  }

  /** In the sidebar this card sits inside a row that is itself a button:
   *  answering a question must not also select the session, or tick its
   *  bulk-select checkbox. The card keeps every click it handles. */
  function mine(e: Event, run: () => void) {
    e.stopPropagation();
    run();
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

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
        data-checked={(view.multi && isChecked(o)) || undefined}
        role={view.multi ? 'checkbox' : undefined}
        aria-checked={view.multi ? isChecked(o) : undefined}
        disabled={busy || writeBlocked !== null || o.key === null || (view.multi && (toggleBlocked !== null || terminalOnly(o)))}
        title={writeBlocked ?? optionTitle(o)}
        onclick={(e) => mine(e, () => choose(o))}
      ><span class="ordinal" aria-hidden="true">{o.n}</span>{#if view.multi}<span class="box" aria-hidden="true">{isChecked(o) ? '✔' : ''}</span>{/if}<span class="label">{o.label}</span></button>
    {/each}
  </div>
  {#if view.multi}
    <div class="multi">
      <button
        type="button"
        class="btn btn--chip continue"
        data-testid="answer-continue"
        disabled={busy || writeBlocked !== null}
        title={writeBlocked ?? 'Keep these ticks and go on (Tab) — Claude asks you to confirm next'}
        onclick={(e) => mine(e, () => void press(MULTI_CONTINUE_KEY, continueLabel()))}>Continue →</button>
      {#if !compact}<span class="hint">Tick every answer that applies, then continue.</span>{/if}
    </div>
  {/if}
  {#if !compact}
    <div class="secondary">
      <button
        type="button"
        class="btn btn--chip btn--quiet"
        data-testid="answer-esc"
        disabled={busy || writeBlocked !== null}
        title={writeBlocked ?? 'Dismiss the dialog (Escape)'}
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
  .box {
    flex: none;
    width: 0.95em;
    height: 0.95em;
    line-height: 0.95em;
    border: 1px solid currentColor;
    border-radius: 2px;
    font-size: 0.85em;
    text-align: center;
    opacity: 0.8;
  }
  .option[data-checked] .box {
    opacity: 1;
    background: color-mix(in srgb, var(--usage-warn) 45%, transparent);
  }
  .multi {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }
  .continue {
    border-color: var(--usage-warn);
    font-weight: 600;
  }
  .hint {
    opacity: 0.75;
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
    color: var(--danger);
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
