<script lang="ts">
  // Layout L2 (master_detail): a resource's records in a list beside one
  // record's editor. The list reads through the resource's list command
  // (which routes to the hub on a paired desktop, so the page shows the
  // hub's records there, read-only); every change runs one of the
  // resource's declared actions, then re-reads the list.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import RecordEditor from './RecordEditor.svelte';
  import ResourceGraph from './ResourceGraph.svelte';
  import ResourceTable from './ResourceTable.svelte';
  import ActionForm from './ActionForm.svelte';
  import FlowView from './FlowView.svelte';
  import OrgSuggestions from '../OrgSuggestions.svelte';
  import PairingResult from './PairingResult.svelte';
  import ActionResult, { type Shown } from './ActionResult.svelte';
  import { catalogStatuses } from '../assets_workspace';
  import { devices, people, type Pairing } from '../devices';
  import { hosts } from '../hosts';
  import { hubStatus } from '../hub';
  import { orgs } from '../orgs';
  import { trackers } from '../trackers';
  import { push, pushError } from '../toasts';
  import WizardDialog from '../forms/WizardDialog.svelte';
  import type { Values } from '../forms/forms';
  import { pairDevice, pairDeviceWizard } from '../forms/pair_device_wizard';
  import { WIZARDS } from '../forms/wizards';
  import type { Page, PageAction } from './pages';
  import PageActionButton from './PageActionButton.svelte';
  import {
    afterChange,
    badgesOf,
    buildArgs,
    hubHost,
    idOf,
    itemValue,
    itemsOf,
    listRecords,
    runAction,
    titleOf,
    type ActionSpec,
    type FieldSpec,
    type OptionSource,
    type ResourceRecord,
    type ResourceType,
  } from './resources';

  let {
    page,
    resource,
    readonly = false,
    reason = null,
    actions = [],
  }: {
    page: Page;
    resource: ResourceType;
    /** Show without changing anything (a paired desktop). */
    readonly?: boolean;
    /** Why it is read-only, and where to change it instead. */
    reason?: string | null;
    /** The page actions an `action` list item names (`pages/actions.rs`). */
    actions?: PageAction[];
  } = $props();

  let records = $state<ResourceRecord[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<string | null>(null);
  /** Bumped on every re-read, so the editor's draft restarts from it. */
  let version = $state(0);
  let adding = $state(false);
  /** The record editor's open tab, kept across re-reads and records. */
  let recordTab = $state('');
  let busy = $state(false);
  /** A create whose answer is shown (`result: pairing`), until dismissed. */
  let pairing = $state<Pairing | null>(null);
  /** A record action's answer (`result: output | image`), until dismissed. */
  let shown = $state<Shown | null>(null);

  const current = $derived(records.find((r) => idOf(resource, r) === selected) ?? null);

  /** Redesign 10.12: "Pair a device" is the pair_device wizard
   *  (one fleet.form/1 spec, the same one the chat shows) rather than the
   *  inline create form; its answer is the same PairingResult. */
  const pairWizard = $derived(resource.id === 'device');
  /** 11.12: Settings › Federation's Link a hub is the link_peer wizard (the
   *  address, then the code), with its Counter-orbit while the hubs trade
   *  keys. */
  const linkWizard = $derived(resource.id === 'peer_link' && !!resource.create);
  let wizardOpen = $state(false);
  let wizardBusy = $state(false);
  let wizardError = $state<string | null>(null);

  async function pairFromWizard(v: Values) {
    wizardBusy = true;
    wizardError = null;
    const r = await pairDevice(v);
    wizardBusy = false;
    if (!r.ok) {
      wizardError = r.error.message;
      return;
    }
    wizardOpen = false;
    pairing = r.value;
    await reload();
    await afterChange(resource);
    const made = records.find((x) => titleOf(resource, x) === r.value.name);
    if (made) selected = idOf(resource, made);
  }

  async function linkFromWizard(v: Values) {
    const c = resource.create;
    if (!c) return;
    wizardBusy = true;
    wizardError = null;
    const r = await runAction(c, buildArgs(c, null, null, { url: String(v.url ?? '').trim(), code: String(v.code ?? '').trim() }));
    wizardBusy = false;
    if (!r.ok) {
      wizardError = r.error.message;
      return;
    }
    wizardOpen = false;
    push({ kind: 'success', message: 'Linked. The hubs connect within seconds.' });
    await reload();
    await afterChange(resource);
    const made = (r.value as { id?: unknown } | null)?.id;
    if (made !== undefined && records.some((x) => idOf(resource, x) === String(made))) selected = String(made);
  }

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
    // The stores the option selects read (hosts, trackers, orgs) and other views
    // keep of this resource, fresh when the page opens.
    void afterChange(resource);
  });

  /** Run an action; a failure is a toast, and the list is re-read either way. */
  async function run(action: ActionSpec, args: Record<string, unknown>): Promise<boolean> {
    busy = true;
    const r = await runAction(action, args);
    busy = false;
    if (!r.ok) pushError(r.error, `${action.label} failed`);
    else if (action.result === 'pairing') pairing = r.value as Pairing;
    else if (action.result === 'output' || action.result === 'image') shown = resultOf(action, r.value);
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

  /** What `shown` holds for an action's answer, titled by the action and
   *  the record it ran on. */
  function resultOf(action: ActionSpec, value: unknown): Shown {
    const title = current ? `${action.label.replace(/…$/, '')} · ${titleOf(resource, current)}` : action.label;
    const v = (value ?? {}) as Record<string, unknown>;
    if (action.result === 'image')
      return { kind: 'image', title, caption: String(v.caption ?? ''), mime: String(v.mime ?? 'image/png'), data: String(v.data ?? '') };
    return {
      kind: 'output',
      title,
      output: String(v.output ?? ''),
      exit_code: Number(v.exit_code ?? 0),
      truncated: v.truncated === true,
    };
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

  /** What an option source offers, from the store the renderer already holds. */
  function sourceOptions(source: OptionSource): { value: string; label: string }[] {
    if (source === 'hosts') return get(hosts).map((h) => ({ value: h.alias, label: h.alias }));
    if (source === 'orgs') return get(orgs).map((o) => ({ value: o.name, label: o.name }));
    if (source === 'devices') return get(devices).map((d) => ({ value: d.name, label: d.name }));
    if (source === 'catalogs') return (get(catalogStatuses) ?? []).map((c) => ({ value: c.name, label: c.name }));
    if (source === 'people')
      return get(people)
        .filter((p) => p.disabled_at === undefined || p.disabled_at === null)
        .map((p) => ({ value: p.name, label: p.display_name && p.display_name !== p.name ? `${p.display_name} (${p.name})` : p.name }));
    return get(trackers).map((t) => ({ value: String(t.id), label: t.name }));
  }

  /** A select's choices for a list field's add form: the source's values,
   *  minus the ones the record already has. */
  function options(field: FieldSpec, param: string): { value: string; label: string }[] {
    // A picked field (a device's org and person, G7.14): the whole source.
    if (field.type === 'pick') return sourceOptions(field.source);
    const action = field.type === 'items' ? field.add.find((a) => a.params.some((p) => p.name === param)) : undefined;
    const spec = action?.params.find((p) => p.name === param);
    // A suggestion leaves nothing out: picking a member again changes their role.
    if (spec?.type === 'suggest') return sourceOptions(spec.source);
    if (!spec || spec.type !== 'options' || !current) return [];
    const have = new Set(itemsOf(field, current).map(itemValue));
    return sourceOptions(spec.source).filter((o) => !have.has(o.value));
  }

  /** A create or record action form's choices: the source's values,
   *  nothing to leave out. */
  function actionOptions(action: ActionSpec | undefined, param: string): { value: string; label: string }[] {
    const spec = action?.params.find((p) => p.name === param);
    return spec?.type === 'options' || spec?.type === 'suggest' ? sourceOptions(spec.source) : [];
  }
  const createOptions = (param: string) => actionOptions(resource.create, param);
</script>

<div class="resource-page" data-testid={`resource-${resource.id}`}>
  {#if reason}<p class="reason" data-testid="resource-readonly">{reason}</p>{/if}

  {#if !readonly}
    {#each page.list_items ?? [] as item, i (i)}
      {#if item.type === 'notice'}
        <p class={`notice ${item.tone}`}>{item.text}</p>
      {:else if item.type === 'custom' && item.component === 'org_suggestions'}
        <OrgSuggestions onchanged={() => void reload()} />
      {:else if item.type === 'action'}
        <!-- A page action over the whole list (G4.5: Scan all hosts). -->
        {@const action = actions.find((a) => a.id === item.action)}
        {#if action}<PageActionButton {action} onran={() => void reload()} />{/if}
      {/if}
    {/each}
  {/if}

  {#if page.graph}
    <ResourceGraph graph={page.graph} {resource} {records} {selected} />
  {/if}

  {#if page.table && records.length}
    <ResourceTable {resource} table={page.table} {records} {selected} onselect={(id) => (selected = id)} />
  {/if}

  <div class="md">
    <div class="list">
      <ul role="listbox" aria-label={resource.plural} data-testid="resource-list">
        {#each records as r (idOf(resource, r))}
          <li role="presentation">
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
              {#each badgesOf(resource, r, hubHost($hubStatus.url)) as b (b)}<span class="badge" data-testid="resource-badge">{b}</span>{/each}
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
      {:else if !readonly && resource.create && (pairWizard || linkWizard)}
        <button
          type="button"
          class="btn"
          data-testid="resource-add"
          onclick={() => {
            wizardError = null;
            wizardOpen = true;
          }}>{resource.create.label}</button
        >
        {#if wizardOpen}
          <WizardDialog
            wizard={linkWizard ? WIZARDS.link_peer : pairDeviceWizard($orgs)}
            busy={wizardBusy}
            error={wizardError}
            run={(v) => void (linkWizard ? linkFromWizard(v) : pairFromWizard(v))}
            onclose={() => (wizardOpen = false)} />
        {/if}
      {:else if !readonly && resource.create}
        {#if adding}
          <ActionForm action={resource.create} {busy} options={createOptions} onrun={(p) => void create(p)} testid="resource-create" />
        {:else}
          <button type="button" class="btn" data-testid="resource-add" onclick={() => (adding = true)}>{resource.create.label}</button>
        {/if}
      {/if}
    </div>

    <div class="detail">
      {#if pairing}
        <PairingResult {pairing} onclose={() => (pairing = null)} />
      {/if}
      {#if shown}
        <ActionResult {shown} onclose={() => (shown = null)} />
      {/if}
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
            tabs={page.tabs ?? []}
            bind:tab={recordTab}
            record={current}
            {readonly}
            {options}
            {actionOptions}
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
    font-size: var(--text-2xs);
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
    border-radius: var(--radius-xs);
    border: 1px solid var(--border);
  }
  .badge {
    font-size: var(--text-2xs);
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .empty,
  .reason {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin: 0;
  }
  .reason {
    margin-bottom: 0.6rem;
  }
  .err {
    font-size: var(--text-2xs);
    color: var(--usage-crit);
  }
  .notice {
    font-size: var(--text-2xs);
    margin: 0 0 0.5rem;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--border);
  }
</style>
