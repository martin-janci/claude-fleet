<script lang="ts">
  // An extra fleet's session, opened (multi-hub Phase 3), laid out as the
  // primary's own: the same SessionTabs header, the same ConversationPanel
  // over the same TerminalView. The row carries its fleet, so every call
  // they make goes to that fleet's hub and what they keep is keyed by
  // `sessionKey`. A host that would mean this machine from here (`local`,
  // `wsl-…`) gets a pane snapshot read through that hub instead of an attach.
  import { untrack } from 'svelte';
  import { errorText } from './error_copy';
  import {
    closeFleetSession,
    connectionWord,
    fleetAttachCommand,
    fleetAttachRefusal,
    fleetCaptureSession,
    fleetTouchViewed,
    type FleetInfo,
  } from './fleets';
  import { showFriendlyNames, type SessionRow } from './sessions';
  import { copyText } from './clipboard';
  import { push } from './toasts';
  import ConversationPanel from './ConversationPanel.svelte';
  import TerminalView from './TerminalView.svelte';
  import Button from './kit/Button.svelte';
  import SessionTabs from './SessionTabs.svelte';
  import { detectMac } from './terminal_keys';
  import { readable, writable } from 'svelte/store';
  import { trackViewedSession } from './session_viewed';

  let { fleet, session }: { fleet: FleetInfo; session: SessionRow } = $props();

  type Tab = 'conversation' | 'agent';
  const AWAY_ONLY = 'Not available for another fleet’s session yet';
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  let tab = $state<Tab>('conversation');
  let snapshot = $state<string | null>(null);
  let snapshotError = $state<string | null>(null);

  const cw = $derived(connectionWord(fleet));
  const name = $derived(($showFriendlyNames && session.friendly_name) || session.tmux_name);
  const key = $derived(`${fleet.id}:${session.id}`);
  const attachRefusal = $derived(fleetAttachRefusal(session));
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
    if (t === 'agent' && attachRefusal) void loadSnapshot();
  }

  async function copyAttach() {
    if (attachRefusal) return;
    if (await copyText(fleetAttachCommand(session))) push({ kind: 'success', message: 'Attach command copied' });
  }
</script>

<section class="fleet-session" style="--fleet: {fleet.color}" data-testid="fleet-session-view">
  <SessionTabs
    session={row}
    {name}
    current={tab}
    disabled={{ terminals: AWAY_ONLY, files: AWAY_ONLY, details: AWAY_ONLY }}
    inspectorOpen={false}
    inspectorAvailable={false}
    {isMac}
    onselect={(t) => pick(t === 'conversation' ? 'conversation' : 'agent')}
    oninspector={() => {}}
  >
    {#snippet extra()}
      <span class="badge" data-testid="fleet-session-badge">{fleet.label}</span>
      {#if cw.tone !== 'ok'}<span class="state {cw.tone}" data-testid="fleet-session-link">{cw.word}</span>{/if}
      <Button
        size="sm"
        variant="quiet"
        testid="fleet-session-copy-attach"
        disabled={!!attachRefusal}
        title={attachRefusal ?? undefined}
        onclick={() => void copyAttach()}>Copy attach</Button
      >
      <Button size="sm" variant="quiet" testid="fleet-session-close" onclick={closeFleetSession}>Close</Button>
    {/snippet}
  </SessionTabs>

  <div class="body">
    {#key key}
      {#if attachRefusal}
        <!-- Its host means this machine from here: no attach, so the agent
             tab reads the pane through that hub instead. -->
        <div class="snapshot" data-testid="fleet-session-snapshot">
          <p class="note muted" data-testid="terminal-no-attach">{attachRefusal}</p>
          <Button size="sm" variant="quiet" onclick={() => void loadSnapshot()}>Refresh</Button>
          {#if snapshotError}
            <p class="note err">{snapshotError}</p>
          {:else if snapshot === null}
            <p class="note muted">Reading the pane…</p>
          {:else}
            <pre>{snapshot}</pre>
          {/if}
        </div>
      {:else}
        <!-- Mounted underneath, as the primary's terminal is, so flipping
             back from the conversation never re-attaches. -->
        <div class="term" data-testid="fleet-session-terminal">
          <TerminalView session={row} />
        </div>
      {/if}
    {/key}
    {#if tab === 'conversation'}
      <div class="conv" data-testid="fleet-session-conversation">
        <ConversationPanel session={row} visible={tab === 'conversation'} onOpenTerminal={() => pick('agent')} />
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
  .badge {
    flex-shrink: 0;
    padding: 1px var(--space-2);
    border-radius: var(--radius-xs);
    background: color-mix(in srgb, var(--fleet) 20%, transparent);
    color: var(--fg);
    border: 1px solid var(--fleet);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .muted {
    color: var(--fg-muted);
  }
  .state {
    flex-shrink: 0;
    font-size: var(--text-xs);
  }
  .state.bad {
    color: var(--danger);
  }
  .state.warn {
    color: var(--brand-amber);
  }
  .body {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
    flex-direction: column;
  }
  /* The panels are the primary's own: they lay themselves out edge to edge.
     The conversation covers the terminal, as it does in the main layout. */
  .term {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
    flex-direction: column;
  }
  .conv {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
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
