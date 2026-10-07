<script lang="ts">
  // The session's Tasks (work graph M14): every link of the session's
  // participant — active (the primary ★ first, then secondary ones),
  // suggested, past and rejected — each with its why. A session can work on
  // several tasks; "Make primary" moves which one groups it (a
  // compare-and-set, so two devices cannot silently overwrite each other),
  // "Remove" unlinks one, "Work on task…" links another (a ticket or an own
  // task) without taking the primary, saying first when that task is already
  // open in another session (task → session J4), and "Show in Work view"
  // opens the task there. A write that lost
  // a race (`E_CONFLICT`) reloads and says so.
  import { onDestroy } from 'svelte';
  import type { SessionRow } from './sessions';
  import { orgs } from './orgs';
  import { hubStatus, hubActionBlocked } from './hub';
  import { sessionBlocked } from './share';
  import { hubConnection } from './hub_connection';
  import { get } from 'svelte/store';
  import { sessions } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import {
    confirmSessionWork,
    crossOrgOf,
    crossOrgSentence,
    linkSessionWork,
    liveElsewhereOf,
    type LiveElsewhere,
    rejectWorkLink,
    unlinkSessionWork,
    type WorkRef,
    workChanged,
  } from './work';
  import { workTickets, type TicketRow } from './trackers';
  import { timeAgo } from './session_status';
  import {
    conflictNotice,
    groupSessionLinks,
    isOlderHub,
    setPrimaryWork,
    showTaskInWorkView,
    taskLabel,
    workSessionTasks,
    type ConflictNotice,
    type SessionTaskLink,
    type SessionTasks,
  } from './work_view';
  import WorkConflictNotice from './WorkConflictNotice.svelte';
  import type { IpcError, Result } from './result';

  let { session, debounceMs = 500 }: { session: SessionRow; debounceMs?: number } = $props();

  // Both halves (multi-user M1): the hub's own refusal first, then who this
  // client is on THIS row. `link_session_work` is `drive` in
  // `share.ts::SESSION_TIER`, and `SessionRowItem` has composed exactly these
  // two for the same action since F2 — this panel asked only the hub's half,
  // so the same control answered differently on the two surfaces and a watcher
  // could still link, unlink, confirm or reject the owner's work here.
  const linkBlocked = $derived(
    hubActionBlocked('link_session_work', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'link_session_work'),
  );
  // The same two halves as `linkBlocked` above, for the same reason: Make
  // primary is `set_primary_work`, `drive` in `share.ts::SESSION_TIER` (which
  // of the session's links groups it is a per-session work-graph write, like
  // `link_session_work`). This one line was the last of the file still asking
  // the hub's half alone.
  const primaryBlocked = $derived(
    hubActionBlocked('set_primary_work', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'set_primary_work'),
  );

  let data = $state<SessionTasks | null>(null);
  // An older hub (or a read that failed): the section stays out of the way;
  // the ticket card above still shows the primary work.
  let unsupported = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | ConflictNotice | null>(null);
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

  // One trigger for a re-read of this session: its own work moving (the
  // sig below) and the global tick land in the same debounced `load` — a
  // `session:updated` that moves `work_rev` does both.
  let loadTimer: ReturnType<typeof setTimeout> | undefined;
  function scheduleLoad() {
    clearTimeout(loadTimer);
    loadTimer = setTimeout(() => void load(session.id), debounceMs);
  }

  // Load at once on another session; re-read (debounced) when this one's
  // work changed: `work_rev` moves on any live link's change (a secondary
  // one too), and a session whose links are all secondary has no `work`
  // for the global tick to notice.
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
      clearTimeout(loadTimer);
      void load(id);
      return;
    }
    scheduleLoad();
  });

  // The tick itself, not its debounced run: the timer above is the debounce.
  let firstTick = true;
  const off = workChanged.subscribe(() => {
    if (firstTick) {
      firstTick = false;
      return;
    }
    scheduleLoad();
  });
  onDestroy(() => {
    off();
    clearTimeout(loadTimer);
    clearTimeout(searchTimer);
  });

  const grouped = $derived(groupSessionLinks(data?.links ?? []));
  const primaryId = $derived(data?.primary_link_id ?? null);

  /**
   * Every per-link write in this panel runs through here, and so does its gate
   * (multi-user M1, F2b): the panel's controls are disabled, but a grant can be
   * narrowed while the panel is open, and the control's `disabled` is not the
   * only way into these handlers. `linkBlocked` is `link_session_work`,
   * `drive` — the same tier as `set_primary_work`, `unlink_session_work`,
   * `confirm_session_work` and `reject_session_work`, which is why one answer
   * covers all five: `share.ts`' refusal is a function of the tier.
   */
  async function act(what: string, call: () => Promise<Result<unknown>>, ok?: string) {
    if (busy || linkBlocked !== null) return;
    busy = true;
    notice = null;
    const r = await call();
    busy = false;
    if (!r.ok) {
      // The current value names a primary by its task, as this list shows it.
      const links = data?.links ?? [];
      notice =
        conflictNotice(r.error, `${what}: it`, (id) => {
          const l = links.find((x) => x.link_id === id);
          return l ? taskLabel(l.task) : null;
        }) ?? `${what}: ${r.error.message}`;
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
  /** P-3: the task is already open elsewhere; the person decides. */
  let liveElsewhere = $state<{ ref: WorkRef; label: string; force: boolean; live: LiveElsewhere[] } | null>(null);

  function openSession(id: number) {
    const r = get(sessions).find((x) => x.id === id);
    if (r) selectSessionExplicitly(r);
  }
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  function onQuery(v: string) {
    query = v;
    crossOrg = null;
    liveElsewhere = null;
    clearTimeout(searchTimer);
    const q = v.trim();
    if (q.length < 2) {
      results = [];
      return;
    }
    searchTimer = setTimeout(async () => {
      searching = true;
      // Own tasks too (P-4): "Work on task…" finds TASK-n beside the tickets.
      const r = await workTickets({ query: q, limit: 10, include_local: true });
      searching = false;
      if (query.trim() === q) results = r.ok && Array.isArray(r.value) ? r.value : [];
    }, 250);
  }

  function orgName(id: number): string | undefined {
    return $orgs.find((o) => o.id === id)?.name;
  }

  // A second task never takes the primary; the first one (a session with no
  // primary) does, so the session is not left with work and no primary.
  // The first try asks to be warned when the task is already open in another
  // session (P-3, J4); "Attach anyway" sends it again acknowledged. An older
  // hub ignores the flag and links at once, as it always did.
  async function add(ref: WorkRef, label: string, force = false, ackLive = false) {
    // The Work on task… panel's entrance is gated; the panel is not, and it
    // stays open across a narrowing. Re-asked here (multi-user M1, F2b).
    if (busy || linkBlocked !== null) return;
    busy = true;
    notice = null;
    const r = await linkSessionWork(session.id, ref, { primary: primaryId == null, forceCrossOrg: force, ackLive });
    busy = false;
    if (!r.ok) {
      const c = crossOrgOf(r.error as IpcError);
      if (c && !force) {
        crossOrg = { ref, label, sentence: crossOrgSentence(label, c, orgName) };
        return;
      }
      const live = liveElsewhereOf(r.error as IpcError);
      if (live && live.length > 0 && !ackLive) {
        liveElsewhere = { ref, label, force, live };
        return;
      }
      notice = `Work on task: ${r.error.message}`;
      return;
    }
    liveElsewhere = null;
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
          liveElsewhere = null;
        }}>Work on task…</button
      >
    </h3>
    {#if notice}
      <p class="notice" role="status" data-testid="session-tasks-notice">
        {#if typeof notice === 'string'}{notice}{:else}<WorkConflictNotice notice={notice} onreload={() => void load(session.id)} />{/if}
      </p>
    {/if}
    {#if adding}
      <div class="add-panel" data-testid="session-tasks-add-panel">
        <input
          type="search"
          placeholder="Search tasks and tickets, or type a key"
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
        {#if liveElsewhere}
          {@const le = liveElsewhere}
          <div class="live-elsewhere" role="alert" data-testid="session-tasks-live-elsewhere">
            <p class="warn">{le.label}: {le.live.map((l) => l.message).join(' ')}</p>
            <span class="choices">
              <button
                class="btn"
                type="button"
                data-testid="session-tasks-attach-anyway"
                disabled={busy}
                onclick={() => void add(le.ref, le.label, le.force, true)}>Attach anyway</button
              >
              {#each le.live.filter((l) => l.session_id != null && $sessions.some((r) => r.id === l.session_id)).slice(0, 1) as l (l.session_id)}
                <button
                  class="btn btn--quiet"
                  type="button"
                  data-testid="session-tasks-open-other"
                  onclick={() => l.session_id != null && openSession(l.session_id)}>Open that one</button
                >
              {/each}
              <button
                class="btn btn--quiet"
                type="button"
                data-testid="session-tasks-live-cancel"
                onclick={() => (liveElsewhere = null)}>Cancel</button
              >
            </span>
          </div>
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
  .live-elsewhere p {
    margin: 0 0 0.2rem;
  }
  .choices {
    display: flex;
    gap: 0.2rem;
    flex-wrap: wrap;
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
