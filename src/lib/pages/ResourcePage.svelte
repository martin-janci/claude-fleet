<script lang="ts">
  // Layout L2 (master_detail): a resource's records in a list beside one
  // record's editor. The list reads through the resource's list command
  // (which routes to the hub on a paired desktop, so the page shows the
  // hub's records there, read-only); every change runs one of the
  // resource's declared actions, then re-reads the list.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import RecordEditor from './RecordEditor.svelte';
  import ActionForm from './ActionForm.svelte';
  import FlowView from './FlowView.svelte';
  import OrgSuggestions from '../OrgSuggestions.svelte';
  import { hosts } from '../hosts';
  import { trackers } from '../trackers';
  import { push, pushError } from '../toasts';
  import type { Page } from './pages';
  import {
    afterChange,
    badgesOf,
    buildArgs,
    idOf,
    itemValue,
    itemsOf,
    listRecords,
    runAction,
    titleOf,
    type ActionSpec,
    type FieldSpec,
    type ResourceRecord,
    type ResourceType,
  } from './resources';

  let {
    page,
    resource,
    readonly = false,
    reason = null,
  }: {
    page: Page;
    resource: ResourceType;
    /** Show without changing anything (a paired desktop). */
    readonly?: boolean;
    /** Why it is read-only, and where to change it instead. */
    reason?: string | null;
  } = $props();

  let records = $state<ResourceRecord[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<string | null>(null);
  /** Bumped on every re-read, so the editor's draft restarts from it. */
  let version = $state(0);
  let adding = $state(false);
  let busy = $state(false);

  const current = $derived(records.find((r) => idOf(resource, r) === selected) ?? null);

  async function reload() {
    const r = await listRecords(resource);
    loaded = true;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    error = null;
    records = Array.isArray(r.value) ? r.value : [];
    version += 1;
    if (!records.some((x) => idOf(resource, x) === selected)) {
      selected = records.length ? idOf(resource, records[0]) : null;
    }
  }

  onMount(() => {
    void reload();
    // The stores the option selects read (hosts, trackers) and other views
    // keep of this resource, fresh when the page opens.
    void afterChange(resource);
  });

  /** Run an action; a failure is a toast, and the list is re-read either way. */
  async function run(action: ActionSpec, args: Record<string, unknown>): Promise<boolean> {
    busy = true;
    const r = await runAction(action, args);
    busy = false;
    if (!r.ok) pushError(r.error, `${action.label} failed`);
    else if (action.report) {
      // `{ ok, error? }`: a test that ran and failed is not a failed call.
      const rep = r.value as { ok?: boolean; error?: string | null } | null;
      if (rep?.ok) push({ kind: 'success', message: `${action.label}: ok` });
      else push({ kind: 'error', message: `${action.label}: ${rep?.error ?? 'failed'}` });
    }
    await reload();
    await afterChange(resource);
    return r.ok;
  }

  async function create(params: Record<string, string>) {
    const c = resource.create;
    if (!c) return;
    const ok = await run(c, buildArgs(c, null, null, params));
    if (ok) {
      adding = false;
      const name = (params[resource.title_field] ?? '').trim();
      const made = records.find((r) => titleOf(resource, r) === name);
      if (made) selected = idOf(resource, made);
    }
  }

  /** A select's choices for a list field's add form: the source's values,
   *  minus the ones the record already has. */
  function options(field: FieldSpec, param: string): { value: string; label: string }[] {
    const action = field.type === 'items' ? field.add.find((a) => a.params.some((p) => p.name === param)) : undefined;
    const spec = action?.params.find((p) => p.name === param);
    if (!spec || spec.type !== 'options' || !current) return [];
    const have = new Set(itemsOf(field, current).map(itemValue));
    const all =
      spec.source === 'hosts'
        ? get(hosts).map((h) => ({ value: h.alias, label: h.alias }))
        : get(trackers).map((t) => ({ value: String(t.id), label: t.name }));
    return all.filter((o) => !have.has(o.value));
  }
</script>

<div class="resource-page" data-testid={`resource-${resource.id}`}>
  {#if reason}<p class="reason" data-testid="resource-readonly">{reason}</p>{/if}

  {#if !readonly}
    {#each page.list_items ?? [] as item, i (i)}
      {#if item.type === 'notice'}
        <p class={`notice ${item.tone}`}>{item.text}</p>
      {:else if item.type === 'custom' && item.component === 'org_suggestions'}
        <OrgSuggestions onchanged={() => void reload()} />
      {/if}
    {/each}
  {/if}

  <div class="md">
    <div class="list">
      <ul role="listbox" aria-label={resource.plural} data-testid="resource-list">
        {#each records as r (idOf(resource, r))}
          <li>
            <button
              type="button"
              role="option"
              aria-selected={selected === idOf(resource, r)}
              data-testid="resource-row"
              onclick={() => (selected = idOf(resource, r))}>
              {#if resource.color_field}
                <span class="swatch" style:background={String(r[resource.color_field] ?? '') || 'transparent'}></span>
              {/if}
              <span class="title">{titleOf(resource, r)}</span>
              {#each badgesOf(resource, r) as b (b)}<span class="badge" data-testid="resource-badge">{b}</span>{/each}
            </button>
          </li>
        {/each}
      </ul>
      {#if loaded && records.length === 0 && !error}
        <p class="empty" data-testid="resource-empty">{resource.empty}</p>
      {/if}
      {#if error}<p class="err" role="alert">{error}</p>{/if}
      {#if !readonly && resource.create_flow}
        <button type="button" class="btn" data-testid="resource-add" disabled={adding} onclick={() => (adding = true)}
          >Add {resource.label.toLowerCase()}</button
        >
      {:else if !readonly && resource.create}
        {#if adding}
          <ActionForm action={resource.create} {busy} onrun={(p) => void create(p)} testid="resource-create" />
        {:else}
          <button type="button" class="btn" data-testid="resource-add" onclick={() => (adding = true)}>{resource.create.label}</button>
        {/if}
      {/if}
    </div>

    <div class="detail">
      {#if adding && resource.create_flow}
        <FlowView
          flow={resource.create_flow}
          oncancel={() => (adding = false)}
          ondone={(message, recordId) => {
            adding = false;
            push({ kind: 'success', message });
            void reload().then(() => {
              if (recordId !== null) selected = String(recordId);
              void afterChange(resource);
            });
          }} />
      {:else if current}
        {#key `${selected}:${version}`}
          <RecordEditor
            {resource}
            sections={page.sections ?? []}
            record={current}
            {readonly}
            {options}
            {run}
            reload={() => void reload()} />
        {/key}
      {:else if loaded && records.length > 0}
        <p class="empty">Pick one.</p>
      {/if}
    </div>
  </div>
</div>

<style>
  .md {
    display: grid;
    grid-template-columns: 13rem minmax(0, 1fr);
    gap: 1rem;
    align-items: start;
  }
  @media (max-width: 720px) {
    .md {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li button {
    width: 100%;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem;
    text-align: left;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    padding: 0.35rem 0.45rem;
    font: inherit;
    font-size: 0.82rem;
    color: var(--fg);
    cursor: pointer;
  }
  li button:hover {
    background: var(--control-bg-hover);
  }
  li button[aria-selected='true'] {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 30%, transparent);
  }
  li button:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .swatch {
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 2px;
    border: 1px solid var(--border);
  }
  .badge {
    font-size: 0.66rem;
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .empty,
  .reason {
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin: 0;
  }
  .reason {
    margin-bottom: 0.6rem;
  }
  .err {
    font-size: 0.78rem;
    color: var(--usage-crit);
  }
  .notice {
    font-size: 0.78rem;
    margin: 0 0 0.5rem;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--border);
  }
</style>
