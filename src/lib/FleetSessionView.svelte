<script lang="ts">
  // An extra fleet's session, opened (multi-hub Phase 2): its conversation
  // read from its own hub, a composer that prompts it there, its terminal
  // attached from this machine, and a read-only pane snapshot.
  //
  // A view of its own rather than ConversationPanel with a `fleet` prop: that
  // panel and the ~80 modules behind it key everything (drafts, viewed-at,
  // the terminal map, rewind, fork, forms) by the primary's session ids, and
  // an extra fleet's id collides with them. Re-keying all of it is the
  // design's Phase 3; this view carries the parts that already route by
  // fleet (`FLEET_SCOPED`) and nothing else.
  import { onDestroy, untrack } from 'svelte';
  import {
    sessionConversation,
    groupItems,
    reuseTurns,
    sameConversation,
    CONVERSATION_POLL_MS,
    CONV_TURNS_STEP,
    CONV_MAX_TURNS,
    blockTypeLabel,
    splitMarker,
    type Conversation,
  } from './conversation';
  import {
    closeFleetSession,
    connectionWord,
    fleetCaptureSession,
    fleetSendKey,
    fleetSendPrompt,
    fleetTouchViewed,
    fleetAddress,
    fleetSessionAt,
    fleets,
    fleetRows,
    openFleetSession,
    type FleetInfo,
  } from './fleets';
  import { sessionStatusWord } from './attention';
  import type { SessionRow } from './sessions';
  import { push } from './toasts';
  import Markdown from './MarkdownView.svelte';
  import ToolLine from './ToolLine.svelte';
  import FleetTerminal from './FleetTerminal.svelte';
  import Button from './kit/Button.svelte';
  import FleetAttachButton from './FleetAttachButton.svelte';
  import { tablistKeys } from './tablist_keys';

  let { fleet, session }: { fleet: FleetInfo; session: SessionRow } = $props();

  type Tab = 'conversation' | 'terminal' | 'snapshot';
  let tab = $state<Tab>('conversation');
  let conv = $state<Conversation | null>(null);
  let convError = $state<string | null>(null);
  let turns = $state(CONV_TURNS_STEP);
  let draft = $state('');
  let sending = $state(false);
  let snapshot = $state<string | null>(null);
  let snapshotError = $state<string | null>(null);
  let nowMs = $state(Date.now());
  let timer: ReturnType<typeof setInterval> | null = null;

  const cw = $derived(connectionWord(fleet));
  const name = $derived(session.friendly_name || session.tmux_name);
  const key = $derived(`${fleet.id}:${session.id}`);

  // Every read goes to another hub over the network, so an answer can land
  // after the person moved to another session, or after a newer read: only
  // the latest read for the session still on screen is applied. A poll tick
  // is skipped while a read is out (ConversationPanel's `inFlight`), so a
  // slow link does not pile reads up on that hub.
  let convSeq = 0;
  let convInFlight = 0;
  let snapshotSeq = 0;
  // Bumped when the composer is reset for another session, so a send that
  // resolves after a switch away and back does not clear the newer draft;
  // and the sessions with a send still in flight, so coming back to one
  // does not allow a second send alongside it.
  let sendSeq = 0;
  const sendsInFlight = new Set<string>();

  async function loadConversation(opts: { poll?: boolean } = {}) {
    if (opts.poll && convInFlight > 0) return;
    const at = key;
    const seq = ++convSeq;
    convInFlight++;
    let r: Awaited<ReturnType<typeof sessionConversation>>;
    try {
      r = await sessionConversation(session.id, turns, undefined, fleet.id);
    } finally {
      convInFlight--;
    }
    if (at !== key || seq !== convSeq) return;
    if (r.ok) {
      convError = null;
      if (!sameConversation(conv, r.value)) conv = reuseTurns(conv, r.value);
    } else {
      convError = r.error.message;
    }
  }

  async function loadSnapshot() {
    const at = key;
    const seq = ++snapshotSeq;
    const r = await fleetCaptureSession(fleet.id, session);
    if (at !== key || seq !== snapshotSeq) return;
    if (r.ok) {
      snapshot = r.value;
      snapshotError = null;
    } else {
      snapshotError = r.error.message;
    }
  }

  // A new session (or fleet) starts over: its conversation, its window, and
  // an empty composer — text typed for one session must never be sent to
  // the next one by an Enter.
  $effect(() => {
    void key;
    untrack(() => {
      conv = null;
      convError = null;
      turns = CONV_TURNS_STEP;
      snapshot = null;
      snapshotError = null;
      snapshotSeq++;
      sendSeq++;
      draft = '';
      sending = sendsInFlight.has(key);
      tab = 'conversation';
      void loadConversation();
      void fleetTouchViewed(fleet.id, session);
    });
  });

  // Re-read when the row says something moved, and on the panel's cadence.
  // Not on its first run for a session: the effect above has just read it.
  let activityKey: string | null = null;
  $effect(() => {
    void session.last_activity_at;
    void session.claude_status;
    const k = key;
    untrack(() => {
      const fresh = activityKey !== k;
      activityKey = k;
      if (!fresh && tab === 'conversation') void loadConversation();
    });
  });

  timer = setInterval(() => {
    nowMs = Date.now();
    if (tab === 'conversation' && typeof document !== 'undefined' && !document.hidden) void loadConversation({ poll: true });
  }, CONVERSATION_POLL_MS);

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  function olderTurns() {
    turns = Math.min(CONV_MAX_TURNS, turns + CONV_TURNS_STEP);
    void loadConversation();
  }

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    const at = key;
    const seq = ++sendSeq;
    const who = `${name} on ${fleet.label}`;
    sending = true;
    sendsInFlight.add(at);
    const r = await fleetSendPrompt(fleet.id, session, text);
    sendsInFlight.delete(at);
    if (!r.ok) push({ kind: 'error', message: `Sending to ${who} failed`, detail: r.error.message });
    if (at === key) sending = false;
    // Moved to another session meanwhile (even if back again since): the
    // composer has been reset and what it holds is no longer this send's.
    if (seq !== sendSeq) return;
    if (r.ok) {
      if (draft.trim() === text) draft = '';
      void loadConversation();
    }
  }

  async function pressKey(k: 'Enter' | 'Escape' | 'C-c') {
    const r = await fleetSendKey(fleet.id, session, k);
    if (!r.ok) push({ kind: 'error', message: `The key did not reach ${name}`, detail: r.error.message });
  }

  function onComposerKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      void send();
    }
  }

  function pick(t: Tab) {
    tab = t;
    if (t === 'snapshot') void loadSnapshot();
    if (t === 'conversation') void loadConversation();
  }
