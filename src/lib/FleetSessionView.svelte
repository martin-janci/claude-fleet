<script lang="ts">
  // An extra fleet's session, opened (multi-hub): its conversation in the
  // same ConversationPanel the primary's sessions use (Phase 3: the row
  // carries its fleet, so every call the panel makes goes to that fleet's
  // hub and what it keeps is keyed by `sessionKey`), its terminal in the
  // same TerminalView (attached from this machine, under its own pty id), and
  // a read-only pane snapshot.
  import { untrack } from 'svelte';
  import { errorText } from './error_copy';
  import {
    closeFleetSession,
    connectionWord,
    fleetCaptureSession,
    fleetTouchViewed,
    type FleetInfo,
  } from './fleets';
  import { sessionStatusWord } from './attention';
  import type { SessionRow } from './sessions';
  import ConversationPanel from './ConversationPanel.svelte';
  import TerminalView from './TerminalView.svelte';
  import Button from './kit/Button.svelte';
  import FleetAttachButton from './FleetAttachButton.svelte';
  import { tablistKeys } from './tablist_keys';
  import { readable, writable } from 'svelte/store';
  import { trackViewedSession } from './session_viewed';

  let { fleet, session }: { fleet: FleetInfo; session: SessionRow } = $props();

  type Tab = 'conversation' | 'terminal' | 'snapshot';
  let tab = $state<Tab>('conversation');
  let snapshot = $state<string | null>(null);
  let snapshotError = $state<string | null>(null);

  const cw = $derived(connectionWord(fleet));
  const name = $derived(session.friendly_name || session.tmux_name);
  const key = $derived(`${fleet.id}:${session.id}`);
  /** The row as the panel reads it: with its fleet, whoever handed it over. */
  const row = $derived(session.fleet === fleet.id ? session : { ...session, fleet: fleet.id });

  // A pane read goes to another hub over the network, so an answer can land
  // after the person moved to another session, or after a newer read: only
  // the latest read for the session still on screen is applied.
  let snapshotSeq = 0;

  async function loadSnapshot() {
    const at = key;
    const seq = ++snapshotSeq;
    const r = await fleetCaptureSession(fleet.id, session);
    if (at !== key || seq !== snapshotSeq) return;
    if (r.ok) {
      snapshot = r.value;
      snapshotError = null;
    } else {
      snapshotError = errorText(r.error);
    }
  }

  // A new session (or fleet) starts over on its conversation.
  $effect(() => {
    void key;
    untrack(() => {
      snapshot = null;
      snapshotError = null;
      snapshotSeq++;
      tab = 'conversation';
    });
  });

  // Its finished turns read as seen on its own hub, by the primary's rule
  // (`session_viewed.ts`): stamped when it opens and again for each turn that
  // ends while it is on screen, never while the window is hidden (review
  // w01-8). This view is mounted only while no fleet page covers it, so it
  // is on screen while it exists. One tracker per session and fleet.
  const viewedRow = writable<SessionRow | null>(null);
  $effect(() => {
    viewedRow.set(session);
  });
  // By its id: a new `fleet` object for the same fleet must not restart it.
  const fleetId = $derived(fleet.id);
  $effect(() => {
    void key;
    const at = fleetId;
    return untrack(() => trackViewedSession(viewedRow, (id) => fleetTouchViewed(at, { id }), undefined, readable(true)));
  });

  function pick(t: Tab) {
    tab = t;
    if (t === 'snapshot') void loadSnapshot();
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
        <ConversationPanel session={row} visible={tab === 'conversation'} onOpenTerminal={() => pick('terminal')} />
      </div>
    {:else if tab === 'terminal'}
      {#key key}
        <div class="term" data-testid="fleet-session-terminal">
          <TerminalView session={row} />
        </div>
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
    gap: var(--space-2);
  }
  /* The panel is the primary's own: it lays itself out edge to edge. */
  .conv,
  .term {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
    flex-direction: column;
  }
  .snapshot pre {
    margin: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    overflow-x: auto;
  }
  .snapshot {
    padding: var(--space-3);
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .note {
    margin: 0;
    font-size: var(--text-sm);
  }
  .err {
    color: var(--danger);
  }
</style>
