<script lang="ts">
  // Automation › Rules (redesign 8.11, "From AI to rule"): the start rules,
  // each a task key pattern that names the repository (and optionally the
  // host) a matching start lands in, before history and Jev. Offers fleet
  // made after five identical starts come first, with Add and Dismiss; an
  // active rule can be edited, turned off or deleted; a dismissed one can be
  // added back. The Automation rail item (8.4) mounts this list.
  import { onMount } from 'svelte';
  import { projects } from './projects';
  import { hosts } from './hosts';
  import { readErrorText } from './work_view';
  import { LAUNCH_EFFORT_OPTIONS, MODEL_OPTIONS } from './conversation';
  import {
    accountChoices,
    acceptStartRule,
    deleteStartRule,
    dismissStartRule,
    listStartRules,
    patternProblem,
    ruleLaunchLine,
    ruleLine,
    ruleProject,
    saveStartRule,
    type StartRuleView,
  } from './start_rules';

  let rules = $state.raw<StartRuleView[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** The rule being edited (`0` = a new one), else `null`. */
  let editing = $state<number | null>(null);
  let pattern = $state('');
  let projectId = $state<number | null>(null);
  let host = $state('');
  // How its starts run (gap plan G7.1).
  let fallback = $state('');
  let profile = $state('');
  let model = $state('');
  let effort = $state('');
  let agent = $state('');

  const accounts = $derived(accountChoices($hosts, host, profile));
  const models = MODEL_OPTIONS.filter((o) => o.value !== 'default');

  const choices = $derived($projects.map((t) => t.project).filter((p) => !p.system));
  const problem = $derived(editing == null ? null : patternProblem(pattern) ?? (projectId == null ? 'Pick a repository.' : null));
  const offers = $derived(rules.filter((r) => r.state === 'offered'));
  const active = $derived(rules.filter((r) => r.state === 'active'));
  const dismissed = $derived(rules.filter((r) => r.state === 'dismissed'));

  onMount(() => void reload());

  async function reload() {
    const r = await listStartRules();
    loaded = true;
    if (r.ok) {
      rules = Array.isArray(r.value) ? r.value : [];
      error = null;
    } else error = readErrorText(r.error);
  }

  async function act(run: () => Promise<{ ok: boolean; error?: unknown }>) {
    if (busy) return;
    busy = true;
    const r = await run();
    busy = false;
    if (!r.ok) {
      error = readErrorText(r.error as never);
      return;
    }
    editing = null;
    await reload();
  }

  function edit(rule: StartRuleView | null) {
    editing = rule?.id ?? 0;
    pattern = rule?.pattern ?? '';
    projectId = rule?.project_id ?? null;
    host = rule?.host_alias ?? '';
    fallback = rule?.fallback_host ?? '';
    profile = rule?.profile ?? '';
    model = rule?.model ?? '';
    effort = rule?.effort ?? '';
    agent = rule?.agent ?? '';
  }

  function save() {
    if (problem || projectId == null) return;
    const id = editing || undefined;
    const pid = projectId;
    void act(() =>
      saveStartRule(
        { pattern: pattern.trim(), project_id: pid, host_alias: host || null, fallback_host: fallback || null, profile: profile || null, model: model || null, effort: effort || null, agent: agent || null },
        id,
      ),
    );
  }
</script>

{#snippet form()}
  <form
    class="form"
    data-testid="start-rules-form"
    onsubmit={(e) => {
      e.preventDefault();
      save();
    }}
  >
    <label>
      <span>Tasks</span>
      <input data-testid="start-rules-pattern" spellcheck="false" placeholder="PD-*" bind:value={pattern} />
    </label>
    <label>
      <span>Repository</span>
      <select
        data-testid="start-rules-project"
        value={projectId ?? ''}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          projectId = v === '' ? null : Number(v);
        }}
      >
        {#if projectId == null}<option value="">Pick a repository…</option>{/if}
        {#each choices as p (p.id)}
          <option value={p.id}>{p.owner}/{p.repo}</option>
        {/each}
      </select>
    </label>
    <label>
      <span>Host</span>
      <select data-testid="start-rules-host" bind:value={host}>
        <option value="">Its last host</option>
        {#each $hosts as h (h.alias)}
          <option value={h.alias}>{h.alias}</option>
        {/each}
      </select>
    </label>
    <label>
      <span>Else</span>
      <select data-testid="start-rules-fallback" bind:value={fallback}>
        <option value="">No fallback host</option>
        {#each $hosts.filter((h) => h.alias !== host) as h (h.alias)}
          <option value={h.alias}>{h.alias}, when {host || 'its last host'} is offline</option>
        {/each}
      </select>
    </label>
    <label>
      <span>Agent</span>
      <select data-testid="start-rules-agent" bind:value={agent}>
        <option value="">Claude Code</option>
        <option value="codex">Codex</option>
      </select>
    </label>
    {#if agent !== 'codex'}
      <label>
        <span>Account</span>
        <select data-testid="start-rules-account" bind:value={profile}>
          <option value="">The host's own login</option>
          {#each accounts as a (a.value)}
            <option value={a.value}>{a.label}</option>
          {/each}
        </select>
      </label>
    {/if}
    <label>
      <span>Model</span>
      <select data-testid="start-rules-model" bind:value={model}>
        <option value="">The host's default</option>
        {#each models as m (m.value)}
          <option value={m.value}>{m.label}</option>
        {/each}
      </select>
    </label>
    <label>
      <span>Effort</span>
      <select data-testid="start-rules-effort" bind:value={effort}>
        <option value="">Default</option>
        {#each LAUNCH_EFFORT_OPTIONS as e (e.value)}
          <option value={e.value}>{e.label}</option>
        {/each}
      </select>
    </label>
    {#if problem && pattern.trim()}<p class="hint" data-testid="start-rules-problem">{problem}</p>{/if}
    <div class="acts">
      <button class="btn btn--quiet" type="button" onclick={() => (editing = null)}>Cancel</button>
      <button class="btn btn--primary" type="submit" data-testid="start-rules-save" disabled={busy || problem !== null}>Save rule</button>
    </div>
  </form>
{/snippet}

<section class="rules" aria-label="Start rules" data-testid="start-rules">
  <header class="head">
    <h3>Rules</h3>
    <span class="sub">Where a task's sessions start: the repository, host, account, model and agent. A rule decides before Jev is asked.</span>
    {#if editing == null}
      <button class="btn btn--quiet" type="button" data-testid="start-rules-new" onclick={() => edit(null)}>+ New rule</button>
    {/if}
  </header>

  {#if error}<p class="err" role="alert" data-testid="start-rules-error">{error}</p>{/if}
  {#if editing === 0}{@render form()}{/if}

  {#if offers.length > 0}
    <ul class="list" data-testid="start-rules-offers">
      {#each offers as r (r.id)}
        <li class="row row--offer">
          <span class="line">Add rule <strong>{ruleLine(r)}</strong>? You started {r.pattern} there {r.confirmations ?? 5} times in a row.</span>
          {#if r.may_change}
            <button class="btn btn--quiet" type="button" data-testid="start-rules-accept" disabled={busy} onclick={() => void act(() => acceptStartRule(r.id))}>Add rule</button>
            <button class="btn btn--quiet" type="button" data-testid="start-rules-dismiss" disabled={busy} onclick={() => void act(() => dismissStartRule(r.id))}>Dismiss</button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if active.length > 0}
    <ul class="list" data-testid="start-rules-active">
      {#each active as r (r.id)}
        <li class="row">
          {#if editing === r.id}
            {@render form()}
          {:else}
            <span class="line">
              <span class="pat">{r.pattern}</span> → {ruleProject(r)}{r.host_alias ? ` on ${r.host_alias}` : ''}
              {#if ruleLaunchLine(r)}<span class="meta" data-testid="start-rules-launch">{ruleLaunchLine(r)}</span>{/if}
              <span class="meta">{r.hits ? `decided ${r.hits} start${r.hits === 1 ? '' : 's'}` : 'not used yet'}</span>
            </span>
            {#if r.may_change}
              <button class="btn btn--quiet" type="button" data-testid="start-rules-edit" disabled={busy} onclick={() => edit(r)}>Edit</button>
              <button class="btn btn--quiet" type="button" data-testid="start-rules-off" disabled={busy} onclick={() => void act(() => dismissStartRule(r.id))}>Turn off</button>
              <button class="btn btn--quiet" type="button" data-testid="start-rules-delete" disabled={busy} onclick={() => void act(() => deleteStartRule(r.id))}>Delete</button>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {:else if loaded && offers.length === 0 && editing == null}
    <p class="empty" data-testid="start-rules-empty">No rules yet. Fleet offers one after you start five tasks of one kind in the same repository.</p>
  {/if}

  {#if dismissed.length > 0}
    <details class="off">
      <summary>Off and dismissed ({dismissed.length})</summary>
      <ul class="list" data-testid="start-rules-dismissed">
        {#each dismissed as r (r.id)}
          <li class="row">
            <span class="line muted">{ruleLine(r)}</span>
            {#if r.may_change}
              <button class="btn btn--quiet" type="button" data-testid="start-rules-restore" disabled={busy} onclick={() => void act(() => acceptStartRule(r.id))}>Turn on</button>
              <button class="btn btn--quiet" type="button" disabled={busy} onclick={() => void act(() => deleteStartRule(r.id))}>Delete</button>
            {/if}
          </li>
        {/each}
      </ul>
    </details>
  {/if}
</section>

<style>
  .rules {
    display: grid;
    gap: 8px;
    font-size: var(--text-xs);
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: baseline;
  }
  .head h3 {
    margin: 0;
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .sub,
  .meta,
  .hint,
  .empty,
  .muted {
    color: var(--fg-muted);
  }
  .head .btn {
    margin-left: auto;
  }
  .list {
    display: grid;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .line {
    flex: 1 1 200px;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .pat {
    font-family: var(--mono);
  }
  .meta {
    margin-left: 6px;
    font-size: var(--text-2xs);
  }
  .form {
    display: grid;
    flex: 1 1 100%;
    gap: 6px;
  }
  .form label {
    display: grid;
    grid-template-columns: 80px minmax(0, 1fr);
    gap: 6px;
    align-items: center;
  }
  .form input,
  .form select {
    min-width: 0;
    height: 24px;
    font: inherit;
  }
  .form input {
    font-family: var(--mono);
  }
  .acts {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
  }
  .hint,
  .empty,
  .err {
    margin: 0;
  }
  .err {
    color: var(--usage-crit);
  }
</style>
