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
  import { hintAnchor } from './hints';
  import { composerPresets, type ComposerPreset } from './composer_presets';
  import {
    sessionConversation,
    sameConversation,
    isPinned,
    emptyStateText,
    relativeTime,
    groupItems,
    toolGroupLabel,
    isLongPrompt,
    turnDuration,
    composerStatus,
    transcriptCarries,
    matchSlashCommands,
    completeSlashCommand,
    sessionActivity,
    indicatorFor,
    composerDrafts,
    rememberDraft,
    newItemCount,
    isQuietStatus,
    shouldFetchTranscript,
    CONVERSATION_POLL_MS,
    ACTIVITY_POLL_MS,
    type Conversation,
    type PendingPrompt,
    type SlashCommand,
    type ActivityProbe,
  } from './conversation';
  import Markdown from './MarkdownView.svelte';

  let {
    session,
    visible,
    onOpenTerminal,
  }: { session: SessionRow; visible: boolean; onOpenTerminal?: () => void } = $props();

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
  // Items that landed while the user was scrolled up; shown on the button.
  let unseen = $state(0);
  // Composer state. `pending` is the prompt just sent, rendered as its own
  // turn until a poll brings back a transcript that carries it.
  let draft = $state('');
  // The session the current `draft` was loaded for. The remember effect keys
  // on this, not on the prop, so a session switch can never write the old
  // text into the new session's slot whatever order the effects run in.
  let draftFor = $state<number | null>(null);
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  let pending = $state<PendingPrompt | null>(null);
  let box: HTMLTextAreaElement | undefined = $state();
  // Slash-command menu: highlighted row, and the draft the user dismissed
  // the menu for (Escape) so it stays hidden until the text changes.
  let slashIndex = $state(0);
  let slashDismissedFor = $state<string | null>(null);
  // Live indicator. `probe` is the latest on-demand pane read, laid over the
  // row's (tick-fresh) status while it is newer than the row; it is dropped
  // as soon as a row event carries a newer state. `sentTurnSeq` marks our
  // own send: until the row's turn counter moves past it, or a probe reports
  // the session idle, the session is treated as working.
  let probe = $state<ActivityProbe | null>(null);
  let sentTurnSeq = $state<number | null>(null);
  let idleSeenSinceSend = $state(false);
  let lastFetchAt = 0;
  let lastFetchTurnSeq = $state<number | null>(null);
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
    lastFetchAt = Date.now();
    lastFetchTurnSeq = session.turn_seq;
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
        if (!pinned) unseen += newItemCount(conv, r.value);
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
      unseen = 0;
      draft = composerDrafts.get(session.id) ?? '';
      draftFor = session.id;
      sendError = null;
      pending = null;
      probe = null;
      sentTurnSeq = null;
      idleSeenSinceSend = false;
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
      if (document.visibilityState !== 'visible') return;
      untrack(() => {
        const quiet = isQuietStatus(liveStatus) && !pending && !optimistic;
        if (
          shouldFetchTranscript({
            quiet,
            sinceLastFetchMs: Date.now() - lastFetchAt,
            turnSeqChanged: session.turn_seq !== lastFetchTurnSeq,
          })
        ) {
          void load({ poll: true });
        }
      });
    }, CONVERSATION_POLL_MS);
    return () => clearInterval(t);
  });

  // A Stop hook bumps the row's turn counter (row event): the transcript has
  // a finished turn to show, so read it now rather than on the next tick.
  $effect(() => {
    const turn = session.turn_seq;
    untrack(() => {
      if (lastFetchTurnSeq !== null && turn !== lastFetchTurnSeq && session.claude_session_id) void load();
    });
  });

  // A row event with a changed status is newer than any probe.
  $effect(() => {
    void session.claude_status;
    void session.stuck_kind;
    untrack(() => (probe = null));
  });

  const liveStatus = $derived(probe?.claude_status ?? session.claude_status);
  const liveStuck = $derived(probe?.stuck_kind ?? session.stuck_kind);
  const liveActivity = $derived(probe?.current_activity ?? session.current_activity);
  const optimistic = $derived(sentTurnSeq !== null && session.turn_seq === sentTurnSeq && !idleSeenSinceSend);
  const indicator = $derived(
    indicatorFor({
      status: liveStatus,
      stuckKind: liveStuck,
      waitingFor: probe?.waiting_for ?? null,
      activity: liveActivity,
      spinner: probe?.spinner ?? null,
      pending: pending !== null,
      optimistic,
    }),
  );

  async function probeNow() {
    const id = session.id;
    const r = await sessionActivity(id);
    if (session.id !== id) return;
    if (!r.ok) return;
    probe = r.value;
    if (sentTurnSeq !== null && isQuietStatus(r.value.claude_status)) idleSeenSinceSend = true;
  }

  // Probe the pane every couple of seconds while something is live. Depends
  // on `visible` and whether the indicator is showing, not on the probe
  // itself, so a fresh probe never restarts the interval.
  $effect(() => {
    const live = visible && indicator !== null;
    if (!live) return;
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') void untrack(probeNow);
    }, ACTIVITY_POLL_MS);
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

  // Keep the unsent text across tab switches (the panel unmounts).
  $effect(() => {
    if (draftFor !== null) rememberDraft(draftFor, draft);
  });

  // Put the cursor in the composer when the tab shows a promptable session,
  // and again when the selection moves to another one.
  $effect(() => {
    void sessionId;
    if (!visible || !canPrompt) return;
    void tick().then(() => box?.focus());
  });
  const canSend = $derived(draft.trim().length > 0 && !sending);
  const statusNote = $derived(composerStatus({ claude_status: liveStatus, stuck_kind: liveStuck }));
  const slashMatches = $derived(slashDismissedFor === draft ? [] : matchSlashCommands(draft));
  const slashOpen = $derived(slashMatches.length > 0);
  // Keep the highlight inside the list as the prefix narrows it.
  $effect(() => {
    if (slashIndex >= slashMatches.length) slashIndex = 0;
  });

  /** A chip fills the box (Shift+click sends at once). A filled command does
   *  not pop the slash menu: the user picked it already. */
  function usePreset(p: ComposerPreset, sendNow: boolean) {
    if (sendNow) {
      void sendText(p.text.trim() || p.text);
      return;
    }
    draft = p.text;
    slashDismissedFor = draft;
    box?.focus();
  }

  function acceptSlash(c: SlashCommand) {
    draft = completeSlashCommand(c);
    slashDismissedFor = draft;
    box?.focus();
  }

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    await sendText(text);
  }

  /** Send `text` as-is. Empty text is a bare Enter (the press_enter chip):
   *  it lands in the REPL but is not a prompt, so nothing is shown pending. */
  async function sendText(text: string) {
    if (sending) return;
    sending = true;
    sendError = null;
    const r = await sendPrompt(session.host_alias, session.tmux_name, text);
    sending = false;
    if (!r.ok) {
      sendError = r.error.message;
      return;
    }
    if (text === '') return;
    draft = '';
    sentTurnSeq = session.turn_seq;
    idleSeenSinceSend = false;
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
    if (e.isComposing) return;
    if (slashOpen) {
      const highlighted = slashMatches[slashIndex] ?? slashMatches[0];
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        slashIndex = (slashIndex + 1) % slashMatches.length;
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        slashIndex = (slashIndex - 1 + slashMatches.length) % slashMatches.length;
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        acceptSlash(highlighted);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        slashDismissedFor = draft;
        return;
      }
      // Enter on a partial name completes it; on the exact name it sends.
      if (e.key === 'Enter' && !e.shiftKey && !e.altKey && draft !== `/${highlighted.name}`) {
        e.preventDefault();
        acceptSlash(highlighted);
        return;
      }
    }
    if (e.key !== 'Enter' || e.shiftKey || e.altKey) return;
    e.preventDefault();
    void send();
  }

  function scrollToBottom() {
    if (!scroller) return;
    scroller.scrollTop = scroller.scrollHeight;
    atBottom = true;
    unseen = 0;
  }

  function onScroll() {
    if (!scroller) return;
    atBottom = isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight);
    if (atBottom) unseen = 0;
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
            {@const isLast = i === conv.turns.length - 1}
            {@const running = isLast && indicator?.kind === 'working'}
            {@const groups = groupItems(turn.items)}
            {@const duration = running ? null : turnDuration(turn.at, turn.ended_at)}
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
                {#each groups as g, j (j)}
                  {#if g.kind === 'text'}
                    <div class="text" data-testid="conv-text"><Markdown source={g.text} /></div>
                  {:else if g.tools.length === 1}
                    <div class="tool" class:err={g.tools[0].error} data-testid="conv-tool" data-error={g.tools[0].error || undefined} title={g.tools[0].error ? `Failed: ${g.tools[0].summary}` : g.tools[0].summary}>{g.tools[0].summary}</div>
                  {:else}
                    <details class="tools" class:has-err={g.tools.some((t) => t.error)} open={running && j === groups.length - 1} data-testid="conv-tools">
                      <summary>{toolGroupLabel(g.tools)}</summary>
                      {#each g.tools as line, k (k)}
                        <div class="tool" class:err={line.error} data-testid="conv-tool" data-error={line.error || undefined} title={line.error ? `Failed: ${line.summary}` : line.summary}>{line.summary}</div>
                      {/each}
                    </details>
                  {/if}
                {/each}
                {#if duration}
                  <div class="duration" data-testid="conv-duration" title="From the prompt to the reply's last entry">{duration}</div>
                {/if}
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
          </section>
        {/if}
        {#if indicator?.kind === 'blocked'}
          <div class="blocked" data-testid="conv-blocked" role="status">
            <div class="blocked-text">
              <strong>Claude is waiting for you in the terminal{indicator.waiting === 'permission' ? ' (permission)' : indicator.waiting === 'input' ? ' (input)' : ''}.</strong>
              {#if indicator.detail}<div class="blocked-detail">{indicator.detail}</div>{/if}
            </div>
            {#if onOpenTerminal}
              <button type="button" class="blocked-btn" data-testid="conv-open-terminal" onclick={onOpenTerminal}>Open terminal</button>
            {/if}
          </div>
        {:else if indicator}
          <div class="indicator" data-testid="conv-indicator" data-kind={indicator.kind} role="status">
            <span class="pulse" aria-hidden="true"><i></i><i></i><i></i></span>
            <span class="indicator-label">{indicator.kind === 'sent' ? 'Sent, waiting for Claude…' : indicator.label}</span>
          </div>
        {/if}
      </div>
    </div>
    {#if !atBottom}
      <button type="button" class="latest" class:fresh={unseen > 0} data-testid="conv-latest" aria-live="polite" onclick={scrollToBottom}
        >↓ {unseen > 0 ? `${unseen} new` : 'Latest'}</button
      >
    {/if}
  {/if}
  </div>
  {#if canPrompt}
    <form
      class="composer"
      data-testid="conv-composer"
      use:hintAnchor={{ id: 'conversation-composer', when: canPrompt }}
      onsubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      {#if slashOpen}
        <ul class="slash-menu" role="listbox" aria-label="Claude Code commands" data-testid="conv-slash-menu">
          {#each slashMatches as c, i (c.name)}
            <li role="option" aria-selected={i === slashIndex} class:active={i === slashIndex} data-testid="conv-slash-item">
              <!-- keyboard handling lives on the textarea (arrows / Tab / Enter);
                   the button only takes the mouse, and mousedown is swallowed
                   so the textarea keeps focus -->
              <button type="button" tabindex="-1" onmousedown={(e) => e.preventDefault()} onclick={() => acceptSlash(c)}>
                <span class="slash-name">/{c.name}</span>
                <span class="slash-desc">{c.description}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      {#if sendError}
        <div class="composer-error" data-testid="conv-composer-error">{sendError}</div>
      {/if}
      <div class="chips" data-testid="conv-chips">
        {#if liveStuck === 'press_enter'}
          <button
            type="button"
            class="chip stuck"
            data-testid="conv-chip-enter"
            title="The session is waiting on a key press. Sends a bare Enter."
            disabled={sending}
            onclick={() => void sendText('')}>⏎ Press Enter</button
          >
        {/if}
        {#each $composerPresets as p, i (i)}
          {#if p.label.trim() && p.text.trim()}
            <button
              type="button"
              class="chip"
              data-testid="conv-chip"
              title={`${p.text}\n\nClick fills the box; Shift+click sends now.`}
              disabled={sending}
              onclick={(e) => usePreset(p, e.shiftKey)}>{p.label}</button
            >
          {/if}
        {/each}
      </div>
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
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    max-width: 80ch;
    margin: 0 auto 0.4rem;
  }
  .chip {
    padding: 0.15rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg);
    color: var(--fg-muted);
    font-size: 0.72rem;
    cursor: pointer;
  }
  .chip:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--fg);
  }
  .chip:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .chip.stuck {
    border-color: #e6a23c;
    color: #e6a23c;
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
  .slash-menu {
    list-style: none;
    max-width: 80ch;
    max-height: 14rem;
    overflow: auto;
    margin: 0 auto 0.4rem;
    padding: 0.25rem 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    font-size: 0.8rem;
  }
  .slash-menu li.active,
  .slash-menu li:hover {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  .slash-menu button {
    display: flex;
    width: 100%;
    gap: 0.75rem;
    align-items: baseline;
    padding: 0.3rem 0.65rem;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .slash-name {
    flex: 0 0 9ch;
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    color: var(--accent);
  }
  .slash-desc {
    flex: 1 1 auto;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .indicator {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0 0.6rem;
    color: var(--fg-muted);
    font-size: 0.8rem;
  }
  .indicator[data-kind='sent'] {
    font-style: italic;
  }
  .pulse {
    display: inline-flex;
    gap: 3px;
  }
  .pulse i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    animation: conv-pulse 1.2s ease-in-out infinite;
  }
  .pulse i:nth-child(2) {
    animation-delay: 0.2s;
  }
  .pulse i:nth-child(3) {
    animation-delay: 0.4s;
  }
  @keyframes conv-pulse {
    0%,
    80%,
    100% {
      opacity: 0.25;
      transform: scale(0.8);
    }
    40% {
      opacity: 1;
      transform: scale(1);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse i {
      animation: none;
      opacity: 0.7;
    }
  }
  .blocked {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin: 0.35rem 0 0.6rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid #e6a23c;
    border-left-width: 3px;
    border-radius: 6px;
    background: color-mix(in srgb, #e6a23c 10%, var(--bg-pane));
    font-size: 0.8rem;
  }
  .blocked-text {
    flex: 1 1 auto;
    min-width: 0;
  }
  .blocked-detail {
    margin-top: 0.2rem;
    color: var(--fg-muted);
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.74rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .blocked-btn {
    flex: 0 0 auto;
    padding: 0.3rem 0.7rem;
    border: 1px solid #e6a23c;
    border-radius: 6px;
    background: transparent;
    color: var(--fg);
    font-size: 0.78rem;
    cursor: pointer;
  }
  .blocked-btn:hover {
    background: color-mix(in srgb, #e6a23c 20%, var(--bg-pane));
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
  .duration {
    margin-top: 0.3rem;
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
  .tool.err {
    color: #e64a4a;
  }
  .tool.err::before {
    content: '✗';
    opacity: 1;
  }
  .tools.has-err summary {
    color: #e64a4a;
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
  .latest.fresh {
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }
</style>
