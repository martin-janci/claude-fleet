<script module lang="ts">
  // Per-instance suffix for the find highlight names, so two panels never
  // paint into (or clear) each other's highlights.
  let panelSeq = 0;
</script>

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
  import { contextLevel } from './attention';
  import { timeAgo } from './session_status';
  import { onTimelineEvent, onConversationsChanged } from './live_events';
  import type { SessionEvent } from './timeline';
  import ConversationHeader from './ConversationHeader.svelte';
  import ToolLine from './ToolLine.svelte';
  import SubagentBlock from './SubagentBlock.svelte';
  import CopyButton from './CopyButton.svelte';
  import { findMatches, turnIndex, rowKey } from './conversation_nav';
  import { detectMac } from './terminal_keys';
  import {
    sessionConversation,
    listConversations,
    switcherEntries,
    buildThread,
    mergeEvents,
    lastEventLabel,
    formatTokens,
    SOURCE_LABELS,
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
    doingNow,
    hasPendingCall,
    formatDuration,
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
    type ConversationSummary,
    type PendingPrompt,
    type SlashCommand,
    type ActivityProbe,
  } from './conversation';
  import { hubStatus, ownsTheFleet } from './hub';
  import Markdown from './MarkdownView.svelte';

  let {
    session,
    visible,
    onOpenTerminal,
    // Find is Cmd+F on macOS (Ctrl+F moves the caret there), Ctrl+F elsewhere.
    isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator),
  }: { session: SessionRow; visible: boolean; onOpenTerminal?: () => void; isMac?: boolean } = $props();

  let conv = $state<Conversation | null>(null);
  // The conversation `conv` was read from (the id the fetch named). Tool
  // details are read from it, not from the row's id at click time: the row
  // can move on (/clear, /resume) before a reload replaces the view.
  let convCid = $state<string | null>(null);
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

  // Conversations this session has run (header switcher), newest first.
  let conversations = $state<ConversationSummary[]>([]);
  // claude_session_id of an earlier conversation being read; null = current.
  let viewing = $state<string | null>(null);
  // A newer conversation started while an earlier one is being viewed.
  let newerAvailable = $state(false);
  // Shown after the session followed a /clear or /resume on its own. The
  // label is derived from the list, so a list that lands after the notice
  // (or is refreshed by a session:conversations push) still names it.
  let switchNotice = $state<{ cid: string } | null>(null);
  // Timeline events pushed live for the conversation on screen; merged with
  // the ones the last read carried. Reset together with `conv`.
  let pushed = $state<SessionEvent[]>([]);
  let listSeq = 0;

  // Paths in reply text open in the Files tab (MarkdownInline reads this).
  setContext<OpenPathFn>(OPEN_PATH_CONTEXT, (path, line) => requestOpenPath(sessionId, path, line));

  async function load(opts: { poll?: boolean; older?: boolean } = {}) {
    const id = session.id;
    if (!session.claude_session_id) return;
    if (opts.poll && (inFlight.get(id) ?? 0) > 0) return;
    // Name the conversation explicitly (the current one too): the backend's
    // row may already have moved on to a newer id this row has not seen yet.
    const cid = viewing ?? session.claude_session_id;
    const mine = ++seq;
    loading = conv === null;
    const fetchedTurnSeq = session.turn_seq;
    const pinned = scroller ? isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight) : true;
    inFlight.set(id, (inFlight.get(id) ?? 0) + 1);
    let r: Awaited<ReturnType<typeof sessionConversation>>;
    try {
      r = await sessionConversation(id, turnsWanted, cid);
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
      convCid = cid;
      if (!sameConversation(conv, r.value)) {
        // Older turns prepended by Load older are history, not news.
        if (!pinned && !opts.older) unseen += newItemCount(conv, r.value);
        conv = r.value;
        if (viewing === null && pending && transcriptCarries(conv, pending)) pending = null;
        // Pushed events the read now carries are the backend's to keep (or
        // age out); holding copies would grow `pushed` without bound.
        const carried = new Set((conv.events ?? []).map((e) => e.id));
        if (pushed.some((e) => carried.has(e.id))) pushed = pushed.filter((e) => !carried.has(e.id));
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

  async function loadConversations() {
    const id = session.id;
    const mine = ++listSeq;
    const r = await listConversations(id);
    if (mine !== listSeq || session.id !== id) return;
    // A failed (or malformed) read keeps the list we had.
    if (r.ok && Array.isArray(r.value)) conversations = r.value;
  }

  /** Drop the conversation on screen: content, live events, errors,
   *  expansions, scroll state and the turn window; a fetch in flight is
   *  made stale. */
  function resetView() {
    seq++;
    conv = null;
    convCid = null;
    pushed = [];
    errorCode = null;
    errorMsg = null;
    expanded = new Set();
    atBottom = true;
    unseen = 0;
    turnsWanted = undefined;
    loadingOlder = false;
    turnsOpen = false;
    findOpen = false;
    findQuery = '';
    findIndex = 0;
    clearHighlights();
  }

  /** resetView plus the send and probe state of the current conversation;
   *  the draft is the caller's business. */
  function resetThread() {
    resetView();
    pending = null;
    setProbe(null);
    probeSeq++;
    sentTurnSeq = null;
    idleSeenSinceSend = false;
  }

  // Reset + immediate fetch on session change.
  $effect(() => {
    void sessionId;
    untrack(() => {
      resetThread();
      viewing = null;
      newerAvailable = false;
      switchNotice = null;
      conversations = [];
      draft = composerDrafts.get(session.id) ?? '';
      draftFor = session.id;
      histIndex = null;
      sendError = null;
      void load();
      void loadConversations();
    });
  });

  // Follow /clear and /resume: the row's claude_session_id moves while the
  // session id stays. The first id seen for a session (mount, session switch,
  // or an id appearing where there was none) is not a switch.
  const claudeId = $derived(session.claude_session_id);
  let seenClaude: { sid: number; cid: string | null } | null = null;
  $effect(() => {
    const cid = claudeId;
    const sid = sessionId;
    untrack(() => {
      const prev = seenClaude;
      seenClaude = { sid, cid };
      if (prev === null || prev.sid !== sid || prev.cid === cid || cid === null) return;
      if (viewing !== null) {
        newerAvailable = true;
        return;
      }
      resetThread();
      void load();
      void loadConversations();
      if (prev.cid !== null) switchNotice = { cid };
    });
  });

  /** Header switcher / notice / banner: read an earlier conversation
   *  (read-only) or go back to the current one (null). */
  function select(id: string | null) {
    if (id === viewing) return;
    const leavingForNewer = id === null && newerAvailable;
    viewing = id;
    switchNotice = null;
    resetView();
    if (id === null) {
      newerAvailable = false;
      // A reading taken before we left is not the current state any more.
      setProbe(null);
      probeSeq++;
    }
    // The current conversation moved on while we were away: a prompt still
    // shown pending belonged to the old one.
    if (leavingForNewer) {
      pending = null;
      sentTurnSeq = null;
      idleSeenSinceSend = false;
    }
    void load();
  }

  function viewPrevious() {
    const prev = switcherEntries(conversations).find((c) => !c.current);
    if (prev) select(prev.claude_session_id);
  }

  // Live pushes for this session: timeline events for the conversation on
  // screen, and conversation-list changes.
  $effect(() => {
    const id = sessionId;
    const offEvents = onTimelineEvent(id, (e) => {
      if (e.claude_session_id === (viewing ?? session.claude_session_id)) pushed = [...pushed, e];
    });
    const offList = onConversationsChanged(id, () => void loadConversations());
    return () => {
      offEvents();
      offList();
    };
  });

  const events = $derived(mergeEvents(conv?.events ?? [], pushed));
  const lastEvent = $derived(lastEventLabel(events, nowMs));
  // The notice waits for a list that knows the new conversation: before
  // that its source is unknown and "View previous" would pick from a stale
  // list.
  const noticeSource = $derived(
    switchNotice === null
      ? null
      : (conversations.find((c) => c.claude_session_id === switchNotice!.cid)?.start_source ?? null),
  );

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
      void untrack(() => {
        if (viewing === null) void load({ poll: true });
      });
    }
    const t = setInterval(() => {
      if (document.visibilityState !== 'visible') return;
      untrack(() => {
        // An earlier conversation is finished: nothing to poll for.
        if (viewing !== null) return;
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
      if (viewing !== null) return;
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

  // What the running turn is doing right now (the current conversation only:
  // an earlier one is read-only and never running).
  const doing = $derived(viewing === null && indicator?.kind === 'working' ? doingNow(conv, true, nowMs) : null);
  const indicatorLabel = $derived(
    indicator?.kind === 'working'
      ? doing
        ? doing.sinceMs !== null
          ? `${doing.label} · ${formatDuration(doing.sinceMs)}`
          : doing.label
        : indicator.label
      : null,
  );
  // In the template, `turnLive` marks the last turn of the current
  // conversation while the indicator shows anything (working, blocked on a
  // prompt in the terminal, or just sent): an unfinished tool call there is
  // pending, not dead, so it keeps its running clock.
  // The conversation tool details are read from: the one on screen.
  const detailCid = $derived(convCid);

  const thread = $derived(
    conv ? buildThread(conv.turns, events, { blocked: viewing === null && indicator?.kind === 'blocked' }, conv.truncated) : [],
  );

  // ─── Find in conversation / turn index ────────────────────────────────────
  let root: HTMLDivElement | undefined = $state();
  let findOpen = $state(false);
  let findQuery = $state('');
  let findIndex = $state(0);
  let findInput: HTMLInputElement | undefined = $state();
  let turnsOpen = $state(false);
  let turnsWrap: HTMLDivElement | undefined = $state();
  let turnsList: HTMLUListElement | undefined = $state();
  let turnsButton: HTMLButtonElement | undefined = $state();

  const matches = $derived(findOpen ? findMatches(thread, findQuery) : []);
  const matchKeys = $derived(new Set(matches.map((m) => m.rowKey)));
  const currentMatch = $derived(matches.length > 0 ? matches[Math.min(findIndex, matches.length - 1)].rowKey : null);
  const findCount = $derived(
    matches.length > 0 ? `${Math.min(findIndex, matches.length - 1) + 1} / ${matches.length}` : findQuery.trim() ? '0 / 0' : '',
  );
  const turnEntries = $derived(turnIndex(thread));

  function rowEl(key: string): HTMLElement | null {
    return scroller?.querySelector<HTMLElement>(`[data-row-key="${key}"]`) ?? null;
  }
  function scrollToRow(key: string) {
    const el = rowEl(key);
    if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'center' });
  }

  async function openFind() {
    turnsOpen = false;
    findOpen = true;
    await tick();
    findInput?.focus();
    findInput?.select();
  }
  function closeFind() {
    findOpen = false;
    findIndex = 0;
    // Hand focus back to the thread so the shortcut keeps working.
    scroller?.focus({ preventScroll: true });
  }
  function stepFind(dir: 1 | -1) {
    if (matches.length === 0) return;
    const cur = Math.min(findIndex, matches.length - 1);
    findIndex = (cur + dir + matches.length) % matches.length;
  }
  function onFindKey(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter') {
      e.preventDefault();
      stepFind(e.shiftKey ? -1 : 1);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      closeFind();
    }
  }

  // Cmd/Ctrl+F while focus is inside the panel; the listener sits on the
  // panel root, so focus elsewhere in the app keeps its own behaviour.
  $effect(() => {
    const el = root;
    if (!el) return;
    function onKey(e: KeyboardEvent) {
      const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
      if (!mod || e.altKey || e.shiftKey || e.key.toLowerCase() !== 'f') return;
      // Nothing to search while the thread is loading or empty.
      if (!threadShown) return;
      e.preventDefault();
      void openFind();
    }
    el.addEventListener('keydown', onKey);
    return () => el.removeEventListener('keydown', onKey);
  });

  // Bring the current match on screen whenever it changes.
  $effect(() => {
    const key = currentMatch;
    if (key === null) return;
    untrack(() => scrollToRow(key));
  });

  // Paint the query inside the matching rows with the CSS Custom Highlight
  // API where it exists; elsewhere (jsdom, older engines) the row outline
  // is the only highlight.
  const hlSuffix = ++panelSeq;
  const HL_ALL = `conv-find-${hlSuffix}`;
  const HL_CURRENT = `conv-find-current-${hlSuffix}`;
  // `::highlight()` names cannot be dynamic in the component's stylesheet:
  // each panel adds (and on unmount removes) the two rules for its own names.
  $effect(() => {
    const el = document.createElement('style');
    el.dataset.convFind = String(hlSuffix);
    el.textContent =
      `::highlight(${HL_ALL}) { background-color: color-mix(in srgb, #e6a23c 35%, transparent); }\n` +
      `::highlight(${HL_CURRENT}) { background-color: color-mix(in srgb, #e6a23c 75%, transparent); color: var(--bg); }`;
    document.head.appendChild(el);
    return () => el.remove();
  });
  const HL_MAX_RANGES = 2_000;
  function highlightRegistry(): { set(n: string, h: unknown): void; delete(n: string): void } | null {
    try {
      const reg = (globalThis.CSS as unknown as { highlights?: unknown } | undefined)?.highlights;
      if (!reg || typeof (globalThis as { Highlight?: unknown }).Highlight !== 'function') return null;
      return reg as { set(n: string, h: unknown): void; delete(n: string): void };
    } catch {
      return null;
    }
  }
  function clearHighlights() {
    const reg = highlightRegistry();
    if (!reg) return;
    reg.delete(HL_ALL);
    reg.delete(HL_CURRENT);
  }
  $effect(() => {
    const q = findQuery.trim().toLowerCase();
    const keys = matchKeys;
    const cur = currentMatch;
    const reg = highlightRegistry();
    if (!reg || !scroller || keys.size === 0 || q === '') {
      clearHighlights();
      return;
    }
    try {
      const all: Range[] = [];
      const current: Range[] = [];
      for (const el of Array.from(scroller.querySelectorAll<HTMLElement>('[data-row-key]'))) {
        const key = el.dataset.rowKey ?? '';
        if (!keys.has(key)) continue;
        // Only the conversation's own text: not button labels (Copy, Show
        // more, a tool row's chrome), times, other controls or hidden
        // chrome (a lone call's group summary).
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
          acceptNode: (n) =>
            n.parentElement?.closest('button, time, input, textarea, select, [role="button"], [aria-hidden="true"]')
              ? NodeFilter.FILTER_REJECT
              : NodeFilter.FILTER_ACCEPT,
        });
        for (let n = walker.nextNode(); n && all.length < HL_MAX_RANGES; n = walker.nextNode()) {
          const text = (n.textContent ?? '').toLowerCase();
          for (let at = text.indexOf(q); at !== -1 && all.length < HL_MAX_RANGES; at = text.indexOf(q, at + q.length)) {
            const r = document.createRange();
            r.setStart(n, at);
            r.setEnd(n, at + q.length);
            all.push(r);
            if (key === cur) current.push(r);
          }
        }
      }
      const H = (globalThis as unknown as { Highlight: new (...r: Range[]) => unknown }).Highlight;
      reg.set(HL_ALL, new H(...all));
      reg.set(HL_CURRENT, new H(...current));
    } catch {
      clearHighlights();
    }
  });
  $effect(() => () => clearHighlights());

  /** Turn index: jump to a turn and close the list. */
  function pickTurn(key: string) {
    turnsOpen = false;
    scrollToRow(key);
  }
  function onTurnsKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      turnsOpen = false;
      turnsButton?.focus();
    }
  }
  $effect(() => {
    if (turnsOpen) turnsList?.querySelector('button')?.focus();
  });
  // Close the list on an outside pointerdown (as ConversationHeader does).
  $effect(() => {
    if (!turnsOpen) return;
    function onDocPointerDown(e: PointerEvent) {
      if (turnsWrap && e.target instanceof Node && !turnsWrap.contains(e.target)) turnsOpen = false;
    }
    document.addEventListener('pointerdown', onDocPointerDown);
    return () => document.removeEventListener('pointerdown', onDocPointerDown);
  });

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
  // `session_activity` is also local-only in remote mode (`peek_session`
  // answers a different shape) — a hub client must not poll it every 2s only
  // to drop an E_LOCAL_ONLY every time.
  const probeLive = $derived(
    visible &&
      !hasNoPane(session) &&
      indicator !== null &&
      viewing === null &&
      ownsTheFleet($hubStatus),
  );
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

  // A running tool's timer needs a finer clock: tick every second while
  // something is running, and stop as soon as nothing is. Keyed on a boolean
  // so each tick (which yields a new `doing`) does not restart the interval.
  // That covers a call pending while Claude waits on the terminal (blocked)
  // or a prompt was just sent, not only while the doing-now label shows.
  const doingSomething = $derived(
    doing !== null || (viewing === null && indicator !== null && hasPendingCall(conv)),
  );
  $effect(() => {
    if (!doingSomething) return;
    untrack(() => (nowMs = Date.now()));
    const t = setInterval(() => (nowMs = Date.now()), 1_000);
    return () => clearInterval(t);
  });

  const empty = $derived(
    viewing !== null && errorCode === 'E_NO_TRANSCRIPT'
      ? 'Transcript no longer on host'
      : emptyStateText(errorCode, !!session.claude_session_id),
  );
  const canPrompt = $derived(!hasNoPane(session));
  // The scroller (and so the thread) is on screen: find has something to search.
  const threadShown = $derived(!(empty && !(pending && viewing === null)) && !loading);

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
  const canSend = $derived(draft.trim().length > 0 && !sending && viewing === null);
  const statusNote = $derived(
    viewing !== null
      ? 'Viewing an earlier conversation — go back to current to send.'
      : composerStatus({ claude_status: liveStatus, stuck_kind: liveStuck }),
  );
  // The context meter lives in the header; at warn/crit the Compact chip is
  // suggested, since that is the one-click remedy.
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
    if (sending || viewing !== null) return;
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
    switchNotice = null;
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
  function autoOpen(node: HTMLDetailsElement, arg: { on: boolean; single: boolean }) {
    // A lone call has no group to fold: always open. When a second call
    // joins, the group stays open (the user was looking at that line).
    if (arg.on || arg.single) node.open = true;
    let prev = arg.on;
    return {
      update(next: { on: boolean; single: boolean }) {
        if ((next.on && !prev) || next.single) node.open = true;
        prev = next.on;
      },
    };
  }

  const CMD_CLAMP_LINES = 8;
  const isLongOutput = (out: string) => out.split('\n').length > CMD_CLAMP_LINES;

  function togglePrompt(key: string) {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    expanded = next;
  }
