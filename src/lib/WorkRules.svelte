<script lang="ts">
  // Placement rules (work graph M14): find, edit, enable / disable and
  // delete the rules that put similar tasks under a group. Enabling a rule
  // (or editing one) goes through the editor's preview; disabling and
  // deleting only take tasks back to where fleet would put them anyway.
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import WorkRuleEditor from './WorkRuleEditor.svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    conflictNotice,
    deleteWorkRule,
    readErrorText,
    saveWorkRule,
    workRules,
    workTreeMeta,
    type ConflictNotice,
    type WorkRule,
    type WorkRuleDraft,
  } from './work_view';
  import WorkConflictNotice from './WorkConflictNotice.svelte';
  import Skeleton from './states/Skeleton.svelte';

  let { onclose }: { onclose: () => void } = $props();

  const saveBlocked = $derived(hubActionBlocked('save_work_rule', $hubStatus, $hubConnection));
  const deleteBlocked = $derived(hubActionBlocked('delete_work_rule', $hubStatus, $hubConnection));

  let rules = $state<WorkRule[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | ConflictNotice | null>(null);
  let query = $state('');
  let editing = $state<WorkRuleDraft | null>(null);
  let confirmDelete = $state<number | null>(null);
  let busy = $state(false);

  const shown = $derived(
    rules.filter((r) => {
      const q = query.trim().toLowerCase();
      if (!q) return true;
      return [r.name, r.group, conditionsText(r)].some((s) => s.toLowerCase().includes(q));
    }),
  );

  async function load() {
    const r = await workRules();
    loaded = true;
    if (r.ok) {
      rules = Array.isArray(r.value) ? r.value : [];
      error = null;
    } else {
      error = readErrorText(r.error);
    }
  }
  onMount(() => void load());

  function trackerName(id: number | null | undefined): string {
    if (id == null) return '';
    return $workTreeMeta.trackers.find((t) => t.id === id)?.name ?? `tracker ${id}`;
  }

  function conditionsText(r: WorkRule): string {
    const c = r.conditions ?? {};
    const parts: string[] = [];
    if (c.tracker_id != null) parts.push(trackerName(c.tracker_id));
    if (c.container) parts.push(`project ${c.container}`);
    if (c.key_prefix) parts.push(`key ${c.key_prefix}-*`);
    if (c.title_contains) parts.push(`title has “${c.title_contains}”`);
    if (c.repo) parts.push(`repo ${c.repo}`);
    return parts.join(' · ') || 'no conditions';
  }

  function edit(r: WorkRule, patch: Partial<WorkRuleDraft> = {}) {
    editing = {
      id: r.id,
      name: r.name,
      enabled: r.enabled,
      conditions: { ...r.conditions },
      group: r.group,
      host_alias: r.host_alias ?? null,
      profile: r.profile ?? null,
      expected_version: r.version,
      ...patch,
    };
  }

  async function onConflict(e: { code: string; message: string; details?: unknown }, what: string) {
    notice = conflictNotice(e, what) ?? e.message;
    await load();
  }

  // Disabling only takes tasks back to where fleet would put them; enabling
  // moves tasks, so it goes through the preview.
  async function toggle(r: WorkRule) {
    if (!r.enabled) {
      edit(r, { enabled: true });
      return;
    }
    if (busy) return;
    busy = true;
    const res = await saveWorkRule({
      id: r.id,
      name: r.name,
      enabled: false,
      conditions: r.conditions,
      group: r.group,
      host_alias: r.host_alias ?? null,
      profile: r.profile ?? null,
      expected_version: r.version,
    });
    busy = false;
    if (!res.ok) return onConflict(res.error, `The rule “${r.name}”`);
    notice = `Disabled “${r.name}”.`;
    await load();
  }

  async function remove(r: WorkRule) {
    if (busy) return;
    busy = true;
    const res = await deleteWorkRule(r.id, r.version);
    busy = false;
    confirmDelete = null;
    if (!res.ok) return onConflict(res.error, `The rule “${r.name}”`);
    notice = `Deleted “${r.name}”.`;
    await load();
  }
</script>

<Modal title="Placement rules" {onclose} width="560px" testid="work-rules">
  <div class="rules">
    <div class="bar">
      <input type="search" placeholder="Find a rule…" aria-label="Find a rule" bind:value={query} data-testid="rules-search" />
      <button
        class="btn"
        type="button"
        data-testid="rules-new"
        disabled={saveBlocked !== null}
        title={saveBlocked ?? 'A new rule (previewed before it is saved)'}
        onclick={() => (editing = { name: '', enabled: true, conditions: {}, group: '', expected_version: 0 })}>New rule…</button
      >
    </div>
    {#if notice}
      <p class="notice" role="status" data-testid="rules-notice">
        {#if typeof notice === 'string'}{notice}{:else}<WorkConflictNotice notice={notice} onreload={() => void load()} />{/if}
      </p>
    {/if}
    {#if error}
      <p class="err" role="alert" data-testid="rules-error">{error}</p>
    {:else if !loaded}
      <Skeleton />
    {:else if rules.length === 0}
      <p class="muted" data-testid="rules-empty">No rules yet. “Place in group…” on a task offers one for similar tasks.</p>
    {:else}
      <ul>
        {#each shown as r (r.id)}
          <li data-testid="rule-row" class:off={!r.enabled}>
            <div class="main">
              <span class="name">{r.name}</span>
              <span class="muted">{conditionsText(r)} → <strong>{r.group}</strong>{#if r.host_alias || r.profile}<span data-testid="rule-starts"> · starts on {r.host_alias ?? "its usual host"}{r.profile ? ` · ${r.profile}` : ""}</span>{/if}</span>
            </div>
            <div class="actions">
              <button
                class="btn btn--chip btn--toggle"
                type="button"
                aria-pressed={r.enabled}
                data-testid="rule-toggle"
                disabled={busy || saveBlocked !== null}
                title={saveBlocked ?? (r.enabled ? 'Disable' : 'Enable (previewed first)')}
                onclick={() => void toggle(r)}>{r.enabled ? 'on' : 'off'}</button
              >
              <button class="btn btn--quiet" type="button" data-testid="rule-edit" disabled={saveBlocked !== null} onclick={() => edit(r)}>Edit…</button>
              {#if confirmDelete === r.id}
                <button class="btn btn--crit" type="button" data-testid="rule-delete-confirm" disabled={busy} onclick={() => void remove(r)}>Delete</button>
                <button class="btn btn--quiet" type="button" onclick={() => (confirmDelete = null)}>Keep</button>
              {:else}
                <button
                  class="btn btn--quiet"
                  type="button"
                  data-testid="rule-delete"
                  disabled={deleteBlocked !== null}
                  title={deleteBlocked ?? 'Delete this rule'}
                  onclick={() => (confirmDelete = r.id)}>Delete…</button
                >
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</Modal>

{#if editing}
  <WorkRuleEditor
    initial={editing}
    onclose={() => (editing = null)}
    onsaved={(r) => {
      notice = `Saved “${r.name}”.`;
      void load();
    }}
    ondeleted={(name) => {
      notice = `Deleted “${name}”.`;
      void load();
    }}
  />
{/if}

<style>
  .rules { display: flex; flex-direction: column; gap: 0.5rem; font-size: var(--text-xs); }
  .bar { display: flex; gap: 0.4rem; }
  .bar input {
    flex: 1 1 auto;
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  ul { list-style: none; margin: 0; padding: 0; }
  li { display: flex; gap: 0.5rem; align-items: center; justify-content: space-between; padding: 0.3rem 0; border-bottom: 1px solid var(--border); }
  li.off .name { color: var(--fg-muted); }
  .main { display: flex; flex-direction: column; min-width: 0; }
  .name { font-weight: 600; overflow-wrap: anywhere; }
  .actions { display: flex; gap: 0.25rem; flex: 0 0 auto; }
  .muted { color: var(--fg-muted); margin: 0; }
  .notice { margin: 0; }
  .err { color: var(--danger); margin: 0; }
</style>
