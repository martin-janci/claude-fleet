<script lang="ts">
  import { tick } from 'svelte';
  import {
    recreateSession,
    dismissGhostSession,
    dismissAgentSession,
    isInactiveAgent,
    showFriendlyNames,
    showRowDetails,
    formatCostMicros,
    formatTokens,
    sessionUsageTokens,
    lostReasonLabel,
    type SessionRow,
  } from './sessions';
  import { selectedSession } from './selection';
  import { forgetSessionUi } from './session_ui';
  import { hostByAlias } from './hosts';
  import { hintAnchor } from './hints';
  import {
    claudeStatusColor,
    claudeStatusLabel,
    contextColor,
    contextLevel,
    ciStatusColor,
    ciStatusLabel,
    rank,
    stuckKindLabel,
    STUCK_COLOR,
  } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { pushError } from './toasts';
  import { rowElapsed, rowPrompt, timeAgo } from './session_status';
  import { hubStatus, hubBlock, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import AnswerPrompt from './AnswerPrompt.svelte';
  import { pendingInputFor } from './pending_input';
  import type { WorkKey } from './work_keys';
  import WorkChip from './WorkChip.svelte';
  import {
    confirmSessionWork,
    describeEvidence,
    linkSessionWork,
    rejectSessionWork,
    rejectWorkLink,
    sessionWorkLinks,
    setWorkProjectTrust,
    unlinkSessionWork,
    workWhy,
    type WorkLink,
  } from './work';
  import { fleetSettings, SETTING_KEYS } from './fleet_settings';
  import type { Result } from './result';

  // Rename and selection state stay in the Sidebar (they must survive a
  // sessions store refresh); the row gets them as props and calls back.
  let {
    sess,
    selectMode,
    isChecked,
    isRenaming,
    renameMode = null,
    renameValue = $bindable(),
    renameInput = $bindable(),
    renameError,
    relatedCount,
    nowSec,
    readOnly = false,
    workKey = null,
    workOf = undefined,
    onSelectSession,
    onKeySession,
    toggleSelected,
    beginRename,
    beginLabelEdit,
    onRenameKey,
    commitRename,
    askRecreate,
    askRestart,
    askKill,
  }: {
    sess: SessionRow;
    selectMode: boolean;
    isChecked: boolean;
    isRenaming: boolean;
    /** What the inline editor changes: the display label or the tmux name. */
    renameMode?: 'label' | 'tmux' | null;
    renameValue: string;
    renameInput: HTMLInputElement | undefined;
    renameError: string | null;
    relatedCount: number;
    nowSec: number;
    /** True for a read-only row (the "Outside fleet" group): name + status
     *  chip only, no rename / restart / recreate / kill actions. Selecting
     *  still works. */
    readOnly?: boolean;
    /** The row's work key (work_keys.ts), drawn as a chip after the name.
     *  Null when it has none, or when the row already sits under its work
     *  group's header, which names the key. */
    workKey?: WorkKey | null;
    /** The row's work key whether or not the chip shows it (inside a work
     *  group the header names it): what the work menu's "Not this" / "Clear"
     *  act on. Defaults to `workKey`. */
    workOf?: WorkKey | null;
    onSelectSession: (sess: SessionRow, e?: MouseEvent) => void;
    /** Handles Enter/Space on the ROW. It must ignore events that bubbled
     *  up from a nested control (the action cluster, the select box, the
     *  rename input): activating a `<button>` is the default action of its
     *  own keydown, so calling `preventDefault()` here would cancel it.
     *  Sidebar's implementation guards on `e.target === e.currentTarget`. */
    onKeySession: (e: KeyboardEvent, sess: SessionRow) => void;
    toggleSelected: (sess: SessionRow) => void;
    beginRename: (sess: SessionRow, e?: Event) => unknown;
    beginLabelEdit: (sess: SessionRow, e?: Event) => unknown;
    onRenameKey: (e: KeyboardEvent) => void;
    commitRename: () => unknown;
    askRecreate: (sess: SessionRow, e?: Event) => void;
    askRestart: (sess: SessionRow, e?: Event) => void;
    askKill: (sess: SessionRow, e?: Event) => void;
  } = $props();

  const sessSelected = $derived($selectedSession?.id === sess.id);
  const ctxLevel = $derived(contextLevel(sess.context_pct));
  const elapsed = $derived(rowElapsed(sess, nowSec));
  // The row's triage bucket (P13). Published as data-bucket because component
  // CSS never reaches jsdom, so this is how tests assert a row's triage state.
  const triage = $derived(rank(sess, { idleSecs: $attentionIdleMinutes * 60, now: nowSec }));
  const promptText = $derived(rowPrompt(sess));
  // The dialog this row is blocked on, straight from the row: the sidebar
  // does not probe (that would be one `capture-pane` per visible row, every
  // couple of seconds). Which is why the card re-reads the pane itself
  // before it sends anything — see AnswerPrompt.
  const answerView = $derived(
    pendingInputFor({
      rowStatus: sess.claude_status,
      rowStuck: sess.stuck_kind,
      rowPending: sess.pending_input,
      probe: null,
    }),
  );
  const primaryIsFriendly = $derived($showFriendlyNames && !!sess.friendly_name);
  const primaryName = $derived(primaryIsFriendly ? sess.friendly_name! : sess.tmux_name);
  // Line 2 names what line 1 does not: the tmux name under a friendly name,
  // else the worktree when it is not already part of the tmux name.
  const secondaryName = $derived.by((): string | null => {
    if (primaryIsFriendly) return sess.tmux_name;
    if (sess.worktree_key && !sess.tmux_name.endsWith(`--${sess.worktree_key}`)) return sess.worktree_key;
    return null;
  });

  async function doRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const r = await recreateSession(sess.id);
    if (!r.ok) pushError(r.error, 'Recreate failed');
  }

  async function doDismissGhost(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const r = await dismissGhostSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Dismiss failed');
      return;
    }
    forgetSessionUi(sess.host_alias, sess.tmux_name);
  }

  /** Remove an inactive bg agent from the list. The row itself disappears
   *  via the `session:removed` event the backend emits on success. */
  async function doDismissAgent(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const r = await dismissAgentSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Remove failed');
    }
  }

  function hostIsReachable(alias: string): boolean {
    return $hostByAlias.get(alias)?.reachable ?? false;
  }

  // kill_session (routed) does the same thing to an inactive agent that
  // dismiss_agent_session does, but this row's Kill button is hidden for an
  // inactive one (`!isInactiveAgent(sess)` below) — so a paired client sees
  // the reason instead of a control that fails at the click.
  const dismissAgentBlocked = $derived(hubBlock('dismiss_agent_session', $hubStatus));
  // These route, so they only need the live connection to be up.
  const killBlocked = $derived(hubActionBlocked('kill_session', $hubStatus, $hubConnection));
  const restartBlocked = $derived(hubActionBlocked('restart_session', $hubStatus, $hubConnection));
  const recreateBlocked = $derived(hubActionBlocked('recreate_session', $hubStatus, $hubConnection));
  const labelBlocked = $derived(hubActionBlocked('set_friendly_name', $hubStatus, $hubConnection));
  const tmuxRenameBlocked = $derived(hubActionBlocked('rename_session', $hubStatus, $hubConnection));
  const ghostDismissBlocked = $derived(
    hubActionBlocked('dismiss_ghost_session', $hubStatus, $hubConnection),
  );
  const workBlocked = $derived(hubActionBlocked('link_session_work', $hubStatus, $hubConnection));

  // ── Work menu (roadmap M1b.2): set the row's work, "Not this", "Clear". ──
  const rowWork = $derived(workOf === undefined ? workKey : workOf);
  let workMenuOpen = $state(false);
  let workDraft = $state('');
  let workBusy = $state(false);

  function toggleWorkMenu(e: Event) {
    e.stopPropagation();
    workMenuOpen = !workMenuOpen;
    workDraft = '';
  }

  async function workAction(
    run: () => Promise<Result<unknown>>,
    failure: string,
  ) {
    if (workBusy) return;
    workBusy = true;
    const r = await run();
    workBusy = false;
    if (!r.ok) {
      pushError(r.error, failure);
      return;
    }
    workMenuOpen = false;
    workDraft = '';
  }

  function setWork(e?: Event) {
    e?.stopPropagation();
    const key = workDraft.trim();
    if (!key) return;
    void workAction(() => linkSessionWork(sess.id, { key }), 'Set work failed');
  }

  function rejectWork(e: Event) {
    e.stopPropagation();
    const w = rowWork;
    if (!w) return;
    // A linked item is rejected by id (its key may be null); anything else
    // by the key the row shows.
    const ref = w.source === 'link' && sess.work?.item_id != null
      ? { item_id: sess.work.item_id }
      : { key: w.key };
    void workAction(() => rejectSessionWork(sess.id, ref), 'Not this failed');
  }

  function clearWork(e: Event) {
    e.stopPropagation();
    const linkId = sess.work?.link_id;
    if (linkId == null) return;
    void workAction(() => unlinkSessionWork(sess.id, linkId), 'Clear work failed');
  }

  // ── Detection (work graph M4.4): the suggestion chip, its evidence, and
  // the decisions. A suggestion never regroups the row; only Confirm does.
  const suggestion = $derived(sess.work_suggested ?? null);
  const suggestionKey = $derived<WorkKey | null>(
    suggestion && (suggestion.key || suggestion.title)
      ? {
          key: (suggestion.key || suggestion.title) as string,
          source: 'link',
          from: suggestion.title,
          why: workWhy({ ...suggestion, state: 'suggested' }),
        }
      : null,
  );
  let workLinks = $state<WorkLink[] | null>(null);
  let workInput = $state<HTMLInputElement | null>(null);
  let trustedOverride = $state<boolean | null>(null);
  const projectTrusted = $derived.by(() => {
    if (trustedOverride !== null) return trustedOverride;
    try {
      const ids: unknown = JSON.parse($fleetSettings[SETTING_KEYS.workTrustedBranchProjects] ?? '[]');
      return Array.isArray(ids) && sess.project_id != null && ids.includes(sess.project_id);
    } catch {
      return false;
    }
  });

  async function loadWorkLinks() {
    const r = await sessionWorkLinks(sess.id);
    workLinks = r.ok && Array.isArray(r.value) ? r.value : [];
  }

  function openWorkMenu(e?: Event) {
    e?.stopPropagation();
    workMenuOpen = true;
    workDraft = '';
    void loadWorkLinks();
  }

  /** The links the popover explains: suggestions, then the primary. */
  const explained = $derived(
    (workLinks ?? []).filter((l) => l.state === 'suggested' || (l.is_primary && l.state === 'confirmed')),
  );

  function linkLabel(l: WorkLink): string {
    return l.ref_key ?? (l.item_id != null ? `item ${l.item_id}` : 'work');
  }

  function confirmLink(linkId: number, e?: Event) {
    e?.stopPropagation();
    void workAction(() => confirmSessionWork(sess.id, linkId), 'Confirm failed');
  }

  function rejectLink(linkId: number, e?: Event) {
    e?.stopPropagation();
    void workAction(() => rejectWorkLink(sess.id, linkId), 'Not this failed');
  }

  function pickAnother(e: Event) {
    e.stopPropagation();
    workInput?.focus();
  }

  async function toggleTrust(e: Event) {
    e.stopPropagation();
    const pid = sess.project_id;
    if (pid == null) return;
    const on = (e.currentTarget as HTMLInputElement).checked;
    const r = await setWorkProjectTrust(pid, on);
    if (!r.ok) {
      pushError(r.error, 'Trust failed');
      return;
    }
    trustedOverride = r.value.includes(pid);
  }

  /** `y` / `n` decide the row's top suggestion, `l` links or picks. */
  function onRowKey(e: KeyboardEvent) {
    if (e.target === e.currentTarget && !e.metaKey && !e.ctrlKey && !e.altKey && workBlocked === null) {
      if (e.key === 'y' && suggestion) {
        e.preventDefault();
        confirmLink(suggestion.link_id);
        return;
      }
      if (e.key === 'n' && suggestion) {
        e.preventDefault();
        rejectLink(suggestion.link_id);
        return;
      }
      if (e.key === 'l') {
        e.preventDefault();
        openWorkMenu();
        void tick().then(() => workInput?.focus());
        return;
      }
    }
    onKeySession(e, sess);
  }

  function onWorkKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === 'Enter') setWork(e);
    else if (e.key === 'Escape') workMenuOpen = false;
  }