</script>

<section class="fleet-session" style="--fleet: {fleet.color}" data-testid="fleet-session-view">
  <header class="head">
    <span class="badge" data-testid="fleet-session-badge">{fleet.label}</span>
    <h2 class="name" title={session.tmux_name}>{name}</h2>
    <span class="muted">{session.host_alias}</span>
    <span class="muted" data-testid="fleet-session-status">· {sessionStatusWord(session)}</span>
    {#if cw.tone !== 'ok'}<span class="state {cw.tone}">{cw.word}</span>{/if}
    <span class="grow"></span>
    <div class="tabs" role="tablist" aria-label="View" use:tablistKeys>
      {#each [['conversation', 'Conversation'], ['terminal', 'Terminal'], ['snapshot', 'Snapshot']] as [id, label] (id)}
        <button
          type="button"
          role="tab"
          class:active={tab === id}
          aria-selected={tab === id}
          data-testid="fleet-session-tab-{id}"
          onclick={() => pick(id as Tab)}>{label}</button
        >
      {/each}
    </div>
    <FleetAttachButton {session} testid="fleet-session-copy-attach" />
    <Button size="sm" variant="quiet" testid="fleet-session-close" onclick={closeFleetSession}>Close</Button>
  </header>

  <div class="body">
    {#if tab === 'conversation'}
      <div class="conv" data-testid="fleet-session-conversation">
        {#if convError && !conv}
          <p class="note err">{convError}</p>
        {:else if !conv}
          <p class="note muted">Reading the conversation from {fleet.label}…</p>
        {:else}
          {#if conv.truncated && turns < CONV_MAX_TURNS}
            <Button size="sm" variant="quiet" onclick={olderTurns}>Load older turns</Button>
          {/if}
          {#if conv.turns.length === 0}
            <p class="note muted">No turns yet.</p>
          {/if}
          {#each conv.turns as t, ti (ti)}
            {#if t.prompt}
              {@const m = splitMarker(t.prompt)}
              {#if m.from}
                <!-- The hub's untrusted-sender line. A federation address
                     (`<fleet-uuid>/session/host/name`) reads by the fleet
                     this window knows it as, and opens there when shown. -->
                {@const a = fleetAddress(m.from, $fleets)}
                {@const at = fleetSessionAt(m.from, $fleets, $fleetRows)}
                <div class="from" data-testid="fleet-session-from">
                  from
                  {#if at}
                    <button type="button" class="linkish" onclick={() => openFleetSession(at.fleet, at.id)}>{a.text}</button>
                  {:else}
                    {a.text}
                  {/if}
                  · untrusted
                </div>
              {/if}
              <div class="prompt" data-testid="fleet-session-prompt">{m.text}</div>
            {/if}
            {#each groupItems(t.items) as g, gi (gi)}
              {#if g.kind === 'text'}
                <div class="reply"><Markdown source={g.text} /></div>
              {:else if g.kind === 'tools'}
                <div class="tools">
                  {#each g.tools as line, li (li)}
                    <ToolLine
                      {line}
                      sessionId={session.id}
                      claudeSessionId={null}
                      {nowMs}
                      live={ti === conv.turns.length - 1}
                      fleet={fleet.id}
                    />
                  {/each}
                </div>
              {:else if g.kind === 'subagent'}
                <div class="block">
                  <strong>{blockTypeLabel(g)}</strong>
                  {#if g.description}<span class="muted"> {g.description}</span>{/if}
                  {#if g.result}<Markdown source={g.result} />{/if}
                </div>
              {:else if g.kind === 'command'}
                <p class="note muted">/{g.name}{g.args ? ` ${g.args}` : ''}</p>
              {:else if g.kind === 'compact'}
                <p class="note muted">Conversation compacted</p>
              {:else if g.kind === 'interrupt'}
                <p class="note muted">Interrupted</p>
              {:else if g.kind === 'bash'}
                <pre class="bash">$ {g.command}{g.stdout ? `\n${g.stdout}` : ''}</pre>
              {/if}
            {/each}
          {/each}
          {#if convError}<p class="note err">{convError}</p>{/if}
        {/if}
      </div>
      <form
        class="composer"
        onsubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <textarea
          bind:value={draft}
          rows="2"
          placeholder="Prompt {name} on {fleet.label}"
          aria-label="Prompt"
          data-testid="fleet-session-composer"
          onkeydown={onComposerKey}
        ></textarea>
        <div class="keys">
          <Button size="sm" variant="primary" type="submit" busy={sending} busyLabel="Sending…" testid="fleet-session-send" disabled={!draft.trim()}>Send</Button>
          <Button size="sm" variant="quiet" title="Press Enter (accept the highlighted answer)" onclick={() => void pressKey('Enter')}>Enter</Button>
          <Button size="sm" variant="quiet" title="Press Escape (interrupt, or dismiss a dialog)" onclick={() => void pressKey('Escape')}>Esc</Button>
          <Button size="sm" variant="quiet" title="Press Ctrl-C" onclick={() => void pressKey('C-c')}>Ctrl-C</Button>
        </div>
      </form>
    {:else if tab === 'terminal'}
      {#key key}
        <FleetTerminal fleet={fleet.id} {session} />
      {/key}
    {:else}
      <div class="snapshot" data-testid="fleet-session-snapshot">
        <Button size="sm" variant="quiet" onclick={() => void loadSnapshot()}>Refresh</Button>
        {#if snapshotError}
          <p class="note err">{snapshotError}</p>
        {:else if snapshot === null}
          <p class="note muted">Reading the pane…</p>
        {:else}
          <pre>{snapshot}</pre>
        {/if}
      </div>
    {/if}
  </div>
</section>

<style>
  .fleet-session {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    color: var(--fg);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .badge {
    padding: 1px var(--space-2);
    border-radius: var(--radius-xs);
    background: color-mix(in srgb, var(--fleet) 20%, transparent);
    color: var(--fg);
    border: 1px solid var(--fleet);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .name {
    margin: 0;
    font-size: var(--text-md);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40ch;
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-sm);
  }
  .grow {
    flex: 1;
  }
  .state {
    font-size: var(--text-xs);
  }
  .state.bad {
    color: var(--danger);
  }
  .state.warn {
    color: var(--brand-amber);
  }
  .tabs {
    display: flex;
    gap: 2px;
  }
  .tabs button {
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .tabs button.active {
    color: var(--fg);
    border-color: var(--border);
    background: var(--bg-raise);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: var(--space-3);
    gap: var(--space-2);
  }
  .conv {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .prompt {
    align-self: flex-end;
    max-width: 80%;
    white-space: pre-wrap;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--fleet) 14%, var(--bg-raise));
  }
  .from {
    align-self: flex-end;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .linkish {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
  }
  .reply {
    max-width: 92%;
  }
  .tools,
  .block {
    border-left: 2px solid var(--border);
    padding-left: var(--space-2);
  }
  .bash,
  .snapshot pre {
    margin: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    overflow-x: auto;
  }
  .snapshot {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .composer {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .composer textarea {
    width: 100%;
    resize: vertical;
    font: inherit;
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    box-sizing: border-box;
  }
  .keys {
    display: flex;
    gap: var(--space-1);
  }
  .note {
    margin: 0;
    font-size: var(--text-sm);
  }
  .err {
    color: var(--danger);
  }
</style>
