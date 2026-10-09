<script lang="ts">
  // What a WATCHER sees where the owner has a terminal (multi-user M1, R5-e).
  //
  // Sharing a session never confers a terminal: `pty_open` spawns this
  // machine's own `ssh … tmux attach`, the hub is not in that path, and so a
  // terminal handed over by a grant could never be taken back by revoking it
  // (spec §4.3 invariant 4). Taking the live view away leaves a hole, though —
  // "B can watch it" is the point of a watch grant — so something has to give
  // it back in a form the hub CAN revoke. That is this pane: a read-only
  // snapshot of the tmux pane, polled through the routed `capture_session`,
  // which the hub authorises on every single call.
  //
  // What this component deliberately does NOT have, each one a channel the
  // hub cannot reach and therefore cannot revoke:
  //
  //   - no `pty_*` call of any kind (not even `pty_close` — it owns no PTY);
  //   - no keyboard handling, no IME proxy, no paste;
  //   - no resize: nothing here can send SIGWINCH to the owner's pane;
  //   - no drop target, so no `upload_to_session` scp onto the owner's host;
  //   - no tmux name on screen and no attach command to copy.
  //
  // The conversation is NOT here: `session_conversation`,
  // `session_conversations`, `session_tool_detail`, `session_activity` and
  // `session_history` are all routed already, so the existing
  // ConversationPanel works for a watcher unchanged and App keeps mounting it
  // over this pane exactly as it does over the terminal.
  import { captureSession, type SessionRow } from './sessions';
  import { noAttachReason, type SessionAccess } from './access';
  import { hubStatus } from './hub';
  import { uiLayout } from './prefs';
  import WatchSummary from './WatchSummary.svelte';
  import { errorSentence } from './error_copy';

  let {
    session,
    access,
    visible = true,
  }: {
    session: SessionRow;
    /** The derived answer from `access.ts`. Never `own`: App mounts the real
     *  terminal for that. `null` means "we could not tell" — the hub is
     *  unreachable or has not said who we are — which is a different thing to
     *  say and a different thing to do about it, so it is not polled. */
    access: SessionAccess;
    /** False while an overlay (Files, Conversation, Hosts, Assets) covers
     *  this pane: a snapshot nobody can see is not worth a routed read. */
    visible?: boolean;
  } = $props();

  /**
   * How often the snapshot is refreshed. Deliberately slower than the
   * terminal's drain floor (`DRAIN_MIN_MS`, 30 ms): each poll is a routed
   * read that ends in a `tmux capture-pane` over the hub's SSH, not a local
   * PTY read, and a watcher is watching rather than typing.
   */
  const POLL_MS = 3_000;
  /** Scrollback asked for above the visible pane, so the snapshot shows how
   *  the current state was arrived at rather than only its last screen. */
  const SCROLLBACK_LINES = 200;
  /**
   * Cap on the lines the reply may carry, passed EXPLICITLY — without it the
   * backend applies its own default of 200 (`CAPTURE_DEFAULT_MAX_LINES`), which
   * trims the scrollback asked for just above straight back off again and
   * leaves roughly one screen. Asking for history and then letting a default
   * throw it away is the sort of thing a comment says is happening while the
   * code does not do it, so the number is here rather than implied: the
   * scrollback plus a generous screen's worth of visible pane.
   */
  const MAX_LINES = SCROLLBACK_LINES + 200;

  let text = $state<string | null>(null);
  let error = $state<string | null>(null);
  /** True only for the FIRST read of a session: a refresh that fails must not
   *  blank a snapshot that is merely a few seconds old. */
  let loading = $state(false);
  let lastAt = $state<number | null>(null);

  const why = $derived(noAttachReason(access, $hubStatus));
  /** A grant is the only state worth polling: with `null` we do not know
   *  whether we may read the pane at all, and asking anyway would be an error
   *  toast loop against a hub that is already saying something is wrong. */
  const pollable = $derived(access === 'watch' || access === 'drive');

  /** `id` is captured by the caller, not re-read after the await: a capture
   *  that lands after the selection moved on belongs to the pane that asked
   *  for it, and painting it under another session's header is the bug the
   *  terminal's own `openGeneration` guard exists for. */
  /** The newest capture asked for. A reply that is not the newest is
   *  dropped: on a host where one capture outlasts the poll interval, replies
   *  can land out of order, and an older pane must not replace a newer one. */
  let generation = 0;
  /** A capture is out. The interval skips its tick rather than stacking
   *  another SSH capture behind a slow one. */
  let inFlight = false;

  async function refresh(id: number, first: boolean) {
    if (!first && inFlight) return;
    const mine = ++generation;
    inFlight = true;
    if (first) loading = true;
    let r: Awaited<ReturnType<typeof captureSession>>;
    try {
      r = await captureSession(id, {
        scrollback_lines: SCROLLBACK_LINES,
        max_lines: MAX_LINES,
      });
    } finally {
      if (mine === generation) inFlight = false;
    }
    if (mine !== generation || id !== session.id) return;
    loading = false;
    if (r.ok) {
      text = r.value;
      error = null;
      lastAt = Date.now();
      return;
    }
    // Shown in place, never toasted: this is a poll, and a host that went
    // away would otherwise raise one every few seconds.
    error = `Couldn't capture the pane: ${errorSentence(r.error)} Trying again on the next poll.`;
  }

  // One effect owns the whole poll, keyed on the three things that change what
  // it should be doing — which session, whether we may read it, and whether
  // anyone can see it. Svelte re-runs it (and runs the teardown) on any of
  // them, so there is no interval to leak and no stale session's text left on
  // screen under a new header. `session.id` is read FIRST, before the early
  // return, so a selection change re-runs it even while the pane is covered.
  $effect(() => {
    const id = session.id;
    if (!pollable || !visible) return;
    // A different session (or a regained grant): start from nothing rather
    // than showing the previous pane while the first read is out.
    text = null;
    error = null;
    lastAt = null;
    void refresh(id, true);
    const t = setInterval(() => void refresh(id, false), POLL_MS);
    return () => clearInterval(t);
  });

  const ageLabel = $derived(
    lastAt === null ? null : `updated ${Math.max(0, Math.round((Date.now() - lastAt) / 1000))}s ago`,
  );
