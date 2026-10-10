<script lang="ts">
  // "Assign org…" (work graph M14): move a LOCAL task to another
  // organisation. The org is the access boundary, so this is a security
  // change: the hub first says exactly what would change (`org_impact`) —
  // which links become cross-org, which hosts and org-bound clients stop or
  // start seeing the task, how much journal and how many summaries move —
  // and the move is sent only with that preview's token. When the impact
  // changed in the meantime the hub refuses (`E_CONFLICT`) and the new
  // impact is shown to be confirmed again.
  //
  // Gap plan G2.2: picking the target reads its impact at once, so the
  // impact list IS the confirm ("Move to Papaya"), and it names who loses or
  // gains access (the people whose org-bound devices do) and where the
  // task's spend stays.
  import Modal from './Modal.svelte';
  import { orgs as orgStore } from './orgs';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    assignWorkOrg,
    conflictOf,
    readErrorText,
    taskLabel,
    taskSpend,
    workOrgImpact,
    workTreeMeta,
    type OrgImpact,
    type WorkTask,
  } from './work_view';

  let {
    task,
    onclose,
    ondone,
  }: {
    task: WorkTask;
    onclose: () => void;
    ondone?: (t: WorkTask) => void;
  } = $props();

  const blocked = $derived(hubActionBlocked('assign_work_org', $hubStatus, $hubConnection));

  const orgOptions = $derived.by(() => {
    const m = new Map<number, string>();
    for (const o of $workTreeMeta.orgs) m.set(o.id, o.name);
    for (const o of $orgStore) if (!m.has(o.id)) m.set(o.id, o.name);
    return [...m].map(([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name));
  });
  function orgName(id: number | null | undefined): string {
    if (id == null || id === 0) return 'no organisation';
    return orgOptions.find((o) => o.id === id)?.name ?? `organisation ${id}`;
  }

  // The task's own org: only a fenced one (tracker / item) is a boundary to
  // move FROM. An unfenced task's org is inferred from its sessions — the
  // item itself has none.
  const currentOrg = $derived(task.org_fenced ? (task.org_id ?? null) : null);
  const inferredOrg = $derived(!task.org_fenced && task.org_id != null ? task.org_id : null);

  let target = $state<string>('');
  // The impact and the target it was read for: an impact answers for one
  // target only, so a changed choice hides it and Confirm waits for a new one.
  let fetched = $state<{ impact: OrgImpact; target: number } | null>(null);
  let busy = $state(false);
  let failure = $state<string | null>(null);
  let changed = $state(false);

  const targetId = $derived(target === '' ? null : Number(target));
  const impact = $derived(fetched && fetched.target === targetId ? fetched.impact : null);

  let reviewSeq = 0;
  async function review() {
    const to = targetId;
    if (to === null || busy) return;
    const mine = ++reviewSeq;
    busy = true;
    failure = null;
    const r = await workOrgImpact(task.task_id, to);
    if (mine !== reviewSeq) return;
    busy = false;
    if (!r.ok) {
      fetched = null;
      failure = readErrorText(r.error);
      return;
    }
    fetched = { impact: r.value, target: to };
  }

  function onTarget() {
    // A review in flight answers for the old target: drop it, and read the
    // new target's.
    reviewSeq++;
    busy = false;
    fetched = null;
    changed = false;
    failure = null;
    void review();
  }

  const spend = $derived(taskSpend(task));

  async function confirm() {
    const imp = impact;
    if (!imp || !imp.allowed || !imp.impact_token || targetId === null || busy || blocked !== null) return;
    busy = true;
    failure = null;
    const r = await assignWorkOrg(task.task_id, targetId, imp.impact_token);
    busy = false;
    if (!r.ok) {
      if (conflictOf(r.error)) {
        // Something moved since the preview: show the new impact, and ask
        // again. Never send the old token twice.
        changed = true;
        fetched = null;
        await review();
        return;
      }
      failure = r.error.message;
      return;
    }
    ondone?.(r.value);
    onclose();
  }

  function reasonText(reason: string | null | undefined): string {
    if (reason === 'tracker_controlled') {
      return "A tracker's task belongs to its tracker's organisation. The fleet's administrator moves the whole tracker (work_admin assign_tracker, master token).";
    }
    if (reason === 'bare_key') {
      return 'A bare key has no item to carry an organisation; name it as local work first ("Name this work…").';
    }
    if (reason === 'same_org') return 'The task is already in that organisation.';
    return reason ?? 'The hub does not allow this move.';
  }

  function plural(n: number, one: string, many = `${one}s`): string {
    return `${n} ${n === 1 ? one : many}`;
  }
</script>

<Modal title="Assign organisation" {onclose} width="520px" testid="work-org-dialog">
  <div class="form">
    <p class="what">{taskLabel(task)}</p>
    <p class="note">
      Now: <strong>{orgName(currentOrg)}</strong>{#if inferredOrg != null}
        (its sessions are in {orgName(inferredOrg)} — inferred, not a boundary){/if}. The organisation is a boundary:
      hosts and clients bound to an organisation see only its work.
    </p>
    <label class="field">
      <span>Move to</span>
      <select
        bind:value={target}
        data-testid="work-org-target"
        onchange={onTarget}
      >
        <option value="" disabled>Pick an organisation…</option>
        {#each orgOptions as o (o.id)}
          {#if o.id !== currentOrg}<option value={String(o.id)}>{o.name}</option>{/if}
        {/each}
        {#if currentOrg != null}<option value="0">No organisation</option>{/if}
      </select>
    </label>

    {#if changed}
      <p class="warn" role="alert" data-testid="work-org-changed">The impact changed since you reviewed it — review it again.</p>
    {/if}

    {#if impact}
      <div class="impact" data-testid="work-org-impact">
        <p>From <strong>{orgName(impact.from_org)}</strong> to <strong>{orgName(impact.to_org)}</strong>.</p>
        {#if !impact.allowed}
          <p class="warn" data-testid="work-org-refused">{reasonText(impact.reason)}</p>
        {:else}
          {#if impact.links.length > 0}
            <p>Its sessions ({impact.links.length}):</p>
            <ul>
              {#each impact.links as l (l.link_id)}
                <li data-testid="work-org-impact-link">
                  {l.name ?? `session ${l.session_id ?? l.link_id}`}{#if l.host}&nbsp;· {l.host}{/if} · {l.state}
                  {#if l.becomes_cross_org}<span class="warn" data-testid="work-org-cross"> · becomes cross-org (a review item until kept or removed)</span>{/if}
                </li>
              {/each}
            </ul>
          {:else}
            <p>No session is linked to it.</p>
          {/if}
          <ul class="facts">
            <li data-testid="work-org-hosts-losing">
              Hosts that stop seeing it: {impact.hosts_losing.length > 0 ? impact.hosts_losing.join(', ') : 'none'}
            </li>
            <li data-testid="work-org-hosts-gaining">
              Hosts that start seeing it: {impact.hosts_gaining.length > 0 ? impact.hosts_gaining.join(', ') : 'none'}
            </li>
            <li data-testid="work-org-clients">
              Org-bound clients: {impact.bound_clients_losing} stop, {impact.bound_clients_gaining} start seeing it
            </li>
            <li data-testid="work-org-journal">
              {plural(impact.journal_entries, 'journal entry', 'journal entries')} and {plural(impact.summaries, 'summary', 'summaries')} move with it
            </li>
            {#each impact.people_losing ?? [] as p (p)}
              <li class="warn" data-testid="work-org-person-losing">
                {p} loses access (their device is bound to {orgName(impact.from_org)}, not {orgName(impact.to_org)})
              </li>
            {/each}
            {#each impact.people_gaining ?? [] as p (p)}
              <li data-testid="work-org-person-gaining">{p} gains access (their device is bound to {orgName(impact.to_org)})</li>
            {/each}
            {#if spend}
              <li data-testid="work-org-spend">
                {spend} spent so far stays on its sessions' organisation budget: spend follows a session, not its task
              </li>
            {/if}
          </ul>
        {/if}
      </div>
    {/if}

    {#if busy && !impact}<p class="note" data-testid="work-org-reading">Reading what this move changes…</p>{/if}
    {#if failure}<p class="err" role="alert" data-testid="work-org-error">{failure}</p>{/if}
    {#if blocked}<p class="note">{blocked}</p>{/if}
    <div class="actions">
      <button type="button" class="btn btn--quiet" onclick={onclose}>Cancel</button>
      {#if failure && !impact && targetId !== null}
        <button type="button" class="btn" data-testid="work-org-review" disabled={busy} onclick={() => void review()}>Try again</button>
      {/if}
      <button
        type="button"
        class="btn btn--primary"
        data-testid="work-org-confirm"
        disabled={!impact || !impact.allowed || !impact.impact_token || busy || blocked !== null}
        onclick={() => void confirm()}>Move to {targetId === null ? '…' : orgName(targetId)}</button
      >
    </div>
  </div>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.5rem; font-size: var(--text-xs); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .field span { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field select {
    font: inherit;
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .what { font-weight: 600; overflow-wrap: anywhere; }
  .note { font-size: var(--text-2xs); color: var(--fg-muted); }
  .impact { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.4rem 0.5rem; }
  .impact ul { margin: 0.2rem 0; padding-left: 1.1rem; }
  .warn { color: var(--usage-warn); }
  .err { color: var(--danger); }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  p { margin: 0; }
</style>
