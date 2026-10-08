<script lang="ts" module>
  export type SessionTab = 'conversation' | 'agent' | 'files' | 'details';
</script>

<script lang="ts">
  // The New layout's session header and its one tab bar (Orbit Fleet
  // redesign step 3.5, Main board): the session's name, state and where it
  // runs, then Conversation · the agent tab · Files · Details. The agent tab
  // is the terminal, named after the agent in it ("Claude Code", "Codex").
  // Terminals 0..N join the bar in 5.3. Assets stays on the right until
  // Toolkit (3.16) takes it; Accounts & hosts is on the rail and ⌘I.
  //
  // App owns what each tab does (the same functions Classic's tabs call);
  // this file only draws them and says which is current.
  import type { SessionRow } from './sessions';
  import { sessionAgent } from './sessions';
  import { AGENT_LABELS, STATE_LABELS } from './row_groups';
  import { attentionState } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { shortcutLabel } from './shortcuts';
  import { accessOf } from './access';
  import { editorBlockedReason, openSessionInEditor } from './editor';

  interface Props {
    session: SessionRow | null;
    /** The name the sidebar row shows for it. */
    name: string;
    /** Which tab is current, or null while a fleet page owns the column. */
    current: SessionTab | null;
    /** Per-tab reason it cannot open now; absent means it can. */
    disabled: Partial<Record<SessionTab, string>>;
    assetsActive: boolean;
    inspectorOpen: boolean;
    /** Whether this layout has room for the inspector now. */
    inspectorAvailable: boolean;
    isMac: boolean;
    onselect: (tab: SessionTab) => void;
    onassets: () => void;
    oninspector: () => void;
  }
  let {
    session,
    name,
    current,
    disabled,
    assetsActive,
    inspectorOpen,
    inspectorAvailable,
    isMac,
    onselect,
    onassets,
    oninspector,
  }: Props = $props();

  const state = $derived(
    session
      ? attentionState(session, { idleSecs: $attentionIdleMinutes * 60, now: Math.floor(Date.now() / 1000) })
      : null,
  );
  const agentLabel = $derived(session ? (AGENT_LABELS[sessionAgent(session)] ?? 'Terminal') : 'Terminal');
  const prNumber = $derived(session?.pr_url?.match(/\/pull\/(\d+)/)?.[1] ?? null);

  const tabs = $derived<{ id: SessionTab; label: string; chord: string | null }[]>([
    { id: 'conversation', label: 'Conversation', chord: null },
    { id: 'agent', label: agentLabel, chord: shortcutLabel('session-view', isMac) },
    { id: 'files', label: 'Files', chord: null },
    { id: 'details', label: 'Details', chord: null },
  ]);
  const inspectorChord = $derived(shortcutLabel('inspector', isMac));
  // Open in VS Code (step 5.5): the worktree on this machine, or over
  // Remote - SSH for a session on another host.
  const editorChord = $derived(shortcutLabel('open-in-editor', isMac));
  const editorBlocked = $derived(editorBlockedReason(session, $accessOf(session)));
</script>

<div class="session-head" data-testid="session-head">
  {#if session && state}
    <div class="title-row">
      <span class="name" data-testid="session-head-name">{name}</span>
      <span class="state state-{state}" data-testid="session-head-state">{STATE_LABELS[state]}</span>
      <span class="grow"></span>
      <button
        type="button"
        class="btn btn--quiet btn--icon editor-open"
        disabled={editorBlocked !== null}
        title={editorBlocked ?? `Open in VS Code (${editorChord})`}
        aria-label="Open in VS Code"
        data-testid="open-in-editor"
        onclick={() => session && void openSessionInEditor(session)}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"
          ><path d="M6 4L2 8l4 4M10 4l4 4-4 4" /></svg
        >
      </button>
      <button
        type="button"
        class="btn btn--quiet btn--icon inspector-toggle"
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
      <span>{session.host_alias}</span>
      {#if session.work?.key}<span class="sep" aria-hidden="true">·</span><span>{session.work.key}</span>{/if}
      {#if session.pr_url}
        <span class="sep" aria-hidden="true">·</span>
        <a href={session.pr_url} target="_blank" rel="noreferrer">{prNumber ? `PR #${prNumber}` : 'PR'}</a>
      {/if}
    </div>
  {/if}
  <div class="bar">
    <div class="tab-strip" role="tablist" aria-label="Session" data-testid="session-tabs">
      {#each tabs as t (t.id)}
        <button
          type="button"
          class="tab-strip__tab"
          role="tab"
          aria-selected={current === t.id}
          disabled={!session || disabled[t.id] !== undefined}
          title={!session ? 'No session selected' : (disabled[t.id] ?? (t.chord ? `${t.label} (${t.chord})` : t.label))}
          data-testid="stab-{t.id}"
          onclick={() => onselect(t.id)}>{t.label}</button
        >
      {/each}
    </div>
    <span class="grow"></span>
    <button
      type="button"
      class="tab-strip__tab fleet"
      aria-pressed={assetsActive}
      title="The asset catalog and its per-host drift state"
      data-testid="stab-assets"
      onclick={onassets}>Assets</button
    >
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
  .tab-strip__tab.fleet[aria-pressed='true'] {
    color: var(--fg);
    font-weight: 500;
    border-bottom-color: var(--accent);
  }
  .editor-open svg,
  .inspector-toggle svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
  }
  .inspector-toggle[aria-pressed='true'] { color: var(--accent); }
</style>
