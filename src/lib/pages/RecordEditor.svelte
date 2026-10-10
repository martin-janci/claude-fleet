<script lang="ts">
  // One record of a resource (layout L4, object editor): its scalar fields
  // are a draft, written together by Apply (the update action, with only the
  // fields that changed; a field with `confirm` is asked about first), and
  // its lists change item by item through their own add / remove actions.
  // The page spec lays the fields out in sections; the resource says what
  // each field is.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import DestructiveConfirm from '../forms/DestructiveConfirm.svelte';
  import ActionForm from './ActionForm.svelte';
  import TrackerExtras from '../TrackerExtras.svelte';
  import OrgSettingsList from './OrgSettingsList.svelte';
  import OrgMembers from './OrgMembers.svelte';
  import type { OrgMember } from '../orgs';
  import Chart from './Chart.svelte';
  import Loader from '../Loader.svelte';
  import Meter from '../kit/Meter.svelte';
  import KitTabs from '../kit/Tabs.svelte';
  import type { OrgSettingRow } from '../orgs';
  import type { TrackerRow } from '../trackers';
  import { evalCondition, type Section, type Tab } from './pages';
  import {
    ago,
    applies,
    budgetMeter,
    dollars,
    labelOf,
    overBudgetDays,
    buildArgs,
    choiceLabel,
    fieldValue,
    rawOf,
    recordLoss,
    updateArgs,
    idOf,
    itemKey,
    itemLabel,
    itemsOf,
    needLine,
    personSpendName,
    type PersonSpend,
    recordValues,
    subLine,
    syncLine,
    type AdminNeed,
    type SyncProgress,
    titleOf,
    type ActionSpec,
    type FieldSpec,
    type FieldValue,
    type ResourceRecord,
    type ResourceType,
  } from './resources';

  let {
    resource,
    sections,
    tabs = [],
    tab = $bindable(''),
    record,
    readonly = false,
    options,
    actionOptions = () => [],
    run,
    reload,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    resource: ResourceType;
    sections: Section[];
    /** The page's tabs, when it lays the record out in tabs instead of
     *  sections (the org's Overview, Members, Devices, Spend, Settings). */
    tabs?: Tab[];
    /** The open tab's title; bound by the page so a re-read (which
     *  remounts the editor) keeps it. Empty: the first. */
    tab?: string;
    record: ResourceRecord;
    readonly?: boolean;
    options: (field: FieldSpec, param: string) => { value: string; label: string }[];
    /** A record action's choices for one of its `options` params. */
    actionOptions?: (action: ActionSpec, param: string) => { value: string; label: string }[];
    /** Run an action with its arguments; resolves once the list is re-read. */
    run: (action: ActionSpec, args: Record<string, unknown>) => Promise<boolean>;
    /** Re-read the list (a custom item changed something). */
    reload: () => void;
    /** Unix seconds; injectable for tests. */
    now?: () => number;
  } = $props();

  const recordActions = $derived((resource.actions ?? []).filter((a) => applies(resource, a, record)));
  /** The record action whose form is open. */
  let openAction = $state<string | null>(null);

  /** A field shown, not edited, in words. */
  function shown(f: FieldSpec): string {
    const raw = rawOf(f, record);
    if (f.type === 'time') return ago(raw, now());
    if (f.type === 'count') return String(typeof raw === 'number' ? raw : 0);
    if (f.type === 'money') return dollars(raw);
    if (f.type === 'bool') return saved[f.id] ? 'On' : 'Off';
    if (f.type === 'inherit') return String(saved[f.id]);
    if (raw === null || raw === undefined || raw === '') return '—';
    if (f.type === 'choice') return choiceLabel(f, String(raw));
    return String(raw);
  }

  const fieldOf = (id: string) => resource.fields.find((f) => f.id === id);
  /** A field's label, `{title}` filled with this record's title. */
  const lbl = (f: FieldSpec) => labelOf(f, titleOf(resource, record));
  const saved = $derived(
    Object.fromEntries(
      resource.fields
        .filter((f) => f.type !== 'items' && f.type !== 'settings' && f.type !== 'money_series' && f.type !== 'sync')
        .map((f) => [f.id, fieldValue(f, record)]),
    ),
  ) as Record<string, FieldValue>;

  // svelte-ignore state_referenced_locally
  let draft = $state<Record<string, FieldValue>>({ ...saved });
  const changed = $derived(
    resource.fields.filter((f) => f.edit && f.type !== 'items' && draft[f.id] !== saved[f.id]),
  );
  const values = $derived({ ...recordValues(resource, record), ...Object.fromEntries(Object.entries(draft).map(([k, v]) => [k, String(v)])) });

  const shownTabs = $derived(tabs.filter((t) => evalCondition(t.when, values)));
  const tabSections = $derived(
    shownTabs.length ? (shownTabs.find((t) => t.title === tab) ?? shownTabs[0]).sections : sections,
  );
  /** A tab's count: how many entries its one list holds (Members 5). */
  function tabCount(t: Tab): number | undefined {
    const lists = t.sections
      .flatMap((s) => s.items)
      .flatMap((i) => (i.type === 'field' ? [fieldOf(i.key)] : []))
      .filter((f): f is FieldSpec => f?.type === 'items' && f.item_label.type !== 'admin_need' && f.item_label.type !== 'person_spend');
    if (lists.length !== 1 || !Array.isArray(record[lists[0].id])) return undefined;
    return itemsOf(lists[0], record).length;
  }
  /** The tab a "Needs an admin" line opens, if the page has it. */
  function needTab(n: AdminNeed): string | null {
    const want = n.kind === 'budget' ? 'Spend' : n.kind === 'untrusted_device' ? 'Devices' : null;
    return want && shownTabs.some((t) => t.title === want) ? want : null;
  }

  let busy = $state(false);
  /** A confirmation waiting on the person: its sentences and what runs. */
  let asking = $state<{ title: string; messages: string[]; go: () => void } | null>(null);

  function ask(title: string, messages: string[], go: () => void) {
    if (messages.length === 0) go();
    else asking = { title, messages, go };
  }

  async function exec(action: ActionSpec, args: Record<string, unknown>) {
    busy = true;
    await run(action, args);
    busy = false;
  }

  function apply() {
    const update = resource.update;
    if (!update || changed.length === 0) return;
    const args = {
      ...buildArgs(update, record, null, {}),
      ...updateArgs(
        record,
        changed.map((f) => ({ f, v: draft[f.id] })),
      ),
    };
    ask(
      `Apply changes to ${titleOf(resource, record)}`,
      changed.flatMap((f) => (f.confirm ? [f.confirm] : [])),
      () => void exec(update, args),
    );
  }

  function runItem(action: ActionSpec, item: unknown, params: Record<string, string>) {
    const args = buildArgs(action, record, item, params);
    ask(action.label, action.confirm ? [action.confirm] : [], () => void exec(action, args));
  }

  /** Removing the record asks through the destructive confirm (G1.4): the
   *  red verb, what goes with it, and the typed name when that is a lot. */
  let removing = $state(false);
  const removeLoss = $derived(recordLoss(resource, record));

  function remove() {
    if (resource.delete) removing = true;
  }

  async function confirmRemove() {
    const del = resource.delete;
    if (!del) return;
    await exec(del, buildArgs(del, record, null, {}));
    // A failure is reported by the page's notice; the confirm closes either way.
    removing = false;
  }

  const set = (id: string, v: FieldValue) => (draft = { ...draft, [id]: v });

  /** A `settings` row's write: the field's `set` action with the row's key
   *  and the chosen value (`null`: inherit). The list is re-read after. */
  async function setting(f: FieldSpec, key: string, value: string | null) {
    if (f.type !== 'settings') return { ok: false as const, error: { code: 'E_INVALID', message: 'not a settings field' } };
    busy = true;
    const ok = await run(f.set, buildArgs(f.set, record, null, { key, value: value ?? '' }));
    busy = false;
    return ok ? { ok: true as const, value: null } : { ok: false as const, error: { code: 'E_INVALID', message: `${f.label}: not saved` } };
  }
