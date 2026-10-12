<script lang="ts">
  // The Work button (task → session spec §2.2): one split button per task.
  // Its primary half does the obvious thing — Open the live session, else
  // Continue the last past one, else Start — and a Start asks for the start
  // preview first: a clean one starts at once, anything else (a repository
  // or host to pick, a live session, another organisation, a done task)
  // opens the start popover. ▾ always offers Start new…, the past sessions
  // to continue, Attach running session… (J3: the attach picker, with Undo
  // for ten seconds after) and Copy key. Alt-click on Start opens the
  // popover too.
  import { onDestroy, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { resumeWork } from './work';
  import { groupSessionLinks, readErrorText, type WorkTask, type WorkTaskLink } from './work_view';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';
  import { copyText } from './clipboard';
  import { startWork, type StartWorkArgs } from './trackers';
  import StartPopover from './StartPopover.svelte';
  import { startAskRequest, takeStartAsk } from './new_task';
  import StartProgressStrip from './StartProgressStrip.svelte';
  import AttachPicker from './AttachPicker.svelte';
  import { operatorRow } from './operator';
  import type { AttachTarget, Attached } from './attach';
  import type { IpcError } from './result';
  import {
    PRIMARY_LABEL,
    baseStartArgs,
    lastStartSettings,
    previewIsClean,
    previewStartWork,
    previewUnsupported,
    primaryAction,
    registerWorkButton,
    startFromPreview,
    taskBriefDrafts,
    type StartPreview,
  } from './start_preview';

  let {
    task,
    /** `row`: compact, in a list; `bar`: the task page's action bar. */
    variant = 'row',
  }: {
    /** What the button reads of a task: a subtask passes one too (6.6). */
    task: Pick<WorkTask, 'task_id' | 'item_id' | 'key' | 'title' | 'project_id' | 'sessions'>;
    variant?: 'row' | 'bar';
  } = $props();

  const grouped = $derived(groupSessionLinks(task.sessions ?? []));
  const liveLink = $derived(
    grouped.active.find((l) => l.session_id != null && $sessions.some((r) => r.id === l.session_id)) ?? null,
  );
  const pastLinks = $derived(task.key ? grouped.past.filter((l) => l.resumable !== false) : []);
  const action = $derived(primaryAction({ live: liveLink !== null, resumable: pastLinks.length > 0 }));
  /** Redesign 6.6: the start is named "Start new", the same words as ▾ and
   *  every other start point. */
  const primaryLabel = $derived(action === 'start' ? 'Start new' : PRIMARY_LABEL[action]);
  const startBlocked = $derived(hubActionBlocked('start_work', $hubStatus, $hubConnection));
  /** Continue re-opens somebody's past conversation: the hub half, then the
   *  access half on the SOURCE session (`share.ts`'s `own` tier for
   *  `resume_work`), as the task page does. */
  const continueBlocked = (l: WorkTaskLink | undefined) =>
    startBlocked ?? $sessionIdBlocked(l?.session_id ?? null, 'resume_work');
  const primaryBlocked = $derived(
    action === 'open' ? null : action === 'continue' ? continueBlocked(pastLinks[0]) : startBlocked,
  );
  const canStart = $derived(task.item_id != null || !!task.key);

  let busy = $state(false);
  let error = $state<string | null>(null);
  /** The session an E_EXISTS refusal points at: the error offers to open it. */
  let existing = $state<number | null>(null);
  let menuOpen = $state(false);
  let popover = $state<StartPreview | null>(null);
  let attaching = $state(false);
  /** The session the last Start made: its progress shows under the button. */
  let progressFor = $state<number | null>(null);
  /** The last attach, undoable for a while (spec §2.4). */
  let undo = $state<{ text: string; run: Attached['undo'] } | null>(null);
  let undoTimer: ReturnType<typeof setTimeout> | undefined;
  const UNDO_MS = 10_000;
  let popPos = $state({ top: 0, left: 0 });
  let root: HTMLElement | undefined = $state();
  let mainBtn: HTMLButtonElement | undefined = $state();

  const base = $derived(baseStartArgs(task));
  /** G7.6: the last session's host and repository, for "Start with last
   *  settings" in the menu. */
  const lastSettings = $derived(lastStartSettings(task, $sessions));
  /** The arguments the open popover was read with (the base, or the base
   *  with the last settings). */
  let popBase = $state<StartWorkArgs | null>(null);
  const attachTarget = $derived<AttachTarget | null>(
    task.item_id != null || task.key
      ? {
          ref: task.item_id != null ? { item_id: task.item_id } : { key: task.key ?? '' },
          key: task.key,
          linkedSessionIds: new Set(grouped.active.map((l) => l.session_id).filter((id): id is number => id != null)),
          operatorId: $operatorRow?.id ?? null,
          projectIds: new Set(
            [task.project_id, ...(task.sessions ?? []).map((l) => $sessions.find((r) => r.id === l.session_id)?.project_id)].filter(
              (id): id is number => id != null,
            ),
          ),
        }
      : null,
  );
  const attachBlocked = $derived(hubActionBlocked('link_session_work', $hubStatus, $hubConnection));

  async function openAttach() {
    menuOpen = false;
    popover = null;
    place();
    attaching = true;
    await tick();
  }

  function attached(a: Attached, how: string) {
    attaching = false;
    error = null;
    clearTimeout(undoTimer);
    undo = { text: how, run: a.undo };
    undoTimer = setTimeout(() => (undo = null), UNDO_MS);
    mainBtn?.focus();
  }

  async function runUndo() {
    const u = undo;
    if (!u?.run || busy) return;
    clearTimeout(undoTimer);
    busy = true;
    const r = await u.run();
    busy = false;
    undo = null;
    if (!r.ok) error = `Undo: ${readErrorText(r.error)}`;
  }
  onDestroy(() => clearTimeout(undoTimer));
  const heading = $derived(`Start ${task.key ?? ''}${task.title ? ` · ${task.title}` : ''}`.trim());

  function started(row: SessionRow) {
    popover = null;
    error = null;
    progressFor = row.id;
    selectSessionExplicitly(row);
  }

  /** Fail with `e`. An E_EXISTS (the work is live already, or being resumed)
   *  names the session to open: its own `details.session_id`, else the live
   *  session this task shows, else one whose primary work is this key. */
  function fail(e: IpcError) {
    error = readErrorText(e);
    existing = null;
    if (e.code !== 'E_EXISTS') return;
    const sid = (e.details as { session_id?: number } | undefined)?.session_id;
    if (typeof sid === 'number') existing = sid;
    else if (liveLink?.session_id != null) existing = liveLink.session_id;
    else {
      const key = task.key?.toUpperCase();
      existing = key ? (get(sessions).find((r) => r.work?.key?.toUpperCase() === key)?.id ?? null) : null;
    }
  }

  function openExisting() {
    const row = get(sessions).find((r) => r.id === existing);
    if (row) selectSessionExplicitly(row);
  }

  function openLive() {
    const row = liveLink?.session_id != null ? get(sessions).find((r) => r.id === liveLink.session_id) : undefined;
    if (row) selectSessionExplicitly(row);
  }

  async function continueFrom(l: WorkTaskLink | undefined) {
    if (!task.key || !l || busy) return;
    // Re-asked at the write, on the SOURCE session, not only on the button.
    const why = startBlocked ?? $sessionIdBlocked(l.session_id ?? null, 'resume_work');
    if (why) {
      error = why;
      return;
    }
    busy = true;
    error = null;
    existing = null;
    const r = await resumeWork({ key: task.key, mode: 'last', linkId: l.link_id });
    busy = false;
    if (r.ok) selectSessionExplicitly(r.value);
    else fail(r.error);
  }

  /** Start: the preview first. `ask` opens the popover whatever it says;
   *  `over` (the last settings) fixes the host and repository. */
  async function start(ask: boolean, over?: Partial<StartWorkArgs>) {
    if (!canStart || busy) return;
    if (startBlocked) {
      error = startBlocked;
      return;
    }
    busy = true;
    error = null;
    existing = null;
    const forTask = task.task_id;
    const args: StartWorkArgs = over ? { ...base, ...over } : base;
    const p = await previewStartWork(args);
    // Another task was opened meanwhile (the task page keeps this button):
    // this preview is not its (review r07), so neither its popover nor a
    // start from it.
    if (task.task_id !== forTask) {
      busy = false;
      return;
    }
    if (!p.ok && previewUnsupported(p.error)) {
      // An older hub: start as before, its refusals shown as they come.
      const r = await startWork(args);
      busy = false;
      if (r.ok) started(r.value);
      else fail(r.error);
      return;
    }
    if (!p.ok) {
      busy = false;
      error = readErrorText(p.error);
      return;
    }
    if (!ask && previewIsClean(p.value)) {
      // A brief drafted in the task page goes with it (G7.6).
      const brief = args.with_brief ? get(taskBriefDrafts).get(task.task_id)?.brief : undefined;
      const r = await startFromPreview(brief ? { ...args, brief } : args, p.value);
      busy = false;
      if (r.ok) started(r.value);
      else fail(r.error);
      return;
    }
    busy = false;
    await openPopover(p.value, args);
  }

  /** The Continue menu's "Start with last settings": a new session where
   *  the last one ran, without asking when nothing is in the way. */
  function startWithLast() {
    if (!lastSettings) return;
    menuOpen = false;
    void start(false, lastSettings);
  }

  async function openPopover(p: StartPreview, args: StartWorkArgs = base) {
    menuOpen = false;
    place();
    popBase = args;
    popover = p;
    await tick();
  }

  // Fixed, below the button and kept on screen: a list in a scrolling
  // sidebar would clip an absolutely placed popover.
  function place() {
    const r = (mainBtn ?? root)?.getBoundingClientRect();
    if (!r) return;
    const width = Math.min(360, window.innerWidth - 32);
    const left = Math.max(16, Math.min(r.right - width, window.innerWidth - width - 16));
    const top = Math.min(r.bottom + 4, Math.max(16, window.innerHeight - 420));
    popPos = { top, left };
  }

  function closePopover(refocus: boolean) {
    popover = null;
    if (refocus) mainBtn?.focus();
  }

  // New task's "Start a session for it now" (G2.1): the first button of
  // the new task to mount opens its start menu.
  $effect(() => {
    const want = $startAskRequest;
    if (want === null || want !== task.task_id || !canStart || busy) return;
    if (takeStartAsk(want)) void tick().then(() => start(true));
  });

  function primary(ev?: MouseEvent) {
    if (action === 'open') openLive();
    else if (action === 'continue') void continueFrom(pastLinks[0]);
    else void start(!!ev?.altKey);
  }

  $effect(() => {
    if (!menuOpen && !popover && !attaching) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (root?.contains(t)) return;
      if ((t as Element).closest?.('[data-testid="start-popover"], [data-testid="attach-picker"]')) return;
      menuOpen = false;
      popover = null;
      attaching = false;
    };
    document.addEventListener('mousedown', away);
    return () => document.removeEventListener('mousedown', away);
  });

  // The list's keyboard (`s` runs the primary half, ⇧S opens the popover).
  const off = registerWorkButton(() => task.task_id, {
    primary: () => primary(),
    ask: () => void start(true),
  });
  onDestroy(off);