</script>

<div class="watch" data-testid="watch-view">
  <div class="header" data-testid="watch-header">
    <!-- Only for a REAL grant. `access === null` is "we could not tell" — an
         unreachable hub, or one that has not said who this device is — and a
         badge reading WATCH there would tell the person the session is shared
         with them, which is the one thing the plan says these two states must
         never say. The `watch-reason` paragraph below gets it right; the
         header used to contradict it, because the ternary treated `null` as
         the default level rather than as the absence of one. -->
    {#if access === 'watch' || access === 'drive'}
      <span class="badge" data-testid="watch-level">{access}</span>
    {/if}
    <span class="label">Read-only snapshot</span>
    {#if ageLabel}
      <span class="age" data-testid="watch-age">{ageLabel}</span>
    {/if}
    <button
      class="refresh"
      onclick={() => void refresh(session.id, false)}
      disabled={!pollable}
      title="Capture the pane again now"
      data-testid="watch-refresh">↻ refresh</button
    >
  </div>

  {#if why}
    <p class="why" data-testid="watch-reason">{why}</p>
  {/if}

  {#if pollable && $uiLayout === 'new'}
    <!-- Orbit Fleet 11.11: what happened since the watcher last looked. -->
    <WatchSummary {session} />
  {/if}

  {#if !pollable}
    <!-- Nothing to show and nothing to poll: `why` above already says which
         of the two states this is (hub unreachable / identity unknown). -->
    <div class="blank" data-testid="watch-unavailable"></div>
  {:else if loading && text === null}
    <div class="blank" data-testid="watch-loading">Capturing the pane…</div>
  {:else}
    {#if error}
      <p class="err" data-testid="watch-error">{error}</p>
    {/if}
    {#if text !== null}
      <!-- A <pre>, not the ANSI Screen: this is a capture of what the pane
           shows, not a stream to parse, and there is no input to round-trip.
           `aria-readonly` is not needed — a <pre> is not an input. -->
      <pre class="pane" data-testid="watch-pane">{text}</pre>
    {/if}
  {/if}
</div>

<style>
  .watch {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    min-height: 0;
  }
  .header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.5rem;
    border-bottom: 1px solid var(--border);
    font-size: 11px;
    color: var(--fg-muted);
    flex: none;
  }
  .badge {
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 11px;
    padding: 0.1rem 0.35rem;
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  .label {
    font-weight: 600;
  }
  .age {
    margin-left: auto;
  }
  .refresh {
    font-size: 11px;
  }
  .why {
    margin: 0;
    padding: 0.45rem 0.6rem;
    font-size: 11px;
    line-height: 1.45;
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .err {
    margin: 0;
    padding: 0.4rem 0.6rem;
    font-size: 11px;
    color: var(--status-failed);
    flex: none;
  }
  .blank {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    font-size: 0.82rem;
  }
  .pane {
    flex: 1;
    margin: 0;
    padding: 0.5rem 0.6rem;
    overflow: auto;
    white-space: pre;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.8rem;
    line-height: 1.25;
    min-height: 0;
  }
</style>
