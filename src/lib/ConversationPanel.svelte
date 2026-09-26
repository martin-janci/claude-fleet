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
  import { untrack, tick, setContext, type Snippet } from 'svelte';
  import { requestOpenPath, OPEN_PATH_CONTEXT, type OpenPathFn } from './app_views';
  import { sendPrompt, hasNoPane, sessions, type SessionRow } from './sessions';
  import AnswerPrompt from './AnswerPrompt.svelte';
  import { pendingInputFor } from './pending_input';
  import { hintAnchor } from './hints';
  import { composerPresets, type ComposerPreset } from './composer_presets';
  import { needsMore, wrapsPastOneLine } from './composer_overflow';
  import { contextLevel } from './attention';
  import { timeAgo } from './session_status';
  import { onTimelineEvent, onConversationsChanged } from './live_events';
  import type { SessionEvent } from './timeline';
  import ConversationHeader from './ConversationHeader.svelte';
  import ToolLine from './ToolLine.svelte';
  import SubagentBlock from './SubagentBlock.svelte';
  import CopyButton from './CopyButton.svelte';
  import {
    findMatches,
    turnIndex,
    rowKey,
    rememberScroll,
    recallScroll,
    forgetScroll,
    anchorAt,
    resolveScroll,
    nearestTurn,
    turnKeyNear,
    adjacentTurn,
  } from './conversation_nav';
  import { detectMac, isEditable } from './terminal_keys';
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
    emptyStateHint,
    relativeTime,
    groupItems,
    toolGroupLabel,
    notificationTone,
    notificationMark,
    notificationLabel,
    transcriptBackground,
    fleetBackground,
    isLongPrompt,
    PROMPT_CLAMP_LINES,
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
    composerInsert,
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
    type BackgroundEntry,
    type ConvGroup,
  } from './conversation';
  import { highlightNames, highlightCss, paintHighlights, clearHighlights } from './conversation_highlight';
  import { invokeCmd } from './result';
  import { addFiles, pastedName, fmtBytes, markNeedsReattach, clearSent, type Attachment, type PickedFile } from './attachments';
  import { withAttachments, tooLong } from './attach_prompt';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { pointInRect } from './geometry';
  import Markdown from './MarkdownView.svelte';
  import BackgroundDetail from './BackgroundDetail.svelte';
  import SpiralLoader from './SpiralLoader.svelte';
  import { selectSessionExplicitly } from './selection';
  import { tasks } from './tasks';

  let {
    session,
    visible,
    onOpenTerminal,
    // Find is Cmd+F on macOS (Ctrl+F moves the caret there), Ctrl+F elsewhere.
    isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator),
    // Opt-out for a host that renders its own prompt UI over this same
    // session. Two composers sending independently into one tmux REPL is the
    // interleaved-paste failure this flag exists to prevent — a host that
    // sets this false owns being the only sender, and owns everything that
    // goes with that (see `promptPrefix`).
    showComposer = true,
    // Text prepended to every prompt this composer sends (AgentPanel's
    // context chip). A prefix is a reason to feed THIS composer, not to
    // build a second one: `pending`, `optimistic` and the immediate refetch
    // are all set in `send()` and nowhere else, so a host that sends around
    // it gets a sheet that shows nothing until the next quiet tick — 15 s
    // later — with no indicator in between. A slash command is exempt: the
    // REPL reads the line exactly as typed.
    promptPrefix = null,
    // Refuse a prompt while the session is working or stuck, instead of
    // letting it queue behind the running turn. OFF by default, which is the
    // Conversation tab's long-standing behaviour: typing ahead of a turn is
    // a deliberate workflow there, and the note under the box ("queued until
    // the current turn ends") is the honest description of it.
    //
    // The agent sheet asks for it (`AgentPanel.svelte`), because that is what
    // its own composer did before it was deleted: the sheet is a one-shot
    // "ask the agent" surface, not a queue, and two prompts pasted into one
    // REPL mid-turn arrive as one mangled line. Restoring the gate there is
    // the conservative move; extending it to the tab would be a new
    // restriction nobody asked for, so the scope is an explicit prop rather
    // than a rule this component invents for both surfaces.
    //
    // A bare key press is never gated — see `sendText`.
    blockWhileBusy = false,
    // Rendered directly above the composer, inside the panel's own layout
    // (AgentPanel's removable context chip). Only shown when there IS a
    // composer to sit above.
    composerAbove,
  }: {
    session: SessionRow;
    visible: boolean;
    onOpenTerminal?: () => void;
    isMac?: boolean;
    showComposer?: boolean;
    promptPrefix?: string | null;
    blockWhileBusy?: boolean;
    composerAbove?: Snippet;
  } = $props();

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
  // True between `resetView()` and the first load that lands on the fresh
  // view. The scroller still holds the OUTGOING conversation's geometry at
  // that point (the reset has not flushed), so measuring it would report
  // "scrolled up" and count the whole incoming transcript as unseen. A view
  // that has just been reset is pinned by definition.
  let justReset = $state(true);
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
  // The chip row holds one line; whatever does not fit collapses behind
  // "More". Measured, not computed, since it depends on layout.
  let chipsRow: HTMLDivElement | undefined = $state();
  let chipsOverflow = $state(false);
  let chipsExpanded = $state(false);
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
  // Set once the hub has told us it does not serve `session_activity` at all
  // (see `PROBE_UNSUPPORTED_CODES`). It stops the poll loop and replaces the
  // live detail with one quiet line, rather than leaving the user with the
  // silent "no live indicator" state the feature was meant to remove while a
  // failing round-trip goes out every 2 s. Cleared on a session change, so
  // an upgraded hub is re-discovered without restarting the app.
  let probeUnsupported = $state(false);
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

  // The background entry whose detail replaces the thread; null = the thread.
  // Keyed by BackgroundEntry.key, not by index: the list re-derives on every
  // poll and a running entry moves as it finishes.
  let background = $state<string | null>(null);
  let backgroundOpen = $state(false);

  // Two groups, each sorted running-first on its own. Sorting across them
  // would interleave exactly what the headings exist to keep apart.
  const bgTranscript = $derived(conv ? transcriptBackground(conv.turns) : []);
  const bgFleet = $derived(fleetBackground($sessions, $tasks, session.id));
  const bgGroups = $derived(
    [
      { title: 'In this conversation', entries: bgTranscript },
      { title: 'Fleet children', entries: bgFleet },
    ].filter((g) => g.entries.length > 0),
  );
  // The flat list behind the count on the button and every key lookup.
  const bgEntries = $derived([...bgTranscript, ...bgFleet]);
  const bgEntry = $derived(bgEntries.find((e) => e.key === background) ?? null);

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
    const pinned = justReset || !scroller ? true : isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight);
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
      justReset = false;
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
   *  made stale. Also the natural place to drop an open background detail —
   *  this runs on every path that changes what the panel shows (a session
   *  switch via resetThread, an automatic /clear or /resume follow via
   *  resetThread, and the header switcher's `select`). */
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
    justReset = true;
    turnsWanted = undefined;
    loadingOlder = false;
    turnsOpen = false;
    background = null;
    backgroundOpen = false;
    findOpen = false;
    findQuery = '';
    findIndex = 0;
    clearHighlights(hlNames);
  }

  /** resetView plus the send and probe state of the current conversation;
   *  the draft is the caller's business. */
  function resetThread() {
    resetView();
    pending = null;
    setProbe(null);
    probeSeq++;
    // Whether the hub serves `session_activity` is a fact about the hub, not
    // about the session — but re-learning it costs exactly one call per
    // session switch, and it is what lets an upgraded hub start answering
    // again without an app restart.
    probeUnsupported = false;
    sentTurnSeq = null;
    idleSeenSinceSend = false;
  }

  // Reset + immediate fetch on session change. Nothing is snapshotted here:
  // the position is remembered as the user scrolls (`rememberHere`), so an
  // unmount — which this effect never sees — is remembered too.
  $effect(() => {
    void sessionId;
    untrack(() => {
      // The outgoing conversation is still on screen and still scrolled
      // where the reader left it: measure it now, before the reset wipes it.
      flushScroll();
      resetThread();
      viewing = null;
      newerAvailable = false;
      switchNotice = null;
      conversations = [];
      draft = composerDrafts.get(session.id) ?? '';
      draftFor = session.id;
      histIndex = null;
      sendError = null;
      // The tray belongs to the session it was filled for. This panel is ONE
      // instance for every session (App.svelte does not `{#key}` it), and the
      // Rust allow-list authorises a path, not a path plus a destination — a
      // picked path stays valid for four hours. So a tile left here would
      // stage the old session's file into the NEW session's worktree, on the
      // new host, and name it in the new prompt. Nothing downstream can
      // catch that; it has to be dropped here.
      attachments = [];
      attachErrors = [];
      // Restore where the returning session was left, once its fetch lands —
      // a snapshot only exists when it was scrolled away from the bottom
      // (`rememberScroll` drops an at-bottom one), so recalling one always
      // means "come back here", not "come back to the bottom".
      const id = sessionId;
      void load().then(() => {
        if (sessionId !== id) return;
        const snap = recallScroll(id);
        if (!snap) return;
        void tick().then(() => {
          // `turnAt` names the turn itself; the key it wears in THIS window
          // is whatever the freshly built index says. No match (the turn
          // fell out of the window, or the row is simply not rendered) →
          // leave the pinned-to-bottom state alone rather than showing
          // "↓ Latest" over a view that is, in fact, already at the bottom.
          const key = resolveScroll(turnEntries, snap);
          if (key !== null && scrollToRow(key)) {
            atBottom = false;
            // The count belongs to the load that just filled this view, not
            // to the reader: nothing here has gone unseen.
            unseen = 0;
            currentTurnPos = nearestTurn(turnEntries, key);
          }
        });
      });
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
      // A followed /clear or /resume replaces the transcript wholesale: a
      // position inside the old one would restore into unrelated content.
      forgetScroll(sid);
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
    // Another conversation entirely — the remembered position belonged to
    // the one being left.
    forgetScroll(sessionId);
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

  // The dialog to answer, if the pane is showing one. A fresh probe is the
  // authority, including when it says there is none: the row is written by
  // the 20 s tick, and buttons that outlive their dialog are worse than no
  // buttons. Never while viewing an earlier conversation — that pane is
  // read-only history.
  const answerView = $derived(
    viewing !== null
      ? null
      : pendingInputFor({
          rowStatus: session.claude_status,
          rowStuck: session.stuck_kind,
          rowPending: session.pending_input,
          probe: liveProbe,
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
  let turnsOpen = $state(false);
  /** The header renders the find box and the turn index; ⌘F needs its input. */
  let header: ConversationHeader | undefined = $state();

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
  /** Scrolls to the row and reports whether one was found: false means the
   *  key names a row outside the currently loaded window (or an empty
   *  thread), so the caller must not act as though the scroll happened. */
  function scrollToRow(key: string): boolean {
    const el = rowEl(key);
    if (!el || typeof el.scrollIntoView !== 'function') return false;
    el.scrollIntoView({ block: 'center' });
    return true;
  }

  /** The row nearest the top of the visible scroller area: the first row (in
   *  document order) whose bottom edge is below the scroller's own top edge.
   *  `el.getBoundingClientRect().bottom > scroller.getBoundingClientRect().top`
   *  is the scroll-position-independent form of "row bottom below
   *  scroller.scrollTop" (the scrollTop term cancels between the two rects).
   *  Backs the per-session scroll memory (`rememberScroll`) and the `[`/`]`
   *  turn stepper's "where am I" when no turn is otherwise current. */
  function topVisibleRowKey(): string | null {
    if (!scroller) return null;
    const top = scroller.getBoundingClientRect().top;
    for (const el of Array.from(scroller.querySelectorAll<HTMLElement>('[data-row-key]'))) {
      if (el.getBoundingClientRect().bottom > top) return el.dataset.rowKey ?? null;
    }
    return null;
  }

  /** Every rendered row key in document order — what `turnKeyNear` walks to
   *  turn an inline event's key into the turn it should resolve to. */
  function rowKeys(): string[] {
    if (!scroller) return [];
    return Array.from(scroller.querySelectorAll<HTMLElement>('[data-row-key]')).map((el) => el.dataset.rowKey ?? '');
  }

  // Position in `turnEntries` nearest the current read position, kept in
  // sync with actual scrolling (see the `onScroll`/`scrollToBottom` calls
  // below) rather than recomputed on every render: `topVisibleRowKey` reads
  // live layout, which is not itself a Svelte reactive dependency. Backs
  // both the `[`/`]` step target and the stepper buttons' disabled state.
  let currentTurnPos = $state(0);
  function refreshCurrentTurnPos() {
    // An inline event is not a turn: resolve it FORWARD to the turn below it
    // rather than letting `nearestTurn` fall back to 0, which sent `]` to
    // the top of the conversation.
    currentTurnPos = nearestTurn(turnEntries, turnKeyNear(rowKeys(), topVisibleRowKey(), 1));
  }

  /** Remember the current read position for this session (see `rememberScroll`).
   *  Anchored on the turn's `at`, so the window may slide or grow before the
   *  reader comes back. */
  function rememberHere(id: number) {
    if (!scroller) return;
    const key = topVisibleRowKey();
    if (atBottom || key === null) {
      // At the bottom there is nothing to come back TO: the default view is
      // already the latest turn, so drop any earlier snapshot.
      forgetScroll(id);
      return;
    }
    rememberScroll(id, {
      turnAt: anchorAt(turnEntries, turnKeyNear(rowKeys(), key, -1)),
      rowKey: key,
      atBottom,
    });
  }
  const prevTurn = $derived(adjacentTurn(turnEntries, currentTurnPos, -1));
  const nextTurn = $derived(adjacentTurn(turnEntries, currentTurnPos, 1));

  /** `[` / `]` and the turn-stepper buttons: jump to the turn adjacent to
   *  the one nearest the current read position. */
  function stepTurn(delta: 1 | -1) {
    if (turnEntries.length === 0) return;
    refreshCurrentTurnPos();
    const next = adjacentTurn(turnEntries, currentTurnPos, delta);
    if (next && scrollToRow(next.rowKey)) currentTurnPos += delta;
  }

  async function openFind() {
    // Nothing to search while the thread is loading or empty. The shortcut
    // checks this too (it also owns preventDefault); the header's ⌕ button
    // is always on screen now, so the guard has to live here as well.
    if (!threadShown) return;
    turnsOpen = false;
    findOpen = true;
    await tick();
    header?.focusFindInput();
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
      // Escape dismisses find wherever focus sits in the panel — clicking a
      // match moves focus into the thread, and the bar must not strand
      // there. An open menu owns the key first, so one Escape closes one
      // thing. The open menus are checked directly rather than through
      // `defaultPrevented`: Svelte delegates `onkeydown` to the document, so
      // this listener runs before the handler that would mark it handled.
      if (e.key === 'Escape') {
        if (findOpen && !slashOpen && !turnsOpen) {
          e.preventDefault();
          closeFind();
        }
        return;
      }
      // `[` / `]` step the turn stepper — only away from any text entry
      // (the composer, the find box), same guard the terminal uses for its
      // own global shortcuts.
      if (
        (e.key === '[' || e.key === ']') &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        !isEditable(e.target as HTMLElement | null)
      ) {
        if (!threadShown) return;
        e.preventDefault();
        stepTurn(e.key === '[' ? -1 : 1);
        return;
      }
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
  const hlNames = highlightNames(hlSuffix);
  // `::highlight()` names cannot be dynamic in the component's stylesheet:
  // each panel adds (and on unmount removes) the two rules for its own names.
  $effect(() => {
    const el = document.createElement('style');
    el.dataset.convFind = String(hlSuffix);
    el.textContent = highlightCss(hlNames);
    document.head.appendChild(el);
    return () => el.remove();
  });
  $effect(() => {
    paintHighlights(scroller, hlNames, { keys: matchKeys, current: currentMatch, query: findQuery });
  });
  $effect(() => () => clearHighlights(hlNames));

  /** Turn index: jump to a turn and close the list. */
  function pickTurn(key: string) {
    turnsOpen = false;
    scrollToRow(key);
  }

  /** Open a background entry. A fleet session is a place, not a report: it
   *  has its own transcript, terminal and composer, so it takes the whole
   *  app rather than this pane. */
  function openBackground(e: BackgroundEntry): void {
    backgroundOpen = false;
    if (e.source === 'fleet_session' && e.sessionId !== null) {
      goToSession(e.sessionId);
      return;
    }
    background = e.key;
  }

  /** Switch the whole app to a fleet session by id, when the store has it.
   *  Shared by `openBackground` and the detail's worker-session link. */
  function goToSession(id: number): void {
    const row = $sessions.find((s) => s.id === id);
    if (row) selectSessionExplicitly(row);
  }

  /** The entry a notification row belongs to: the call it named, which is
   *  exactly how `transcriptBackground` keys one. A task id is deliberately
   *  not tried — two calls can report the same one. */
  function entryForNotification(n: { tool_use_id: string | null }): BackgroundEntry | null {
    if (n.tool_use_id === null) return null;
    return bgEntries.find((e) => e.key === `tool:${n.tool_use_id}`) ?? null;
  }

  /** The switcher entry for a subagent block, so the block can offer a way
   *  into its detail. Null while the switcher does not list it (a finished
   *  foreground call), and the block then shows no control. */
  function entryForSubagent(id: string | null): BackgroundEntry | null {
    if (id === null) return null;
    return bgEntries.find((e) => e.key === `tool:${id}`) ?? null;
  }

  /** Probe failures that mean *this hub will never answer this call*, as
   *  opposed to "not right now". Matched by CODE, never by message text.
   *
   *  `session_activity` is new in this release and the desktop routes it to
   *  the hub, so a desktop paired with a hub pinned to an older tag calls a
   *  tool that hub's router does not know:
   *
   *  - `E_HUB_PROTOCOL` is the JSON-RPC `error` the router answers with for
   *    an unknown tool. `src-tauri/src/backend/remote.rs` gives it its own
   *    code for exactly this purpose — "so a caller built on a tool an older
   *    hub does not serve can degrade to what it did before that tool
   *    existed".
   *  - `E_FORBIDDEN` is the same fact seen through the hub's client gate.
   *    `session_activity` is `Access::Client` + readonly in the policy table
   *    of every hub that serves it (`mcp/guard.rs`), so neither gate can
   *    refuse a paired client for it — and both fail closed on the tool NAME,
   *    so a hub with no row for it refuses with "not a client-callable tool"
   *    rather than "no such tool". A policy refusal for THIS tool therefore
   *    means the name is unknown there. (Same reasoning, same pair of codes,
   *    as `NewSessionDialog.svelte`'s `HUB_CANNOT_SCAN`.)
   *
   *  Everything else keeps retrying, because it can clear without this panel
   *  being remounted: `E_HUB_UNREACHABLE` / `E_HUB_TIMEOUT` (the hub or the
   *  host is having a moment), `E_HUB_CONTRACT` (refused before the transport
   *  is touched — no round-trip, no hub log line — and cleared by the next
   *  in-range `ready` frame), `E_UNAUTHORIZED` (cleared by re-pairing) and
   *  every per-session backend error such as `E_INVALID_STATE`. */
  const PROBE_UNSUPPORTED_CODES = ['E_HUB_PROTOCOL', 'E_FORBIDDEN'];

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
    if (!r.ok) {
      // A permanent refusal stops the loop instead of being repeated every
      // 2 s per open panel for as long as the panel is open. A transient one
      // is swallowed as before: the next tick is the retry.
      if (PROBE_UNSUPPORTED_CODES.includes(r.error.code)) probeUnsupported = true;
      return;
    }
    setProbe(r.value);
    if (sentTurnSeq !== null && isQuietStatus(r.value.claude_status)) idleSeenSinceSend = true;
  }

  // Probe the pane every couple of seconds while something is live: once at
  // once, then on the interval. `probeLive` is a boolean derived, so a fresh
  // probe (which yields a new `indicator` object) never restarts the timer.
  // bg / external rows have no pane: the backend would reject every probe.
  //
  // A hub client probes too. It used to be excluded — `session_activity` was
  // local-only, so the call could only ever have returned E_LOCAL_ONLY — and
  // the cost was that a remote desktop had NO live signal at all between one
  // row status change and the next: no spinner, no activity line, nothing
  // moving for the length of a turn. The command routes to the hub's own
  // tool now, which reads the pane over the ssh connection that can actually
  // reach the host.
  const probeLive = $derived(
    visible && !hasNoPane(session) && indicator !== null && viewing === null && !probeUnsupported,
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

  const gone = $derived(viewing !== null && errorCode === 'E_NO_TRANSCRIPT');
  const empty = $derived(gone ? 'Transcript no longer on host' : emptyStateText(errorCode, !!session.claude_session_id));
  const canPrompt = $derived(!hasNoPane(session));
  const emptyHint = $derived(
    gone
      ? 'The host no longer keeps this conversation’s transcript file.'
      : emptyStateHint(errorCode, !!session.claude_session_id, canPrompt),
  );
  // The scroller (and so the thread) is on screen: find has something to search.
  const threadShown = $derived(!(empty && !(pending && viewing === null)) && !loading);

  // Keep the unsent text across tab switches (the panel unmounts).
  $effect(() => {
    if (draftFor !== null) rememberDraft(draftFor, draft);
  });

  // "Insert into composer" from the ticket card (work graph M9.2): adopt the
  // new draft when it is for the session shown, and put the cursor at its
  // end. It is never sent from here.
  $effect(() => {
    const ins = $composerInsert;
    if (!ins || ins.sessionId !== session.id) return;
    untrack(() => {
      // The store keeps the last insert: on a later mount it must not undo
      // what was typed since — only a draft still equal to it is adopted.
      if (composerDrafts.get(session.id) !== ins.draft) return;
      draft = ins.draft;
      draftFor = session.id;
      void tick().then(() => {
        box?.focus();
        box?.setSelectionRange(draft.length, draft.length);
      });
    });
  });

  // Put the cursor in the composer when the tab shows a promptable session,
  // and again when the selection moves to another one.
  $effect(() => {
    void sessionId;
    if (!visible || !canPrompt) return;
    void tick().then(() => box?.focus());
  });
  function measureChips() {
    if (!chipsRow) return;
    const row = chipsRow;
    // Untracked: the effect below calls this, and reading `chipsExpanded`
    // there would make every toggle re-create the observer.
    const expanded = untrack(() => chipsExpanded);
    chipsOverflow = expanded
      ? wrapsPastOneLine(Array.from(row.children, (c) => (c as HTMLElement).offsetTop))
      : needsMore(row.scrollWidth, row.clientWidth);
    if (!chipsOverflow) chipsExpanded = false;
  }
  $effect(() => {
    if (!chipsRow) return;
    // The row's own border-box need not change when the preset count does,
    // so a ResizeObserver on it alone can miss a preset-list change. Read
    // the store here so the effect re-runs (and re-measures) whenever it does.
    void $composerPresets;
    measureChips();
    // ResizeObserver is absent in jsdom; the resize listener is what the
    // component test drives, and both paths call the same measurement.
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measureChips);
    ro?.observe(chipsRow);
    window.addEventListener('resize', measureChips);
    return () => {
      ro?.disconnect();
      window.removeEventListener('resize', measureChips);
    };
  });
  // ONE signal, two uses: the sentence under the box, and — only where the
  // host asked for it — the gate that refuses the send. Both read this same
  // `$derived`, so they cannot disagree about whether the session is busy.
  // (The pre-refactor code claimed exactly that in a comment while the two
  // composers shared only the note; that is how the gate was lost. The
  // comment is true now because there is one expression, not two.)
  const busyNote = $derived(composerStatus({ claude_status: liveStatus, stuck_kind: liveStuck }));
  const busyBlocked = $derived(blockWhileBusy && busyNote !== null);
  const canSend = $derived(draft.trim().length > 0 && !sending && viewing === null && !busyBlocked);
  const statusNote = $derived(
    viewing !== null ? 'Viewing an earlier conversation — go back to current to send.' : busyNote,
  );
  // The context meter lives in the header; at warn/crit the Compact chip is
  // suggested, since that is the one-click remedy.
  const ctxLevel = $derived(contextLevel(session.context_pct));
  const suggestCompact = $derived(ctxLevel === 'warn' || ctxLevel === 'crit');
  const isCompactPreset = (p: ComposerPreset) => /^\/compact\b/.test(p.text.trim());
  const slashMatches = $derived(slashDismissedFor === draft ? [] : matchSlashCommands(draft));
  // Ids for the combobox wiring, per panel instance so two panels never hand
  // the same id to assistive tech.
  const SLASH_LIST_ID = `conv-slash-list-${hlSuffix}`;
  const slashOptionId = (i: number) => `conv-slash-opt-${hlSuffix}-${i}`;
  const COMPOSER_HINT_ID = `conv-composer-hint-${hlSuffix}`;
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
    // A gated composer (see `blockWhileBusy`) degrades Shift+click to a
    // plain click rather than swallowing it: the chip still fills the box,
    // the note under the box says why it did not go, and one press of Send
    // finishes the job once the turn ends. A disabled chip would take the
    // fill away too, and a silent no-op would say nothing at all.
    if (sendNow && !busyBlocked) {
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

  /** What a send IS, which is what decides the prefix, the attachments, the
   *  busy gate and whether a pending turn is recorded. Derived once, here,
   *  rather than re-tested as `startsWith('/')` at each of those four places
   *  — the accident that let a context paragraph ride in on a bare Enter.
   *
   *  - `key`     — a bare key press (the press_enter chip's `''`). Not a
   *                prompt at all: no prefix, no attachments, nothing pending,
   *                and never gated, because it is the recovery path OUT of
   *                the stuck state a gate would be reading.
   *  - `command` — a slash command. The REPL reads the line exactly as
   *                typed, so nothing may be glued to its nose.
   *  - `prompt`  — text Claude reads. The only kind the context prefix rides
   *                in front of, deliberately including a preset chip sent
   *                with Shift+click: the chip above the composer promises
   *                that everything this composer sends carries the context
   *                until it is dismissed, and "continue" needs that context
   *                more than a long typed prompt does. */
  type SendKind = 'key' | 'command' | 'prompt';
  const sendKind = (text: string): SendKind =>
    text === '' ? 'key' : text.startsWith('/') ? 'command' : 'prompt';

  /** Send `text` as-is. Empty text is a bare Enter (the press_enter chip):
   *  it lands in the REPL but is not a prompt, so nothing is shown pending. */
  async function sendText(text: string, opts: { fromDraft?: boolean } = {}) {
    if (sending || viewing !== null) return;
    const kind = sendKind(text);
    if (kind !== 'key' && busyBlocked) return;
    sending = true;
    sendError = null;
    const id = session.id;

    // Attachments upload first; their remote paths ride along in the prompt
    // text. A pasted entry has an empty `path` (see attachments.ts) — never
    // authorised for the allow-list — so it is left out of `local_paths`
    // rather than sent to `upload_attachments` at all. With nothing
    // uploadable, nothing can fail: the draft (if any) still goes out on its
    // own rather than being refused over a tile that was already showing its
    // own honest error, and the tile itself is never touched below — it was
    // never attempted, so it is not this send's to clear or flag.
    // A key press carries nothing: it must not upload, spend or clear a tile
    // the user attached for the prompt they have not sent yet.
    const toUpload = kind === 'key' ? [] : attachments.filter((a) => a.path !== '');
    const toUploadIds = new Set(toUpload.map((a) => a.id));

    // `upload_attachments` consumes each path's allow-list entry the moment
    // it clears the byte budget — before a byte moves, and not undone by a
    // later failure (`UploadAllowList::consume` in
    // `src-tauri/src/commands/upload.rs`). So a failure anywhere downstream
    // of that point — the upload call itself, this side's own `tooLong`
    // refusal, or a failed `sendPrompt` — can leave a tile in `toUpload`
    // looking untouched while its local path is already unusable for a
    // second attempt: pressing Send again would call `upload_attachments`
    // with the same path and get back an authorisation error instead of a
    // real retry. Rather than guess which specific failure actually
    // consumed it, every failure below marks the attempted tiles as spent —
    // occasionally more cautious than strictly necessary (a size-budget
    // refusal happens before `consume`), never wrong.
    let paths: string[] = [];
    if (toUpload.length > 0) {
      const up = await invokeCmd<string[]>('upload_attachments', {
        args: {
          host_alias: session.host_alias,
          session_name: session.tmux_name,
          local_paths: toUpload.map((a) => a.path),
        },
      });
      if (session.id !== id) {
        sending = false;
        return;
      }
      if (!up.ok) {
        // The draft stays: a prompt without its attachment is a worse
        // outcome than no prompt at all.
        sendError = up.error.message;
        attachments = markNeedsReattach(attachments, toUploadIds);
        sending = false;
        return;
      }
      paths = up.value;
    }

    // The prefix rides in front of the attachment line, and in front of a
    // `prompt` ONLY. Not a `command` — `/clear` with a paragraph glued to its
    // nose is not a command the REPL runs — and not a `key`, where the whole
    // point is that a bare Enter reaches the pane as a bare Enter.
    const prefixed =
      promptPrefix && kind === 'prompt' ? `${promptPrefix}\n\n${text}` : text;
    const body = withAttachments(prefixed, paths);
    // A remote send is quoted twice (see attach_prompt.ts's header comment
    // for why that compounds rather than doubles), so the bound applied
    // here must match the host the prompt is actually going to.
    if (tooLong(body, session.host_alias === 'local')) {
      // The file(s), if any, already uploaded successfully — only the
      // prompt text is refused — so a retry needs a shorter prompt AND,
      // since the upload above already spent the tile, a fresh attach.
      sendError = 'That prompt is too long to send through tmux. Shorten it.';
      attachments = markNeedsReattach(attachments, toUploadIds);
      sending = false;
      return;
    }

    const r = await sendPrompt(session.host_alias, session.tmux_name, body);
    sending = false;
    // The selection moved while the send was on the wire: the prompt landed
    // in the old session; none of its state belongs to the new one.
    if (session.id !== id) return;
    if (!r.ok) {
      sendError = r.error.message;
      attachments = markNeedsReattach(attachments, toUploadIds);
      return;
    }
    // A key press is spent here: it went into the pane, it is not a prompt,
    // so there is no draft to clear, no tray to spend and nothing pending to
    // show. Returning BEFORE the attachment bookkeeping is the point — the
    // tray belongs to the prompt the user is still composing.
    if (kind === 'key') return;
    // Only the attachments this send actually uploaded are spent; anything
    // it could not upload (a pasted entry) was never attempted and stays,
    // so the evidence that it did not go is not lost.
    attachments = clearSent(attachments, toUploadIds);
    // The notes were about the tray that just went; they do not carry over.
    attachErrors = [];
    switchNotice = null;
    // Only the box's own text is spent by a send; a chip sent with
    // Shift+click leaves whatever the user was typing.
    if (opts.fromDraft) draft = '';
    histIndex = null;
    // A slash command is handled by the REPL itself: it is not recorded as a
    // prompt (and /clear even moves to a new session id), so no pending
    // turn, and nothing to wait for beyond a fresh read.
    if (kind === 'command') {
      box?.focus();
      void load();
      return;
    }
    sentTurnSeq = session.turn_seq;
    idleSeenSinceSend = false;
    pending = {
      prompt: body,
      at: new Date().toISOString(),
      // how many turns already carried this exact text, so a repeat of an
      // earlier prompt is not mistaken for the transcript catching up
      seen: conv?.turns.filter((t) => t.prompt === body).length ?? 0,
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
    refreshCurrentTurnPos();
  }

  // Reading every row's rect is a forced layout, and a scroll fires dozens
  // of events per gesture: one pending frame collapses a flick into a single
  // measurement. `scrollFrameFor` is the session the pending frame measured,
  // so a switch that flushes it still writes the snapshot under the OUTGOING
  // session's id (`sessionId` is the incoming one by then).
  let scrollFrame: number | null = null;
  let scrollFrameFor: number | null = null;
  function afterScroll() {
    const id = scrollFrameFor;
    scrollFrame = null;
    scrollFrameFor = null;
    if (id === null || !scroller) return;
    if (id === sessionId) refreshCurrentTurnPos();
    rememberHere(id);
  }
  /** Run a pending measurement now — a session switch or an unmount, either
   *  of which would drop the frame and lose the last scroll. */
  function flushScroll() {
    if (scrollFrame === null) return;
    cancelAnimationFrame(scrollFrame);
    afterScroll();
  }
  function onScroll() {
    if (!scroller) return;
    atBottom = isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight);
    if (atBottom) unseen = 0;
    if (scrollFrame === null) {
      scrollFrameFor = sessionId;
      scrollFrame = requestAnimationFrame(afterScroll);
    }
  }
  // The panel is ONE instance for every session, so an unmount is the only
  // place a pending frame is lost for good.
  $effect(() => () => flushScroll());

  /** Run a mutation that changes the composer's height without moving the
   *  transcript under the reader. The composer is flex: 0 0 auto at the
   *  bottom of a column flex, so it grows by taking from the scroller. */
  export function preserveThread(mutate: () => void): void {
    const el = scroller;
    if (!el) {
      mutate();
      return;
    }
    const wasAtBottom = atBottom;
    const before = el.clientHeight;
    mutate();
    requestAnimationFrame(() => {
      if (wasAtBottom) {
        el.scrollTop = el.scrollHeight;
        return;
      }
      el.scrollTop += before - el.clientHeight;
    });
  }

  // ---- attachments -------------------------------------------------------
  //
  // The composer holds the list; the limits, the naming and every rejection
  // sentence live in `attachments.ts`. Only two origins can ever produce a
  // path the Rust side will read or upload: the OS picker
  // (`pick_attachments` authorises its own result) and an OS drop (the Tauri
  // window event puts the paths on the allow-list before the webview sees
  // anything). A pasted file has neither, and `addFiles` says so.

  let attachments = $state<Attachment[]>([]);
  let attachErrors = $state<string[]>([]);
  /** dragleave fires for every child, so DOM nesting is counted, not flagged. */
  let dragDepth = $state(0);
  /** The veil's other source: Tauri's drag-drop event, hit-tested against the
   *  shell. Kept apart from `dragDepth` because the two arrive independently
   *  and neither can clear the other's state. */
  let dragOverShell = $state(false);
  let shellEl = $state<HTMLDivElement | null>(null);
  const dragging = $derived(dragDepth > 0 || dragOverShell);

  /**
   * Every sentence the composer owes the user about attaching: the
   * rejections `addFiles` returned, plus the per-tile errors — a pasted
   * file's above all, which would otherwise live only in a tooltip and read
   * as a bare red square. Deduped: the same sentence twice says nothing
   * twice (and `{#each}` needs the key to be unique).
   */
  const attachNotes = $derived([
    ...new Set([
      ...attachErrors,
      ...attachments.flatMap((a) => (a.state === 'error' && a.error ? [a.error] : [])),
    ]),
  ]);

  function hasFiles(dt: DataTransfer | null): boolean {
    return !!dt && Array.from(dt.types).includes('Files');
  }

  async function attach(picked: PickedFile[]) {
    const { next, rejected } = addFiles(attachments, picked);
    // Appended, not replaced: two gestures that each rejected a file owe the
    // user two sentences. `attachNotes` dedupes, so a repeat says it once,
    // and a send clears the lot.
    attachErrors = [...attachErrors, ...rejected];
    preserveThread(() => (attachments = next));
    // `pasted` is belt and braces: addFiles already lands a pasted entry as
    // `error`, never `reading`. Previewing one would come back E_FORBIDDEN
    // (no path was ever authorised) and replace an honest sentence with a
    // permission error.
    for (const a of next.filter((x) => !x.pasted && x.state === 'reading')) {
      const r = await invokeCmd<string | null>('attachment_preview', { path: a.path });
      attachments = attachments.map((x) =>
        x.id !== a.id
          ? x
          : r.ok
            ? // null is an ordinary answer: not an inlineable image, or over
              // 2 MiB. The tile falls back to its extension, not to an error.
              { ...x, thumb: r.value, state: 'ready' as const }
            : { ...x, state: 'error' as const, error: r.error.message },
      );
    }
  }

  /** The tile is the 44px control; the 18px × on it is a pointer shortcut.
   *  Backspace and Delete are what a composer's attachment chip does
   *  everywhere else, and without them the only way to remove one is that
   *  sub-24px button. */
  function onTileKey(e: KeyboardEvent, id: string) {
    if (e.key !== 'Backspace' && e.key !== 'Delete') return;
    e.preventDefault();
    removeAttachment(id);
  }

  function removeAttachment(id: string) {
    preserveThread(() => (attachments = attachments.filter((a) => a.id !== id)));
  }

  async function pickFiles() {
    const r = await invokeCmd<PickedFile[]>('pick_attachments', {});
    if (r.ok) await attach(r.value ?? []);
    else attachErrors = [r.error.message];
  }

  function onShellDragEnter(e: DragEvent) {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    dragDepth++;
  }
  function onShellDragOver(e: DragEvent) {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
  }
  function onShellDragLeave() {
    dragDepth = Math.max(0, dragDepth - 1);
  }
  /**
   * The DOM drop carries NO usable file: in a WKWebView a dropped `File` has
   * no filesystem path, and a path is the only thing the Rust allow-list
   * matches on. So this handler exists for one reason — to stop the event
   * reaching App.svelte's window-level guard, which would otherwise let the
   * webview navigate to `file://` and take the whole app state with it. The
   * paths arrive on Tauri's own drag-drop event instead (see below).
   */
  function onShellDrop(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    dragDepth = 0;
  }

  function pointInShell(px: number, py: number): boolean {
    // `.view-slot` is `position: absolute; inset: 0`, so App.svelte's Hosts
    // and Assets overlays cover a panel that is still mounted and still laid
    // out at these very coordinates. Without this, a drop while one of them
    // is open attaches a file under an opaque overlay — the veil drawn
    // beneath it, the user seeing nothing happen.
    if (!visible || !shellEl) return false;
    // NOT divided by devicePixelRatio: the event's position is already in
    // logical points (see the contract on `pointInRect` in geometry.ts).
    return pointInRect(px, py, shellEl.getBoundingClientRect());
  }

  /**
   * Measures dropped paths in Rust (`attachment_describe`) before attaching
   * them — Tauri's drag-drop event carries paths only, with no size, so
   * without this round trip a dropped file would sail past `addFiles`'s
   * `MAX_BYTES`/`MAX_TOTAL` the way a picked one never can. A failure (an
   * expired allow-list entry, most likely) is shown the same way a failed
   * pick is, rather than falling back to an unmeasured attachment.
   */
  async function describeDroppedPaths(paths: string[]) {
    const r = await invokeCmd<PickedFile[]>('attachment_describe', { paths });
    if (r.ok) await attach(r.value ?? []);
    else attachErrors = [r.error.message];
  }

  function onDroppedPaths(paths: string[]) {
    if (paths.length === 0) return;
    // These paths are already on the Rust allow-list — `lib.rs` recorded them
    // from this very event before the webview heard about it.
    const real = paths.filter((p) => p !== '');
    if (real.length === 0) return;
    void describeDroppedPaths(real);
  }

  /**
   * Tauri's drag-drop event: the only place the webview learns a dropped
   * file's path, and the same event the backend authorises those paths from.
   * Hit-tested against the shell, the way TerminalView hit-tests its grid, so
   * a drop anywhere else in the window does nothing at all.
   */
  $effect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === 'enter' || p.type === 'over') {
          dragOverShell = pointInShell(p.position.x, p.position.y);
        } else if (p.type === 'leave') {
          dragOverShell = false;
        } else if (p.type === 'drop') {
          const over = pointInShell(p.position.x, p.position.y);
          dragOverShell = false;
          dragDepth = 0;
          if (over) onDroppedPaths(p.paths ?? []);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      // Subscribing can reject (no Tauri host, a webview torn down
      // mid-call). Dropping the drag-drop feed costs the drop target, not
      // the panel — so it must not surface as an unhandled rejection.
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  function onComposerPaste(e: ClipboardEvent) {
    const dt = e.clipboardData;
    if (!dt || dt.files.length === 0) return;
    // A rich-text paste carries both; the text half wins.
    if (dt.getData('text/plain').trim().length > 0) return;
    e.preventDefault();
    void attach(
      Array.from(dt.files).map((f) => ({
        // Deliberately empty: clipboard bytes have no filesystem path, and
        // `addFiles` keys the "not supported yet" tile off exactly that.
        path: '',
        name: f.name === 'image.png' ? pastedName(new Date()) : f.name,
        size: f.size,
        kind: f.type.startsWith('image/') ? ('image' as const) : ('binary' as const),
      })),
    );
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

  /** Grow the box with its content, chat-composer style, so a long prompt
   *  stays visible while it is typed. The CSS min/max-height are the floor
   *  and the ceiling; past the ceiling the box scrolls. */
  function autoGrow(node: HTMLTextAreaElement, _value: string) {
    const fit = () => {
      node.style.height = 'auto';
      const h = node.scrollHeight;
      // 0 means nothing measurable (jsdom, a detached or hidden node): an
      // explicit 0px would collapse the composer, so leave the CSS alone.
      if (h > 0) node.style.height = `${h}px`;
      else node.style.removeProperty('height');
    };
    fit();
    return { update: fit };
  }

  function togglePrompt(key: string) {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    expanded = next;
  }
</script>

<div class="conversation-panel" data-testid="conversation-panel" bind:this={root}>
  <ConversationHeader
    {session}
    {conversations}
    {viewing}
    {lastEvent}
    {newerAvailable}
    onSelect={select}
    bind:this={header}
    {findOpen}
    findDisabled={!threadShown}
    {findQuery}
    {findCount}
    matchCount={matches.length}
    {turnEntries}
    {turnsOpen}
    {nowMs}
    onFindOpen={() => void openFind()}
    onFindClose={closeFind}
    onFindInput={(q) => {
      findQuery = q;
      findIndex = 0;
    }}
    {onFindKey}
    onFindStep={stepFind}
    onTurnsToggle={() => (turnsOpen = !turnsOpen)}
    onPickTurn={pickTurn}
    {bgGroups}
    {backgroundOpen}
    onBackgroundToggle={() => (backgroundOpen = !backgroundOpen)}
    onPickBackground={openBackground}
  />
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
  {#if bgEntry}
    <BackgroundDetail entry={bgEntry} onBack={() => (background = null)} onOpenSession={goToSession} />
  {:else if empty && !(pending && viewing === null)}
    <div class="empty-state" data-testid="conv-empty-state">
      <p class="empty-title" data-testid="conv-empty">{empty}</p>
      {#if emptyHint}<p class="empty-hint">{emptyHint}</p>{/if}
    </div>
  {:else if loading}
    <p class="muted conv-loading" data-testid="conv-loading" role="status">
      <SpiralLoader size={16} />Loading conversation…
    </p>
  {:else}
    <!-- A scrollable region has to be in the tab order, or the transcript
         can only be scrolled with a pointer; being focusable is also what
         lets Cmd/Ctrl+F find from inside the thread. The rule below does
         not know about scroll containers, which are the documented
         exception: a region that scrolls must be focusable. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="scroller"
      data-testid="conv-scroller"
      role="region"
      aria-label="Conversation transcript"
      tabindex="0"
      bind:this={scroller}
      onscroll={onScroll}
    >
      <div class="thread">
        {#if errorMsg && !empty}
          <div class="error-row">
            <span class="err" data-testid="conv-error">{errorMsg}</span>
            <button type="button" class="btn btn--quiet is-bounded" data-testid="conv-retry" onclick={() => void load()}>Retry</button>
          </div>
        {/if}
        {#if conv?.truncated}
          <p class="muted truncated">
            Older turns not shown{#if (turnsWanted ?? CONV_TURNS_STEP) < CONV_MAX_TURNS}
              ·
              <button type="button" class="linkish" data-testid="conv-load-older" disabled={loadingOlder} onclick={() => void loadOlder()}
                >{#if loadingOlder}<SpiralLoader size={12} class="inline-spiral" />Loading…{:else}Load older{/if}</button
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
                  <div
                    class="prompt-text"
                    class:clamped={long && !expanded.has(turnKey(turn, i))}
                    style:--clamp-lines={PROMPT_CLAMP_LINES}
                  >{turn.prompt}</div>
                  {#if long}
                    <button type="button" class="linkish" data-testid="conv-prompt-toggle" onclick={() => togglePrompt(turnKey(turn, i))}
                      >{expanded.has(turnKey(turn, i)) ? 'Show less' : 'Show more'}</button
                    >
                  {/if}
                </div>
              {/if}
              {#if turn.reminders?.length}
                <details class="reminders" data-testid="conv-reminders">
                  <summary
                    >{turn.reminders.length === 1
                      ? 'system reminder'
                      : `${turn.reminders.length} system reminders`}</summary
                  >
                  {#each turn.reminders as r, k (k)}
                    <pre class="reminder-body" data-testid="conv-reminder-body">{r}</pre>
                  {/each}
                </details>
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
                        <pre
                          class="command-out"
                          class:clamped={longOut && !expanded.has(cmdKey)}
                          style:--clamp-lines={CMD_CLAMP_LINES}>{g.output}</pre>
                        {#if longOut}
                          <button type="button" class="linkish" data-testid="conv-command-toggle" onclick={() => togglePrompt(cmdKey)}
                            >{expanded.has(cmdKey) ? 'Show less' : 'Show more'}</button
                          >
                        {/if}
                      {/if}
                    </div>
                  {:else if g.kind === 'bash'}
                    {@const bashKey = `bash:${turnKey(turn, i)}:${j}`}
                    {@const out = [g.stdout, g.stderr].filter((o) => o !== null).join('\n')}
                    {@const longOut = out !== '' && isLongOutput(out)}
                    <div class="command" data-testid="conv-bash">
                      <code><span class="bang">!</span>{g.command}</code>
                      {#if out !== ''}
                        <pre
                          class="command-out"
                          class:err={g.stdout === null && g.stderr !== null}
                          class:clamped={longOut && !expanded.has(bashKey)}
                          style:--clamp-lines={CMD_CLAMP_LINES}>{out}</pre>
                        {#if longOut}
                          <button type="button" class="linkish" data-testid="conv-bash-toggle" onclick={() => togglePrompt(bashKey)}
                            >{expanded.has(bashKey) ? 'Show less' : 'Show more'}</button
                          >
                        {/if}
                      {/if}
                    </div>
                  {:else if g.kind === 'harness'}
                    <details class="harness" data-testid="conv-harness">
                      <summary>{g.tag}</summary>
                      <pre class="harness-body">{g.body}</pre>
                    </details>
                  {:else if g.kind === 'interrupt'}
                    <div class="interrupt" data-testid="conv-interrupt">Interrupted{g.during_tool ? ' during a tool call' : ''}</div>
                  {:else if g.kind === 'notification'}
                    {@const target = entryForNotification(g)}
                    {#snippet noteBody(n: Extract<ConvGroup, { kind: 'notification' }>)}
                      <span class="note-mark" aria-hidden="true">{notificationMark(n.status)}</span>
                      <span class="note-label">{notificationLabel(n)}</span>
                      {#if n.at}<time class="note-time" datetime={n.at}>{relativeTime(n.at, nowMs)}</time>{/if}
                    {/snippet}
                    {#if target}
                      <button
                        type="button"
                        class="notification clickable"
                        data-testid="conv-notification"
                        data-tone={notificationTone(g.status)}
                        onclick={() => openBackground(target)}
                      >
                        {@render noteBody(g)}
                      </button>
                    {:else}
                      <div class="notification" data-testid="conv-notification" data-tone={notificationTone(g.status)}>
                        {@render noteBody(g)}
                      </div>
                    {/if}
                  {:else if g.kind === 'subagent'}
                    {@const bg = entryForSubagent(g.id)}
                    <SubagentBlock item={g} {nowMs} live={turnLive} onOpen={bg ? () => openBackground(bg) : undefined} />
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
        {#if answerView}
          <AnswerPrompt {session} view={answerView} {onOpenTerminal} />
        {:else if indicator?.kind === 'blocked'}
          <div class="blocked" data-testid="conv-blocked" role="status">
            <div class="blocked-text">
              <strong>Claude is waiting for you in the terminal{indicator.waiting === 'permission' ? ' (permission)' : indicator.waiting === 'input' ? ' (input)' : ''}.</strong>
              {#if indicator.detail}<div class="blocked-detail">{indicator.detail}</div>{/if}
            </div>
            {#if onOpenTerminal}
              <button type="button" class="blocked-btn" data-testid="conv-open-terminal" onclick={onOpenTerminal}>Open terminal</button>
            {/if}
          </div>
        {:else}
          <!-- Always mounted, faded out when idle: the row keeps its height so
               the thread does not jump each time a turn starts or ends. -->
          <div
            class="indicator"
            class:is-idle={!indicator}
            data-testid={indicator ? 'conv-indicator' : undefined}
            data-kind={indicator?.kind ?? 'idle'}
            role={indicator ? 'status' : undefined}
            aria-hidden={indicator ? undefined : 'true'}
          >
            <SpiralLoader size={16} paused={!indicator} class="indicator-spiral" />
            <span class="indicator-label"
              >{!indicator ? '\u00a0' : indicator.kind === 'sent' ? 'Sent, waiting for Claude…' : indicatorLabel}</span
            >
          </div>
        {/if}
        {#if probeUnsupported}
          <!-- Said once, quietly, and then left alone: the loop has stopped,
               so this is not a state that can repeat. A toast would fire
               every poll (or need its own dedup) for a fact that is a
               property of the pairing, and the missing live detail belongs
               where the live detail would have been. -->
          <p class="probe-off" data-testid="conv-probe-unsupported" role="status">
            Live pane detail is off: this hub is older than this app and does not answer
            <code>session_activity</code>. The status above still follows the session row. Update the hub to get it back.
          </p>
        {/if}
        {/if}
      </div>
    </div>
    {#if turnEntries.length > 1 || !atBottom}
      <div class="scroll-actions">
        {#if turnEntries.length > 1}
          <div class="turn-nav" role="group" aria-label="Step turns">
            <button
              type="button"
              class="turn-step"
              data-testid="conv-turn-prev"
              aria-label="Previous turn"
              title="Previous turn ([)"
              disabled={prevTurn === null}
              onclick={() => prevTurn && stepTurn(-1)}
              >‹ Prev turn</button
            >
            <button
              type="button"
              class="turn-step"
              data-testid="conv-turn-next"
              aria-label="Next turn"
              title="Next turn (])"
              disabled={nextTurn === null}
              onclick={() => nextTurn && stepTurn(1)}
              >Next turn ›</button
            >
          </div>
        {/if}
        {#if !atBottom}
          <button type="button" class="latest" class:fresh={unseen > 0} data-testid="conv-latest" aria-live="polite" onclick={scrollToBottom}
            >↓ {unseen > 0 ? `${unseen} new` : 'Latest'}</button
          >
        {/if}
      </div>
    {/if}
  {/if}
  </div>
  {#if composerAbove && showComposer && canPrompt && bgEntry === null}
    <div class="composer-above">{@render composerAbove()}</div>
  {/if}
  {#if showComposer && canPrompt && bgEntry === null}
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
        <ul class="slash-menu" role="listbox" id={SLASH_LIST_ID} aria-label="Claude Code commands" data-testid="conv-slash-menu">
          {#each slashMatches as c, i (c.name)}
            <li role="presentation" class:active={i === slashIndex} data-testid="conv-slash-item">
              <!-- The button IS the option: role="option" must not wrap an
                   interactive element, and the box points at this id through
                   aria-activedescendant. Keyboard handling lives on the
                   textarea (arrows / Tab / Enter); the button only takes the
                   mouse, and mousedown is swallowed so the box keeps focus. -->
              <button
                type="button"
                role="option"
                id={slashOptionId(i)}
                aria-selected={i === slashIndex}
                tabindex="-1"
                onmousedown={(e) => e.preventDefault()}
                onclick={() => acceptSlash(c)}>
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
      {#if liveStuck === 'press_enter'}
        <div class="stuck-row">
          <button
            type="button"
            class="btn btn--chip btn--warn"
            data-testid="conv-chip-enter"
            title="The session is waiting on a key press. Sends a bare Enter."
            disabled={sending || viewing !== null}
            onclick={() => void sendText('')}>⏎ Press Enter</button>
        </div>
      {/if}
      <div class="chips" data-testid="conv-chips" data-expanded={chipsExpanded} bind:this={chipsRow}>
        {#each $composerPresets as p, i (i)}
          {#if p.label.trim() && p.text.trim()}
            {@const suggested = suggestCompact && isCompactPreset(p)}
            <button
              type="button"
              class="btn btn--chip"
              class:btn--warn={suggested}
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
      {#if chipsOverflow}
        <button
          type="button"
          class="btn btn--chip chips-more"
          data-testid="conv-chips-more"
          aria-expanded={chipsExpanded}
          onclick={() => preserveThread(() => (chipsExpanded = !chipsExpanded))}>{chipsExpanded ? 'Less' : 'More'} ▾</button>
      {/if}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="composer-shell"
        bind:this={shellEl}
        class:is-dragging={dragging}
        ondragenter={onShellDragEnter}
        ondragover={onShellDragOver}
        ondragleave={onShellDragLeave}
        ondrop={onShellDrop}
      >
        {#if attachments.length}
          <ul class="attach-strip" data-testid="conv-attachments" aria-label="Attachments">
            {#each attachments as a (a.id)}
              <!-- The tile is deliberately focusable: it is the 44px control,
                   and Backspace/Delete on it is the keyboard path the 18px ×
                   is too small to be on its own. A role would be a lie (the
                   tile is a list item that CONTAINS a button, not a button),
                   so the two rules are suppressed rather than papered over
                   with role="button". The accessible name says what the key
                   does. -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
              <li class="attach" data-testid="conv-attachment" data-state={a.state}
                tabindex="0" onkeydown={(e) => onTileKey(e, a.id)}
                aria-label="{a.name}, press Backspace to remove"
                title="{a.name}{a.size > 0 ? ` · ${fmtBytes(a.size)}` : ''}{a.error ? ` · ${a.error}` : ''}">
                {#if a.thumb}
                  <!-- A preview can be an SVG data URL (classify maps .svg to
                       an image). Inside <img> it cannot run script; inlined as
                       markup it would, in this app's origin. Never an html tag. -->
                  <img class="attach-img" src={a.thumb} alt="" />
                {:else}
                  <span class="attach-ext">{a.name.split('.').pop() ?? 'file'}</span>
                {/if}
                <button type="button" class="btn btn--icon btn--quiet attach-x" data-testid="conv-attachment-remove"
                  aria-label="Remove {a.name}" title="Remove {a.name}" onclick={() => removeAttachment(a.id)}>×</button>
              </li>
            {/each}
          </ul>
        {/if}
        {#each attachNotes as msg (msg)}
          <p class="attach-error" role="status" data-testid="conv-attach-error">{msg}</p>
        {/each}
        <textarea
          class="composer-input"
          data-testid="conv-composer-input"
          aria-label="Prompt"
          aria-controls={slashOpen ? SLASH_LIST_ID : undefined}
          aria-activedescendant={slashOpen ? slashOptionId(Math.min(slashIndex, slashMatches.length - 1)) : undefined}
          aria-describedby={COMPOSER_HINT_ID}
          bind:this={box}
          bind:value={draft}
          oninput={onComposerInput}
          onkeydown={onComposerKey}
          rows="2"
          use:autoGrow={draft}
          onpaste={onComposerPaste}
          placeholder="Send a prompt…"
          disabled={sending || viewing !== null}
        ></textarea>
        <div class="composer-actions">
          <button
            type="button"
            class="btn btn--icon btn--quiet"
            data-testid="conv-attach-button"
            aria-label="Attach files"
            title="Attach files"
            onclick={pickFiles}>⌾</button>
          <span class="composer-hint" id={COMPOSER_HINT_ID}>↵ send · ⇧↵ newline · ↑ history</span>
          <button
            type="submit"
            class="btn btn--icon btn--primary composer-send"
            data-testid="conv-composer-send"
            aria-label="Send prompt"
            title="Send (Enter)"
            aria-keyshortcuts="Enter"
            disabled={!canSend}>{sending ? '…' : '↑'}</button>
        </div>
        {#if dragging}
          <div class="drop-veil" aria-hidden="true">Drop to attach</div>
        {/if}
      </div>
      <!-- Always mounted (one line reserved) so the composer does not grow
           and shrink as the session flips between working and idle. -->
      <div
        class="composer-status"
        class:is-idle={!statusNote}
        data-testid={statusNote ? 'conv-composer-status' : undefined}
        aria-hidden={statusNote ? undefined : 'true'}
      >{statusNote ?? '\u00a0'}</div>
    </form>
  {:else if showComposer && !canPrompt}
    <p class="muted readonly" data-testid="conv-readonly">Read-only: this agent runs outside tmux, so there is no terminal to prompt.</p>
  {/if}
</div>

<style>
  .composer-above {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    padding: 0 var(--chat-inset);
  }
  .conversation-panel {
    /* The reading column every part of the thread lines up with: the turns,
       the sticky header, the chips, the slash menu and the composer. */
    --chat-col: 80ch;
    /* The one horizontal inset: the header, the turns and the composer all
       start on this edge. Before this existed, the textarea sat 15px left
       of the bubbles it produced. */
    --chat-inset: max(1.1rem, calc((100% - var(--chat-col)) / 2 + 1.1rem));
    /* The tab is a resizable pane, not the window: what adapts below has to
       ask this element's width, so the whole chat is one query container. */
    container-type: inline-size;
    container-name: chat;
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
    /* The end of the transcript is the end of the scroll: do not hand the
       rest of the gesture to whatever is behind the pane. */
    overscroll-behavior: contain;
  }
  .scroller:focus {
    outline: none;
  }
  [data-match] {
    outline: 1px dashed color-mix(in srgb, var(--accent) 55%, transparent);
    outline-offset: 3px;
    border-radius: 4px;
  }
  [data-current-match] {
    outline: 2px solid var(--accent);
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
    padding: 0.55rem var(--chat-inset) 0.6rem;
  }
  .stuck-row {
    display: flex;
    margin: 0 0 6px;
  }
  .stuck-row .btn {
    width: 100%;
    justify-content: flex-start;
  }
  .chips {
    display: flex;
    gap: var(--control-gap);
    margin: 0 0 6px;
    /* One row by default; growth is a deliberate toggle, not a reflow. */
    flex-wrap: nowrap;
    overflow: hidden;
  }
  .chips[data-expanded='true'] {
    flex-wrap: wrap;
    overflow: visible;
  }
  .composer-shell {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 6px 4px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--control-bg);
  }
  .composer-shell:focus-within {
    border-color: var(--accent);
  }
  .attach-strip {
    display: flex;
    flex-wrap: wrap;
    gap: var(--control-gap);
    margin: 0 0 2px;
    padding: 0 2px;
    list-style: none;
    /* Exactly two rows, then scroll: growth is quantised to 50px so
       preserveThread corrects by a clean integer. */
    max-height: 94px;
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  .attach {
    position: relative;
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    overflow: hidden;
  }
  .attach:focus-visible {
    outline: var(--ring-w) solid var(--accent);
    outline-offset: 1px;
  }
  .attach-img { width: 100%; height: 100%; object-fit: cover; display: block; }
  .attach-ext {
    font-family: var(--mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--control-fg-quiet);
  }
  /* 18px, below the 24px target floor, and bounded to this one case. What
     makes that survivable is the tile itself: 44px, focusable, and removing
     on Backspace/Delete (`onTileKey`), so the × is a pointer shortcut rather
     than the only way out. On a coarse pointer it is also always visible
     instead of hover-revealed. */
  .attach-x {
    position: absolute;
    top: 2px;
    right: 2px;
    width: 18px;
    height: 18px;
    padding: 0;
    background: color-mix(in srgb, var(--bg) 78%, transparent);
    color: var(--fg);
    font-size: 13px;
    opacity: 0;
  }
  .attach:hover .attach-x,
  .attach:focus-within .attach-x,
  .attach-x:focus-visible { opacity: 1; }
  @media (hover: none) { .attach-x { opacity: 1; } }
  .attach[data-state='reading'] { opacity: 0.6; }
  .attach[data-state='error'] {
    border-color: var(--usage-crit);
    box-shadow: inset 0 0 0 1px var(--usage-crit);
  }
  .attach-error {
    margin: 0 0 2px;
    padding: 0 2px;
    color: var(--usage-crit);
    font-size: var(--control-font-sm);
  }
  .composer-shell.is-dragging {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .drop-veil {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--bg) 82%, transparent);
    color: var(--accent);
    font-size: var(--control-font);
    font-weight: 600;
    /* Must not eat the drop event. */
    pointer-events: none;
  }
  .composer-input {
    min-height: 40px;
    max-height: 168px;
    /* The box sizes itself to the draft (see autoGrow); a manual drag would
       only be overwritten on the next keystroke. */
    resize: none;
    padding: 2px 4px;
    border: 0;
    background: none;
    color: var(--fg);
    font: inherit;
    font-size: 13px;
    line-height: 1.45;
  }
  .composer-input:focus {
    outline: none;
  }
  .composer-actions {
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    min-height: var(--control-h-lg);
  }
  /* Send sits on the right edge by itself. The hint's flex-grow used to be
     what pushed it there, so where the hint hides (a narrow chat: the agent
     sheet, a phone) Send slid left beside the attach button. */
  .composer-send {
    margin-left: auto;
  }
  .composer-hint {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--control-fg-quiet);
    font-size: var(--control-font-sm);
  }
  @container chat (max-width: 26rem) {
    .composer-hint { display: none; }
  }
  .slash-menu {
    list-style: none;
    max-height: 14rem;
    overflow: auto;
    overscroll-behavior: contain;
    margin: 0 0 0.4rem;
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
  .composer-error {
    margin: 0 0 0.35rem;
    color: var(--usage-crit);
    font-size: 0.75rem;
  }
  .composer-status {
    margin: 0.35rem 0 0;
    color: var(--fg-muted);
    font-size: 0.75rem;
    transition: opacity 0.2s ease;
  }
  .composer-status.is-idle {
    opacity: 0;
  }
  .indicator {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0 0.6rem;
    color: var(--fg-muted);
    font-size: 0.8rem;
    transition: opacity 0.2s ease;
  }
  .indicator.is-idle {
    opacity: 0;
    pointer-events: none;
  }
  .indicator[data-kind='sent'] {
    font-style: italic;
  }
  .probe-off {
    margin: 0;
    padding: 0.35rem 0 0.6rem;
    color: var(--fg-muted);
    font-size: 0.75rem;
  }
  .probe-off code {
    font-size: inherit;
  }
  .indicator :global(.indicator-spiral) {
    color: var(--accent);
  }
  .conv-loading {
    /* Fills the thread area and sits in its middle, where the transcript
       (and the empty state) will be — not pinned to the top-left corner. */
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    margin: 0;
    padding: 1.5rem 1.1rem;
    color: var(--fg-muted);
  }
  .linkish :global(.inline-spiral) {
    margin-right: 0.3em;
    vertical-align: -1px;
  }
  @media (prefers-reduced-motion: reduce) {
    .tools summary::before {
      transition: none;
    }
  }
  .blocked {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin: 0.35rem 0 0.6rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--usage-warn);
    border-left-width: 3px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--usage-warn) 10%, var(--bg-pane));
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
    border: 1px solid var(--usage-warn);
    border-radius: 6px;
    background: transparent;
    color: var(--fg);
    font-size: 0.78rem;
    cursor: pointer;
  }
  .blocked-btn:hover {
    background: color-mix(in srgb, var(--usage-warn) 20%, var(--bg-pane));
  }
  .thread {
    /* No max-width/margin centering here: a centered chat-col box plus a
       --chat-inset padding would double-count the outer margin on wide
       panes. A full-width box with only the inset padding gives the same
       effective column (chat-col minus the gutters) and, critically, the
       same left edge as the header and the composer, which use the same
       recipe. */
    padding: 1rem var(--chat-inset) 2.5rem;
  }
  .muted { color: var(--fg-muted); font-style: italic; font-size: 0.8rem; margin: 0.6rem; }
  .empty-state {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.3rem;
    padding: 1.5rem 1.1rem;
    text-align: center;
  }
  .empty-title {
    margin: 0;
    color: var(--fg);
    font-size: 0.9rem;
  }
  .empty-hint {
    margin: 0;
    max-width: 44ch;
    color: var(--fg-muted);
    font-size: 0.8rem;
    line-height: 1.5;
  }
  .truncated { text-align: center; margin: 0 0 1rem; }
  .error-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }
  .err { color: var(--usage-crit); font-size: 0.8rem; }
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
    /* --clamp-lines comes from PROMPT_CLAMP_LINES, the same constant
       isLongPrompt decides on: a copy here drifts into a "Show more" over
       text nothing clipped. */
    -webkit-line-clamp: var(--clamp-lines);
    line-clamp: var(--clamp-lines);
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
    color: var(--usage-crit);
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
    color: var(--usage-warn);
  }
  .event[data-tone='error'] {
    color: var(--usage-crit);
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
    /* --clamp-lines comes from CMD_CLAMP_LINES; 1.45em is this block's own
       line-height and 0.7rem its vertical padding. */
    max-height: calc(var(--clamp-lines) * 1.45em + 0.7rem);
    overflow: hidden;
  }
  .command-out.err {
    color: var(--usage-crit);
  }
  .command .bang {
    opacity: 0.6;
    margin-right: 0.15rem;
  }
  /* A system reminder and an unrecognised harness block are both noise the
     harness added, not the human's words: folded to one quiet line, opened
     only when someone wants to see what was in it. */
  .reminders,
  .harness {
    margin: 0.25rem 0 0.4rem;
    font-size: 0.72rem;
  }
  .reminders > summary,
  .harness > summary {
    cursor: pointer;
    display: inline-block;
    padding: 0.05rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--fg-muted);
    background: var(--bg-pane);
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
  }
  .reminder-body,
  .harness-body {
    margin: 0.3rem 0 0;
    padding: 0.35rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-pane);
    color: var(--fg-muted);
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .interrupt {
    margin: 0.3rem 0 0.5rem;
    color: var(--usage-warn);
    font-size: 0.76rem;
    font-style: italic;
  }
  .notification {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    margin: 0.3rem 0;
    padding: 0.2rem 0.5rem;
    border-left: 3px solid var(--border);
    border-radius: 4px;
    font-size: 0.82rem;
    color: var(--fg-muted);
    background: var(--bg-pane);
  }
  .notification[data-tone='warn'] {
    border-left-color: var(--usage-warn);
  }
  .notification[data-tone='error'] {
    border-left-color: var(--usage-crit);
  }
  .notification.clickable {
    width: 100%;
    text-align: left;
    font: inherit;
    cursor: pointer;
  }
  .notification.clickable:hover {
    border-left-color: var(--accent);
  }
  .note-mark {
    flex: 0 0 auto;
  }
  .note-label {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .note-time {
    flex: 0 0 auto;
    margin-left: auto;
    padding-left: 0.4rem;
    font-size: 0.72rem;
  }
  .scroll-actions {
    position: absolute;
    right: 1rem;
    bottom: 1rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .turn-nav {
    display: flex;
    gap: 0.35rem;
  }
  .turn-step,
  .latest {
    padding: 0.3rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-pane);
    color: var(--fg);
    font-size: 0.75rem;
    cursor: pointer;
    box-shadow: 0 2px 8px color-mix(in srgb, var(--fg) 15%, transparent);
  }
  .turn-step:hover,
  .latest:hover {
    border-color: var(--accent);
  }
  .turn-step:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .turn-step:disabled:hover {
    border-color: var(--border);
  }
  .latest.fresh {
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }

  /* A pane narrow enough that the 1.1rem gutters cost more than they give,
     and the blocked notice can no longer hold its text and button on one
     line. */
  @container chat (max-width: 34rem) {
    .thread,
    .composer,
    .readonly,
    .viewing,
    .switch-notice {
      padding-inline: 0.6rem;
    }
    .blocked {
      flex-direction: column;
      align-items: stretch;
      gap: 0.45rem;
    }
    .scroll-actions {
      right: 0.5rem;
      bottom: 0.5rem;
    }
  }
</style>
