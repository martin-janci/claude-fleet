<script lang="ts">
  // Conversation tab: transcript view backed by `session_conversation`
  // (spec §6). Reuses the Files-overlay mechanism at the call site, so a
  // tmux session's PTY stays mounted underneath while this is shown.
  //
  // A tmux-backed session also gets a composer at the bottom: the prompt
  // goes through the same `send_prompt` as the Send-prompt dialog (tmux
  // send-keys into the REPL), so anything typed here is what the terminal
  // would have received. bg / external rows have no REPL to type into, so
  // they stay read-only.
  import { untrack, tick } from 'svelte';
  import { sendPrompt, hasNoPane, type SessionRow } from './sessions';
  import {
    sessionConversation,
    sameConversation,
    isPinned,
    emptyStateText,
    relativeTime,
    groupItems,
    toolGroupLabel,
    isLongPrompt,
    composerStatus,
    transcriptCarries,
    CONVERSATION_POLL_MS,
    type Conversation,
    type PendingPrompt,
  } from './conversation';
  import Markdown from './MarkdownView.svelte';

  let { session, visible }: { session: SessionRow; visible: boolean } = $props();

  let conv = $state<Conversation | null>(null);
  let errorCode = $state<string | null>(null);
  let errorMsg = $state<string | null>(null);
  let loading = $state(false);
  let scroller: HTMLDivElement | undefined = $state();
  let nowMs = $state(Date.now());
  // Turn indexes whose long prompt the user expanded.
  let expanded = $state<Set<number>>(new Set());
  // False once the user scrolls away from the bottom; drives "↓ Latest".
  let atBottom = $state(true);
  // Composer state. `pending` is the prompt just sent, rendered as its own
  // turn until a poll brings back a transcript that carries it.
  let draft = $state('');
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  let pending = $state<PendingPrompt | null>(null);
  let box: HTMLTextAreaElement | undefined = $state();
  let seq = 0;
  // Fetches still pending, per session id. A poll tick never starts a read
  // while one is in flight for the same session (a remote read can take up
  // to its 20 s wall clock); a manual Retry or a session switch still does.
  const inFlight = new Map<number, number>();
  // Keyed on the id, not the row object: a store patch hands a new object
  // for the same session, which must neither reset nor refetch.
  const sessionId = $derived(session.id);

  async function load(opts: { poll?: boolean } = {}) {
    const id = session.id;
    if (!session.claude_session_id) return;
    if (opts.poll && (inFlight.get(id) ?? 0) > 0) return;
    const mine = ++seq;
    loading = conv === null;
    const pinned = scroller ? isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight) : true;
    inFlight.set(id, (inFlight.get(id) ?? 0) + 1);
    let r: Awaited<ReturnType<typeof sessionConversation>>;
    try {
      r = await sessionConversation(id);
    } finally {
      const left = (inFlight.get(id) ?? 1) - 1;
      if (left > 0) inFlight.set(id, left);
      else inFlight.delete(id);
    }
    // Drop a stale response: either a newer fetch has started, or the
    // session prop moved on while this one was in flight.
    if (mine !== seq || session.id !== id) return;
    loading = false;
    if (r.ok) {
      errorCode = null;
      errorMsg = null;
      if (!sameConversation(conv, r.value)) {
        conv = r.value;
        if (pending && transcriptCarries(conv, pending)) pending = null;
        if (pinned) {
          await tick();
          scrollToBottom();
        }
      }
    } else {
      errorCode = r.error.code;
      errorMsg = r.error.message;
    }
  }

  // Reset + immediate fetch on session change.
  $effect(() => {
    void sessionId;
    untrack(() => {
      seq++;
      conv = null;
      errorCode = null;
      errorMsg = null;
      expanded = new Set();
      atBottom = true;
      draft = '';
      sendError = null;
      pending = null;
      void load();
    });
  });

  // Poll while shown, and refetch at once when it becomes shown again (not
  // up to a poll interval later). Depends only on `visible` so a `conv`
  // update from `load()` does not tear down and restart the interval.
  let wasVisible = untrack(() => visible);
  $effect(() => {
    if (!visible) {
      wasVisible = false;
      return;
    }
    if (!wasVisible) {
      wasVisible = true;
      void untrack(() => load({ poll: true }));
    }
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') void untrack(() => load({ poll: true }));
    }, CONVERSATION_POLL_MS);
    return () => clearInterval(t);
  });

  // Independent clock for the relative-time labels, decoupled from content
  // changes: an unchanged conversation (the common case between polls) must
  // still see "just now" age into "1m ago" etc. (same pattern as
  // SessionDetails.svelte's `nowSec` ticker).
  $effect(() => {
    const t = setInterval(() => (nowMs = Date.now()), 30_000);
    return () => clearInterval(t);
  });

  const empty = $derived(emptyStateText(errorCode, !!session.claude_session_id));
  const canPrompt = $derived(!hasNoPane(session));
  const canSend = $derived(draft.trim().length > 0 && !sending);
  const statusNote = $derived(composerStatus(session));

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    sending = true;
    sendError = null;
    const r = await sendPrompt(session.host_alias, session.tmux_name, text);
    sending = false;
    if (!r.ok) {
      sendError = r.error.message;
      return;
    }
    draft = '';
    pending = {
      prompt: text,
      at: new Date().toISOString(),
      // how many turns already carried this exact text, so a repeat of an
      // earlier prompt is not mistaken for the transcript catching up
      seen: conv?.turns.filter((t) => t.prompt === text).length ?? 0,
    };
    await tick();
    scrollToBottom();
    box?.focus();
    // Refetch now rather than up to a poll interval later.
    void load();
  }

  function onComposerKey(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.shiftKey || e.altKey || e.isComposing) return;
    e.preventDefault();
    void send();
  }

  function scrollToBottom() {
    if (!scroller) return;
    scroller.scrollTop = scroller.scrollHeight;
    atBottom = true;
  }

  function onScroll() {
    if (scroller) atBottom = isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight);
  }

  function togglePrompt(i: number) {
    const next = new Set(expanded);
    if (next.has(i)) next.delete(i);
    else next.add(i);
    expanded = next;
  }
