<script lang="ts">
  // The session's Tasks (work graph M14): every link of the session's
  // participant — active (the primary ★ first, then secondary ones),
  // suggested, past and rejected — each with its why. A session can work on
  // several tasks; "Make primary" moves which one groups it (a
  // compare-and-set, so two devices cannot silently overwrite each other),
  // "Remove" unlinks one, "Add task…" links another without taking the
  // primary, and "Show in Work view" opens the task there. A write that lost
  // a race (`E_CONFLICT`) reloads and says so.
  import { onDestroy } from 'svelte';
  import type { SessionRow } from './sessions';
  import { orgs } from './orgs';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    confirmSessionWork,
    crossOrgOf,
    crossOrgSentence,
    linkSessionWork,
    rejectWorkLink,
    unlinkSessionWork,
    type WorkRef,
    onWorkChangedDebounced,
  } from './work';
  import { workTickets, type TicketRow } from './trackers';
  import { timeAgo } from './session_status';
  import {
    conflictOf,
    groupSessionLinks,
    isOlderHub,
    setPrimaryWork,
    showTaskInWorkView,
    taskLabel,
    workSessionTasks,
    type SessionTaskLink,
    type SessionTasks,
  } from './work_view';
  import type { IpcError, Result } from './result';

  let { session, debounceMs = 500 }: { session: SessionRow; debounceMs?: number } = $props();

  const linkBlocked = $derived(hubActionBlocked('link_session_work', $hubStatus, $hubConnection));
  const primaryBlocked = $derived(hubActionBlocked('set_primary_work', $hubStatus, $hubConnection));

  let data = $state<SessionTasks | null>(null);
  // An older hub (or a read that failed): the section stays out of the way;
  // the ticket card above still shows the primary work.
  let unsupported = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state(false);

  let seq = 0;
  async function load(id: number) {
    const mine = ++seq;
    const r = await workSessionTasks(id);
    if (mine !== seq) return;
    if (!r.ok) {
      unsupported = isOlderHub(r.error);
      error = unsupported ? null : r.error.message;
      data = null;
      return;
    }
    unsupported = false;
    error = null;
    data = r.value && Array.isArray(r.value.links) ? r.value : null;
  }

  // Reload on another session, and when this one's work changed:
  // `work_rev` moves on any live link's change (a secondary one too).
  const workSig = $derived(
    `${session.id}|${session.work?.link_id ?? ''}|${session.work_suggested?.link_id ?? ''}|${session.work_suggested?.suggestions ?? ''}|${session.work_rev ?? 0}`,
  );
  let loadedSig: string | null = null;
  let loadedId: number | null = null;
  $effect(() => {
    const sig = workSig;
    const id = session.id;
    if (sig === loadedSig) return;
    loadedSig = sig;
    if (id !== loadedId) {
      loadedId = id;
      data = null;
      notice = null;
      adding = false;
    }
    void load(id);
  });

  const off = onWorkChangedDebounced(() => void load(session.id), () => debounceMs);
  onDestroy(() => {
    off();
    clearTimeout(searchTimer);
  });

  const grouped = $derived(groupSessionLinks(data?.links ?? []));
  const primaryId = $derived(data?.primary_link_id ?? null);

  async function act(what: string, call: () => Promise<Result<unknown>>, ok?: string) {
    if (busy) return;
    busy = true;
    notice = null;
    const r = await call();
    busy = false;
    if (!r.ok) {
      notice = conflictOf(r.error)
        ? `${what}: it changed elsewhere (another window or device); reloaded — check it and try again.`
        : `${what}: ${r.error.message}`;
      await load(session.id);
      return;
    }
    if (ok) notice = ok;
    await load(session.id);
  }

  const makePrimary = (l: SessionTaskLink) =>
    act('Make primary', () => setPrimaryWork(session.id, l.link_id, primaryId ?? 0), `${taskLabel(l.task)} is the primary work now`);
  const remove = (l: SessionTaskLink) =>
    act('Remove', () => unlinkSessionWork(session.id, l.link_id, { expectedVersion: l.link_version }));
  const confirm = (l: SessionTaskLink) =>
    act('Confirm', () => confirmSessionWork(session.id, l.link_id, { primary: primaryId == null, expectedVersion: l.link_version }));
  const reject = (l: SessionTaskLink) =>
    act('Not this', () => rejectWorkLink(session.id, l.link_id, { expectedVersion: l.link_version }));

  // ── Add task… ──
  let adding = $state(false);
  let query = $state('');
  let results = $state<TicketRow[]>([]);
  let searching = $state(false);
  let crossOrg = $state<{ ref: WorkRef; label: string; sentence: string } | null>(null);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  function onQuery(v: string) {
    query = v;
    crossOrg = null;
    clearTimeout(searchTimer);
    const q = v.trim();
    if (q.length < 2) {
      results = [];
      return;
    }
    searchTimer = setTimeout(async () => {
      searching = true;
      const r = await workTickets({ query: q, limit: 10 });
      searching = false;
      if (query.trim() === q) results = r.ok && Array.isArray(r.value) ? r.value : [];
    }, 250);
  }

  function orgName(id: number): string | undefined {
    return $orgs.find((o) => o.id === id)?.name;
  }

  // A second task never takes the primary; the first one (a session with no
  // primary) does, so the session is not left with work and no primary.
  async function add(ref: WorkRef, label: string, force = false) {
    if (busy) return;
    busy = true;
    notice = null;
    const r = await linkSessionWork(session.id, ref, { primary: primaryId == null, forceCrossOrg: force });
    busy = false;
    if (!r.ok) {
      const c = crossOrgOf(r.error as IpcError);
      if (c && !force) {
        crossOrg = { ref, label, sentence: crossOrgSentence(label, c, orgName) };
        return;
      }
      notice = `Add task: ${r.error.message}`;
      return;
    }
    crossOrg = null;
    adding = false;
    query = '';
    results = [];
    notice = `Added ${label}`;
    await load(session.id);
  }
