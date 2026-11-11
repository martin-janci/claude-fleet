<script lang="ts">
  // A session's diff with review notes (docs/review-notes.md). Click a line
  // number to write a note on that line; notes stay drafts on this device
  // until "Send N notes to Claude", which types them into the session as one
  // numbered prompt. Sent notes show under their line with who sent them,
  // and a person resolves each one. A viewer can write drafts but not send.
  import AskReviewDialog from './AskReviewDialog.svelte';
  import DiffView, { type NoteLine } from './DiffView.svelte';
  import { accessOf } from './access';
  import { errorText } from './error_copy';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import { push } from './toasts';
  import type { SessionRow } from './sessions';
  import {
    lineKey,
    listReviewNotes,
    noteDrafts,
    removeDraft,
    resolveReviewNote,
    saveDraft,
    sendReviewNotes,
    type NoteDraft,
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

  const drafts = $derived(noteDrafts(session));
  let sent = $state<ReviewNote[]>([]);
  /** The line a note is being written on, and the draft it edits. */
  let editing = $state<{ key: string; line: NoteLine; draftId?: string } | null>(null);
  let text = $state('');
  let sending = $state(false);
  let failure = $state<string | null>(null);

  async function loadSent(sid: number): Promise<void> {
    const r = await listReviewNotes(sid);
    if (r.ok && sid === session.id && Array.isArray(r.value)) sent = r.value;
  }
  $effect(() => {
    void loadSent(session.id);
  });

  const draftsHere = $derived(
    new Map(
      $drafts
        .filter((d) => d.path === path)
        .map((d) => [lineKey(d.path, d.side, d.line), d] as const),
    ),
  );
  const sentHere = $derived.by(() => {
    const m = new Map<string, ReviewNote[]>();
    for (const n of sent) {
      if (n.path !== path) continue;
      const k = lineKey(n.path, n.side, n.line);
      m.set(k, [...(m.get(k) ?? []), n]);
    }
    return m;
  });

  const blocked = $derived(
    hubActionBlocked('review_notes', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'review_notes'),
  );

  function start(l: NoteLine, draft?: NoteDraft): void {
    editing = { key: lineKey(path, l.side, l.line), line: l, draftId: draft?.id };
    text = draft?.body ?? '';
    failure = null;
  }
  function save(): void {
    if (!editing) return;
    saveDraft(session, {
      id: editing.draftId,
      path,
      line: editing.line.line,
      side: editing.line.side,
      quote: editing.line.text,
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
    if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      save();
    }
  }

  async function send(): Promise<void> {
    if (sending || blocked || $drafts.length === 0) return;
    sending = true;
    failure = null;
    // A send whose answer never came is retried under its own id (kept with
    // the drafts), so nothing is typed twice.
    const r = await sendReviewNotes(session, $drafts);
    sending = false;
    if (!r.ok) {
      failure = errorText(r.error);
      return;
    }
    const kept = r.value.kept;
    push({
      kind: 'info',
      message: r.value.repeat
        ? kept > 0
          ? `Those notes were already sent. ${kept} ${kept === 1 ? 'note' : 'notes'} added or changed since ${kept === 1 ? 'is' : 'are'} still here to send.`
          : 'Those notes were already sent.'
        : r.value.delivered
          ? `Sent ${r.value.notes.length} ${r.value.notes.length === 1 ? 'note' : 'notes'} to ${agentName}.`
          : `${r.value.notes.length === 1 ? 'The note goes' : 'The notes go'} to ${agentName} when its turn ends.`,
    });
    await loadSent(session.id);
  }

  async function resolve(n: ReviewNote, on: boolean): Promise<void> {
    if (blocked !== null) return;
    const r = await resolveReviewNote(session.id, n.id, on);
    if (!r.ok) {
      push({ kind: 'error', message: errorText(r.error) });
      return;
    }
    sent = sent.map((x) => (x.id === n.id ? r.value : x));
  }

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
        disabled={sending || blocked !== null}
        title={blocked ?? `Type them into the session as one message to ${agentName}`}
        onclick={() => void send()}
        >{sending ? 'Sending…' : `Send ${$drafts.length} ${$drafts.length === 1 ? 'note' : 'notes'} to ${agentName}`}</button
      >
    {/if}
    {#if openSent > 0}
      <span class="count muted" data-testid="review-notes-open">{openSent} sent, not resolved</span>
    {/if}
    {#if failure}
      <span class="err" role="alert" data-testid="review-notes-error">{failure}</span>
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

<DiffView {diff} onnote={(l) => start(l, draftsHere.get(lineKey(path, l.side, l.line)))}>
  {#snippet below(l: NoteLine)}
    {@const k = lineKey(path, l.side, l.line)}
    {@const mine = draftsHere.get(k)}
    {@const theirs = sentHere.get(k) ?? []}
    {#each theirs as n (n.id)}
      <div class="note sent" class:resolved={!!n.resolved_at} data-testid="review-note-sent">
        <div class="head">
          <strong>{n.author}</strong>
          <span class="muted">{n.resolved_at ? `Resolved by ${n.resolved_by ?? 'someone'}` : 'Sent'}</span>
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
    {/each}
    {#if editing?.key === k}
      <div class="note editor" data-testid="review-note-editor">
        <!-- svelte-ignore a11y_autofocus -->
        <textarea
          rows="3"
          bind:value={text}
          onkeydown={onKey}
          autofocus
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
                if (editing?.draftId) removeDraft(session, editing.draftId);
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
          <button type="button" class="link" data-testid="review-note-edit" onclick={() => start(l, mine)}>Edit</button>
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
