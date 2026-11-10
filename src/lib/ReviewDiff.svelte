<script lang="ts">
  // A session's diff with review notes (docs/review-notes.md). Click a line
  // number to write a note on that line; notes stay drafts on this device
  // until "Send N notes to Claude", which types them into the session as one
  // numbered prompt. Sent notes show under their line with who sent them,
  // and a person resolves each one. A viewer can write drafts but not send.
  //
  // A sent note sits under a row only where that row still reads as the
  // note's quote; the rest (another file, a line that changed or is gone, a
  // whole-file note) are listed above the diff, each with Resolve.
  import { get } from 'svelte/store';
  import AskReviewDialog from './AskReviewDialog.svelte';
  import DiffView, { parseUnifiedDiff, type NoteLine } from './DiffView.svelte';
  import { accessOf } from './access';
  import { errorText } from './error_copy';
  import { focusNow } from './focus_now';
  import { formKeys } from './forms/form_frame';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import { push } from './toasts';
  import { sessionIdentity, type SessionRow } from './sessions';
  import {
    lineKey,
    listReviewNotes,
    NOTE_MAX,
    NOTES_MAX,
    noteDrafts,
    pendingSend,
    quoteOf,
    removeDraft,
    resolveReviewNote,
    saveDraft,
    sendFailureText,
    sendReviewNotes,
    type ReviewNote,
  } from './review_notes';

  let {
    session,
    path,
    diff,
    agentName = 'Claude',
  }: {
    session: SessionRow;
    path: string;
    diff: string;
    agentName?: string;
  } = $props();

  // Keyed on the id and the identity, not the row object: a row event hands
  // a new object for the same session, which must neither list again (an
  // audited call on a hub) nor swap the drafts.
  const sid = $derived(session.id);
  const ident = $derived(sessionIdentity(session));
  const drafts = $derived(noteDrafts(ident));
  const pending = $derived(pendingSend(ident));
  /** A send of this session is running, from this view or another. */
  const sending = $derived(!!$pending?.inFlight);
  let sent = $state<ReviewNote[]>([]);
  /** The line a note is being written on, and the draft it edits. */
  let editing = $state<{ key: string; line: NoteLine; draftId?: string } | null>(null);
  let text = $state('');
  let failure = $state<string | null>(null);

  async function loadSent(id: number): Promise<void> {
    const r = await listReviewNotes(id);
    if (r.ok && id === sid && Array.isArray(r.value)) sent = r.value;
  }
  $effect(() => {
    void loadSent(sid);
  });

  const draftsHere = $derived(
    new Map(
      $drafts
        .filter((d) => d.path === path)
        .map((d) => [lineKey(d.path, d.side, d.line), d] as const),
    ),
  );
  /** Each noteable row's quote, by its key. */
  const rowQuotes = $derived.by(() => {
    const m = new Map<string, string>();
    for (const r of parseUnifiedDiff(diff)) {
      if (r.kind === 'del' && r.oldNo !== null) m.set(lineKey(path, 'old', r.oldNo), quoteOf(r.text));
      else if ((r.kind === 'add' || r.kind === 'ctx') && r.newNo !== null) m.set(lineKey(path, 'new', r.newNo), quoteOf(r.text));
    }
    return m;
  });
  /** Sent notes placed on a row of this diff (the row reads as the note's
   *  quote), and the rest, listed above it. */
  const placed = $derived.by(() => {
    const here = new Map<string, ReviewNote[]>();
    const elsewhere: ReviewNote[] = [];
    for (const n of sent) {
      const k = lineKey(n.path, n.side, n.line);
      const row = n.path === path && n.line != null ? rowQuotes.get(k) : undefined;
      if (row !== undefined && row === quoteOf(n.quote)) here.set(k, [...(here.get(k) ?? []), n]);
      else elsewhere.push(n);
    }
    return { here, elsewhere };
  });

  const blocked = $derived(
    hubActionBlocked('review_notes', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'review_notes'),
  );
  const tooMany = $derived(
    $drafts.length > NOTES_MAX ? `At most ${NOTES_MAX} notes go in one send: delete ${$drafts.length - NOTES_MAX} first.` : null,
  );

  function start(l: NoteLine): void {
    const k = lineKey(path, l.side, l.line);
    // A note being typed on another line is kept, not dropped.
    if (editing && text.trim()) save();
    const draft = get(noteDrafts(ident)).find((d) => d.path === path && lineKey(d.path, d.side, d.line) === k);
    editing = { key: k, line: l, draftId: draft?.id };
    text = draft?.body ?? '';
    failure = null;
  }
  function save(): void {
    if (!editing) return;
    saveDraft(ident, {
      id: editing.draftId,
      path,
      line: editing.line.line,
      side: editing.line.side,
      quote: quoteOf(editing.line.text),
      body: text,
    });
    editing = null;
    text = '';
  }
  function cancel(): void {
    editing = null;
    text = '';
  }
  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape' && !e.isComposing) {
      e.preventDefault();
      cancel();
    }
  }

  async function send(): Promise<void> {
    if (sending || blocked || tooMany || $drafts.length === 0) return;
    failure = null;
    const r = await sendReviewNotes(sid, ident);
    if (!r.ok) {
      failure = sendFailureText(r.error);
      return;
    }
    push({
      kind: 'info',
      message: r.value.delivered
        ? `Sent ${r.value.notes.length} ${r.value.notes.length === 1 ? 'note' : 'notes'} to ${agentName}.`
        : r.value.repeat
          ? 'Those notes were already sent.'
          : `${r.value.notes.length === 1 ? 'The note goes' : 'The notes go'} to ${agentName} when its turn ends.`,
    });
    await loadSent(sid);
  }

  async function resolve(n: ReviewNote, on: boolean): Promise<void> {
    if (blocked !== null) return;
    const r = await resolveReviewNote(sid, n.id, on);
    if (!r.ok) {
      push({ kind: 'error', message: errorText(r.error) });
      return;
    }
    sent = sent.map((x) => (x.id === n.id ? r.value : x));
  }

  /** A note is "Sent" once its prompt was typed; a queued one says so. */
  const deliveryWords = (d: ReviewNote['delivery']) =>
    d === 'waiting'
      ? 'Waiting to be sent'
      : d === 'not_delivered'
        ? 'Not delivered'
        : d === 'uncertain'
          ? 'May not have arrived'
          : 'Sent';
  const openSent = $derived(sent.filter((n) => !n.resolved_at).length);
  /** "Ask for review…" is the owner's (it shares the session), and only on a
   *  hub: a desktop with no hub has nobody to ask. */
  const owned = $derived($accessOf(session) === 'own' && $hubStatus.remote);
  let asking = $state(false);
