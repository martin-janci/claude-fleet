<script lang="ts" module>
  export type SessionTab = 'conversation' | 'agent' | 'terminals' | 'files' | 'details';
</script>

<script lang="ts">
  import { tablistKeys } from './tablist_keys';
  // The session header and its one tab bar (Orbit Fleet
  // redesign step 3.5, Main board): the session's name, state and where it
  // runs, then Conversation · the agent tab · Files · Details. The agent tab
  // is the terminal, named after the agent in it ("Claude Code", "Codex").
  // Terminals (5.3) is the session's shells, with their count. Assets are not
  // the session's: they live in Toolkit (rail, ⌘K); Accounts & hosts is on the rail and ⌘I.
  //
  // App owns what each tab does;
  // this file only draws them and says which is current.
  import type { SessionRow } from './sessions';
  import { sessionAgent } from './sessions';
  import AgentMark from './AgentMark.svelte';
  import { AGENT_LABELS, STATE_LABELS } from './row_groups';
  import { attentionState } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { shortcutLabel } from './shortcuts';
  import { accessOf } from './access';
  import { editorBlockedReason, openSessionInEditor } from './editor';
  import PresenceStrip from './PresenceStrip.svelte';
  import VisibilityBadge from './VisibilityBadge.svelte';
  import SharedWithYou from './SharedWithYou.svelte';
  import { myGrantInfo } from './access';
  import { orgs as orgList } from './orgs';
  import { isSharedAccess } from './session_scope';
  import { recipientStateLabel, sharedByMeta, sharerName } from './shared_view';
  import { sessionBlocked, shareSheetFor } from './share';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { contextColor, contextLevel } from './attention';
  import { formatTokens } from './sessions';
  import { hostByAlias } from './hosts';
  import { accountByUuid } from './accounts';
  import Meter from './kit/Meter.svelte';

  interface Props {
    session: SessionRow | null;
    /** The name the sidebar row shows for it. */
    name: string;
    /** Which tab is current, or null while a fleet page owns the column. */
    current: SessionTab | null;
    /** How many shell terminals the session has open (5.3). */
    terminalCount?: number;
    /** Per-tab reason it cannot open now; absent means it can. */
    disabled: Partial<Record<SessionTab, string>>;
    inspectorOpen: boolean;
    /** Whether this layout has room for the inspector now. */
    inspectorAvailable: boolean;
    isMac: boolean;
    onselect: (tab: SessionTab) => void;
    oninspector: () => void;
  }
  let {
    session,
    name,
    current,
    terminalCount = 0,
    disabled,
    inspectorOpen,
    inspectorAvailable,
    isMac,
    onselect,
    oninspector,
  }: Props = $props();

  const state = $derived(
    session
      ? attentionState(session, { idleSecs: $attentionIdleMinutes * 60, now: Math.floor(Date.now() / 1000) })
      : null,
  );
  const agent = $derived(session ? sessionAgent(session) : null);
  const agentLabel = $derived(session ? (AGENT_LABELS[sessionAgent(session)] ?? 'Terminal') : 'Terminal');
  const prNumber = $derived(session?.pr_url?.match(/\/pull\/(\d+)/)?.[1] ?? null);
  // The header's meta line and context meter (UX audit 2026-10-09, H2 and
  // H3): host · account · worktree · PR, and how full the context window is.
  const accountEmail = $derived.by(() => {
    if (!session) return null;
    const uuid = session.claude_profile
      ? session.account_uuid
      : (session.account_uuid ?? $hostByAlias.get(session.host_alias)?.account_uuid ?? null);
    return uuid ? ($accountByUuid.get(uuid)?.email ?? null) : null;
  });
  // Gap plan G4.2: someone the session is shared with reads who shared it,
  // at what level and since when, and "Waiting for Martin" for its state.
  const sharedLevel = $derived.by(() => {
    const a = session ? $accessOf(session) : null;
    return isSharedAccess(a) ? a : null;
  });
  const shareInfo = $derived(session ? $myGrantInfo.get(session.id) : undefined);
  const sharer = $derived(session && sharedLevel ? sharerName(session, shareInfo, $orgList) : null);
  const stateLabel = $derived(
    state ? ((sharedLevel ? recipientStateLabel(state, sharer) : null) ?? STATE_LABELS[state]) : '',
  );
  const ctxPct = $derived(session?.context_pct ?? null);
  const ctxLevel = $derived(contextLevel(ctxPct));

  const tabs = $derived<{ id: SessionTab; label: string; chord: string | null; count?: number }[]>([
    { id: 'conversation', label: 'Conversation', chord: null },
    { id: 'agent', label: agentLabel, chord: shortcutLabel('session-view', isMac) },
    { id: 'terminals', label: 'Terminals', chord: shortcutLabel('new-terminal', isMac), count: terminalCount },
    { id: 'files', label: 'Files', chord: null },
    { id: 'details', label: 'Details', chord: null },
  ]);
  const inspectorChord = $derived(shortcutLabel('inspector', isMac));
  // Open in VS Code (step 5.5): the worktree on this machine, or over
  // Remote - SSH for a session on another host.
  const editorChord = $derived(shortcutLabel('open-in-editor', isMac));
  const editorBlocked = $derived(editorBlockedReason(session, $accessOf(session)));
  // Share (step 5.8): the app's one Share sheet, gated exactly as the
  // inspector's Share… is (owner only; the hub link up when paired).
  const shareBlocked = $derived(
    session
      ? (hubActionBlocked('session_share', $hubStatus, $hubConnection) ??
          $sessionBlocked(session, 'session_share'))
      : 'No session selected',
  );
