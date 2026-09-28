<script lang="ts">
  // The Review tab of the Work view (work graph M14): link suggestions and
  // conflicts (a cross-org link, an unavailable ticket, a session with work
  // but no primary), each with its why. One at a time — Confirm / Reject /
  // Change… for a suggestion, Keep / Remove for a conflict, Make primary for
  // a session without one — or several at once: tick them and "Confirm n",
  // "Reject n" or "Keep n" sends one `decide_work_batch`, each decision
  // checked on its own; what failed stays, with the hub's sentence. The last
  // confirm / reject can be undone (back to a suggestion).
  //
  // j/k (or ↓/↑) move, y confirms, n rejects, x ticks — from the list only.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { confirmSessionWork, linkSessionWork, rejectWorkLink, unlinkSessionWork } from './work';
  import { workTickets, type TicketRow } from './trackers';
  import { timeAgo } from './session_status';
  import {
    ackWorkLink,
    conflictOf,
    decideWorkBatch,
    openTask,
    readErrorText,
    reconsiderWorkLink,
    reviewKindLabel,
    setPrimaryWork,
    taskLabel,
    undoOf,
    workChanged,
    workReview,
    type BatchDecision,
    type Decision,
    type ReviewItem,
  } from './work_view';
  import type { IpcError, Result } from './result';

  let {
    onchanged,
    /** The refetch debounce, ms; injectable for tests. */
    debounceMs = 500,
  }: { onchanged?: () => void; debounceMs?: number } = $props();

  const blocked = $derived(hubActionBlocked('decide_work_batch', $hubStatus, $hubConnection));

  let items = $state<ReviewItem[]>([]);
  let total = $state(0);
  let cursor = $state<string | null>(null);
  let loaded = $state(false);
  let loadError = $state<IpcError | null>(null);
  let busy = $state(false);
  let picked = $state<Set<string>>(new Set());
  // link id → why the last decision on it failed (kept until it succeeds).
  let failures = $state<Map<number, string>>(new Map());
  let summary = $state<string | null>(null);
  let undo = $state<{ label: string; decisions: BatchDecision[] } | null>(null);
  let focusIdx = $state(0);
  let changing = $state<string | null>(null);

  let loadSeq = 0;
  async function load(more = false) {
    const mine = ++loadSeq;
    const r = await workReview({ cursor: more ? cursor : null, limit: 50 });
    if (mine !== loadSeq) return;
    loaded = true;
    if (!r.ok) {
      loadError = r.error;
      return;
    }
    loadError = null;
    const got = Array.isArray(r.value?.items) ? r.value.items : [];
    items = more ? [...items, ...got.filter((x) => !items.some((y) => y.review_id === x.review_id))] : got;
    total = typeof r.value?.total === 'number' ? r.value.total : items.length;
    cursor = r.value?.next_cursor ?? null;
    // Ticks on items that were decided elsewhere go away.
    picked = new Set([...picked].filter((id) => items.some((x) => x.review_id === id)));
    if (focusIdx >= items.length) focusIdx = Math.max(0, items.length - 1);
  }

  async function reload() {
    await load();
    onchanged?.();
  }

  let timer: ReturnType<typeof setTimeout> | undefined;
  let first = true;
  const off = workChanged.subscribe(() => {
    if (first) {
      first = false;
      return;
    }
    clearTimeout(timer);
    timer = setTimeout(() => void load(), debounceMs);
  });
  onMount(() => void load());
  onDestroy(() => {
    off();
    clearTimeout(timer);
    clearTimeout(changeTimer);
  });

  /** The session has a primary: a confirm here then adds a secondary link
   *  rather than moving the primary (a person moves it with Make primary). */
  function hasPrimary(sessionId: number): boolean {
    return get(sessions).some((r) => r.id === sessionId && r.work != null);
  }

  function sessionName(it: ReviewItem): string {
    return it.session_name ?? `session ${it.session_id}`;
  }

  function fail(linkId: number, e: IpcError) {
    failures = new Map(failures).set(linkId, conflictOf(e) ? 'It changed elsewhere; reloaded — check it again.' : e.message);
  }
  function cleared(linkId: number) {
    if (!failures.has(linkId)) return;
    const m = new Map(failures);
    m.delete(linkId);
    failures = m;
  }

  async function run(it: ReviewItem, what: string, call: () => Promise<Result<unknown>>, undoable?: Decision) {
    if (busy || blocked !== null) return;
    busy = true;
    summary = null;
    const r = await call();
    busy = false;
    if (!r.ok) {
      fail(it.link_id, r.error);
      if (conflictOf(r.error)) await reload();
      return;
    }
    cleared(it.link_id);
    const d = undoable ? undoOf({ session_id: it.session_id, link_id: it.link_id, decision: undoable }) : null;
    undo = d ? { label: `${what} ${taskLabel(it.task)} for ${sessionName(it)}`, decisions: [d] } : null;
    summary = `${what}: ${taskLabel(it.task)} · ${sessionName(it)}`;
    await reload();
  }

  const confirm = (it: ReviewItem) =>
    run(
      it,
      'Confirmed',
      () => confirmSessionWork(it.session_id, it.link_id, { primary: !hasPrimary(it.session_id), expectedVersion: it.link_version }),
      'confirm',
    );
  const reject = (it: ReviewItem) =>
    run(it, 'Rejected', () => rejectWorkLink(it.session_id, it.link_id, { expectedVersion: it.link_version }), 'reject');
  const keep = (it: ReviewItem) => run(it, 'Kept', () => ackWorkLink(it.session_id, it.link_id, it.link_version));
  const remove = (it: ReviewItem) =>
    run(it, 'Removed', () => unlinkSessionWork(it.session_id, it.link_id, { expectedVersion: it.link_version }));
  const makePrimary = (it: ReviewItem) => run(it, 'Made primary', () => setPrimaryWork(it.session_id, it.link_id, 0));

  // ── Change…: the right task instead, then "not this" for the guess ──
  let changeQuery = $state('');
  let changeResults = $state<TicketRow[]>([]);
  let changeTimer: ReturnType<typeof setTimeout> | undefined;
  function onChangeQuery(v: string) {
    changeQuery = v;
    clearTimeout(changeTimer);
    const q = v.trim();
    if (q.length < 2) {
      changeResults = [];
      return;
    }
    changeTimer = setTimeout(async () => {
      const r = await workTickets({ query: q, limit: 8 });
      if (changeQuery.trim() === q) changeResults = r.ok && Array.isArray(r.value) ? r.value : [];
    }, 250);
  }
  function openChange(it: ReviewItem) {
    changing = changing === it.review_id ? null : it.review_id;
    changeQuery = '';
    changeResults = [];
  }
  async function changeTo(it: ReviewItem, to: { link_id?: number | null; item_id?: number; key?: string }) {
    const primary = !hasPrimary(it.session_id);
    await run(it, 'Changed', async () => {
      const linked =
        to.link_id != null
          ? await confirmSessionWork(it.session_id, to.link_id, { primary })
          : await linkSessionWork(it.session_id, to.item_id != null ? { item_id: to.item_id } : { key: to.key ?? '' }, { primary });
      if (!linked.ok) return linked;
      return rejectWorkLink(it.session_id, it.link_id, { expectedVersion: it.link_version });
    });
    changing = null;
  }

  // ── several at once ──
  const pickedItems = $derived(items.filter((x) => picked.has(x.review_id)));
  const pickedSuggestions = $derived(pickedItems.filter((x) => x.kind === 'suggestion'));
  const pickedConflicts = $derived(pickedItems.filter((x) => x.kind === 'cross_org' || x.kind === 'unavailable'));

  function togglePick(it: ReviewItem) {
    const next = new Set(picked);
    if (next.has(it.review_id)) next.delete(it.review_id);
    else next.add(it.review_id);
    picked = next;
  }

  async function batch(decision: Decision, list: ReviewItem[]) {
    if (busy || blocked !== null || list.length === 0) return;
    // Only the first confirm of a session without a primary takes it.
    const tookPrimary = new Set<number>();
    const decisions: BatchDecision[] = list.map((it) => {
      const d: BatchDecision = { session_id: it.session_id, link_id: it.link_id, decision };
      if (it.link_version !== undefined) d.expected_version = it.link_version;
      if (decision === 'confirm') {
        d.primary = !hasPrimary(it.session_id) && !tookPrimary.has(it.session_id);
        if (d.primary) tookPrimary.add(it.session_id);
      }
      return d;
    });
    busy = true;
    summary = null;
    const r = await decideWorkBatch(decisions);
    busy = false;
    if (!r.ok) {
      summary = readErrorText(r.error);
      return;
    }
    const results = Array.isArray(r.value?.results) ? r.value.results : [];
    const byLink = new Map(results.map((x) => [x.link_id, x]));
    const nextFail = new Map(failures);
    const undos: BatchDecision[] = [];
    let ok = 0;
    for (const d of decisions) {
      const res = byLink.get(d.link_id);
      if (res?.ok) {
        ok++;
        nextFail.delete(d.link_id);
        const u = undoOf(d, res.version);
        if (u) undos.push(u);
      } else {
        nextFail.set(d.link_id, res?.message ?? res?.code ?? 'no answer for this item');
      }
    }
    failures = nextFail;
    const failed = decisions.length - ok;
    const verb = decision === 'confirm' ? 'confirmed' : decision === 'reject' ? 'rejected' : 'kept';
    summary = `${ok} ${verb}${failed > 0 ? ` · ${failed} failed` : ''}`;
    undo = undos.length > 0 ? { label: `${undos.length} ${verb}`, decisions: undos } : null;
    // What failed stays ticked, with its reason.
    const failedLinks = new Set(decisions.filter((d) => !byLink.get(d.link_id)?.ok).map((d) => d.link_id));
    picked = new Set(list.filter((x) => failedLinks.has(x.link_id)).map((x) => x.review_id));
    await reload();
  }

  async function runUndo() {
    const u = undo;
    if (!u || busy) return;
    busy = true;
    const r =
      u.decisions.length === 1
        ? await reconsiderWorkLink(u.decisions[0].session_id, u.decisions[0].link_id, u.decisions[0].expected_version)
        : await decideWorkBatch(u.decisions);
    busy = false;
    undo = null;
    summary = r.ok ? `Undone: ${u.label} — back to suggestions` : `Undo failed: ${readErrorText(r.error)}`;
    await reload();
  }

  function openSession(it: ReviewItem) {
    const row = get(sessions).find((r) => r.id === it.session_id);
    if (row) selectSessionExplicitly(row);
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const target = e.target as HTMLElement | null;
    if (target && target !== e.currentTarget && !target.classList.contains('item')) {
      if (!['j', 'k', 'ArrowDown', 'ArrowUp'].includes(e.key)) return;
      if (target.tagName === 'INPUT' || target.tagName === 'SELECT') return;
    }
    const it = items[focusIdx];
    switch (e.key) {
      case 'j':
      case 'ArrowDown':
        focusIdx = Math.min(items.length - 1, focusIdx + 1);
        break;
      case 'k':
      case 'ArrowUp':
        focusIdx = Math.max(0, focusIdx - 1);
        break;
      case 'y':
        if (it?.kind === 'suggestion') void confirm(it);
        else if (it && it.kind !== 'no_primary') void keep(it);
        else if (it) void makePrimary(it);
        break;
      case 'n':
        if (it?.kind === 'suggestion') void reject(it);
        break;
      case 'x':
        if (it) togglePick(it);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<section class="work-review" data-testid="work-review" aria-label="Review">
  {#if blocked}
    <p class="muted" data-testid="work-review-blocked">{blocked}</p>
  {/if}
  {#if summary}
    <div class="summary" role="status" data-testid="work-review-summary">
      <span>{summary}</span>
      {#if undo}
        <button class="btn btn--quiet" type="button" data-testid="work-review-undo" disabled={busy} onclick={() => void runUndo()}
          >Undo</button
        >
      {/if}
    </div>
  {/if}

  {#if pickedItems.length > 0}
    <div class="bulk" role="toolbar" aria-label="Decide the ticked items" data-testid="work-review-bulk">
      <span>{pickedItems.length} ticked</span>
      {#if pickedSuggestions.length > 0}
        <button class="btn btn--primary" type="button" data-testid="work-review-confirm-n" disabled={busy || blocked !== null} onclick={() => void batch('confirm', pickedSuggestions)}
          >Confirm {pickedSuggestions.length}</button
        >
        <button class="btn" type="button" data-testid="work-review-reject-n" disabled={busy || blocked !== null} onclick={() => void batch('reject', pickedSuggestions)}
          >Reject {pickedSuggestions.length}</button
        >
      {/if}
      {#if pickedConflicts.length > 0}
        <button class="btn" type="button" data-testid="work-review-keep-n" disabled={busy || blocked !== null} onclick={() => void batch('ack', pickedConflicts)}
          >Keep {pickedConflicts.length}</button
        >
      {/if}
      <button class="btn btn--quiet" type="button" onclick={() => (picked = new Set())}>clear</button>
    </div>
  {/if}

  {#if loadError}
    <div class="error" role="alert" data-testid="work-review-error">
      <p>{readErrorText(loadError)}</p>
      <button class="btn" type="button" onclick={() => void load()}>Retry</button>
    </div>
  {:else if !loaded}
    <p class="muted" data-testid="work-review-loading">Loading…</p>
  {:else if items.length === 0}
    <p class="muted" data-testid="work-review-empty">Nothing to review.</p>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <ul class="items" tabindex="0" aria-label="Review items (j/k move, y confirm, n reject, x tick)" onkeydown={onKey}>
      {#each items as it, i (it.review_id)}
        <li class="item" class:focused={i === focusIdx} data-testid="work-review-item" data-kind={it.kind} data-link-id={it.link_id}>
          <div class="head">
            <input
              type="checkbox"
              aria-label={`Tick ${taskLabel(it.task)} for ${sessionName(it)}`}
              data-testid="work-review-pick"
              checked={picked.has(it.review_id)}
              disabled={it.kind === 'no_primary'}
              onchange={() => togglePick(it)}
            />
            <span class="kind kind--{it.kind}" data-testid="work-review-kind">{reviewKindLabel(it.kind)}</span>
            <button class="link" type="button" title="Open the task" onclick={() => openTask(it.task.task_id)}>{taskLabel(it.task)}</button>
          </div>
          <div class="sub">
            <button class="link muted" type="button" title="Open the session" onclick={() => openSession(it)}
              >{sessionName(it)}{#if it.host}&nbsp;· {it.host}{/if}</button
            >
            {#if it.strength}<span class="muted">· {it.strength}{#if it.rule}&nbsp;{it.rule}{/if}</span>{/if}
            {#if it.created_at}<span class="muted">· {timeAgo(it.created_at)}</span>{/if}
          </div>
          {#each it.why ?? [] as w, wi (wi)}
            <p class="why" data-testid="work-review-why">{w}</p>
          {/each}
          {#if failures.get(it.link_id)}
            <p class="fail" role="alert" data-testid="work-review-item-error">{failures.get(it.link_id)}</p>
          {/if}
          <div class="actions">
            {#if it.kind === 'suggestion'}
              <button class="btn btn--primary" type="button" data-testid="work-review-confirm" disabled={busy || blocked !== null} onclick={() => void confirm(it)}>Confirm</button>
              <button class="btn" type="button" data-testid="work-review-reject" disabled={busy || blocked !== null} onclick={() => void reject(it)}>Reject</button>
              <button class="btn btn--quiet" type="button" data-testid="work-review-change" aria-expanded={changing === it.review_id} disabled={busy || blocked !== null} onclick={() => openChange(it)}>Change…</button>
            {:else if it.kind === 'no_primary'}
              <button class="btn btn--primary" type="button" data-testid="work-review-make-primary" disabled={busy || blocked !== null} onclick={() => void makePrimary(it)}>Make primary</button>
            {:else}
              <button class="btn" type="button" data-testid="work-review-keep" title="Keep it on purpose" disabled={busy || blocked !== null} onclick={() => void keep(it)}>Keep</button>
              <button class="btn btn--quiet" type="button" data-testid="work-review-remove" disabled={busy || blocked !== null} onclick={() => void remove(it)}>Remove</button>
            {/if}
          </div>
          {#if changing === it.review_id}
            <div class="change" data-testid="work-review-change-panel">
              {#if (it.alternatives ?? []).length > 0}
                <p class="muted">Also suggested:</p>
                {#each it.alternatives ?? [] as alt (alt.task_id)}
                  <button class="btn btn--quiet" type="button" data-testid="work-review-alt" onclick={() => void changeTo(it, { link_id: alt.link_id, key: alt.key ?? undefined })}
                    >{taskLabel(alt)}</button
                  >
                {/each}
              {/if}
              <input
                type="search"
                placeholder="Search a ticket or type a key"
                aria-label="Another task"
                data-testid="work-review-change-query"
                value={changeQuery}
                oninput={(e) => onChangeQuery((e.currentTarget as HTMLInputElement).value)}
              />
              {#each changeResults as t (t.id)}
                <button class="btn btn--quiet" type="button" data-testid="work-review-change-result" onclick={() => void changeTo(it, { item_id: t.id })}
                  >{t.key ? `${t.key} ` : ''}{t.title}</button
                >
              {/each}
              {#if changeQuery.trim()}
                <button class="btn btn--quiet" type="button" data-testid="work-review-change-key" onclick={() => void changeTo(it, { key: changeQuery.trim() })}
                  >Link “{changeQuery.trim()}”</button
                >
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if cursor}
      <button class="btn btn--quiet" type="button" data-testid="work-review-more" onclick={() => void load(true)}
        >Load more ({Math.max(0, total - items.length)} left)</button
      >
    {/if}
  {/if}
</section>

<style>
  .work-review {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    font-size: 0.82rem;
  }
  .items {
    list-style: none;
    margin: 0;
    padding: 0;
    outline: none;
  }
  .items:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .item {
    padding: 0.35rem 0.3rem;
    border-bottom: 1px solid var(--border);
    border-radius: 4px;
  }
  .item.focused {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .head,
  .sub,
  .actions,
  .summary,
  .bulk {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .kind {
    font-size: 0.68rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 0.35rem;
    color: var(--fg-muted);
  }
  .kind--cross_org,
  .kind--unavailable {
    color: var(--usage-warn, #b45309);
    border-color: var(--usage-warn, #b45309);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--fg);
    text-align: left;
    cursor: pointer;
    overflow-wrap: anywhere;
  }
  .link:hover {
    text-decoration: underline;
  }
  .why {
    margin: 0.1rem 0 0 1.4rem;
    color: var(--fg-muted);
    font-size: 0.75rem;
  }
  .fail {
    margin: 0.1rem 0 0 1.4rem;
    color: var(--usage-crit, #c62828);
    font-size: 0.75rem;
  }
  .actions {
    margin: 0.25rem 0 0 1.4rem;
  }
  .change {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    margin: 0.3rem 0 0 1.4rem;
  }
  .change input {
    font: inherit;
    padding: 0.15rem 0.3rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg);
  }
  .change p {
    margin: 0;
  }
  .bulk {
    padding: 0.3rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-pane);
  }
  .muted {
    color: var(--fg-muted);
  }
  .error {
    color: var(--usage-crit, #c62828);
  }
</style>