</script>

{#snippet row(l: SessionTaskLink, kind: 'active' | 'suggested' | 'past' | 'rejected')}
  <li class="task task--{kind}" class:primary={l.primary} data-testid="session-task" data-kind={kind} data-link-id={l.link_id}>
    <div class="head">
      <span class="mark" aria-hidden="true">{l.primary ? '★' : kind === 'suggested' ? '?' : kind === 'active' ? '○' : '·'}</span>
      <span class="label" class:unavailable={l.task.unavailable}>
        {#if l.task.key}<span class="key">{l.task.key}</span>{/if}
        <span class="title">{l.task.title || (l.task.key ? '' : l.task.task_id)}</span>
      </span>
      {#if l.task.status_name || l.task.status_category}<span class="muted">{l.task.status_name ?? l.task.status_category}</span>{/if}
      {#if l.primary}<span class="tag">primary</span>{/if}
      {#if l.cross_org}<span class="warn">cross-org</span>{/if}
      {#if kind === 'past' && l.ended_at}<span class="muted">ended {timeAgo(l.ended_at)}</span>{/if}
    </div>
    {#if l.why}<p class="why" data-testid="session-task-why">{l.why}</p>{/if}
    <div class="actions">
      {#if kind === 'active' && !l.primary}
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="session-task-make-primary"
          disabled={busy || primaryBlocked !== null}
          title={primaryBlocked ?? 'Group this session under this task (the other tasks stay linked)'}
          onclick={() => void makePrimary(l)}>Make primary</button
        >
      {/if}
      {#if kind === 'active'}
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="session-task-remove"
          disabled={busy || linkBlocked !== null}
          title={linkBlocked ?? 'Unlink this task (not a rejection: it may be suggested again)'}
          onclick={() => void remove(l)}>Remove</button
        >
      {/if}
      {#if kind === 'suggested'}
        <button class="btn btn--quiet" type="button" data-testid="session-task-confirm" disabled={busy || linkBlocked !== null} onclick={() => void confirm(l)}>Confirm</button>
        <button class="btn btn--quiet" type="button" data-testid="session-task-reject" disabled={busy || linkBlocked !== null} onclick={() => void reject(l)}>Not this</button>
      {/if}
      <button class="btn btn--quiet" type="button" data-testid="session-task-show" onclick={() => showTaskInWorkView(l.task.task_id)}>Show in Work view</button>
    </div>
  </li>
{/snippet}

{#if data && !unsupported}
  <section class="block session-tasks" data-testid="session-tasks" aria-label="Tasks">
    <h3>
      Tasks
      <button
        class="btn btn--quiet add"
        type="button"
        data-testid="session-tasks-add"
        aria-expanded={adding}
        disabled={linkBlocked !== null}
        title={linkBlocked ?? 'Link another task to this session'}
        onclick={() => {
          adding = !adding;
          crossOrg = null;
        }}>Add task…</button
      >
    </h3>
    {#if notice}<p class="notice" role="status" data-testid="session-tasks-notice">{notice}</p>{/if}
    {#if adding}
      <div class="add-panel" data-testid="session-tasks-add-panel">
        <input
          type="search"
          placeholder="Search tickets, or type a key"
          aria-label="Task to add"
          data-testid="session-tasks-query"
          value={query}
          oninput={(e) => onQuery((e.currentTarget as HTMLInputElement).value)}
        />
        {#if searching}<p class="muted">Searching…</p>{/if}
        {#each results as t (t.id)}
          <button
            class="btn btn--quiet result"
            type="button"
            data-testid="session-tasks-result"
            disabled={busy}
            onclick={() => void add({ item_id: t.id }, t.key ?? t.title)}>{t.key ? `${t.key} ` : ''}{t.title}</button
          >
        {/each}
        {#if query.trim()}
          <button
            class="btn btn--quiet result"
            type="button"
            data-testid="session-tasks-add-key"
            disabled={busy}
            onclick={() => void add({ key: query.trim() }, query.trim())}>Link “{query.trim()}”</button
          >
        {/if}
        {#if crossOrg}
          <p class="warn" role="alert" data-testid="session-tasks-cross-org">{crossOrg.sentence}</p>
          <button
            class="btn"
            type="button"
            data-testid="session-tasks-force"
            disabled={busy}
            onclick={() => crossOrg && void add(crossOrg.ref, crossOrg.label, true)}
            >Link anyway</button
          >
        {/if}
      </div>
    {/if}

    {#if data.links.length === 0}
      <p class="muted" data-testid="session-tasks-empty">No task yet.</p>
    {/if}
    {#if grouped.active.length > 0}
      <ul class="list">{#each grouped.active as l (l.link_id)}{@render row(l, 'active')}{/each}</ul>
    {/if}
    {#if grouped.suggested.length > 0}
      <h4>Suggested</h4>
      <ul class="list">{#each grouped.suggested as l (l.link_id)}{@render row(l, 'suggested')}{/each}</ul>
    {/if}
    {#if grouped.past.length > 0}
      <h4>Past</h4>
      <ul class="list">{#each grouped.past as l (l.link_id)}{@render row(l, 'past')}{/each}</ul>
    {/if}
    {#if grouped.rejected.length > 0}
      <h4>Rejected</h4>
      <ul class="list">{#each grouped.rejected as l (l.link_id)}{@render row(l, 'rejected')}{/each}</ul>
    {/if}
  </section>
{:else if error}
  <p class="muted" data-testid="session-tasks-error">Tasks: {error}</p>
{/if}

<style>
  h3 {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
  }
  h4 {
    margin: 0.35rem 0 0.1rem;
    font-size: 0.75rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .add {
    margin-left: auto;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .task {
    padding: 0.2rem 0;
    border-bottom: 1px solid var(--border);
  }
  .task--suggested {
    border-bottom-style: dashed;
  }
  .task--past,
  .task--rejected {
    opacity: 0.7;
  }
  .head {
    display: flex;
    gap: 0.35rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .task.primary .mark {
    color: var(--accent);
  }
  .label {
    display: flex;
    gap: 0.3rem;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .label.unavailable {
    text-decoration: line-through;
  }
  .key {
    font-family: var(--mono);
  }
  .why {
    margin: 0 0 0 1.1rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
  .actions {
    display: flex;
    gap: 0.2rem;
    flex-wrap: wrap;
    margin-left: 0.9rem;
  }
  .add-panel {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    margin-bottom: 0.4rem;
  }
  .add-panel input {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg);
  }
  .result {
    text-align: left;
  }
  .notice {
    margin: 0 0 0.3rem;
    font-size: 0.85rem;
  }
  .muted {
    color: var(--fg-muted);
    margin: 0;
  }
  .warn {
    color: var(--usage-warn, #b45309);
  }
</style>
