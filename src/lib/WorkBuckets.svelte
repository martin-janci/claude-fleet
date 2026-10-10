<script lang="ts">
  // Sprints and releases (sprints design 2026-09-28 §5, §6): list them with
  // their roll-ups, create one, start a planned sprint, release a planned
  // release, close a sprint (E9: a person confirms which unfinished tasks
  // carry over, all of them preselected) and delete one. Every write is
  // `work_admin`, so a paired desktop shows why it cannot and leaves the
  // list readable; putting tasks in a sprint is the Work view's selection.
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import Skeleton from './states/Skeleton.svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { readErrorText, workTreeMeta } from './work_view';
  import {
    advanceBucket,
    bucketSummary,
    closeSprint,
    createBucket,
    deleteBucket,
    openBuckets,
    workBucket,
    workBuckets,
    type BucketKind,
    type BucketMemberRow,
    type BucketRow,
  } from './work_buckets';

  let {
    onclose,
    onchanged,
    closeId,
  }: {
    onclose: () => void;
    onchanged?: () => void;
    /** Open straight on closing this sprint (the board's *Close sprint…*);
     *  the dialog closes with it. */
    closeId?: number;
  } = $props();

  const adminBlocked = $derived(hubActionBlocked('work_bucket_admin', $hubStatus, $hubConnection));
  const orgs = $derived($workTreeMeta?.orgs ?? []);

  let buckets = $state<BucketRow[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state(false);
  let showClosed = $state(false);

  // The new bucket's form.
  let creating = $state(false);
  let kind = $state<BucketKind>('sprint');
  let name = $state('');
  let orgId = $state<number | null>(null);
  let startsOn = $state('');
  let endsOn = $state('');
  let goal = $state('');

  // Closing a sprint: its unfinished tasks, which of them carry, and where.
  let closing = $state<BucketRow | null>(null);
  let unfinished = $state<BucketMemberRow[]>([]);
  let carry = $state<Set<number>>(new Set());
  let carryTo = $state<number | null>(null);
  let closeLoading = $state(false);

  let confirmDelete = $state<number | null>(null);

  const shown = $derived(buckets.filter((b) => showClosed || b.state !== 'closed'));
  const closedCount = $derived(buckets.filter((b) => b.state === 'closed').length);
  const orgName = (id: number | null | undefined) => (id == null ? 'Unassigned' : (orgs.find((o) => o.id === id)?.name ?? `Org ${id}`));

  async function load() {
    const r = await workBuckets();
    loaded = true;
    if (r.ok) {
      buckets = Array.isArray(r.value) ? r.value : [];
      error = null;
    } else {
      error = readErrorText(r.error);
    }
  }
  onMount(async () => {
    await load();
    const b = closeId != null ? buckets.find((x) => x.id === closeId && x.kind === 'sprint' && x.state !== 'closed') : undefined;
    if (b) void startClose(b);
  });

  function changed(msg: string) {
    notice = msg;
    onchanged?.();
    void load();
  }

  async function create(e: Event) {
    e.preventDefault();
    if (!name.trim() || busy) return;
    busy = true;
    const r = await createBucket({ kind, name, org_id: orgId, starts_on: startsOn, ends_on: endsOn, goal });
    busy = false;
    if (!r.ok) {
      notice = readErrorText(r.error);
      return;
    }
    creating = false;
    name = '';
    startsOn = '';
    endsOn = '';
    goal = '';
    changed(r.value.warning ? `Created “${r.value.bucket.name}”. ${r.value.warning}.` : `Created “${r.value.bucket.name}”.`);
  }

  async function advance(b: BucketRow) {
    busy = true;
    const r = await advanceBucket(b);
    busy = false;
    if (!r.ok) {
      notice = readErrorText(r.error);
      return;
    }
    const verb = b.kind === 'sprint' ? 'Started' : 'Released';
    changed(r.value.warning ? `${verb} “${b.name}”. ${r.value.warning}.` : `${verb} “${b.name}”.`);
  }

  async function startClose(b: BucketRow) {
    closing = b;
    closeLoading = true;
    unfinished = [];
    const r = await workBucket(b.id);
    closeLoading = false;
    if (!r.ok) {
      notice = readErrorText(r.error);
      closing = null;
      return;
    }
    unfinished = (r.value.members ?? []).filter((m) => m.removed_at == null && m.item.status_category !== 'done');
    // E9: everything unfinished is preselected; the next open sprint of the
    // same org is the default target.
    carry = new Set(unfinished.map((m) => m.item.id));
    carryTo = targets(b)[0]?.id ?? null;
  }

  /** Sprints the closing one may carry to: open, same org, not itself. */
  function targets(b: BucketRow): BucketRow[] {
    return openBuckets(buckets, 'sprint').filter((o) => o.id !== b.id && (o.org_id ?? null) === (b.org_id ?? null));
  }

  function toggleCarry(id: number) {
    const next = new Set(carry);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    carry = next;
  }

  async function confirmClose() {
    const b = closing;
    if (!b) return;
    busy = true;
    const r = await closeSprint(b, carryTo, [...carry]);
    busy = false;
    if (!r.ok) {
      notice = readErrorText(r.error);
      return;
    }
    closing = null;
    if (closeId != null) {
      onchanged?.();
      onclose();
      return;
    }
    const to = r.value.carry_to != null ? buckets.find((x) => x.id === r.value.carry_to)?.name : null;
    const n = r.value.carried.length;
    changed(to && n > 0 ? `Closed “${b.name}”; ${n} task${n === 1 ? '' : 's'} carried to “${to}”.` : `Closed “${b.name}”.`);
  }

  async function remove(b: BucketRow) {
    busy = true;
    const r = await deleteBucket(b);
    busy = false;
    confirmDelete = null;
    if (!r.ok) {
      notice = readErrorText(r.error);
      return;
    }
    changed(`Deleted “${b.name}”.`);
  }
</script>

<Modal title="Sprints & releases" {onclose} width="600px" testid="work-buckets">
  <div class="buckets">
    {#if closing}
      {@const options = targets(closing)}
      <section class="close" data-testid="bucket-close">
        <h4>Close “{closing.name}”</h4>
        {#if closeLoading}
          <Skeleton rows={2} label="Loading its tasks" />
        {:else if unfinished.length === 0}
          <p class="muted">Every task in it is done.</p>
        {:else}
          <label class="field">
            <span>Carry unfinished tasks to</span>
            <select bind:value={carryTo} data-testid="bucket-carry-to">
              {#each options as o (o.id)}
                <option value={o.id}>{o.name} ({o.state})</option>
              {/each}
              <option value={null}>No sprint (leave them unplanned)</option>
            </select>
          </label>
          {#if carryTo != null}
            <ul class="carry" aria-label="Unfinished tasks">
              {#each unfinished as m (m.item.id)}
                <li>
                  <label>
                    <input type="checkbox" checked={carry.has(m.item.id)} onchange={() => toggleCarry(m.item.id)} data-testid="bucket-carry-item" />
                    {#if m.item.key}<span class="key">{m.item.key}</span>{/if}
                    <span>{m.item.title}</span>
                  </label>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="muted">{unfinished.length} unfinished task{unfinished.length === 1 ? '' : 's'} will have no sprint. The closed sprint still records them.</p>
          {/if}
        {/if}
        <div class="actions">
          <button class="btn btn--primary" type="button" data-testid="bucket-close-confirm" disabled={busy || closeLoading} onclick={() => void confirmClose()}
            >Close sprint</button
          >
          <button class="btn btn--quiet" type="button" onclick={() => (closeId != null ? onclose() : (closing = null))}>Cancel</button>
        </div>
      </section>
    {:else}
      <div class="bar">
        <button
          class="btn"
          type="button"
          data-testid="bucket-new"
          disabled={adminBlocked !== null}
          title={adminBlocked ?? 'A new sprint or release'}
          onclick={() => (creating = !creating)}>New…</button
        >
        {#if closedCount > 0}
          <label class="muted toggle">
            <input type="checkbox" bind:checked={showClosed} data-testid="bucket-show-closed" /> Show closed ({closedCount})
          </label>
        {/if}
      </div>
      {#if adminBlocked}
        <p class="muted" data-testid="bucket-admin-blocked">{adminBlocked}</p>
      {/if}
      {#if creating}
        <form class="create" onsubmit={create} data-testid="bucket-create">
          <div class="seg" role="radiogroup" aria-label="Kind">
            <label><input type="radio" bind:group={kind} value="sprint" /> Sprint</label>
            <label><input type="radio" bind:group={kind} value="release" /> Release</label>
          </div>
          <label class="field">
            <span>Name</span>
            <!-- svelte-ignore a11y_autofocus -->
            <input type="text" bind:value={name} maxlength="120" required autofocus placeholder={kind === 'sprint' ? 'Sprint 24' : '0.3.0'} data-testid="bucket-name" />
          </label>
          {#if orgs.length > 0}
            <label class="field">
              <span>Organisation</span>
              <select bind:value={orgId} data-testid="bucket-org">
                <option value={null}>Unassigned</option>
                {#each orgs as o (o.id)}
                  <option value={o.id}>{o.name}</option>
                {/each}
              </select>
            </label>
          {/if}
          <div class="dates">
            {#if kind === 'sprint'}
              <label class="field"><span>Starts</span><input type="date" bind:value={startsOn} data-testid="bucket-starts" /></label>
            {/if}
            <label class="field"><span>{kind === 'sprint' ? 'Ends' : 'Target date'}</span><input type="date" bind:value={endsOn} data-testid="bucket-ends" /></label>
          </div>
          <label class="field">
            <span>Goal</span>
            <input type="text" bind:value={goal} maxlength="500" placeholder="Optional" data-testid="bucket-goal" />
          </label>
          <div class="actions">
            <button class="btn btn--primary" type="submit" disabled={busy || !name.trim()} data-testid="bucket-create-save">Create</button>
            <button class="btn btn--quiet" type="button" onclick={() => (creating = false)}>Cancel</button>
          </div>
        </form>
      {/if}
      {#if notice}
        <p class="notice" role="status" data-testid="buckets-notice">{notice}</p>
      {/if}
      {#if error}
        <p class="err" role="alert" data-testid="buckets-error">{error}</p>
      {:else if !loaded}
        <Skeleton />
      {:else if shown.length === 0}
        <p class="muted" data-testid="buckets-empty">No sprints or releases yet. Create one, then select tasks in the Work view to plan them in.</p>
      {:else}
        <ul class="list">
          {#each shown as b (b.id)}
            <li data-testid="bucket-row" data-kind={b.kind} class:closed={b.state === 'closed'}>
              <div class="main">
                <span class="name"><span class="kind">{b.kind === 'sprint' ? 'Sprint' : 'Release'}</span> {b.name}</span>
                <span class="muted">{bucketSummary(b)}{orgs.length > 0 ? ` · ${orgName(b.org_id)}` : ''}</span>
                {#if b.goal}<span class="muted goal">{b.goal}</span>{/if}
              </div>
              <div class="actions">
                {#if b.state === 'planned'}
                  <button class="btn btn--quiet" type="button" data-testid="bucket-advance" disabled={busy || adminBlocked !== null} title={adminBlocked ?? undefined} onclick={() => void advance(b)}
                    >{b.kind === 'sprint' ? 'Start' : 'Mark released'}</button
                  >
                {/if}
                {#if b.kind === 'sprint' && b.state !== 'closed'}
                  <button class="btn btn--quiet" type="button" data-testid="bucket-close-open" disabled={busy || adminBlocked !== null} title={adminBlocked ?? undefined} onclick={() => void startClose(b)}
                    >Close…</button
                  >
                {/if}
                {#if confirmDelete === b.id}
                  <button class="btn btn--crit" type="button" data-testid="bucket-delete-confirm" disabled={busy} onclick={() => void remove(b)}>Delete</button>
                  <button class="btn btn--quiet" type="button" onclick={() => (confirmDelete = null)}>Keep</button>
                {:else}
                  <button
                    class="btn btn--quiet"
                    type="button"
                    data-testid="bucket-delete"
                    disabled={adminBlocked !== null}
                    title={adminBlocked ?? 'Delete it; its tasks stay, with no sprint or release'}
                    onclick={() => (confirmDelete = b.id)}>Delete…</button
                  >
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
</Modal>

<style>
  .buckets { display: flex; flex-direction: column; gap: 0.5rem; font-size: var(--text-xs); }
  .bar { display: flex; gap: 0.6rem; align-items: center; }
  .toggle { display: inline-flex; gap: 0.25rem; align-items: center; }
  .create, .close {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .close h4 { margin: 0; font-size: var(--text-sm); }
  .seg { display: flex; gap: 0.8rem; }
  .field { display: flex; flex-direction: column; gap: 0.15rem; }
  .field span { color: var(--fg-muted); }
  .field input, .field select {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .dates { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .actions { display: flex; gap: 0.25rem; flex: 0 0 auto; flex-wrap: wrap; }
  ul { list-style: none; margin: 0; padding: 0; }
  .list li { display: flex; gap: 0.5rem; align-items: center; justify-content: space-between; padding: 0.3rem 0; border-bottom: 1px solid var(--border); }
  .list li.closed .name { color: var(--fg-muted); }
  .carry { max-height: 14rem; overflow: auto; }
  .carry li label { display: flex; gap: 0.35rem; align-items: baseline; padding: 0.1rem 0; }
  .key { font-family: var(--font-mono); color: var(--fg-muted); }
  .main { display: flex; flex-direction: column; min-width: 0; }
  .name { font-weight: 600; overflow-wrap: anywhere; }
  .kind { font-weight: 400; color: var(--fg-muted); }
  .goal { font-style: italic; }
  .muted { color: var(--fg-muted); margin: 0; }
  .notice { margin: 0; }
  .err { color: var(--danger); margin: 0; }
</style>
