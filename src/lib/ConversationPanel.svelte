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
  import { untrack, tick, setContext } from 'svelte';
  import { requestOpenPath, OPEN_PATH_CONTEXT, type OpenPathFn } from './app_views';
  import { sendPrompt, hasNoPane, type SessionRow } from './sessions';
  import { hintAnchor } from './hints';
  import { composerPresets, type ComposerPreset } from './composer_presets';
  import { contextLevel, contextColor, contextTint } from './attention';
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
    promptHistory,
    CONV_TURNS_STEP,
    CONV_MAX_TURNS,
    PROBE_TTL_MS,
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
  // Turns whose long prompt the user expanded, keyed by the turn's identity
  // (its timestamp, else its index) so a window that grows or slides does
  // not move the expansion to a different turn.
  let expanded = $state<Set<string>>(new Set());
  const turnKey = (turn: { at: string | null }, i: number) => turn.at ?? `#${i}`;
  // False once the user scrolls away from the bottom; drives "↓ Latest".
  let atBottom = $state(true);
  // Items that landed while the user was scrolled up; shown on the button.
  let unseen = $state(0);
  // Turn window asked of the backend; undefined = its default. "Load older"
  // grows it; polls keep using it so loaded history does not vanish.
  let turnsWanted = $state<number | undefined>(undefined);
  let loadingOlder = $state(false);
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
  // Prompt recall: ArrowUp in an empty box walks earlier prompts newest
  // first, ArrowDown walks back and past the newest empties the box again.
  // Editing, a chip or a slash completion ends the walk. Inside a recalled
  // multi-line prompt the arrows move the caret unless it sits on the first
  // (ArrowUp) or last (ArrowDown) line, shell style.
  let histIndex = $state<number | null>(null);
  // Live indicator. `probe` is the latest on-demand pane read, laid over the
  // row's (tick-fresh) status while it is newer than the row; it is dropped
  // as soon as a row event carries a newer state. `sentTurnSeq` marks our
  // own send: until the row's turn counter moves past it, or a probe reports
  // the session idle, the session is treated as working.
  let probe = $state<ActivityProbe | null>(null);
  // A probe outranks the row only while fresh: once the loop stops (the
  // indicator went away) a stale reading must not shadow a row that keeps
  // saying "working"; after the TTL the row wins again and, if it still
  // says so, the loop restarts and takes a new reading.
  let probeFresh = $state(false);
  let probeTimer: ReturnType<typeof setTimeout> | undefined;
  function setProbe(next: ActivityProbe | null) {
    probe = next;
    clearTimeout(probeTimer);
    probeFresh = next !== null;
    if (next !== null) probeTimer = setTimeout(() => (probeFresh = false), PROBE_TTL_MS);
  }
  let sentTurnSeq = $state<number | null>(null);
  let idleSeenSinceSend = $state(false);
  let lastFetchAt = 0;
  let lastFetchTurnSeq = $state<number | null>(null);
  let seq = 0;
  // Fetches still pending, per session id. A poll tick never starts a read
  // while one is in flight for the same session (a remote read can take up
  // to its 30 s wall clock); a manual Retry or a session switch still does.
  const inFlight = new Map<number, number>();
  // Keyed on the id, not the row object: a store patch hands a new object
  // for the same session, which must neither reset nor refetch.
  const sessionId = $derived(session.id);

  // Paths in reply text open in the Files tab (MarkdownInline reads this).
  setContext<OpenPathFn>(OPEN_PATH_CONTEXT, (path, line) => requestOpenPath(sessionId, path, line));

  async function load(opts: { poll?: boolean; older?: boolean } = {}) {
    const id = session.id;
    if (!session.claude_session_id) return;
    if (opts.poll && (inFlight.get(id) ?? 0) > 0) return;
    const mine = ++seq;
    loading = conv === null;
    const fetchedTurnSeq = session.turn_seq;
    const pinned = scroller ? isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight) : true;
    inFlight.set(id, (inFlight.get(id) ?? 0) + 1);
    let r: Awaited<ReturnType<typeof sessionConversation>>;
    try {
      r = await sessionConversation(id, turnsWanted);
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
      // Stamped on success only: a failed read on a quiet session is retried
      // at the normal cadence, not after the quiet one.
      lastFetchAt = Date.now();
      lastFetchTurnSeq = fetchedTurnSeq;
      errorCode = null;
      errorMsg = null;
      if (!sameConversation(conv, r.value)) {
        // Older turns prepended by Load older are history, not news.
        if (!pinned && !opts.older) unseen += newItemCount(conv, r.value);
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
      turnsWanted = undefined;
      loadingOlder = false;
      draft = composerDrafts.get(session.id) ?? '';
      draftFor = session.id;
      histIndex = null;
      sendError = null;
      pending = null;
      setProbe(null);
      probeSeq++;
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
      if (visible && lastFetchTurnSeq !== null && turn !== lastFetchTurnSeq && session.claude_session_id) void load({ poll: true });
    });
  });

  // A row event with a changed status is newer than any probe. Keyed on the
  // two values, not the row object: every store patch hands a new object.
  const rowStatus = $derived(session.claude_status);
  const rowStuck = $derived(session.stuck_kind);
  $effect(() => {
    void rowStatus;
    void rowStuck;
    untrack(() => setProbe(null));
  });
  $effect(() => () => clearTimeout(probeTimer));

  const liveProbe = $derived(probeFresh ? probe : null);
  const liveStatus = $derived(liveProbe?.claude_status ?? session.claude_status);
  const liveStuck = $derived(liveProbe?.stuck_kind ?? session.stuck_kind);
  const liveActivity = $derived(liveProbe?.current_activity ?? session.current_activity);
  const optimistic = $derived(sentTurnSeq !== null && session.turn_seq === sentTurnSeq && !idleSeenSinceSend);
  const indicator = $derived(
    indicatorFor({
      status: liveStatus,
      stuckKind: liveStuck,
      waitingFor: liveProbe?.waiting_for ?? null,
      activity: liveActivity,
      spinner: liveProbe?.spinner ?? null,
      pending: pending !== null,
      optimistic,
    }),
  );

  // One probe in flight at a time (a wedged host must not stack ssh
  // processes every 2 s), and a slow one never overwrites a newer result.
  let probing = false;
  let probeSeq = 0;
  async function probeNow() {
    if (probing) return;
    probing = true;
    const id = session.id;
    const mine = ++probeSeq;
    let r: Awaited<ReturnType<typeof sessionActivity>>;
    try {
      r = await sessionActivity(id);
    } finally {
      probing = false;
    }
    if (session.id !== id || mine !== probeSeq) return;
    if (!r.ok) return;
    setProbe(r.value);
    if (sentTurnSeq !== null && isQuietStatus(r.value.claude_status)) idleSeenSinceSend = true;
  }

  // Probe the pane every couple of seconds while something is live: once at
  // once, then on the interval. `probeLive` is a boolean derived, so a fresh
  // probe (which yields a new `indicator` object) never restarts the timer.
  // bg / external rows have no pane: the backend would reject every probe.
  const probeLive = $derived(visible && !hasNoPane(session) && indicator !== null);
  $effect(() => {
    if (!probeLive) return;
    if (document.visibilityState === 'visible') void untrack(probeNow);
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
  // Context-window meter beside the composer; at warn/crit the Compact chip
  // is suggested, since that is the one-click remedy.
  const ctxLevel = $derived(contextLevel(session.context_pct));
  const suggestCompact = $derived(ctxLevel === 'warn' || ctxLevel === 'crit');
  const isCompactPreset = (p: ComposerPreset) => /^\/compact\b/.test(p.text.trim());
  const slashMatches = $derived(slashDismissedFor === draft ? [] : matchSlashCommands(draft));
  const history = $derived(promptHistory(conv, pending));

  /** ArrowUp / ArrowDown recall. Returns true when the key was consumed. */
  function caretOnEdgeLine(dir: -1 | 1): boolean {
    if (!box) return true;
    return dir === -1
      ? box.value.lastIndexOf('\n', box.selectionStart - 1) === -1
      : box.value.indexOf('\n', box.selectionEnd) === -1;
  }

  function recall(dir: -1 | 1): boolean {
    if (histIndex === null) {
      if (dir === 1 || draft !== '' || history.length === 0) return false;
      histIndex = history.length - 1;
    } else {
      if (!caretOnEdgeLine(dir)) return false;
      const next = histIndex + dir;
      if (next < 0) return true;
      if (next >= history.length) {
        histIndex = null;
        draft = '';
        return true;
      }
      histIndex = next;
    }
    draft = history[histIndex];
    return true;
  }

  function onComposerInput() {
    histIndex = null;
  }
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
    histIndex = null;
    box?.focus();
  }

  function acceptSlash(c: SlashCommand) {
    draft = completeSlashCommand(c);
    slashDismissedFor = draft;
    histIndex = null;
    box?.focus();
  }

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    await sendText(text, { fromDraft: true });
  }

  /** Send `text` as-is. Empty text is a bare Enter (the press_enter chip):
   *  it lands in the REPL but is not a prompt, so nothing is shown pending. */
  async function sendText(text: string, opts: { fromDraft?: boolean } = {}) {
    if (sending) return;
    sending = true;
    sendError = null;
    const id = session.id;
    const r = await sendPrompt(session.host_alias, session.tmux_name, text);
    sending = false;
    // The selection moved while the send was on the wire: the prompt landed
    // in the old session; none of its state belongs to the new one.
    if (session.id !== id) return;
    if (!r.ok) {
      sendError = r.error.message;
      return;
    }
    if (text === '') return;
    // Only the box's own text is spent by a send; a chip sent with
    // Shift+click leaves whatever the user was typing.
    if (opts.fromDraft) draft = '';
    histIndex = null;
    // A slash command is handled by the REPL itself: it is not recorded as a
    // prompt (and /clear even moves to a new session id), so no pending
    // turn, and nothing to wait for beyond a fresh read.
    if (text.startsWith('/')) {
      box?.focus();
      void load();
      return;
    }
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
    // WebKit fires the composition-confirming Enter with isComposing false
    // and keyCode 229; treat it as composition too.
    if (e.isComposing || e.keyCode === 229) return;
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
    // Recall only consumes an arrow it acted on; the slash menu's own arrow
    // handling ran above, and an unconsumed arrow keeps its caret movement.
    if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && !e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey) {
      if (recall(e.key === 'ArrowUp' ? -1 : 1)) {
        e.preventDefault();
        return;
      }
    }
    if (e.key !== 'Enter' || e.shiftKey || e.altKey) return;
    e.preventDefault();
    void send();
  }

  /** Ask for another window of turns and keep the view where it is: the
   *  older turns render above, so the scroll offset is corrected by the
   *  height they added. */
  async function loadOlder() {
    if (loadingOlder) return;
    loadingOlder = true;
    turnsWanted = (turnsWanted ?? CONV_TURNS_STEP) + CONV_TURNS_STEP;
    const before = scroller ? scroller.scrollHeight - scroller.scrollTop : 0;
    try {
      await load({ older: true });
    } finally {
      loadingOlder = false;
    }
    await tick();
    if (scroller) scroller.scrollTop = scroller.scrollHeight - before;
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

  /** Open a tool group when it becomes the running turn's last group; never
   *  close one, and never re-open one the user closed while it stays the
   *  running group, so manual toggles survive transcript refreshes. */
  function autoOpen(node: HTMLDetailsElement, on: boolean) {
    if (on) node.open = true;
    let prev = on;
    return {
      update(next: boolean) {
        if (next && !prev) node.open = true;
        prev = next;
      },
    };
  }

  function togglePrompt(key: string) {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
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
          <p class="muted truncated">
            Older turns not shown{#if (turnsWanted ?? CONV_TURNS_STEP) < CONV_MAX_TURNS}
              ·
              <button type="button" class="linkish" data-testid="conv-load-older" disabled={loadingOlder} onclick={() => void loadOlder()}
                >{loadingOlder ? 'Loading…' : 'Load older'}</button
              >{/if}
          </p>
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
                  <div class="prompt-text" class:clamped={long && !expanded.has(turnKey(turn, i))}>{turn.prompt}</div>
                  {#if long}
                    <button type="button" class="linkish" data-testid="conv-prompt-toggle" onclick={() => togglePrompt(turnKey(turn, i))}
                      >{expanded.has(turnKey(turn, i)) ? 'Show less' : 'Show more'}</button
                    >
                  {/if}
                </div>
              {/if}
              <div class="reply">
                {#each groups as g, j (j)}
                  {#if g.kind === 'text'}
                    <div class="text" data-testid="conv-text"><Markdown source={g.text} /></div>
                  {:else if g.kind === 'tools' && g.tools.length === 1}
                    <div class="tool" class:err={g.tools[0].error} data-testid="conv-tool" data-error={g.tools[0].error || undefined} title={g.tools[0].error ? `Failed: ${g.tools[0].summary}` : g.tools[0].summary}>{g.tools[0].summary}</div>
                  {:else if g.kind === 'tools'}
                    <details class="tools" class:has-err={g.tools.some((t) => t.error)} use:autoOpen={running && j === groups.length - 1} data-testid="conv-tools">
                      <summary>{toolGroupLabel(g.tools)}</summary>
                      {#each g.tools as line, k (k)}
                        <div class="tool" class:err={line.error} data-testid="conv-tool" data-error={line.error || undefined} title={line.error ? `Failed: ${line.summary}` : line.summary}>{line.summary}</div>
                      {/each}
                    </details>
                  {:else}
                    <!-- compact / command / interrupt: rendered starting Task 4 -->
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
            {@const suggested = suggestCompact && isCompactPreset(p)}
            <button
              type="button"
              class="chip"
              class:suggest={suggested}
              data-testid="conv-chip"
              data-suggested={suggested || undefined}
              title={suggested
                ? `Context window is ${Math.round(session.context_pct ?? 0)}% used. Compacting frees space.\n\nClick fills the box; Shift+click sends now.`
                : `${p.text}\n\nClick fills the box; Shift+click sends now.`}
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
          oninput={onComposerInput}
          onkeydown={onComposerKey}
          rows="2"
          placeholder="Send a prompt to this session (Enter to send, Shift+Enter for a new line, ↑ recalls earlier prompts)"
          disabled={sending}
        ></textarea>
        <button type="submit" data-testid="conv-composer-send" disabled={!canSend}>{sending ? 'Sending…' : 'Send'}</button>
      </div>
      {#if statusNote || ctxLevel !== null}
        <div class="composer-foot">
          {#if statusNote}
            <div class="composer-status" data-testid="conv-composer-status">{statusNote}</div>
          {/if}
          {#if ctxLevel !== null && session.context_pct !== null}
            <span
              class="ctx"
              data-testid="conv-ctx"
              data-level={ctxLevel}
              role="meter"
              aria-valuemin="0"
              aria-valuemax="100"
              aria-valuenow={Math.round(session.context_pct)}
              aria-label="context usage"
              title="Context window {Math.round(session.context_pct)}% used"
              style="color: {contextColor(ctxLevel)}; border-color: {contextTint(ctxLevel)};"
              ><span class="ctx-bar" style="width: {Math.min(100, Math.max(0, session.context_pct))}%; background: {contextColor(ctxLevel)};"></span><span class="ctx-pct">ctx {Math.round(session.context_pct)}%</span></span
            >
          {/if}
        </div>
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
  .composer-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    max-width: 80ch;
    margin: 0.35rem auto 0;
  }
  .composer-status {
    margin: 0;
    color: var(--fg-muted);
    font-size: 0.75rem;
  }
  .ctx {
    position: relative;
    display: inline-block;
    flex: 0 0 auto;
    margin-left: auto;
    padding: 0.1rem 0.45rem;
    border: 1px solid;
    border-radius: 999px;
    overflow: hidden;
    font-size: 0.68rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .ctx-bar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    opacity: 0.25;
  }
  .ctx-pct {
    position: relative;
  }
  .chip.suggest {
    border-color: var(--usage-warn, #e6a23c);
    color: var(--fg);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--usage-warn, #e6a23c) 25%, transparent);
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