</script>

<div class="conversation-panel" data-testid="conversation-panel" bind:this={root}>
  <ConversationHeader {session} {conversations} {viewing} {lastEvent} {newerAvailable} onSelect={select} />
  {#if viewing !== null}
    <div class="viewing" data-testid="conv-viewing-banner">
      Viewing an earlier conversation · <button type="button" class="linkish" data-testid="conv-back-current" onclick={() => select(null)}>Back to current</button>
    </div>
  {:else if switchNotice && noticeSource}
    <div class="switch-notice" data-testid="conv-switch-notice" role="status">
      New conversation{#if noticeSource !== 'unknown'}{` (${SOURCE_LABELS[noticeSource]})`}{/if} ·
      <button type="button" class="linkish" data-testid="conv-view-previous" onclick={viewPrevious}>View previous</button>
      <button type="button" class="dismiss" aria-label="Dismiss" data-testid="conv-switch-dismiss" onclick={() => (switchNotice = null)}>×</button>
    </div>
  {/if}
  <div class="thread-area">
  {#if empty && !(pending && viewing === null)}
    <p class="muted" data-testid="conv-empty">{empty}</p>
  {:else if loading}
    <p class="muted">Loading…</p>
  {:else}
    <!-- tabindex -1: a click in the thread focuses it, so Cmd/Ctrl+F finds -->
    <div class="scroller" data-testid="conv-scroller" tabindex="-1" bind:this={scroller} onscroll={onScroll}>
      {#if findOpen}
        <div class="find" data-testid="conv-find">
          <input
            type="search"
            data-testid="conv-find-input"
            aria-label="Find in conversation"
            placeholder="Find in conversation"
            bind:this={findInput}
            bind:value={findQuery}
            oninput={() => (findIndex = 0)}
            onkeydown={onFindKey}
          />
          <span class="find-count" data-testid="conv-find-count" aria-live="polite">{findCount}</span>
          <button type="button" class="tb-btn" data-testid="conv-find-prev" aria-label="Previous match" disabled={matches.length === 0} onclick={() => stepFind(-1)}>↑</button>
          <button type="button" class="tb-btn" data-testid="conv-find-next" aria-label="Next match" disabled={matches.length === 0} onclick={() => stepFind(1)}>↓</button>
          <button type="button" class="tb-btn" data-testid="conv-find-close" aria-label="Close find" onclick={closeFind}>×</button>
        </div>
      {:else if conv}
        <div class="toolbar" data-testid="conv-toolbar">
          <button type="button" class="tb-btn" data-testid="conv-find-button" aria-label="Find in conversation" title="Find (⌘F / Ctrl+F)" onclick={() => void openFind()}>⌕ Find</button>
          {#if turnEntries.length > 0}
            <div class="turns-wrap" bind:this={turnsWrap}>
              <button
                type="button"
                class="tb-btn"
                data-testid="conv-turns-button"
                aria-expanded={turnsOpen}
                bind:this={turnsButton}
                onclick={() => (turnsOpen = !turnsOpen)}>{turnEntries.length} turn{turnEntries.length === 1 ? '' : 's'}</button
              >
              {#if turnsOpen}
                <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                <ul class="turn-index" aria-label="Turns" data-testid="conv-turn-index" bind:this={turnsList} onkeydown={onTurnsKey}>
                  {#each turnEntries as t (t.rowKey)}
                    <li>
                      <button type="button" data-testid="conv-turn-index-item" onclick={() => pickTurn(t.rowKey)}>
                        <span class="ti-label">{t.label}</span>
                        {#if t.at}<time datetime={t.at}>{relativeTime(t.at, nowMs)}</time>{/if}
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
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
          {#each thread as row (rowKey(row))}
            {@const key = rowKey(row)}
            {#if row.kind === 'event'}
              <div
                class="event"
                data-testid="conv-event"
                data-tone={row.event.tone}
                data-row-key={key}
                data-match={matchKeys.has(key) || undefined}
                data-current-match={currentMatch === key || undefined}
              >
                <span class="label">{row.event.label}</span>{#if row.event.detail}<span class="detail">{row.event.detail}</span>{/if}<time datetime={new Date(row.event.at * 1000).toISOString()}>{timeAgo(row.event.at, nowMs)}</time>
              </div>
            {:else}
            {@const turn = row.turn}
            {@const i = row.index}
            {@const isLast = i === conv.turns.length - 1}
            {@const turnRunning = isLast && viewing === null && indicator?.kind === 'working'}
            {@const turnLive = isLast && viewing === null && indicator !== null}
            {@const groups = groupItems(turn.items)}
            {@const duration = turnRunning ? null : turnDuration(turn.at, turn.ended_at)}
            <section
              class="turn"
              data-row-key={key}
              data-match={matchKeys.has(key) || undefined}
              data-current-match={currentMatch === key || undefined}
            >
              {#if turn.prompt !== null}
                {@const long = isLongPrompt(turn.prompt)}
                <div class="prompt" data-testid="conv-prompt">
                  <div class="prompt-head">
                    <span class="who">You</span>
                    <span class="head-right">
                      <span class="copy-slot"><CopyButton text={turn.prompt} label="Copy prompt" /></span>
                      {#if turn.at}
                        <time datetime={turn.at} title={new Date(turn.at).toLocaleString()}
                          >{relativeTime(turn.at, nowMs)}</time
                        >
                      {/if}
                    </span>
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
                    <div class="text" data-testid="conv-text">
                      <Markdown source={g.text} />
                      <span class="copy-slot text-copy"><CopyButton text={g.text} label="Copy reply" /></span>
                    </div>
                  {:else if g.kind === 'tools'}
                    <!-- One structure for a lone call and a folded group, so a
                         line keeps its component (open detail, cache) when a
                         second call joins it. A lone call's group is always
                         open with its summary hidden. -->
                    {@const single = g.tools.length === 1}
                    <details
                      class="tools"
                      class:single
                      class:has-err={g.tools.some((t) => t.error)}
                      use:autoOpen={{ on: turnRunning && j === groups.length - 1, single }}
                      data-testid={single ? undefined : 'conv-tools'}
                    >
                      <summary aria-hidden={single || undefined} tabindex={single ? -1 : undefined}>{toolGroupLabel(g.tools)}</summary>
                      <div class="tools-body">
                        {#each g.tools as line, k (k)}
                          <ToolLine {line} sessionId={session.id} claudeSessionId={detailCid} {nowMs} live={turnLive} />
                        {/each}
                      </div>
                    </details>
                  {:else if g.kind === 'compact'}
                    <details class="compact" data-testid="conv-compact">
                      <summary>Compacted ({g.trigger ?? 'unknown'}){#if g.pre_tokens}{` · was ${formatTokens(g.pre_tokens)} tokens`}{/if}</summary>
                      {#if g.summary}<Markdown source={g.summary} />{:else}<p class="muted">Summary not in the loaded tail.</p>{/if}
                    </details>
                  {:else if g.kind === 'command'}
                    {@const cmdKey = `cmd:${turnKey(turn, i)}:${j}`}
                    {@const longOut = g.output !== null && isLongOutput(g.output)}
                    <div class="command" data-testid="conv-command">
                      <code>{g.name}{g.args ? ` ${g.args}` : ''}</code>
                      {#if g.output}
                        <pre class="command-out" class:clamped={longOut && !expanded.has(cmdKey)}>{g.output}</pre>
                        {#if longOut}
                          <button type="button" class="linkish" data-testid="conv-command-toggle" onclick={() => togglePrompt(cmdKey)}
                            >{expanded.has(cmdKey) ? 'Show less' : 'Show more'}</button
                          >
                        {/if}
                      {/if}
                    </div>
                  {:else if g.kind === 'interrupt'}
                    <div class="interrupt" data-testid="conv-interrupt">Interrupted{g.during_tool ? ' during a tool call' : ''}</div>
                  {:else if g.kind === 'subagent'}
                    <SubagentBlock item={g} {nowMs} live={turnLive} />
                  {/if}
                {/each}
                {#if duration}
                  <div class="duration" data-testid="conv-duration" title="From the prompt to the reply's last entry">{duration}</div>
                {/if}
              </div>
            </section>
            {/if}
          {/each}
        {/if}
        {#if pending && viewing === null}
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
        {#if viewing === null}
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
            <span class="indicator-label">{indicator.kind === 'sent' ? 'Sent, waiting for Claude…' : indicatorLabel}</span>
          </div>
        {/if}
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
            disabled={sending || viewing !== null}
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
              disabled={sending || viewing !== null}
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
          disabled={sending || viewing !== null}
        ></textarea>
        <button type="submit" data-testid="conv-composer-send" disabled={!canSend}>{sending ? 'Sending…' : 'Send'}</button>
      </div>
      {#if statusNote}
        <div class="composer-foot">
          <div class="composer-status" data-testid="conv-composer-status">{statusNote}</div>
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
  .scroller:focus {
    outline: none;
  }
  .toolbar,
  .find {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.25rem 1.1rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    font-size: 0.74rem;
  }
  .toolbar {
    justify-content: flex-end;
  }
  .find input {
    flex: 1 1 auto;
    min-width: 0;
    max-width: 40ch;
    padding: 0.2rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-pane);
    color: var(--fg);
    font: inherit;
  }
  .find input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .find-count {
    min-width: 4.5ch;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .tb-btn {
    padding: 0.1rem 0.45rem;
    border: 1px solid transparent;
    border-radius: 4px;
    background: none;
    color: var(--fg-muted);
    font-size: 0.74rem;
    cursor: pointer;
  }
  .tb-btn:hover:not(:disabled),
  .tb-btn:focus-visible {
    border-color: var(--border);
    color: var(--fg);
  }
  .tb-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .turns-wrap {
    position: relative;
  }
  .turn-index {
    position: absolute;
    right: 0;
    top: calc(100% + 0.25rem);
    z-index: 3;
    width: min(60ch, 80vw);
    max-height: 22rem;
    overflow: auto;
    margin: 0;
    padding: 0.25rem 0;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    box-shadow: 0 4px 14px color-mix(in srgb, var(--fg) 15%, transparent);
  }
  .turn-index:focus {
    outline: none;
  }
  .turn-index button {
    display: flex;
    width: 100%;
    align-items: baseline;
    gap: 0.75rem;
    padding: 0.3rem 0.65rem;
    border: none;
    background: none;
    color: var(--fg);
    font: inherit;
    font-size: 0.78rem;
    text-align: left;
    cursor: pointer;
  }
  .turn-index button:hover,
  .turn-index button:focus-visible {
    outline: none;
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  .ti-label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  [data-match] {
    outline: 1px dashed color-mix(in srgb, var(--accent) 55%, transparent);
    outline-offset: 3px;
    border-radius: 4px;
  }
  [data-current-match] {
    outline: 2px solid var(--accent);
  }
  .copy-slot {
    opacity: 0;
    transition: opacity 0.1s ease;
  }
  .prompt:hover .copy-slot,
  .prompt:focus-within .copy-slot,
  .text:hover > .copy-slot,
  .text:focus-within > .copy-slot {
    opacity: 1;
  }
  .head-right {
    display: inline-flex;
    align-items: baseline;
    gap: 0.45rem;
  }
  .text-copy {
    position: absolute;
    top: 0;
    right: 0;
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
    position: relative;
    margin: 0.35rem 0 0.6rem;
  }
  .duration {
    margin-top: 0.3rem;
    color: var(--fg-muted);
    font-size: 0.7rem;
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
  .tools-body {
    margin-left: 1.1rem;
  }
  .tools.single {
    margin: 0;
  }
  .tools.single summary {
    display: none;
  }
  .tools.single .tools-body {
    margin-left: 0;
  }
  .viewing,
  .switch-notice {
    flex: 0 0 auto;
    display: flex;
    align-items: baseline;
    justify-content: center;
    gap: 0.35rem;
    padding: 0.3rem 1.1rem;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-pane));
    color: var(--fg-muted);
    font-size: 0.78rem;
  }
  .viewing .linkish,
  .switch-notice .linkish {
    margin-top: 0;
  }
  .dismiss {
    margin-left: 0.4rem;
    padding: 0 0.3rem;
    border: none;
    background: none;
    color: var(--fg-muted);
    font-size: 0.95rem;
    line-height: 1;
    cursor: pointer;
  }
  .dismiss:hover {
    color: var(--fg);
  }
  .event {
    display: flex;
    align-items: baseline;
    justify-content: center;
    flex-wrap: wrap;
    gap: 0.45rem;
    margin: 0.2rem 0 0.9rem;
    color: var(--fg-muted);
    font-size: 0.74rem;
    text-align: center;
  }
  .event .detail {
    overflow-wrap: anywhere;
  }
  .event[data-tone='warn'] {
    color: #e6a23c;
  }
  .event[data-tone='error'] {
    color: #e64a4a;
  }
  .compact {
    margin: 0.4rem 0 0.6rem;
    padding: 0.3rem 0.6rem;
    border: 1px dashed var(--border);
    border-radius: 6px;
  }
  .compact summary {
    cursor: pointer;
    color: var(--fg-muted);
    font-size: 0.76rem;
    user-select: none;
  }
  .command {
    margin: 0.35rem 0 0.5rem;
  }
  .command code {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.76rem;
    color: var(--accent);
  }
  .command-out {
    margin: 0.25rem 0 0;
    padding: 0.35rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-pane);
    color: var(--fg-muted);
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.72rem;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .command-out.clamped {
    max-height: calc(8 * 1.45em + 0.7rem);
    overflow: hidden;
  }
  .interrupt {
    margin: 0.3rem 0 0.5rem;
    color: #e6a23c;
    font-size: 0.76rem;
    font-style: italic;
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
