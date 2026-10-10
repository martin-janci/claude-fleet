<script lang="ts">
  // A placement rule (work graph M14): "tasks like these go under group X".
  // Navigation only — a rule never links a session, never changes access,
  // and never touches the tracker. A rule is saved only after a preview of
  // exactly the draft being saved: which tasks move from where to where,
  // and how many a person placed by hand (those stay where they are).
  // The preview runs by itself a moment after each edit (gap plan G2.2):
  // "Matches 6 open tasks now: …" follows the draft as it is typed, and
  // Save waits for the preview of exactly what is on screen.
  import { onDestroy, untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { trackers as trackerStore } from './trackers';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { hosts } from './hosts';
  import { accountChoices } from './start_rules';
  import {
    conflictOf,
    deleteWorkRule,
    ruleWire,
    saveWorkRule,
    taskLabel,
    workRulePreview,
    workRules,
    workTreeMeta,
    type RulePreview,
    type WorkRule,
    type WorkRuleDraft,
  } from './work_view';

  let {
    initial,
    onclose,
    onsaved,
    ondeleted,
    previewDebounceMs = 400,
  }: {
    /** The rule to edit (with its `version` as `expected_version`), or a
     *  prefilled new one. */
    initial: WorkRuleDraft;
    onclose: () => void;
    onsaved?: (r: WorkRule) => void;
    /** "Delete rule" inside the editor (gap plan G7.1): the rule's name. */
    ondeleted?: (name: string) => void;
    /** How long the draft must be still before it is previewed, ms;
     *  injectable for tests. */
    previewDebounceMs?: number;
  } = $props();

  const saveBlocked = $derived(hubActionBlocked('save_work_rule', $hubStatus, $hubConnection));
  const deleteBlocked = $derived(hubActionBlocked('delete_work_rule', $hubStatus, $hubConnection));

  const start = untrack(() => initial);
  let name = $state(start.name ?? '');
  let enabled = $state(start.enabled ?? true);
  let group = $state(start.group ?? '');
  let trackerId = $state<string>(start.conditions?.tracker_id != null ? String(start.conditions.tracker_id) : '');
  let container = $state(start.conditions?.container ?? '');
  let keyPrefix = $state(start.conditions?.key_prefix ?? '');
  let titleContains = $state(start.conditions?.title_contains ?? '');
  let repo = $state(start.conditions?.repo ?? '');
  // "When a task matches, its sessions start here" (gap plan G7.1).
  let host = $state(start.host_alias ?? '');
  let profile = $state(start.profile ?? '');
  const accounts = $derived(accountChoices($hosts, host, profile));
  let confirmDelete = $state(false);
  let expectedVersion = $state<number | undefined>(start.id != null ? start.expected_version : 0);

  let preview = $state<RulePreview | null>(null);
  let previewedSig = $state<string | null>(null);
  let busy = $state(false);
  let failure = $state<string | null>(null);
  // A save that lost a race: kept while the fresh preview runs.
  let conflict = $state<string | null>(null);

  const trackerOptions = $derived(
    $workTreeMeta.trackers.length > 0
      ? $workTreeMeta.trackers.map((t) => ({ id: t.id, name: t.name }))
      : $trackerStore.map((t) => ({ id: t.id, name: t.name })),
  );

  const draft = $derived<WorkRuleDraft>({
    ...(start.id != null ? { id: start.id } : {}),
    name,
    enabled,
    group,
    conditions: {
      tracker_id: trackerId === '' ? null : Number(trackerId),
      container,
      key_prefix: keyPrefix,
      title_contains: titleContains,
      repo,
    },
    host_alias: host || null,
    profile: profile || null,
  });
  // What the preview was of: the draft as the wire takes it.
  const sig = $derived(JSON.stringify(ruleWire(draft)));
  const hasCondition = $derived(Object.values(ruleWire(draft).conditions).some((v) => v != null));
  const complete = $derived(name.trim() !== '' && group.trim() !== '' && hasCondition);
  const previewCurrent = $derived(previewedSig === sig && preview !== null);

  // One preview in flight at a time is not needed: each answers for the
  // draft it was asked of, and only the newest one's answer is kept.
  let previewSeq = 0;
  let previewing = $state(false);
  let previewTimer: ReturnType<typeof setTimeout> | undefined;

  async function runPreview() {
    if (!complete) return;
    const mine = ++previewSeq;
    previewing = true;
    const s = sig;
    const r = await workRulePreview(draft);
    if (mine !== previewSeq) return;
    previewing = false;
    if (!r.ok) {
      failure = r.error.message;
      return;
    }
    failure = null;
    preview = {
      affected: Array.isArray(r.value?.affected) ? r.value.affected : [],
      total: r.value?.total ?? 0,
      kept_manual: r.value?.kept_manual ?? 0,
      matched: typeof r.value?.matched === 'number' ? r.value.matched : undefined,
      matched_sample: Array.isArray(r.value?.matched_sample) ? r.value.matched_sample : [],
    };
    previewedSig = s;
  }

  // Live: a complete draft not yet previewed is previewed once it has been
  // still for `previewDebounceMs`.
  $effect(() => {
    const want = complete && sig !== previewedSig;
    clearTimeout(previewTimer);
    if (!want) {
      previewSeq++;
      previewing = false;
      return;
    }
    previewTimer = setTimeout(() => void runPreview(), previewDebounceMs);
  });
  onDestroy(() => clearTimeout(previewTimer));

  const matchLine = $derived.by(() => {
    const p = preview;
    if (!p || !previewCurrent || p.matched === undefined) return null;
    if (p.matched === 0) return 'Matches no open task now.';
    const names = p.matched_sample ?? [];
    const more = p.matched - names.length;
    const head = `Matches ${p.matched} open task${p.matched === 1 ? '' : 's'} now`;
    return names.length > 0 ? `${head}: ${names.join(', ')}${more > 0 ? ` +${more}` : ''}` : `${head}.`;
  });

  async function save() {
    if (!previewCurrent || busy || saveBlocked !== null) return;
    busy = true;
    failure = null;
    conflict = null;
    const r = await saveWorkRule({ ...draft, expected_version: expectedVersion });
    busy = false;
    if (!r.ok) {
      if (conflictOf(r.error)) {
        // Someone else saved (or deleted) it: take the current version and
        // ask for a fresh preview before saving over it.
        const list = await workRules();
        const now = list.ok && Array.isArray(list.value) ? list.value.find((x) => x.id === start.id) : undefined;
        expectedVersion = now ? now.version : start.id != null ? undefined : 0;
        previewedSig = null;
        conflict = now
          ? `This rule changed elsewhere — now “${now.name}”, ${now.enabled ? 'on' : 'off'}, placing in “${now.group}” (version ${now.version}). Check the new preview: it shows what saving now would do.`
          : start.id != null
            ? 'This rule was deleted elsewhere.'
            : 'A rule like this was saved elsewhere at the same time. Check the new preview before saving.';
        return;
      }
      failure = r.error.message;
      return;
    }
    onsaved?.(r.value);
    onclose();
  }

  async function remove() {
    if (start.id == null || busy || deleteBlocked !== null) return;
    busy = true;
    failure = null;
    const r = await deleteWorkRule(start.id, expectedVersion);
    busy = false;
    confirmDelete = false;
    if (!r.ok) {
      failure = conflictOf(r.error) ? 'This rule changed elsewhere. Close it and open it again before deleting.' : r.error.message;
      return;
    }
    ondeleted?.(start.name ?? name);
    onclose();
  }
</script>

<Modal title={start.id != null ? 'Edit placement rule' : 'Make a rule for similar tasks'} {onclose} width="520px" testid="work-rule-editor">
  <form
    class="form"
    onsubmit={(e) => {
      e.preventDefault();
      if (previewCurrent) void save();
    }}
  >
    <p class="note">
      A rule puts matching tasks under a group in the Work view. When a task matches, its sessions start on the host and
      account below. It never links sessions, never changes who sees what, and never changes the tracker.
    </p>
    <label class="field">
      <span>Name</span>
      <input type="text" bind:value={name} data-testid="rule-name" data-autofocus="" maxlength="80" />
    </label>
    <fieldset class="conds">
      <legend>Tasks that match all of</legend>
      <label class="field">
        <span>Tracker</span>
        <select bind:value={trackerId} data-testid="rule-tracker">
          <option value="">any tracker</option>
          {#each trackerOptions as t (t.id)}
            <option value={String(t.id)}>{t.name}</option>
          {/each}
        </select>
      </label>
      <label class="field">
        <span>Tracker project / container</span>
        <input type="text" bind:value={container} placeholder="e.g. PAY, acme/api, Asana project gid" data-testid="rule-container" />
      </label>
      <label class="field">
        <span>Key prefix</span>
        <input type="text" bind:value={keyPrefix} placeholder="e.g. PAY" data-testid="rule-key-prefix" />
      </label>
      <label class="field">
        <span>Title contains</span>
        <input type="text" bind:value={titleContains} data-testid="rule-title-contains" />
      </label>
      <label class="field">
        <span>Repository</span>
        <input type="text" bind:value={repo} placeholder="owner/repo" data-testid="rule-repo" />
      </label>
    </fieldset>
    <label class="field">
      <span>Group</span>
      <input type="text" bind:value={group} placeholder="e.g. Payments" data-testid="rule-group" maxlength="80" />
    </label>
    <div class="pair">
      <label class="field">
        <span>Host</span>
        <select bind:value={host} data-testid="rule-host">
          <option value="">its usual host</option>
          {#each $hosts as h (h.alias)}
            <option value={h.alias}>{h.alias}</option>
          {/each}
        </select>
      </label>
      <label class="field">
        <span>Account</span>
        <select bind:value={profile} data-testid="rule-account">
          <option value="">the host's own login</option>
          {#each accounts as a (a.value)}
            <option value={a.value}>{a.label}</option>
          {/each}
        </select>
      </label>
    </div>
    <label class="check">
      <input type="checkbox" bind:checked={enabled} data-testid="rule-enabled" />
      Rule is on
    </label>

    {#if preview && previewCurrent}
      <div class="preview" data-testid="rule-preview">
        {#if matchLine}<p class="match" data-testid="rule-match-count">{matchLine}</p>{/if}
        <p>
          {preview.total === 0 ? 'No task moves.' : `${preview.total} task${preview.total === 1 ? '' : 's'} would move.`}
          {#if preview.kept_manual > 0}
            <span data-testid="rule-preview-kept">{preview.kept_manual} placed by a person stay where they are.</span>
          {/if}
        </p>
        {#if preview.affected.length > 0}
          <ul>
            {#each preview.affected as a (a.task_id)}
              <li data-testid="rule-preview-row">
                <span class="task">{taskLabel(a)}</span>
                <span class="muted">{a.from?.source === 'none' ? 'No group' : a.from?.label} → {a.to?.label}</span>
              </li>
            {/each}
          </ul>
          {#if preview.total > preview.affected.length}
            <p class="muted">…and {preview.total - preview.affected.length} more.</p>
          {/if}
        {/if}
      </div>
    {:else if complete}
      <p class="muted" data-testid="rule-preview-stale" aria-live="polite">
        {previewing || preview === null ? 'Checking which tasks it matches…' : 'The rule changed; checking again…'}
      </p>
    {/if}
    {#if !hasCondition}
      <p class="muted">Give the rule at least one condition.</p>
    {/if}
    {#if conflict}
      <p class="err" role="alert" data-testid="rule-error">{conflict}</p>
    {/if}
    {#if failure}
      <p class="err" role="alert" data-testid="rule-error">{failure}</p>
    {/if}
    <div class="actions">
      {#if start.id != null}
        {#if confirmDelete}
          <button type="button" class="btn btn--crit" data-testid="rule-editor-delete-confirm" disabled={busy} onclick={() => void remove()}>Delete</button>
          <button type="button" class="btn btn--quiet" onclick={() => (confirmDelete = false)}>Keep</button>
        {:else}
          <button
            type="button"
            class="btn btn--quiet"
            data-testid="rule-editor-delete"
            disabled={busy || deleteBlocked !== null}
            title={deleteBlocked ?? 'Delete this rule'}
            onclick={() => (confirmDelete = true)}>Delete rule</button
          >
        {/if}
        <span class="spacer"></span>
      {/if}
      <button type="button" class="btn btn--quiet" onclick={onclose}>Cancel</button>
      <button
        type="button"
        class="btn btn--primary"
        data-testid="rule-save"
        disabled={!previewCurrent || busy || saveBlocked !== null}
        title={saveBlocked ?? (previewCurrent ? 'Save the rule' : 'Wait for the preview of this rule')}
        onclick={() => void save()}>Save rule</button
      >
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.5rem; font-size: var(--text-xs); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .field span, legend { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input, .field select {
    font: inherit;
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .conds { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.4rem 0.5rem; margin: 0; display: flex; flex-direction: column; gap: 0.35rem; }
  .check { display: flex; gap: 0.4rem; align-items: center; }
  .note { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .preview { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.4rem 0.5rem; max-height: 14rem; overflow: auto; }
  .preview p { margin: 0 0 0.3rem; }
  .preview .match { font-weight: 600; }
  .preview ul { margin: 0; padding-left: 1rem; }
  .preview li { display: flex; gap: 0.4rem; flex-wrap: wrap; }
  .task { overflow-wrap: anywhere; }
  .muted { color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions .spacer { flex: 1; }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem; }
</style>