</script>

{#if asking}
  <AskReviewDialog {session} onclose={() => (asking = false)} />
{/if}

{#if $drafts.length > 0 || openSent > 0 || owned}
  <div class="notes-bar" data-testid="review-notes-bar" role="group" aria-label="Review notes">
    {#if $drafts.length > 0}
      <span class="count" data-testid="review-notes-drafts"
        >{$drafts.length} {$drafts.length === 1 ? 'note' : 'notes'} not sent</span
      >
      <button
        type="button"
        class="primary"
        data-testid="review-notes-send"
        disabled={sending || blocked !== null || tooMany !== null}
        title={blocked ?? tooMany ?? `Type them into the session as one message to ${agentName}`}
        onclick={() => void send()}
        >{sending ? 'Sending…' : `Send ${$drafts.length} ${$drafts.length === 1 ? 'note' : 'notes'} to ${agentName}`}</button
      >
    {/if}
    {#if openSent > 0}
      <span class="count muted" data-testid="review-notes-open">{openSent} sent, not resolved</span>
    {/if}
    {#if failure || tooMany}
      <span class="err" role="alert" data-testid="review-notes-error">{failure ?? tooMany}</span>
    {/if}
    {#if owned}
      <button
        type="button"
        class="ask"
        data-testid="review-ask"
        title="Share this session with a colleague and ask them to review its changes"
        onclick={() => (asking = true)}>Ask for review…</button
      >
    {/if}
  </div>
{/if}

{#snippet sentNote(n: ReviewNote, where: boolean)}
  <div class="note sent" class:resolved={!!n.resolved_at} data-testid={where ? 'review-note-elsewhere' : 'review-note-sent'}>
    {#if where}
      <div class="where">
        <span class="path">{n.path}{n.line != null ? `:${n.line}` : ''}{n.side === 'old' ? ' (removed)' : ''}</span>
        {#if n.quote}<code class="quote">{n.quote}</code>{/if}
      </div>
    {/if}
    <div class="head">
      <strong>{n.author}</strong>
      <span class="muted" title={n.delivery_error ?? undefined} data-testid="review-note-state"
        >{n.resolved_at ? `Resolved by ${n.resolved_by ?? 'someone'}` : deliveryWords(n.delivery)}</span
      >
      <button
        type="button"
        class="link"
        data-testid="review-note-resolve"
        disabled={blocked !== null}
        title={blocked ?? ''}
        onclick={() => void resolve(n, !n.resolved_at)}>{n.resolved_at ? 'Reopen' : 'Resolve'}</button
      >
    </div>
    <p class="body">{n.body}</p>
  </div>
{/snippet}

{#if placed.elsewhere.length > 0}
  <section class="elsewhere" data-testid="review-notes-elsewhere" aria-label="Notes elsewhere or outdated">
    <h4>Elsewhere / outdated</h4>
    {#each placed.elsewhere as n (n.id)}
      {@render sentNote(n, true)}
    {/each}
  </section>
{/if}

<DiffView {diff} onnote={(l) => start(l)}>
  {#snippet below(l: NoteLine)}
    {@const k = lineKey(path, l.side, l.line)}
    {@const mine = draftsHere.get(k)}
    {#each placed.here.get(k) ?? [] as n (n.id)}
      {@render sentNote(n, false)}
    {/each}
    {#if editing?.key === k}
      <div class="note editor" data-testid="review-note-editor" use:formKeys={{ submit: save, enabled: !!text.trim() }}>
        <textarea
          rows="3"
          bind:value={text}
          onkeydown={onKey}
          use:focusNow
          maxlength={NOTE_MAX}
          aria-label="Note on line {l.line}"
          placeholder="What should change here?"
          data-testid="review-note-text"
        ></textarea>
        <div class="row-actions">
          <button type="button" onclick={cancel}>Cancel</button>
          {#if editing.draftId}
            <button
              type="button"
              data-testid="review-note-delete"
              onclick={() => {
                if (editing?.draftId) removeDraft(ident, editing.draftId);
                cancel();
              }}>Delete</button
            >
          {/if}
          <button type="button" class="primary" data-testid="review-note-save" disabled={!text.trim()} onclick={save}
            >Save note</button
          >
        </div>
      </div>
    {:else if mine}
      <div class="note draft" data-testid="review-note-draft">
        <div class="head">
          <span class="muted">Not sent</span>
          <button type="button" class="link" data-testid="review-note-edit" onclick={() => start(l)}>Edit</button>
        </div>
        <p class="body">{mine.body}</p>
      </div>
    {/if}
  {/snippet}
</DiffView>

<style>
  .notes-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem 0.6rem;
    padding: 0.35rem 0.5rem;
    border-bottom: 1px solid var(--border);
    font-size: var(--text-xs);
  }
  .count { color: var(--fg); }
  .muted { color: var(--fg-muted); }
  .err { color: var(--danger); }
  .ask { margin-left: auto; }
  .note {
    margin: 0.2rem 0.5rem 0.3rem 4.6em;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    font-family: var(--font-sans, inherit);
    font-size: var(--text-xs);
    white-space: normal;
  }
  .note.draft { border-style: dashed; }
  .elsewhere {
    padding: 0.3rem 0 0.2rem;
    border-bottom: 1px solid var(--border);
  }
  .elsewhere h4 {
    margin: 0 0.5rem;
    font-size: var(--text-2xs);
    font-weight: 500;
    color: var(--fg-muted);
  }
  .elsewhere .note { margin-left: 0.5rem; }
  .where { display: flex; flex-direction: column; gap: 0.1rem; margin-bottom: 0.2rem; }
  .path { font-family: var(--mono); color: var(--fg-muted); overflow-wrap: anywhere; }
  .quote {
    font-family: var(--mono);
    font-size: var(--text-2xs);
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .note.resolved { opacity: 0.6; }
  .head { display: flex; gap: 0.5rem; align-items: baseline; }
  .head .link { margin-left: auto; }
  .body { margin: 0.2rem 0 0; white-space: pre-wrap; }
  textarea {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
    resize: vertical;
  }
  .row-actions { display: flex; gap: 0.4rem; justify-content: flex-end; margin-top: 0.3rem; }
  button {
    font-size: var(--text-xs);
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button.primary { border-color: var(--accent); }
  button.link { border: 0; padding: 0; color: var(--accent); background: none; }
</style>