</script>

<div class="session-head" data-testid="session-head">
  {#if session && state}
    <div class="title-row">
      <span class="name" data-testid="session-head-name">{name}</span>
      <span class="state state-{state}" data-testid="session-head-state">{stateLabel}</span>
      <VisibilityBadge {session} />
      <SharedWithYou {session} />
      <span class="grow"></span>
      {#if ctxPct !== null && ctxLevel !== null}
        <span class="ctx" data-testid="session-head-context" data-level={ctxLevel} title="Context window used">
          <span class="ctx-text"
            >Context <span style="color: {contextColor(ctxLevel)};">{Math.round(ctxPct)}%</span
            >{#if session.context_tokens}<span class="ctx-of"
                >{' · '}{formatTokens(session.context_tokens)}{#if session.context_window}{' of '}{formatTokens(session.context_window)}{/if}</span
              >{/if}</span
          >
          <Meter value={ctxPct / 100} level={ctxLevel} label="{Math.round(ctxPct)}% of the context window used" />
        </span>
      {/if}
      <PresenceStrip sessionId={session.id} />
      <button
        type="button"
        class="btn btn--quiet head-btn editor-open"
        disabled={editorBlocked !== null}
        title={editorBlocked ?? `Open in VS Code (${editorChord})`}
        aria-label="Open in VS Code"
        data-testid="open-in-editor"
        onclick={() => session && void openSessionInEditor(session)}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"
          ><path d="M6 4L2 8l4 4M10 4l4 4-4 4" /></svg
        ><span class="lbl">Open in VS Code</span><span class="of-kbd lbl" aria-hidden="true">{editorChord}</span>
      </button>
      <button
        type="button"
        class="btn btn--quiet head-btn share-open"
        disabled={shareBlocked !== null}
        title={shareBlocked ?? 'Share this session — watch or drive, revocable, and never a terminal'}
        aria-label="Share"
        data-testid="share-from-header"
        onclick={() => session && shareSheetFor.set(session.id)}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"
          ><circle cx="6" cy="5.5" r="2.2" /><path d="M2 13c.4-2.3 2-3.6 4-3.6s3.6 1.3 4 3.6" /><path
            d="M11 4.2a2 2 0 010 3.6M12.2 9.6c1 .5 1.6 1.7 1.8 3.4"
          /></svg
        ><span class="lbl">Share…</span>
      </button>
      <button
        type="button"
        class="btn btn--quiet head-btn inspector-toggle"
        aria-pressed={inspectorOpen}
        disabled={!inspectorAvailable}
        title={`Inspector (${inspectorChord})`}
        aria-label="Inspector"
        data-testid="inspector-toggle"
        onclick={oninspector}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"
          ><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M10 3v10" /></svg
        >
      </button>
    </div>
    <div class="meta" data-testid="session-head-meta">
      {#if sharedLevel}<span data-testid="session-head-shared-by">{sharedByMeta(sharedLevel, shareInfo, sharer)}</span
        ><span class="sep" aria-hidden="true">·</span>{/if}
      <span>{session.host_alias}</span>
      {#if accountEmail}<span class="sep" aria-hidden="true">·</span><span data-testid="session-head-account">{accountEmail}</span>{/if}
      {#if session.worktree_key}<span class="sep" aria-hidden="true">·</span><span class="mono" data-testid="session-head-worktree">{session.worktree_key}</span>{/if}
      {#if session.work?.key}<span class="sep" aria-hidden="true">·</span><span>{session.work.key}</span>{/if}
      {#if session.pr_url}
        <span class="sep" aria-hidden="true">·</span>
        <a href={session.pr_url} target="_blank" rel="noreferrer">{prNumber ? `PR #${prNumber}` : 'PR'}</a>
      {/if}
    </div>
  {/if}
  <div class="bar">
    <div class="tab-strip" role="tablist" aria-label="Session" data-testid="session-tabs" use:tablistKeys>
      {#each tabs as t (t.id)}
        <button
          type="button"
          class="tab-strip__tab"
          role="tab"
          aria-selected={current === t.id}
          disabled={!session || disabled[t.id] !== undefined}
          title={!session ? 'No session selected' : (disabled[t.id] ?? (t.chord ? `${t.label} (${t.chord})` : t.label))}
          data-testid="stab-{t.id}"
          onclick={() => onselect(t.id)}
          >{#if t.id === 'agent' && agent}<AgentMark {agent} />{/if}{t.label}{#if t.count}<span
              class="count"
              data-testid="stab-{t.id}-count">{t.count}</span
            >{/if}{#if t.chord}<span
              class="of-kbd"
              aria-hidden="true">{t.chord}</span
            >{/if}</button
        >
      {/each}
    </div>
  </div>
</div>

<style>
  .session-head {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2) var(--space-4) 0;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .state {
    flex-shrink: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .state-action_required,
  .state-blocked { color: var(--status-waiting); font-weight: 600; }
  .state-failed { color: var(--status-failed); font-weight: 600; }
  .state-working { color: var(--status-working); }
  .state-done { color: var(--status-done); }
  .meta {
    display: flex;
    gap: 0.3rem;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
  }
  .sep { opacity: 0.6; }
  .bar {
    display: flex;
    align-items: flex-end;
  }
  .bar .tab-strip { border-bottom: 0; }
  .grow { flex: 1 1 auto; }
  .tab-strip__tab:disabled { opacity: 0.5; cursor: default; }
  .tab-strip__tab[role='tab'] {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .count {
    font-size: var(--text-xs);
    font-weight: 500;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--bg-sunk);
    color: var(--fg-2);
  }
  .session-head { container-type: inline-size; }
  .head-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .ctx {
    display: inline-flex;
    flex-direction: column;
    gap: 3px;
    min-width: 7rem;
    flex-shrink: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .ctx-of { color: var(--fg-muted); white-space: nowrap; }
  .ctx-text { white-space: nowrap; }
  .mono { font-family: var(--font-mono); }
  /* A narrow column keeps the icons and drops the words (the title and
     aria-label still name each one). */
  @container (max-width: 600px) {
    .lbl { display: none; }
    .ctx-of { display: none; }
  }
  .editor-open svg,
  .share-open svg,
  .inspector-toggle svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
  }
  .inspector-toggle[aria-pressed='true'] { color: var(--accent); }
</style>