</script>

<span class="wb wb--{variant}" bind:this={root} data-testid="work-button">
  <span class="split">
    <button
      class="btn main"
      class:btn--primary={variant === 'bar'}
      type="button"
      bind:this={mainBtn}
      data-testid="work-button-primary"
      data-action={action}
      disabled={busy || primaryBlocked !== null || (action === 'start' && !canStart)}
      title={primaryBlocked ??
        (action === 'open'
          ? `Open ${liveLink?.name ?? 'the live session'}`
          : action === 'continue'
            ? `Resume the last conversation of ${pastLinks[0]?.name ?? 'the last session'}`
            : 'Start a session for this task (Alt-click to choose where)')}
      onclick={(e) => primary(e)}>{busy ? '…' : primaryLabel}</button
    ><button
      class="btn caret"
      class:btn--primary={variant === 'bar'}
      type="button"
      aria-haspopup="menu"
      aria-expanded={menuOpen}
      aria-label="More ways to work on this task"
      data-testid="work-button-menu"
      disabled={busy}
      onclick={() => (menuOpen = !menuOpen)}>▾</button
    >
  </span>
  {#if menuOpen}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <span
      class="menu"
      role="menu"
      tabindex="-1"
      data-testid="work-button-menu-list"
      onkeydown={(e) => {
        if (e.key === 's' && !e.metaKey && !e.ctrlKey && !e.altKey && lastSettings && canStart && !startBlocked) {
          e.preventDefault();
          e.stopPropagation();
          startWithLast();
        }
      }}
    >
      <button
        type="button"
        role="menuitem"
        data-testid="work-button-start-new"
        disabled={!canStart || startBlocked !== null}
        title={startBlocked ?? 'Choose where a new session starts'}
        onclick={() => void start(true)}>Start new…</button
      >
      {#each pastLinks.slice(0, 3) as l (l.link_id)}
        <button
          type="button"
          role="menuitem"
          data-testid="work-button-continue"
          disabled={continueBlocked(l) !== null}
          title={continueBlocked(l) ?? `Resume ${l.name ?? 'that session'}'s last conversation`}
          onclick={() => {
            menuOpen = false;
            void continueFrom(l);
          }}>Continue {l.name ?? `session ${l.link_id}`}</button
        >
      {/each}
      {#if lastSettings}
        <button
          type="button"
          role="menuitem"
          class="with-key"
          data-testid="work-button-start-last"
          disabled={!canStart || startBlocked !== null}
          title={startBlocked ??
            `A new session on ${lastSettings.host_alias}, where the last one ran`}
          onclick={startWithLast}><span>Start with last settings</span><kbd>s</kbd></button
        >
      {/if}
      {#if attachTarget}
        <button
          type="button"
          role="menuitem"
          data-testid="work-button-attach"
          disabled={attachBlocked !== null}
          title={attachBlocked ?? 'Put a session that is already running on this task'}
          onclick={() => void openAttach()}>Attach running session…</button
        >
      {/if}
      {#if task.key}
        <button
          type="button"
          role="menuitem"
          data-testid="work-button-copy-key"
          onclick={() => {
            menuOpen = false;
            void copyText(task.key ?? '');
          }}>Copy key</button
        >
      {/if}
    </span>
  {/if}
  {#if error}<span class="err" role="alert" data-testid="work-button-error"
      >{error}{#if existing !== null}
        <button class="btn btn--quiet" type="button" data-testid="work-button-open-existing" onclick={openExisting}>Open it</button
        >{/if}</span
    >{/if}
  {#if progressFor != null}
    {#key progressFor}
      <StartProgressStrip sessionId={progressFor} onclose={() => (progressFor = null)} />
    {/key}
  {/if}
  {#if undo}
    <span class="undo" role="status" data-testid="work-button-undo-notice"
      >{undo.text}{#if undo.run}
        <button class="btn btn--quiet" type="button" data-testid="work-button-undo" disabled={busy} onclick={() => void runUndo()}>Undo</button>{/if}</span
    >
  {/if}
</span>

{#if attaching && attachTarget}
  <div class="pop-anchor" style:top="{popPos.top}px" style:left="{popPos.left}px">
    <AttachPicker
      target={attachTarget}
      heading={`Attach a running session to ${task.key ?? task.title ?? 'this task'}`}
      onclose={(refocus) => {
        attaching = false;
        if (refocus) mainBtn?.focus();
      }}
      onattached={attached}
    />
  </div>
{/if}

{#if popover}
  <div class="pop-anchor" style:top="{popPos.top}px" style:left="{popPos.left}px">
    <StartPopover base={popBase ?? base} preview={popover} held={$taskBriefDrafts.get(task.task_id) ?? null} {heading} blocked={startBlocked} onclose={closePopover} onstarted={started} />
  </div>
{/if}

<style>
  .wb {
    position: relative;
    display: inline-grid;
    justify-items: end;
    gap: 2px;
  }
  .split {
    display: inline-flex;
  }
  .main {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }
  .caret {
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left-width: 0;
    padding-inline: 4px;
  }
  .wb--row .main {
    min-width: 64px;
  }
  .menu {
    position: absolute;
    top: calc(100% + 2px);
    right: 0;
    z-index: 20;
    display: grid;
    min-width: 180px;
    padding: 4px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg);
    box-shadow: var(--shadow-pop);
  }
  .menu button {
    background: none;
    border: 0;
    padding: 4px 8px;
    text-align: left;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
    border-radius: var(--radius-sm);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .menu button:hover:not(:disabled),
  .menu button:focus-visible {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .menu .with-key {
    display: flex;
    gap: 12px;
    justify-content: space-between;
  }
  .menu kbd {
    color: var(--fg-muted);
    font: inherit;
  }
  .menu button:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
  .err {
    max-width: 260px;
    color: var(--usage-crit);
    font-size: var(--text-2xs);
    text-align: right;
  }
  .undo {
    display: inline-flex;
    gap: 4px;
    align-items: baseline;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .pop-anchor {
    position: fixed;
    z-index: 30;
  }
</style>
