<script lang="ts">
  // The Work button (task → session spec §2.2): one split button per task.
  // Its primary half does the obvious thing — Open the live session, else
  // Continue the last past one, else Start — and a Start asks for the start
  // preview first: a clean one starts at once, anything else (a repository
  // or host to pick, a live session, another organisation, a done task)
  // opens the start popover. ▾ always offers Start new…, the past sessions
  // to continue, and Copy key. Alt-click on Start opens the popover too.
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
  import { startWork } from './trackers';
  import StartPopover from './StartPopover.svelte';
  import {
    PRIMARY_LABEL,
    baseStartArgs,
    previewIsClean,
    previewStartWork,
    previewUnsupported,
    primaryAction,
    registerWorkButton,
    startFromPreview,
    type StartPreview,
  } from './start_preview';

  let {
    task,
    /** `row`: compact, in a list; `bar`: the task page's action bar. */
    variant = 'row',
  }: { task: WorkTask; variant?: 'row' | 'bar' } = $props();

  const grouped = $derived(groupSessionLinks(task.sessions ?? []));
  const liveLink = $derived(
    grouped.active.find((l) => l.session_id != null && $sessions.some((r) => r.id === l.session_id)) ?? null,
  );
  const pastLinks = $derived(task.key ? grouped.past.filter((l) => l.resumable !== false) : []);
  const action = $derived(primaryAction({ live: liveLink !== null, resumable: pastLinks.length > 0 }));
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
  let menuOpen = $state(false);
  let popover = $state<StartPreview | null>(null);
  let popPos = $state({ top: 0, left: 0 });
  let root: HTMLElement | undefined = $state();
  let mainBtn: HTMLButtonElement | undefined = $state();

  const base = $derived(baseStartArgs(task));
  const heading = $derived(`Start ${task.key ?? ''}${task.title ? ` · ${task.title}` : ''}`.trim());

  function started(row: SessionRow) {
    popover = null;
    error = null;
    selectSessionExplicitly(row);
  }

  function openLive() {
    const row = liveLink?.session_id != null ? get(sessions).find((r) => r.id === liveLink.session_id) : undefined;
    if (row) selectSessionExplicitly(row);
  }

  async function continueFrom(l: WorkTaskLink | undefined) {
    if (!task.key || !l || busy) return;
    const why = continueBlocked(l);
    if (why) {
      error = why;
      return;
    }
    busy = true;
    error = null;
    const r = await resumeWork({ key: task.key, mode: 'last', linkId: l.link_id });
    busy = false;
    if (r.ok) selectSessionExplicitly(r.value);
    else error = readErrorText(r.error);
  }

  /** Start: the preview first. `ask` opens the popover whatever it says. */
  async function start(ask: boolean) {
    if (!canStart || busy) return;
    if (startBlocked) {
      error = startBlocked;
      return;
    }
    busy = true;
    error = null;
    const p = await previewStartWork(base);
    if (!p.ok && previewUnsupported(p.error)) {
      // An older hub: start as before, its refusals shown as they come.
      const r = await startWork(base);
      busy = false;
      if (r.ok) started(r.value);
      else error = readErrorText(r.error);
      return;
    }
    if (!p.ok) {
      busy = false;
      error = readErrorText(p.error);
      return;
    }
    if (!ask && previewIsClean(p.value)) {
      const r = await startFromPreview(base, p.value);
      busy = false;
      if (r.ok) started(r.value);
      else error = readErrorText(r.error);
      return;
    }
    busy = false;
    await openPopover(p.value);
  }

  async function openPopover(p: StartPreview) {
    menuOpen = false;
    place();
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

  function primary(ev?: MouseEvent) {
    if (action === 'open') openLive();
    else if (action === 'continue') void continueFrom(pastLinks[0]);
    else void start(!!ev?.altKey);
  }

  $effect(() => {
    if (!menuOpen && !popover) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (root?.contains(t)) return;
      if ((t as Element).closest?.('[data-testid="start-popover"]')) return;
      menuOpen = false;
      popover = null;
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
      onclick={(e) => primary(e)}>{busy ? '…' : PRIMARY_LABEL[action]}</button
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
    <span class="menu" role="menu" data-testid="work-button-menu-list">
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
  {#if error}<span class="err" role="alert" data-testid="work-button-error">{error}</span>{/if}
</span>

{#if popover}
  <div class="pop-anchor" style:top="{popPos.top}px" style:left="{popPos.left}px">
    <StartPopover base={base} preview={popover} {heading} blocked={startBlocked} onclose={closePopover} onstarted={started} />
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
    box-shadow: 0 6px 18px color-mix(in srgb, #000 16%, transparent);
  }
  .menu button {
    background: none;
    border: 0;
    padding: 4px 8px;
    text-align: left;
    color: var(--fg);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    border-radius: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .menu button:hover:not(:disabled),
  .menu button:focus-visible {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .menu button:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
  .err {
    max-width: 260px;
    color: var(--usage-crit, #c62828);
    font-size: 11px;
    text-align: right;
  }
  .pop-anchor {
    position: fixed;
    z-index: 30;
  }
</style>