</script>

<div class="conversation-panel" data-testid="conversation-panel">
  <div class="thread-area">
  {#if empty && !pending}
    <p class="muted" data-testid="conv-empty">{empty}</p>
  {:else if loading}
    <p class="muted">Loading…</p>
  {:else}
    <div class="scroller" data-testid="conv-scroller" bind:this={scroller} onscroll={onScroll}>
      <div class="thread">
        {#if errorMsg && !empty}
          <div class="error-row">
            <span class="err" data-testid="conv-error">{errorMsg}</span>
            <button type="button" data-testid="conv-retry" onclick={() => void load()}>Retry</button>
          </div>
        {/if}
        {#if conv?.truncated}
          <p class="muted truncated">Older turns not shown</p>
        {/if}
        {#if conv}
          {#each conv.turns as turn, i (i)}
            <section class="turn">
              {#if turn.prompt !== null}
                {@const long = isLongPrompt(turn.prompt)}
                <div class="prompt" data-testid="conv-prompt">
                  <div class="prompt-head">
                    <span class="who">You</span>
                    {#if turn.at}
                      <time datetime={turn.at} title={new Date(turn.at).toLocaleString()}
                        >{relativeTime(turn.at, nowMs)}</time
                      >
                    {/if}
                  </div>
                  <div class="prompt-text" class:clamped={long && !expanded.has(i)}>{turn.prompt}</div>
                  {#if long}
                    <button type="button" class="linkish" data-testid="conv-prompt-toggle" onclick={() => togglePrompt(i)}
                      >{expanded.has(i) ? 'Show less' : 'Show more'}</button
                    >
                  {/if}
                </div>
              {/if}
              <div class="reply">
                {#each groupItems(turn.items) as g, j (j)}
                  {#if g.kind === 'text'}
                    <div class="text" data-testid="conv-text"><Markdown source={g.text} /></div>
                  {:else if g.tools.length === 1}
                    <div class="tool" data-testid="conv-tool" title={g.tools[0]}>{g.tools[0]}</div>
                  {:else}
                    <details class="tools" data-testid="conv-tools">
                      <summary>{toolGroupLabel(g.tools)}</summary>
                      {#each g.tools as summary, k (k)}
                        <div class="tool" data-testid="conv-tool" title={summary}>{summary}</div>
                      {/each}
                    </details>
                  {/if}
                {/each}
              </div>
            </section>
          {/each}
        {/if}
        {#if pending}
          <section class="turn pending" data-testid="conv-pending">
            <div class="prompt">
              <div class="prompt-head">
                <span class="who">You</span>
                <time datetime={pending.at}>{relativeTime(pending.at, nowMs)}</time>
              </div>
              <div class="prompt-text">{pending.prompt}</div>
            </div>
            <div class="reply waiting">Sent. Waiting for the transcript…</div>
          </section>
        {/if}
      </div>
    </div>
    {#if !atBottom}
      <button type="button" class="latest" data-testid="conv-latest" onclick={scrollToBottom}>↓ Latest</button>
    {/if}
  {/if}
  </div>
  {#if canPrompt}
    <form
      class="composer"
      data-testid="conv-composer"
      onsubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      {#if sendError}
        <div class="composer-error" data-testid="conv-composer-error">{sendError}</div>
      {/if}
      <div class="composer-row">
        <textarea
          data-testid="conv-composer-input"
          bind:this={box}
          bind:value={draft}
          onkeydown={onComposerKey}
          rows="2"
          placeholder="Send a prompt to this session (Enter to send, Shift+Enter for a new line)"
          disabled={sending}
        ></textarea>
        <button type="submit" data-testid="conv-composer-send" disabled={!canSend}>{sending ? 'Sending…' : 'Send'}</button>
      </div>
      {#if statusNote}
        <div class="composer-status" data-testid="conv-composer-status">{statusNote}</div>
      {/if}
    </form>
  {:else}
    <p class="muted readonly" data-testid="conv-readonly">Read-only: this agent runs outside tmux, so there is no terminal to prompt.</p>
  {/if}
</div>

<style>
  .conversation-panel {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }
  .thread-area {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .scroller {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .readonly {
    flex: 0 0 auto;
    border-top: 1px solid var(--border);
    margin: 0;
    padding: 0.5rem 1.1rem;
  }
  .composer {
    flex: 0 0 auto;
    border-top: 1px solid var(--border);
    background: var(--bg-pane);
    padding: 0.55rem 1.1rem 0.6rem;
  }
  .composer-row {
    display: flex;
    align-items: flex-end;
    gap: 0.5rem;
    max-width: 80ch;
    margin: 0 auto;
  }
  .composer textarea {
    flex: 1 1 auto;
    min-height: 2.6rem;
    max-height: 12rem;
    resize: vertical;
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--fg);
    font: inherit;
    font-size: 0.85rem;
    line-height: 1.45;
  }
  .composer textarea:focus {
    outline: none;
    border-color: var(--accent);
  }
  .composer button {
    flex: 0 0 auto;
    padding: 0.45rem 0.9rem;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--accent);
    color: var(--bg);
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
  }
  .composer button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .composer-error,
  .composer-status {
    max-width: 80ch;
    margin: 0 auto 0.35rem;
    font-size: 0.75rem;
  }
  .composer-error {
    color: #e64a4a;
  }
  .composer-status {
    margin: 0.35rem auto 0;
    color: var(--fg-muted);
  }
  .waiting {
    color: var(--fg-muted);
    font-style: italic;
    font-size: 0.8rem;
  }
  .thread {
    max-width: 80ch;
    margin: 0 auto;
    padding: 1rem 1.1rem 2.5rem;
  }
  .muted { color: var(--fg-muted); font-style: italic; font-size: 0.8rem; margin: 0.6rem; }
  .truncated { text-align: center; margin: 0 0 1rem; }
  .error-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }
  .err { color: #e64a4a; font-size: 0.8rem; }
  .turn {
    padding: 0.2rem 0 1.1rem;
  }
  .turn + .turn {
    border-top: 1px solid var(--border);
    padding-top: 1.1rem;
  }
  .prompt {
    margin: 0 0 0.8rem;
    padding: 0.5rem 0.75rem 0.55rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 6%, var(--bg-pane));
  }
  .prompt-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.6rem;
    margin-bottom: 0.2rem;
  }
  .who {
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--accent);
  }
  time {
    flex: 0 0 auto;
    color: var(--fg-muted);
    font-size: 0.7rem;
    white-space: nowrap;
  }
  .prompt-text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 0.85rem;
    line-height: 1.5;
    color: var(--fg);
  }
  .prompt-text.clamped {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 6;
    line-clamp: 6;
    overflow: hidden;
  }
  .linkish {
    margin-top: 0.25rem;
    padding: 0;
    background: none;
    border: none;
    color: var(--accent);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .reply {
    font-size: 0.875rem;
    line-height: 1.6;
    color: var(--fg);
    overflow-wrap: break-word;
  }
  .text {
    margin: 0.35rem 0 0.6rem;
  }
  .tool {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.74rem;
    line-height: 1.5;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0.1rem 0;
    padding-left: 1.1rem;
    position: relative;
  }
  .tool::before {
    content: '⚙';
    position: absolute;
    left: 0;
    opacity: 0.6;
  }
  .tools {
    margin: 0.3rem 0 0.5rem;
  }
  .tools summary {
    cursor: pointer;
    font-size: 0.76rem;
    color: var(--fg-muted);
    list-style: none;
    user-select: none;
  }
  .tools summary::-webkit-details-marker {
    display: none;
  }
  .tools summary::before {
    content: '›';
    display: inline-block;
    width: 0.7rem;
    margin-right: 0.4rem;
    text-align: center;
    font-size: 1rem;
    line-height: 1;
    transition: transform 0.12s ease;
  }
  .tools[open] summary::before {
    transform: rotate(90deg);
  }
  .tools summary:hover {
    color: var(--fg);
  }
  .tools .tool {
    margin-left: 1.1rem;
  }
  .latest {
    position: absolute;
    right: 1rem;
    bottom: 1rem;
    padding: 0.3rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-pane);
    color: var(--fg);
    font-size: 0.75rem;
    cursor: pointer;
    box-shadow: 0 2px 8px color-mix(in srgb, var(--fg) 15%, transparent);
  }
  .latest:hover {
    border-color: var(--accent);
  }
</style>