</script>

<div class="record" data-testid={`record-${resource.id}-${idOf(resource, record)}`}>
  <header>
    {#if resource.color_field}
      <span class="swatch" style:background={String(record[resource.color_field] ?? '') || 'transparent'}></span>
    {/if}
    <h5>{titleOf(resource, record)}</h5>
    {#if !readonly}
      {#each recordActions as a (a.id)}
        <button
          type="button"
          class="btn"
          disabled={busy}
          data-testid={`record-action-${a.id}`}
          aria-expanded={a.params.length ? openAction === a.id : undefined}
          onclick={() => {
            if (a.params.length) openAction = openAction === a.id ? null : a.id;
            else runItem(a, null, {});
          }}>{a.label}</button
        >
      {/each}
    {/if}
    {#if !readonly && resource.delete}
      <button type="button" class="btn btn--quiet" disabled={busy} data-testid="record-delete" onclick={remove}
        >{resource.delete.label}</button
      >
    {/if}
  </header>

  {#each recordActions.filter((a) => a.params.length && openAction === a.id) as a (a.id)}
    <div class="action-panel">
      <ActionForm
        action={a}
        {busy}
        options={(p) => actionOptions(a, p)}
        onrun={(params) => {
          openAction = null;
          runItem(a, null, params);
        }} />
    </div>
  {/each}

  {#if shownTabs.length > 1}
    <div class="tabs">
      <KitTabs
        tabs={shownTabs.map((t) => ({ id: t.title, label: t.title, count: tabCount(t) }))}
        selected={shownTabs.some((t) => t.title === tab) ? tab : shownTabs[0].title}
        onselect={(id) => (tab = id)}
        label={titleOf(resource, record)}
        testid="record-tabs" />
    </div>
  {/if}

  {#each tabSections.filter((s) => evalCondition(s.when, values)) as section (section.title)}
    <section class="section">
      <h6>{section.title}</h6>
      {#if section.tiles}
        <!-- A record's numbers at a glance: a money field the caller may not
             see is left out, as on a row. -->
        <div class="tiles">
          {#each section.items as item, i (i)}
            {@const f = item.type === 'field' ? fieldOf(item.key) : undefined}
            {#if f && !(f.type === 'money' && record[f.id] === undefined)}
              {@const sub = subLine(f.sub, record[f.id], record)}
              {@const meter = budgetMeter(f.sub, record[f.id], record)}
              <div class="tile" data-testid={`tile-${f.id}`} title={f.help}>
                <span class="tile-label">{lbl(f)}</span>
                <span class="tile-value" data-testid={`value-${f.id}`}>{shown(f)}</span>
                {#if meter}<Meter value={meter.value} level={meter.level} label={`${lbl(f)}: ${sub}`} testid={`tile-meter-${f.id}`} />{/if}
                {#if sub}<span class="tile-sub" data-testid={`tile-sub-${f.id}`}>{sub}</span>{/if}
              </div>
            {/if}
          {/each}
        </div>
      {:else}
      {#each section.items as item, i (i)}
        {#if item.type === 'notice'}
          <p class={`notice ${item.tone}`}>{item.text}</p>
        {:else if item.type === 'custom' && !readonly && item.component === 'tracker_extras'}
          <TrackerExtras tracker={record as unknown as TrackerRow} onchanged={reload} />
        {:else if item.type === 'field' && evalCondition(item.when, values)}
          {@const f = fieldOf(item.key)}
          <!-- A list the record does not carry at all is not known here (a
               hub too old for it, or a list only the operator is shown):
               left out, rather than shown as empty. -->
          {#if f && !(['items', 'money', 'money_series', 'settings', 'sync'].includes(f.type) && record[f.id] == null)}
            <div class="field" class:changed={changed.includes(f)} data-testid={`record-field-${f.id}`}>
              <span class="label" id={`rf-${f.id}`}>{lbl(f)}</span>
              <div class="control">
                {#if f.type === 'items' && f.item_label.type === 'admin_need'}
                  <ul class="needs" aria-labelledby={`rf-${f.id}`}>
                    {#each itemsOf(f, record) as it, n (n)}
                      {@const line = needLine(it as AdminNeed, now())}
                      {@const go = needTab(it as AdminNeed)}
                      <li class={`need ${line.tone}`} data-testid={`item-${f.id}`}>
                        <span class="need-mark" aria-hidden="true">{line.tone === 'warn' ? '!' : 'i'}</span>
                        <span class="need-text">{line.text}</span>
                        <span class="need-detail">{line.detail}</span>
                        {#if go}
                          <button
                            type="button"
                            class="btn btn--quiet need-go"
                            data-testid={`need-go-${f.id}`}
                            onclick={() => (tab = go)}>{go}</button
                          >
                        {/if}
                      </li>
                    {:else}
                      <li class="none">Nothing needs an admin.</li>
                    {/each}
                  </ul>
                {:else if f.type === 'sync'}
                  <!-- 11.12: a Constellation with the real count while a
                       queue drains; a Counter-orbit while both ends trade. -->
                  {@const s = record[f.id] as unknown as SyncProgress}
                  <div class="sync" data-testid={`sync-${f.id}`}>
                    {#if s.total > s.done}
                      <Loader name="constellation" size={56} label={`${f.label}: ${syncLine(f.unit, s, now())}`} testid={`sync-loader-${f.id}`} />
                      <span class="value" data-testid={`sync-count-${f.id}`}>{syncLine(f.unit, s, now())}</span>
                    {:else}
                      <Loader name="counter-orbit" size={32} label="Both hubs are trading" testid={`sync-loader-${f.id}`} />
                      <span class="value" data-testid={`sync-count-${f.id}`}>Trading both ways</span>
                    {/if}
                  </div>
                {:else if f.type === 'items' && f.item_label.type === 'person_spend'}
                  <table class="by-person" aria-labelledby={`rf-${f.id}`}>
                    <thead>
                      <tr><th scope="col">Person</th><th scope="col">Today</th><th scope="col">7 days</th><th scope="col">Month</th></tr>
                    </thead>
                    <tbody>
                      {#each itemsOf(f, record) as it, n (n)}
                        {@const p = it as PersonSpend}
                        <tr data-testid={`item-${f.id}`}>
                          <td>{personSpendName(p)}</td>
                          <td>{dollars(p.today_micros)}</td>
                          <td>{dollars(p.week_micros)}</td>
                          <td>{dollars(p.month_micros)}</td>
                        </tr>
                      {:else}
                        <tr><td class="none" colspan="4">Nothing spent this month.</td></tr>
                      {/each}
                    </tbody>
                  </table>
                {:else if f.type === 'money_series'}
                  {@const points = (record[f.id] as { day: string; cost_micros: number }[]) ?? []}
                  {@const daily = record.budget_daily_usd}
                  {@const over = overBudgetDays(points, daily)}
                  <div class="series">
                    <Chart
                      points={points as unknown as Record<string, unknown>[]}
                      x={{ id: 'day', label: 'Day', ty: 'day' }}
                      y={{ id: 'cost_micros', label: lbl(f), ty: 'usd_micros' }}
                      kind="bar"
                      title={lbl(f)}
                      limit={typeof daily === 'number' && daily > 0
                        ? { value: daily * 1_000_000, label: `daily budget $${daily.toLocaleString('en-US')}` }
                        : null}
                      testid={`chart-${f.id}`} />
                    {#if over.length}
                      <p class="over" data-testid={`chart-over-${f.id}`}>{over.join(' · ')}. Fleet warns; it never stops a session.</p>
                    {/if}
                  </div>
                {:else if f.type === 'items' && f.item_label.type === 'member'}
                  <OrgMembers
                    {record}
                    members={itemsOf(f, record) as OrgMember[]}
                    addAction={f.add[0]}
                    removeAction={f.remove}
                    {readonly}
                    {busy}
                    options={(p) => options(f, p)}
                    run={async (a, args) => {
                      busy = true;
                      const ok = await run(a, args);
                      busy = false;
                      return ok;
                    }}
                    {now} />
                {:else if f.type === 'items'}
                  <div class="chips" aria-labelledby={`rf-${f.id}`}>
                    {#each itemsOf(f, record) as it (itemKey(it))}
                      <span class="chip" data-testid={`item-${f.id}`}
                        >{itemLabel(f.item_label, it)}{#if !readonly && f.remove}<button
                            type="button"
                            class="x"
                            disabled={busy}
                            aria-label={`${f.remove.label}: ${itemLabel(f.item_label, it)}`}
                            data-testid={`item-remove-${f.id}`}
                            onclick={() => runItem(f.remove!, it, {})}>×</button
                          >{/if}</span
                      >
                    {:else}
                      <span class="none">None</span>
                    {/each}
                  </div>
                  {#if !readonly}
                    {#each f.add as a (a.id)}
                      <ActionForm
                        action={a}
                        {busy}
                        options={(p) => options(f, p)}
                        argsFor={(params) => buildArgs(a, record, null, params)}
                        onrun={(params) => runItem(a, null, params)} />
                    {/each}
                  {/if}
                {:else if f.type === 'settings'}
                  <OrgSettingsList
                    rows={(record[f.id] as OrgSettingRow[]) ?? []}
                    {readonly}
                    {busy}
                    onset={(key, value) => setting(f, key, value)} />
                {:else if readonly || !f.edit || f.type === 'time'}
                  <span class="value" data-testid={`value-${f.id}`}>{shown(f)}</span>
                {:else if f.type === 'text'}
                  <input
                    type="text"
                    maxlength={f.max}
                    aria-labelledby={`rf-${f.id}`}
                    data-testid={`edit-${f.id}`}
                    disabled={busy}
                    value={String(draft[f.id])}
                    oninput={(e) => set(f.id, (e.currentTarget as HTMLInputElement).value)} />
                {:else if f.type === 'color'}
                  <input
                    type="color"
                    aria-labelledby={`rf-${f.id}`}
                    data-testid={`edit-${f.id}`}
                    disabled={busy}
                    value={String(draft[f.id]) || '#888888'}
                    oninput={(e) => set(f.id, (e.currentTarget as HTMLInputElement).value)} />
                {:else if f.type === 'bool'}
                  <input
                    type="checkbox"
                    role="switch"
                    aria-labelledby={`rf-${f.id}`}
                    data-testid={`edit-${f.id}`}
                    disabled={busy}
                    checked={draft[f.id] === true}
                    onchange={(e) => set(f.id, (e.currentTarget as HTMLInputElement).checked)} />
                {:else if f.type === 'choice'}
                  <div role="radiogroup" aria-labelledby={`rf-${f.id}`} class="choices">
                    {#each f.options as [o, label] (o)}
                      <label class="choice">
                        <input
                          type="radio"
                          name={`rf-${f.id}`}
                          value={o}
                          checked={draft[f.id] === o}
                          disabled={busy}
                          data-testid={`edit-${f.id}-${o}`}
                          onchange={() => set(f.id, o)} />
                        {label}
                      </label>
                    {/each}
                  </div>
                {:else if f.type === 'inherit'}
                  <div role="radiogroup" aria-labelledby={`rf-${f.id}`} class="choices">
                    {#each ['inherit', 'on', 'off'] as o (o)}
                      <label class="choice">
                        <input
                          type="radio"
                          name={`rf-${f.id}`}
                          value={o}
                          checked={draft[f.id] === o}
                          disabled={busy}
                          data-testid={`edit-${f.id}-${o}`}
                          onchange={() => set(f.id, o)} />
                        {o === 'inherit' ? 'Inherit' : o === 'on' ? 'On' : 'Off'}
                      </label>
                    {/each}
                  </div>
                {/if}
              </div>
              <p class="help">{f.help}</p>
            </div>
          {/if}
        {/if}
      {/each}
      {/if}
    </section>
  {/each}

  {#if !readonly && changed.length > 0}
    <footer class="apply" data-testid="record-apply-bar">
      <span>{changed.length} unsaved {changed.length === 1 ? 'change' : 'changes'}</span>
      <button type="button" class="btn btn--quiet" disabled={busy} data-testid="record-discard" onclick={() => (draft = { ...saved })}
        >Discard</button
      >
      <button type="button" class="btn btn--primary" disabled={busy} data-testid="record-apply" onclick={apply}>Apply</button>
    </footer>
  {/if}
</div>

{#if removing && resource.delete}
  <DestructiveConfirm
    title={`${resource.delete.label} "${titleOf(resource, record)}"?`}
    lead={[resource.delete.confirm ?? `Remove ${titleOf(resource, record)}?`, removeLoss.line].filter(Boolean).join(' ')}
    verb={resource.delete.label}
    name={titleOf(resource, record)}
    noun={resource.label.toLowerCase()}
    loss={removeLoss.loss}
    {busy}
    testid="record-remove"
    confirmTestid="record-confirm"
    onconfirm={() => void confirmRemove()}
    onclose={() => (removing = false)}
  />
{/if}

{#if asking}
  <ConfirmDialog
    title={asking.title}
    message={asking.messages.join(' ')}
    confirmLabel="Go ahead"
    danger
    confirmTestId="record-confirm"
    onconfirm={() => {
      const go = asking?.go;
      asking = null;
      go?.();
    }}
    oncancel={() => (asking = null)} />
{/if}

<style>
  .record header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .record header .btn {
    margin-left: auto;
  }
  h5 {
    margin: 0;
    font-size: var(--text-sm);
  }
  h6 {
    margin: 0 0 0.35rem;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .swatch {
    width: 0.8rem;
    height: 0.8rem;
    border-radius: var(--radius-xs);
    border: 1px solid var(--border);
  }
  .section {
    border-top: 1px solid var(--border);
    padding: 0.6rem 0 0.4rem;
    margin-top: 0.6rem;
  }
  .field {
    border-left: 2px solid transparent;
    padding: 0.3rem 0 0.3rem 0.6rem;
    margin-left: -0.6rem;
  }
  .field.changed {
    border-left-color: var(--accent);
  }
  .label {
    font-size: var(--text-xs);
    font-weight: 500;
  }
  .control {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.35rem;
    margin-top: 0.25rem;
  }
  .control input[type='text'] {
    font: inherit;
    font-size: var(--text-2xs);
    max-width: 20rem;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }
  .chip {
    font-size: var(--text-2xs);
    padding: 0.05rem 0.45rem;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border);
  }
  .none,
  .value {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .x {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0 0 0 0.25rem;
  }
  .choices {
    display: flex;
    gap: 0.75rem;
  }
  .choice {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: var(--text-2xs);
  }
  .help {
    margin: 0.2rem 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
    gap: 0.5rem;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    padding: 0.5rem 0.65rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
  }
  .tile-label,
  .tile-sub {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .tile-value {
    font-size: var(--text-lg);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .series {
    width: 100%;
  }
  .over {
    margin: 0.3rem 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .tabs {
    margin-top: 0.6rem;
  }
  .need-go {
    grid-column: 3;
    grid-row: 1 / span 2;
    align-self: center;
  }
  .needs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    width: 100%;
  }
  .need {
    display: grid;
    grid-template-columns: 1.1rem 1fr auto;
    column-gap: 0.4rem;
    font-size: var(--text-2xs);
  }
  .need-mark {
    grid-row: span 2;
    width: 1.1rem;
    height: 1.1rem;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: var(--text-2xs);
    font-weight: 700;
    border: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .need.warn .need-mark {
    border-color: var(--usage-warn);
    color: var(--usage-warn);
  }
  .need-text,
  .need-detail {
    grid-column: 2;
  }
  .need-detail {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .notice {
    font-size: var(--text-2xs);
    margin: 0.2rem 0;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--border);
  }
  .notice.warn {
    border-left-color: var(--usage-warn);
  }
  .action-panel {
    margin-top: 0.5rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .apply {
    position: sticky;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0;
    background: var(--bg);
    border-top: 1px solid var(--border);
    font-size: var(--text-2xs);
  }
  .apply span {
    margin-right: auto;
    color: var(--fg-muted);
  }
  .sync {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .by-person {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  .by-person th {
    text-align: left;
    font-weight: 500;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
    padding: 0.2rem 0.4rem;
  }
  .by-person td {
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid var(--border);
    font-variant-numeric: tabular-nums;
  }
  .by-person th:not(:first-child),
  .by-person td:not(:first-child) {
    text-align: right;
  }
</style>
