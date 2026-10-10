<script lang="ts">
  import { viewKey } from './shortcuts';
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
  import { sessionIdBlocked } from './share';
  import {
    confirmSessionWork,
    linkSessionWork,
    onWorkChangedDebounced,
    rejectWorkLink,
    unlinkSessionWork,
    type WorkRef,
  } from './work';
  import { workTickets, type TicketRow } from './trackers';
  import { shortAge } from './session_status';
  import {
    ackWorkLink,
    conflictNotice,
    decideWorkBatch,
    isHighConfidence,
    openTask,
    readErrorText,
    reconsiderWorkLink,
    reviewKindLabel,
    reviewDuplicateProposal,
    reviewProposal,
    setPrimaryWork,
    taskLabel,
    undoOf,
    workReview,
    workSessionTasks,
    type BatchDecision,
    type BatchResult,
    type ConflictNotice,
    type Decision,
    type ReviewItem,
    type SessionTaskLink,
  } from './work_view';
  import WorkConflictNotice from './WorkConflictNotice.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { preselect } from './ai_proposal';
  import type { IpcError, Result } from './result';

  let {
    onchanged,
    /** The refetch debounce, ms; injectable for tests. */
    debounceMs = 500,
  }: { onchanged?: () => void; debounceMs?: number } = $props();

  const blocked = $derived(hubActionBlocked('decide_work_batch', $hubStatus, $hubConnection));
  /**
   * The access half (multi-user M1, F2a), per ITEM. A `ReviewItem` carries a
   * `session_id` and a name, never the row's `owner_person_id`, so the row has
   * to be resolved before the access half can be asked — the reason this
   * surface was skipped by F2.
   *
   * Every decision here writes per session (`confirmSessionWork`,
   * `rejectWorkLink`, `unlinkSessionWork`, `setPrimaryWork`, and the batch
   * `decide_work_batch`), which `share.ts::SESSION_TIER` puts at `drive`. A
   * list is narrowed per target rather than answered once: one review list
   * mixes this person's sessions with the ones shared with them.
   *
   * ── Not knowing is not permission (F2d) ─────────────────────────────────
   *
   * F2a resolved the row by hand and let an item whose row this client does not
   * hold through — `decidable` ended `|| !rowById.has(it.session_id)` — on the
   * argument that the Review list and `$sessions` load independently. They do,
   * and that is the hazard rather than the excuse: the Review read is the hub's
   * own org-scoped one, so on a paired desktop an item here is not necessarily
   * this person's, and an unresolvable row is *someone else's, or gone*.
   *
   * So it is `share.ts::sessionIdBlocked` that resolves now, once for the four
   * surfaces that had hand-rolled it: it fails closed with
   * `UNKNOWN_SESSION_REASON` on a fleet this client does not own and answers
   * `null` on a standalone desktop, where the master owns everything
   * (`access.ts::sessionAccess` rule 1), so a single-user install is untouched.
   */
  function accessBlocked(it: ReviewItem): string | null {
    return $sessionIdBlocked(it.session_id, 'decide_work_batch');
  }
  /** One item's own gate: the hub's refusal first, then this client's access. */
  function itemBlocked(it: ReviewItem): string | null {
    return blocked ?? accessBlocked(it);
  }
  /** `list` narrowed to the items this client may decide, per target. */
  function decidable(list: readonly ReviewItem[]): ReviewItem[] {
    return list.filter((it) => accessBlocked(it) === null);
  }

  let items = $state<ReviewItem[]>([]);
  let total = $state(0);
  let cursor = $state<string | null>(null);
  let loaded = $state(false);
  let loadError = $state<IpcError | null>(null);
  let busy = $state(false);
  let picked = $state<Set<string>>(new Set());
  // link id → why the last decision on it failed (kept until it succeeds);
  // a conflict carries the current value and Reload.
  let failures = $state<Map<number, string | ConflictNotice>>(new Map());
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

  const off = onWorkChangedDebounced(() => void load(), () => debounceMs);
  onMount(() => void load());
  onDestroy(() => {
    off();
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
    failures = new Map(failures).set(linkId, conflictNotice(e, 'It') ?? e.message);
  }
  function cleared(linkId: number) {
    if (!failures.has(linkId)) return;
    const m = new Map(failures);
    m.delete(linkId);
    failures = m;
  }

  async function run(it: ReviewItem, what: string, call: () => Promise<Result<unknown>>, undoable?: Decision) {
    // Per item, and re-asked at the call: a revoke can arrive while the list
    // is on screen, and y/n decide the focused row without touching a button.
    if (busy || itemBlocked(it) !== null) return;
    busy = true;
    summary = null;
    const r = await call();
    busy = false;
    if (!r.ok) {
      fail(it.link_id, r.error);
      if (r.error.code === 'E_CONFLICT') await reload();
      return;
    }
    cleared(it.link_id);
    summary = `${what}: ${taskLabel(it.task)} · ${sessionName(it)}`;
    undo = null;
    if (undoable) {
      // The Undo is a compare-and-set too (M14.3): it names the version the
      // decision left — answered with the write (`link_version`), else read
      // back from the session's links, where a link someone moved on
      // meanwhile (not in the state this decision left) offers no Undo
      // rather than one that would overwrite them.
      const answered = (r.value as { link_version?: unknown } | null)?.link_version;
      const version = typeof answered === 'number' ? answered : await versionAfter(it, undoable);
      const d = undoOf({ session_id: it.session_id, link_id: it.link_id, decision: undoable }, version);
      if (d?.expected_version != null) undo = { label: `${what} ${taskLabel(it.task)} for ${sessionName(it)}`, decisions: [d] };
    }
    await reload();
  }

  /** Link `it`'s version now, if it is in the state `decision` left it in
   *  (null otherwise, or when it cannot be read). The fallback for a hub
   *  that does not answer the version with the decision. */
  async function versionAfter(it: ReviewItem, decision: Decision): Promise<number | null> {
    const r = await workSessionTasks(it.session_id);
    if (!r.ok) return null;
    const l = (r.value?.links ?? []).find((x) => x.link_id === it.link_id);
    if (!l || typeof l.link_version !== 'number') return null;
    const want = decision === 'confirm' ? ['active', 'confirmed'] : ['rejected'];
    return want.includes(l.state) ? l.link_version : null;
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
  /** A key as the backend keeps it: a ticket key upper-cased, any other
   *  reference trimmed. */
  function normRef(k: string): string {
    const t = k.trim();
    return /^[A-Za-z][A-Za-z0-9_]*-\d+$/.test(t) ? t.toUpperCase() : t;
  }

  /** The session's live link to `to`, if it has one. */
  function linkTo(links: readonly SessionTaskLink[], to: { link_id?: number | null; item_id?: number; key?: string }) {
    const live = links.filter((l) => l.state !== 'ended');
    if (to.link_id != null) return live.find((l) => l.link_id === to.link_id) ?? null;
    if (to.item_id != null) return live.find((l) => l.task?.task_id === `item:${to.item_id}`) ?? null;
    const k = normRef(to.key ?? '');
    return live.find((l) => (l.task?.key != null && normRef(l.task.key) === k) || l.task?.task_id === `ref:${k}`) ?? null;
  }

  /** Change…: link the right task, then "not this" for the guess — as one
   *  change. Both writes name the version they were decided on (the new
   *  task's link as the session's links say now, 0 when it has none); when
   *  the second is refused the first is taken back, so a refused Change…
   *  never leaves the session with both. */
  async function changeTo(it: ReviewItem, to: { link_id?: number | null; item_id?: number; key?: string }) {
    const primary = !hasPrimary(it.session_id);
    await run(it, 'Changed', async () => {
      const before = await workSessionTasks(it.session_id);
      if (!before.ok) return before;
      const had = linkTo(before.value?.links ?? [], to);
      const ref: WorkRef | null = to.item_id != null ? { item_id: to.item_id } : to.key ? { key: to.key } : null;
      let linked: Result<unknown>;
      if (to.link_id != null || !ref) {
        if (to.link_id == null) return { ok: false, error: { code: 'E_INVALID', message: 'nothing to change to' } };
        linked = await confirmSessionWork(it.session_id, to.link_id, { primary, expectedVersion: had?.link_version });
      } else {
        linked = await linkSessionWork(it.session_id, ref, { primary, expectedVersion: had?.link_version ?? 0 });
      }
      if (!linked.ok) return linked;
      const rejected = await rejectWorkLink(it.session_id, it.link_id, { expectedVersion: it.link_version });
      if (rejected.ok) return rejected;
      const back = await takeBack(it.session_id, to, had);
      if (back === null) return rejected;
      return { ok: false, error: { ...rejected.error, message: `${rejected.error.message} (and taking the new link back failed: ${back})` } };
    });
    changing = null;
  }

  /** Undo the first half of a Change…: a link that was a suggestion goes
   *  back to one, a link that did not exist is removed. Null when done,
   *  else why not. */
  async function takeBack(
    sessionId: number,
    to: { link_id?: number | null; item_id?: number; key?: string },
    had: SessionTaskLink | null,
  ): Promise<string | null> {
    if (had && had.state !== 'suggested') return null; // it was already linked: nothing changed
    // `takeBack` is reached from inside `run`'s thunk, so it is not lexically
    // inside the gate `run` asked — and it writes (`reconsider` or `unlink`).
    // Re-asked here with the session's own row (multi-user M1, F2b).
    if ($sessionIdBlocked(sessionId, 'decide_work_batch') !== null) {
      return 'this session is not yours to change';
    }
    const now = await workSessionTasks(sessionId);
    if (!now.ok) return now.error.message;
    const l = linkTo(now.value?.links ?? [], to);
    if (!l) return null;
    const r = had
      ? await reconsiderWorkLink(sessionId, l.link_id, l.link_version)
      : await unlinkSessionWork(sessionId, l.link_id, { expectedVersion: l.link_version });
    return r.ok ? null : r.error.message;
  }

  // ── several at once ──
  const pickedItems = $derived(items.filter((x) => picked.has(x.review_id)));
  // The bulk buttons count what they will actually send (multi-user M1): an
  // item this client may not decide is not ticked in the first place (the
  // checkbox is disabled), and the narrowing holds even so.
  const pickedSuggestions = $derived(decidable(pickedItems.filter((x) => x.kind === 'suggestion')));
  // "Confirm all high-confidence" (redesign 6.5): every suggestion this
  // client may decide whose detection confidence clears the bar. It is still
  // a person's click, with the batch's Undo.
  const highConfidence = $derived(decidable(items.filter(isHighConfidence)));
  const pickedConflicts = $derived(
    decidable(pickedItems.filter((x) => x.kind === 'cross_org' || x.kind === 'unavailable')),
  );

  function togglePick(it: ReviewItem) {
    // x ticks the focused row from the keyboard, past the checkbox's own
    // `disabled`, so the gate is here as well.
    if (itemBlocked(it) !== null) return;
    const next = new Set(picked);
    if (next.has(it.review_id)) next.delete(it.review_id);
    else next.add(it.review_id);
    picked = next;
  }

  async function batch(decision: Decision, list: ReviewItem[]) {
    if (busy || blocked !== null) return;
    // Narrowed per target, not answered once for the batch: `decide_work_batch`
    // writes one decision per session, so a list mixing this person's sessions
    // with ones shared at `watch` sends only the former.
    list = decidable(list);
    if (list.length === 0) return;
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
    const { ok, failed, byLink } = countBatch(decisions, r.value);
    const undos: BatchDecision[] = [];
    for (const d of decisions) {
      const res = byLink.get(d.link_id);
      // Only a decision whose new version is known can be undone safely.
      const u = res?.ok && res.version != null ? undoOf(d, res.version) : null;
      if (u) undos.push(u);
    }
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
    // Narrowed per target, then re-asked (multi-user M1, F2b): the Undo offer
    // outlives the batch that made it, so a grant narrowed while it is on
    // screen must not be undone through it. Every decision in `u` is a write to
    // one session, so each is checked against that session's own row.
    const mine = u.decisions.filter(
      (d) => $sessionIdBlocked(d.session_id, 'decide_work_batch') === null,
    );
    if (mine.length === 0) {
      undo = null;
      summary = 'Nothing left to undo: those sessions are not yours any more.';
      return;
    }
    busy = true;
    if (mine.length === 1) {
      const d = mine[0];
      const r = await reconsiderWorkLink(d.session_id, d.link_id, d.expected_version);
      busy = false;
      undo = null;
      if (r.ok) {
        summary = `Undone: ${u.label} — back to suggestions`;
      } else {
        summary = `Undo failed: ${readErrorText(r.error)}`;
        fail(d.link_id, r.error);
      }
      await reload();
      return;
    }
    const r = await decideWorkBatch(mine);
    busy = false;
    undo = null;
    if (!r.ok) {
      summary = `Undo failed: ${readErrorText(r.error)}`;
      await reload();
      return;
    }
    // Each undo is checked on its own, like the batch it undoes.
    const { ok, failed } = countBatch(mine, r.value);
    summary =
      failed === 0
        ? `Undone: ${u.label} — back to suggestions`
        : `Undone: ${ok} of ${u.decisions.length} — ${failed} changed elsewhere or failed`;
    await reload();
  }

  /** How many decisions of a batch answer succeeded; the failures are kept
   *  per link with their reason. */
  function countBatch(decisions: readonly BatchDecision[], value: BatchResult | null | undefined) {
    const results = Array.isArray(value?.results) ? value.results : [];
    const byLink = new Map(results.map((x) => [x.link_id, x]));
    const nextFail = new Map(failures);
    let ok = 0;
    for (const d of decisions) {
      const res = byLink.get(d.link_id);
      if (res?.ok) {
        ok++;
        nextFail.delete(d.link_id);
      } else {
        nextFail.set(d.link_id, res?.message ?? res?.code ?? 'no answer for this item');
      }
    }
    failures = nextFail;
    return { ok, failed: decisions.length - ok, byLink };
  }

  function openSession(it: ReviewItem) {
    const row = get(sessions).find((r) => r.id === it.session_id);
    if (row) selectSessionExplicitly(row);
  }

  function onKey(e: KeyboardEvent) {
    // The keys are the registry's `work-review` rows (step 0.1).
    const act = viewKey('work-review', e);
    if (!act) return;
    const target = e.target as HTMLElement | null;
    if (target && target !== e.currentTarget && !target.classList.contains('item')) {
      if (act !== 'work-review.down' && act !== 'work-review.up') return;
      if (target.tagName === 'INPUT' || target.tagName === 'SELECT') return;
    }
    const it = items[focusIdx];
    switch (act) {
      case 'work-review.down':
        focusIdx = Math.min(items.length - 1, focusIdx + 1);
        break;
      case 'work-review.up':
        focusIdx = Math.max(0, focusIdx - 1);
        break;
      case 'work-review.yes':
        if (it?.kind === 'suggestion') void confirm(it);
        else if (it && it.kind !== 'no_primary') void keep(it);
        else if (it) void makePrimary(it);
        break;
      case 'work-review.no':
        if (it?.kind === 'suggestion') void reject(it);
        break;
      case 'work-review.pick':
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

  {#if highConfidence.length > 0 && pickedItems.length === 0}
    <div class="bulk" role="toolbar" aria-label="Confirm the high-confidence suggestions" data-testid="work-review-high">
      <button class="btn" type="button" data-testid="work-review-confirm-high" disabled={busy || blocked !== null} onclick={() => void batch('confirm', highConfidence)}
        >Confirm all high-confidence ({highConfidence.length})</button
      >
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
        {@const mine = itemBlocked(it)}
        <li class="item" class:focused={i === focusIdx} data-testid="work-review-item" data-kind={it.kind} data-link-id={it.link_id}>
          <div class="head">
            <input
              type="checkbox"
              aria-label={`Tick ${taskLabel(it.task)} for ${sessionName(it)}`}
              data-testid="work-review-pick"
              checked={picked.has(it.review_id)}
              disabled={it.kind === 'no_primary' || mine !== null}
              title={mine ?? ''}
              onchange={() => togglePick(it)}
            />
            <span class="kind kind--{it.kind}" data-testid="work-review-kind">{reviewKindLabel(it.kind)}</span>
            <button class="link" type="button" title="Open the task" onclick={() => openTask(it.task.task_id, [{ session_id: it.session_id }])}>{taskLabel(it.task)}</button>
          </div>
          {#if it.kind === 'suggestion' && it.proposed_by}
            <!-- Redesign 6.8: the decision model's suggestion (J1) says so,
                 with its reason; Change opens the same panel as Change…. -->
            <ProposedBy
              proposal={reviewProposal(it)}
              field="work_link"
              testid="work-review-proposed-by"
              onchange={mine === null && !busy ? () => openChange(it) : undefined}
            />
          {/if}
          {#if it.kind === 'suggestion' && it.duplicate_of && preselect('tracker_duplicate', reviewDuplicateProposal(it)) != null}
            {@const dup = it.duplicate_of}
            {@const dupLabel = dup.key ?? taskLabel({ task_id: dup.task_id, title: dup.title })}
            <!-- Redesign 6.8, J7: this local task may be the same work as a
                 tracker ticket. Linking the ticket instead is a person's
                 click (the same Change… as above); nothing merges. -->
            <div class="dup" data-testid="work-review-duplicate">
              <span>May duplicate {dupLabel}</span>
              <ProposedBy
                proposal={reviewDuplicateProposal(it)}
                field="tracker_duplicate"
                testid="work-review-duplicate-proposed-by"
                onchange={mine === null && !busy ? () => openChange(it) : undefined}
              />
              <button
                class="btn btn--quiet"
                type="button"
                data-testid="work-review-duplicate-link"
                disabled={busy || mine !== null}
                title={mine ?? ''}
                onclick={() => void changeTo(it, { item_id: dup.item_id })}>Link {dupLabel} instead</button
              >
            </div>
          {/if}
          <div class="sub">
            <button class="link muted" type="button" title="Open the session" onclick={() => openSession(it)}
              >{sessionName(it)}{#if it.host}&nbsp;· {it.host}{/if}</button
            >
            {#if it.confidence != null}<span class="conf" class:conf--high={isHighConfidence(it)} title="Confidence from detection" data-testid="work-review-confidence">{it.confidence}%</span>{/if}
            {#if it.strength}<span class="muted">· {it.strength}{#if it.rule}&nbsp;{it.rule}{/if}</span>{/if}
            {#if it.created_at}<span class="muted">· {shortAge(it.created_at)}</span>{/if}
          </div>
          {#each it.why ?? [] as w, wi (wi)}
            <p class="why" data-testid="work-review-why">{w}</p>
          {/each}
          {#if failures.get(it.link_id)}
            {@const f = failures.get(it.link_id)}
            <p class="fail" role="alert" data-testid="work-review-item-error">
              {#if typeof f === 'string'}{f}{:else if f}<WorkConflictNotice notice={f} onreload={() => void reload()} />{/if}
            </p>
          {/if}
          <div class="actions">
            {#if mine}
              <p class="muted" data-testid="work-review-item-not-mine">{mine}</p>
            {/if}
            {#if it.kind === 'suggestion'}
              <button class="btn btn--primary" type="button" data-testid="work-review-confirm" disabled={busy || mine !== null} title={mine ?? ''} onclick={() => void confirm(it)}>Confirm</button>
              <button class="btn" type="button" data-testid="work-review-reject" disabled={busy || mine !== null} title={mine ?? ''} onclick={() => void reject(it)}>Reject</button>
              <button class="btn btn--quiet" type="button" data-testid="work-review-change" aria-expanded={changing === it.review_id} disabled={busy || mine !== null} title={mine ?? ''} onclick={() => openChange(it)}>Change…</button>
            {:else if it.kind === 'no_primary'}
              <button class="btn btn--primary" type="button" data-testid="work-review-make-primary" disabled={busy || mine !== null} title={mine ?? ''} onclick={() => void makePrimary(it)}>Make primary</button>
            {:else}
              <button class="btn" type="button" data-testid="work-review-keep" title={mine ?? 'Keep it on purpose'} disabled={busy || mine !== null} onclick={() => void keep(it)}>Keep</button>
              <button class="btn btn--quiet" type="button" data-testid="work-review-remove" disabled={busy || mine !== null} title={mine ?? ''} onclick={() => void remove(it)}>Remove</button>
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
    font-size: var(--text-2xs);
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
    border-radius: var(--radius-sm);
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
    font-size: var(--text-2xs);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 0 0.35rem;
    color: var(--fg-muted);
  }
  .kind--cross_org,
  .kind--unavailable {
    color: var(--usage-warn);
    border-color: var(--usage-warn);
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
    font-size: var(--text-2xs);
  }
  .fail {
    margin: 0.1rem 0 0 1.4rem;
    color: var(--usage-crit);
    font-size: var(--text-2xs);
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
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .change p {
    margin: 0;
  }
  .bulk {
    padding: 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
  }
  .muted {
    color: var(--fg-muted);
  }
  .conf {
    font-variant-numeric: tabular-nums;
    color: var(--fg-muted);
  }
  .conf--high {
    color: var(--fg);
  }
  .error {
    color: var(--usage-crit);
  }
  .dup {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
</style>