</script>

<div
  class="sess-row"
  class:selected={sessSelected}
  class:renaming={isRenaming}
  class:checked={isChecked}
  class:stuck={sess.stuck_kind !== null}
  data-testid="sess-row"
  data-session-id={sess.id}
  data-stuck={sess.stuck_kind ?? undefined}
  data-bucket={triage.bucket}
  role="button"
  tabindex="0"
  ondblclick={(e) => sess.status !== 'ghost' && !readOnly && beginLabelEdit(sess, e)}
  onclick={(e) => !isRenaming && (sess.status !== 'ghost' || selectMode) && onSelectSession(sess, e)}
  onkeydown={(e) => !isRenaming && (sess.status !== 'ghost' || selectMode) && onRowKey(e)}
  use:hintAnchor={{ id: 'session-actions', when: !!sess.claude_session_id && sess.status !== 'ghost' }}
>
  {#if selectMode && !readOnly}
    <!-- a11y smell, known: an <input> nested in a role="button" row. The
         row is the click target for open/toggle; the box is a visible
         affordance for the same toggle and stops propagation so the two
         never double-fire. Splitting the row into a real <button> plus a
         sibling checkbox is the proper fix (F5 sidebar split). -->
    <input
      type="checkbox"
      class="select-box"
      checked={isChecked}
      data-testid="select-box"
      aria-label="Select {sess.tmux_name}"
      onclick={(e) => { e.stopPropagation(); toggleSelected(sess); }}
    />
  {/if}
  {#if isRenaming}
    <input
      bind:this={renameInput}
      class="rename-input"
      data-testid={renameMode === 'label' ? 'label-input' : 'rename-input'}
      aria-label={renameMode === 'label'
        ? `Label for ${sess.tmux_name} (empty clears it)`
        : `New tmux session name for ${sess.tmux_name}`}
      placeholder={renameMode === 'label' ? sess.tmux_name : undefined}
      bind:value={renameValue}
      onkeydown={onRenameKey}
      onblur={commitRename}
    />
  {:else}
    {#if readOnly}
      <!-- "Outside fleet": a Claude session running entirely outside tmux.
           Read-only — name and status chip only, no actions. Checked before
           the ghost branch: a ghosted external row must not offer
           Recreate / Dismiss either. -->
      <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
      <span class="sess-name" title={sess.tmux_name}>{primaryName}</span>
      {#if sess.stuck_kind}
        <span
          class="claude-chip stuck-chip"
          data-testid="stuck-chip"
          style="background: {STUCK_COLOR}22; color: {STUCK_COLOR}; border-color: {STUCK_COLOR}66;"
          title="Stuck: {stuckKindLabel(sess.stuck_kind)}"
        >⚠ stuck: {stuckKindLabel(sess.stuck_kind)}</span>
      {:else if sess.claude_status}
        <span
          class="claude-chip"
          data-testid="claude-chip"
          style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
          title="Claude: {sess.claude_status}"
        >{claudeStatusLabel(sess.claude_status)}</span>
      {/if}
    {:else if sess.status === 'ghost'}
      <span class="status-dot status-ghost" title="ghost — session lost" aria-hidden="true"></span>
      <span class="host-badge" data-testid="host-badge" aria-label="host {sess.host_alias}">{sess.host_alias}</span>
      <span class="sess-name" title={sess.tmux_name}>{
        $showFriendlyNames && sess.friendly_name ? sess.friendly_name : sess.tmux_name
      }</span>
      {#if sess.lost_at}
        <span class="lost-at" title="Lost at {new Date(sess.lost_at * 1000).toLocaleString()}">
          lost {timeAgo(sess.lost_at)}{#if lostReasonLabel(sess.lost_reason)}<span data-testid="lost-reason"> · {lostReasonLabel(sess.lost_reason)}</span>{/if}
        </span>
      {/if}
      <div class="row-actions">
        <button
          class="icon-btn small"
          data-testid="ghost-recreate"
          onclick={(e) => doRecreate(sess, e)}
          disabled={!hostIsReachable(sess.host_alias) || recreateBlocked !== null}
          title={recreateBlocked ?? (hostIsReachable(sess.host_alias) ? 'Recreate tmux session' : 'Host is offline')}
          aria-label="Recreate"
        >↺</button>
        <button
          class="icon-btn small danger"
          data-testid="ghost-dismiss"
          onclick={(e) => doDismissGhost(sess, e)}
          disabled={ghostDismissBlocked !== null}
          title={ghostDismissBlocked ?? 'Dismiss ghost session'}
          aria-label="Dismiss"
        >×</button>
      </div>
    {:else}
      <div class="sess-lines">
        <div class="sess-line1">
          <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
          {#if relatedCount > 0}
            <span
              class="related-badge"
              data-testid="related-badge"
              role="img"
              title="{relatedCount} related session(s)"
              aria-label="{relatedCount} related sessions"
            >🔗{relatedCount}</span>
          {/if}
          {#if sess.kind === 'review'}
            <span class="review-badge" role="img" title="review session" aria-label="review session">🔍</span>
          {/if}
          {#if sess.kind === 'shell'}
            <span class="shell-badge" title="shell session">▶</span>
          {/if}
          {#if sess.kind === 'bg'}
            <span class="bg-badge" role="img" title="background agent" aria-label="background agent">🤖</span>
          {/if}
          <span class="sess-name" title={sess.tmux_name}>{primaryName}</span>
          {#if workKey}
            <WorkChip {workKey} />
          {/if}
          {#if suggestionKey && suggestion}
            <WorkChip
              workKey={suggestionKey}
              suggested
              testid="work-suggestion"
              onclick={(e) => (workBlocked === null ? openWorkMenu(e) : e.stopPropagation())}
            />
          {/if}
          {#if sess.stuck_kind}
            <!-- Stuck outranks claude_status: one red chip, no green "working"
                 next to it to soften the signal. -->
            <span
              class="claude-chip stuck-chip"
              data-testid="stuck-chip"
              style="background: {STUCK_COLOR}22; color: {STUCK_COLOR}; border-color: {STUCK_COLOR}66;"
              title="Stuck: {stuckKindLabel(sess.stuck_kind)}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
            >⚠ stuck: {stuckKindLabel(sess.stuck_kind)}</span>
          {:else if isInactiveAgent(sess)}
            <!-- A bg agent whose CLI process is gone: shown as stopped
                 (grey), offering Remove from list instead of the usual
                 claude_status chip. -->
            <span class="claude-chip inactive-chip" data-testid="inactive-chip">inactive</span>
          {:else if sess.claude_status}
            <span
              class="claude-chip"
              data-testid="claude-chip"
              style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
              title="Claude: {sess.claude_status}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
            >{claudeStatusLabel(sess.claude_status)}</span>
          {/if}
          <div class="row-actions">
            {#if isInactiveAgent(sess)}
              <button
                class="icon-btn small danger"
                data-testid="remove-from-list"
                disabled={dismissAgentBlocked !== null}
                onclick={(e) => doDismissAgent(sess, e)}
                title={dismissAgentBlocked ?? 'Remove from list'}
                aria-label="Remove from list"
              >×</button>
            {/if}
            <button
              class="icon-btn small"
              data-testid="restart-session"
              onclick={(e) => askRestart(sess, e)}
              disabled={restartBlocked !== null}
              title={restartBlocked ?? 'Restart claude in this session'}
              aria-label="Restart"
            >↻</button>
            <button
              class="icon-btn small"
              data-testid="work-menu"
              onclick={(e) => (workMenuOpen ? toggleWorkMenu(e) : openWorkMenu(e))}
              disabled={workBlocked !== null}
              title={workBlocked ?? 'Work: set, "Not this", clear'}
              aria-label="Work"
              aria-expanded={workMenuOpen}
            >#</button>
            <button
              class="icon-btn small"
              data-testid="edit-label"
              onclick={(e) => beginLabelEdit(sess, e)}
              disabled={labelBlocked !== null}
              title={labelBlocked ?? 'Edit label (double-click the row)'}
              aria-label="Edit label"
            >🏷</button>
            <button
              class="icon-btn small"
              data-testid="rename-tmux"
              onclick={(e) => beginRename(sess, e)}
              disabled={tmuxRenameBlocked !== null}
              title={tmuxRenameBlocked ?? 'Rename tmux session'}
              aria-label="Rename tmux session"
            >✎</button>
            <button
              class="icon-btn small"
              data-testid="recreate-live"
              onclick={(e) => askRecreate(sess, e)}
              disabled={!hostIsReachable(sess.host_alias) || recreateBlocked !== null}
              title={recreateBlocked ?? (hostIsReachable(sess.host_alias)
                ? 'Recreate: kill the tmux session and start it fresh in the same worktree'
                : 'Host is offline')}
              aria-label="Recreate"
            >♻</button>
            {#if !isInactiveAgent(sess)}
              <!-- An inactive agent's daemon is gone: Remove from list is its
                   only removal action. -->
              <button
                class="icon-btn small danger"
                onclick={(e) => askKill(sess, e)}
                disabled={killBlocked !== null}
                title={killBlocked ?? 'Kill session'}
                aria-label="Kill"
              >×</button>
            {/if}
          </div>
        </div>
        {#if workMenuOpen}
          <!-- Every control stops its click: the panel sits inside the row,
               whose own click selects the session. -->
          <div
            class="work-menu"
            data-testid="work-menu-panel"
            role="group"
            aria-label="Work for {primaryName}"
          >
            {#if explained.length > 0}
              <div class="work-why" data-testid="work-why">
                {#each explained as l (l.id)}
                  <div class="why-link" data-testid="why-link" data-state={l.state}>
                    <span class="why-key">{linkLabel(l)}</span>
                    <span class="why-what">{workWhy(l)}</span>
                    {#each l.evidence ?? [] as ev, i (i)}
                      <span class="why-ev" data-testid="why-evidence" title={ev.snippet ?? ''}>{describeEvidence(ev)}</span>
                    {/each}
                    {#if l.state === 'suggested'}
                      <span class="why-actions">
                        <button class="work-btn" data-testid="why-confirm" disabled={workBusy}
                          title="Confirm (↵ / y)" onclick={(e) => confirmLink(l.id, e)}>Confirm</button>
                        <button class="work-btn" data-testid="why-reject" disabled={workBusy}
                          title="Not this (⌫ / n): never suggested again" onclick={(e) => rejectLink(l.id, e)}>Not this</button>
                        <button class="work-btn" data-testid="why-pick" disabled={workBusy}
                          title="Type or paste another key or ticket URL" onclick={pickAnother}>Pick another…</button>
                      </span>
                    {/if}
                  </div>
                {/each}
                {#if sess.project_id != null}
                  <!-- The label only stops the row's own click (it would
                       select the session); the checkbox is the control. -->
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                  <label class="why-trust" onclick={(e) => e.stopPropagation()}>
                    <input type="checkbox" data-testid="why-trust" checked={projectTrusted} onchange={toggleTrust} />
                    Trust branch keys in this repo
                  </label>
                {/if}
              </div>
            {/if}
            <input
              bind:this={workInput}
              class="work-input"
              data-testid="work-input"
              aria-label="Work key or name"
              onclick={(e) => e.stopPropagation()}
              placeholder={rowWork ? `Replace ${rowWork.key}…` : 'ABC-123 or a name'}
              bind:value={workDraft}
              onkeydown={onWorkKey}
              disabled={workBusy}
            />
            <button
              class="work-btn"
              data-testid="work-set"
              disabled={workBusy || !workDraft.trim()}
              onclick={setWork}
            >Set</button>
            {#if rowWork}
              <button
                class="work-btn"
                data-testid="work-reject"
                disabled={workBusy}
                title="{rowWork.key} is not this session's work; it will not be suggested again"
                onclick={rejectWork}
              >Not {rowWork.key}</button>
              {#if rowWork.source === 'link' && sess.work}
                <button
                  class="work-btn"
                  data-testid="work-unlink"
                  disabled={workBusy}
                  title="Remove the link (it may be recognised again)"
                  onclick={clearWork}
                >Clear</button>
              {/if}
            {/if}
          </div>
        {/if}
        {#if answerView}
          <!-- Claude is asking this row a question. The "Needs you" filter
               shows exactly these rows, so the answer belongs here and not
               only behind a click into the session. -->
          <AnswerPrompt session={sess} view={answerView} compact />
        {/if}
        {#if $showRowDetails}
          <div class="sess-details" data-testid="sess-details">
            <span class="host-badge" data-testid="host-badge" aria-label="host {sess.host_alias}">{sess.host_alias}</span>
            {#if secondaryName}
              <span class="sep" aria-hidden="true">·</span>
              <span class="sess-secondary" data-testid="sess-tmux-name">{secondaryName}</span>
            {/if}
            {#if elapsed}
              <span class="sep" aria-hidden="true">·</span>
              <span class="sess-elapsed">{elapsed}</span>
            {/if}
            {#if ctxLevel !== null && sess.context_pct !== null}
              <span class="sep" aria-hidden="true">·</span>
              <span
                class="ctx-badge ctx-{ctxLevel}"
                data-testid="context-badge"
                data-level={ctxLevel}
                style="color: {contextColor(ctxLevel)}; border-color: {contextColor(ctxLevel)};"
                title="Context window {Math.round(sess.context_pct)}% used"
                role="meter"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={Math.round(sess.context_pct)}
                aria-label="context usage"
              ><span class="ctx-bar" style="width: {Math.min(100, Math.max(0, sess.context_pct))}%; background: {contextColor(ctxLevel)};"></span><span class="ctx-pct">{Math.round(sess.context_pct)}%</span></span>
            {/if}
            {#if sessionUsageTokens(sess) > 0}
              {@const priced = (sess.usage_cost_micros ?? 0) > 0}
              <span class="sep" aria-hidden="true">·</span>
              <span
                class="cost-badge"
                data-testid="cost-badge"
                data-priced={priced}
                title={priced
                  ? `Estimated cost ${formatCostMicros(sess.usage_cost_micros)} · ${formatTokens(sessionUsageTokens(sess))} tokens${sess.usage_model ? ' · ' + sess.usage_model : ''}`
                  : `Unpriced: no price for ${sess.usage_model ?? 'an unknown model'} · ${formatTokens(sessionUsageTokens(sess))} tokens`}
              >{priced ? formatCostMicros(sess.usage_cost_micros) : 'unpriced'}</span>
            {/if}
            {#if sess.effort_level}
              <span class="sep" aria-hidden="true">·</span>
              <span class="effort-badge" title="Effort: {sess.effort_level}">{sess.effort_level}</span>
            {/if}
            {#if sess.pr_url}
              <span class="sep" aria-hidden="true">·</span>
              <a
                class="pr-link"
                href={sess.pr_url}
                onclick={(e) => e.stopPropagation()}
                title="Open pull request"
                target="_blank"
                rel="noreferrer"
              >PR↗</a>
              {#if sess.ci_status}
                <span class="sep" aria-hidden="true">·</span>
                <span
                  class="ci-badge"
                  data-testid="ci-badge"
                  style="color: {ciStatusColor(sess.ci_status)};"
                  title="CI checks: {sess.ci_status}"
                >{ciStatusLabel(sess.ci_status)}</span>
              {/if}
            {/if}
            {#if promptText}
              <span class="sep" aria-hidden="true">·</span>
              <span class="sess-meta" data-testid="sess-meta" title={sess.last_prompt ?? undefined}>{promptText}</span>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>
{#if isRenaming && renameError}
  <p class="err inline-err">{renameError}</p>
{/if}

<style>
  .icon-btn {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 5px;
    font-size: 0.9rem;
    line-height: 1;
    cursor: pointer;
    min-width: 1.6rem;
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }
  .icon-btn.small {
    padding: 0.1rem 0.35rem;
    font-size: 0.85rem;
    min-width: 1.4rem;
    border-color: transparent;
  }
  .icon-btn.small:hover { border-color: var(--border); }
  .icon-btn.danger:hover { color: #e64a4a; border-color: #e64a4a; }

  /* Line 1's dot/badges/name sit near the row's vertical center; align the
     checkbox with that line instead of the two-line row's overall center
     (align-items: center on .sess-row would otherwise split the difference
     and visually float the box between the two lines). */
  .select-box { margin: 0; margin-top: 0.2rem; flex-shrink: 0; align-self: flex-start; }
  .sess-row.checked { outline: 1px solid var(--accent); }
  .sess-row.stuck { background: rgba(230, 74, 74, 0.06); }

  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .related-badge {
    font-size: 0.65rem;
    color: var(--fg-muted);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .review-badge { font-size: 0.7rem; margin-left: 0.2rem; }
  .shell-badge { font-size: 0.7rem; margin-left: 0.2rem; color: var(--fg-muted); }
  .bg-badge { font-size: 0.7rem; margin-left: 0.2rem; }

  .err { color: #e64a4a; font-size: 0.8rem; padding: 0.2rem 0; margin: 0; }
  .inline-err { padding-left: 1.6rem; font-size: 0.75rem; }

  .sess-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.82rem;
    padding: 0.22rem 0.4rem 0.22rem 1.4rem;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
    user-select: none;
  }
  .sess-row:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .sess-row.selected { background: color-mix(in srgb, var(--accent) 22%, transparent); }
  .sess-row.renaming { background: var(--bg-pane); }
  /* The row is the app's primary navigation surface and is a tabbable
     role="button". Without this a keyboard user tabbing the session list
     sees nothing move at all (WCAG 2.4.7). Drawn inward: the row is inside
     a scrolling list that clips an outset ring. */
  .sess-row:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .sess-row .row-actions {
    display: none;
    gap: 0.05rem;
  }
  .sess-row:hover .row-actions,
  .sess-row:focus-within .row-actions,
  .sess-row.selected .row-actions { display: flex; }

  /* Line 1's actions must never take flex width: reserving space for them
     permanently narrows the name, and NOT reserving space (the old rule)
     let them pop in at display:flex and squeeze the name to a sliver the
     instant the pointer entered. Take them out of flow entirely instead —
     absolutely positioned over the name's tail — with a solid strip in the
     row's current background plus a short fade so the truncated text reads
     cleanly right up to the overlay. */
  .sess-line1 .row-actions {
    position: absolute;
    right: 0;
    top: 50%;
    transform: translateY(-50%);
    padding-left: 0.15rem;
    background: var(--bg-pane);
  }
  .sess-line1 .row-actions::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    right: 100%;
    width: 1rem;
    background: linear-gradient(to right, transparent, var(--bg-pane));
  }
  .sess-row:focus-within .sess-line1 .row-actions,
  .sess-row:hover .sess-line1 .row-actions {
    background: color-mix(in srgb, var(--accent) 10%, var(--bg-pane));
  }
  .sess-row:focus-within .sess-line1 .row-actions::before,
  .sess-row:hover .sess-line1 .row-actions::before {
    background: linear-gradient(to right, transparent, color-mix(in srgb, var(--accent) 10%, var(--bg-pane)));
  }
  .sess-row.selected .sess-line1 .row-actions {
    background: color-mix(in srgb, var(--accent) 22%, var(--bg-pane));
  }
  .sess-row.selected .sess-line1 .row-actions::before {
    background: linear-gradient(to right, transparent, color-mix(in srgb, var(--accent) 22%, var(--bg-pane)));
  }

  .status-dot {
    width: 0.45rem;
    height: 0.45rem;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--fg-muted);
  }
  .status-dot.status-running { background: rgb(80, 200, 110); }
  .status-dot.status-frozen { background: rgb(140, 180, 240); }
  .status-dot.status-orphan { background: rgb(220, 130, 130); }
  .status-dot.status-ghost { background: rgb(160, 120, 200); opacity: 0.55; }
  .lost-at {
    font-size: 0.7em;
    opacity: 0.6;
    margin-left: auto;
    padding-right: 0.25rem;
    white-space: nowrap;
  }

  .claude-chip {
    font-size: 0.65rem;
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    border: 1px solid;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .stuck-chip { font-weight: 600; }
  .inactive-chip {
    background: color-mix(in srgb, var(--fg-muted) 18%, transparent);
    color: var(--fg-muted);
    border-color: color-mix(in srgb, var(--fg-muted) 40%, transparent);
  }
  .ctx-badge {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 2.6rem;
    height: 0.95rem;
    font-size: 0.6rem;
    border: 1px solid;
    border-radius: 3px;
    overflow: hidden;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }
  .ctx-bar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    opacity: 0.25;
  }
  .ctx-pct { position: relative; }
  .cost-badge {
    font-size: 0.6rem;
    flex-shrink: 0;
    white-space: nowrap;
    opacity: 0.75;
    font-variant-numeric: tabular-nums;
  }
  .ci-badge {
    font-size: 0.6rem;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .effort-badge {
    font-size: 0.6rem;
    padding: 0.05rem 0.25rem;
    border-radius: 3px;
    background: color-mix(in srgb, var(--fg) 10%, transparent);
    color: var(--fg-muted);
    flex-shrink: 0;
    white-space: nowrap;
    text-transform: uppercase;
  }
  .work-why {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    width: 100%;
    font-size: 0.7rem;
  }
  .why-link {
    display: flex;
    flex-wrap: wrap;
    gap: 0.15rem 0.4rem;
    align-items: baseline;
  }
  .why-key {
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .why-what,
  .why-ev {
    color: var(--fg-muted);
  }
  .why-ev {
    flex-basis: 100%;
    padding-left: 0.6rem;
  }
  .why-actions {
    display: flex;
    gap: 0.3rem;
    flex-basis: 100%;
  }
  .why-trust {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    color: var(--fg-muted);
  }
  .work-menu {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    align-items: center;
    padding: 0.2rem 0 0.1rem;
  }
  .work-input {
    flex: 1 1 8rem;
    min-width: 0;
    font-size: 0.7rem;
    padding: 0.1rem 0.3rem;
  }
  .work-btn {
    font-size: 0.65rem;
    padding: 0.05rem 0.35rem;
    white-space: nowrap;
  }
  .pr-link {
    font-size: 0.65rem;
    color: var(--accent);
    text-decoration: none;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .pr-link:hover { text-decoration: underline; }

  .sess-lines { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 0.1rem; }
  .sess-line1 { position: relative; display: flex; align-items: center; gap: 0.4rem; min-width: 0; }
  .sess-line1 .sess-name { flex: 1; }
  .sess-details {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.35rem;
    row-gap: 0.15rem;
    min-width: 0;
    padding-left: 0.85rem;
    font-size: 0.65rem;
    color: var(--fg-muted);
  }
  /* The line wraps instead of clipping — hiding the prompt preview (or any
     badge) with no visible trace that it exists would defeat the point of
     the line. Extra height is the user's choice: they opted into this line
     via the details toggle and can collapse it. */
  .sess-details > * { flex-shrink: 0; }
  .sess-details > .sess-secondary { flex-shrink: 1; min-width: 0; }
  .sess-details > .sess-meta { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .sess-details .sep { color: var(--fg-muted); opacity: 0.6; }
  .sess-name {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.8rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sess-secondary,
  .sess-meta {
    font-size: 0.65rem;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sess-secondary { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }

  .rename-input {
    flex: 1;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.8rem;
    padding: 0.1rem 0.3rem;
    border: 1px solid var(--accent);
    background: var(--bg);
    color: var(--fg);
    border-radius: 3px;
    outline: none;
    min-width: 0;
  }

</style>
