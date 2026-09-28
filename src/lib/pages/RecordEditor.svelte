<script lang="ts">
  // One record of a resource (layout L4, object editor): its scalar fields
  // are a draft, written together by Apply (the update action, with only the
  // fields that changed; a field with `confirm` is asked about first), and
  // its lists change item by item through their own add / remove actions.
  // The page spec lays the fields out in sections; the resource says what
  // each field is.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import ActionForm from './ActionForm.svelte';
  import { evalCondition, type Section } from './pages';
  import {
    buildArgs,
    encodeField,
    fieldValue,
    idOf,
    itemKey,
    itemLabel,
    itemsOf,
    recordValues,
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
    record,
    readonly = false,
    options,
    run,
  }: {
    resource: ResourceType;
    sections: Section[];
    record: ResourceRecord;
    readonly?: boolean;
    options: (field: FieldSpec, param: string) => { value: string; label: string }[];
    /** Run an action with its arguments; resolves once the list is re-read. */
    run: (action: ActionSpec, args: Record<string, unknown>) => Promise<boolean>;
  } = $props();

  const fieldOf = (id: string) => resource.fields.find((f) => f.id === id);
  const saved = $derived(
    Object.fromEntries(resource.fields.filter((f) => f.type !== 'items').map((f) => [f.id, fieldValue(f, record)])),
  ) as Record<string, FieldValue>;

  // svelte-ignore state_referenced_locally
  let draft = $state<Record<string, FieldValue>>({ ...saved });
  const changed = $derived(
    resource.fields.filter((f) => f.edit && f.type !== 'items' && draft[f.id] !== saved[f.id]),
  );
  const values = $derived({ ...recordValues(resource, record), ...Object.fromEntries(Object.entries(draft).map(([k, v]) => [k, String(v)])) });

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
    const args = buildArgs(update, record, null, {});
    for (const f of changed) args[f.edit!] = encodeField(f, draft[f.id]);
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

  function remove() {
    const del = resource.delete;
    if (!del) return;
    // Removing a record is always asked, whether or not it says why.
    ask(
      `${del.label}: ${titleOf(resource, record)}`,
      [del.confirm ?? `Remove ${titleOf(resource, record)}?`],
      () => void exec(del, buildArgs(del, record, null, {})),
    );
  }

  const set = (id: string, v: FieldValue) => (draft = { ...draft, [id]: v });
</script>

<div class="record" data-testid={`record-${resource.id}-${idOf(resource, record)}`}>
  <header>
    {#if resource.color_field}
      <span class="swatch" style:background={String(record[resource.color_field] ?? '') || 'transparent'}></span>
    {/if}
    <h5>{titleOf(resource, record)}</h5>
    {#if !readonly && resource.delete}
      <button type="button" class="btn btn--quiet" disabled={busy} data-testid="record-delete" onclick={remove}
        >{resource.delete.label}</button
      >
    {/if}
  </header>

  {#each sections.filter((s) => evalCondition(s.when, values)) as section (section.title)}
    <section class="section">
      <h6>{section.title}</h6>
      {#each section.items as item, i (i)}
        {#if item.type === 'notice'}
          <p class={`notice ${item.tone}`}>{item.text}</p>
        {:else if item.type === 'field' && evalCondition(item.when, values)}
          {@const f = fieldOf(item.key)}
          {#if f}
            <div class="field" class:changed={changed.includes(f)} data-testid={`record-field-${f.id}`}>
              <span class="label" id={`rf-${f.id}`}>{f.label}</span>
              <div class="control">
                {#if f.type === 'items'}
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
                      <ActionForm action={a} {busy} options={(p) => options(f, p)} onrun={(params) => runItem(a, null, params)} />
                    {/each}
                  {/if}
                {:else if readonly || !f.edit}
                  <span class="value" data-testid={`value-${f.id}`}>{f.type === 'bool' ? (saved[f.id] ? 'On' : 'Off') : saved[f.id] || '—'}</span>
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
    font-size: 0.95rem;
  }
  h6 {
    margin: 0 0 0.35rem;
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .swatch {
    width: 0.8rem;
    height: 0.8rem;
    border-radius: 3px;
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
    font-size: 0.85rem;
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
    font-size: 0.82rem;
    max-width: 20rem;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }
  .chip {
    font-size: 0.75rem;
    padding: 0.05rem 0.45rem;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border);
  }
  .none,
  .value {
    font-size: 0.8rem;
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
    font-size: 0.82rem;
  }
  .help {
    margin: 0.2rem 0 0;
    font-size: 0.75rem;
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .notice {
    font-size: 0.78rem;
    margin: 0.2rem 0;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--border);
  }
  .notice.warn {
    border-left-color: var(--usage-warn);
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
    font-size: 0.8rem;
  }
  .apply span {
    margin-right: auto;
    color: var(--fg-muted);
  }
</style>
