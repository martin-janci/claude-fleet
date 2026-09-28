<script lang="ts">
  // One setting on a generated page. Everything shown comes from the
  // registry's descriptor — label, help, bounds, unit, danger, restart,
  // owner — so a page spec only says *where* the setting goes. Writes go
  // through `set_fleet_setting` (the same validated path as before) and are
  // saved as they change; the backend's E_INVALID message is what the row
  // shows when it refuses.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { setFleetSetting, type SettingKey } from '../fleet_settings';
  import {
    fromDisplay,
    optionLabel,
    rangeText,
    toDisplay,
    UNIT_WORDS,
    type Descriptor,
    type Widget,
  } from './pages';

  let {
    d,
    value,
    widget,
    hint,
    highlighted = false,
    readonly: forceReadonly = false,
  }: {
    d: Descriptor;
    value: string;
    widget?: Widget;
    hint?: string;
    highlighted?: boolean;
    /** Show without editing (a hub client, where the backend refuses). */
    readonly?: boolean;
  } = $props();

  const DEFAULT_WIDGET: Record<Descriptor['kind']['type'], Widget> = {
    bool: 'switch',
    secs: 'duration',
    int: 'number',
    choice: 'select',
    choice_set: 'multiselect',
    path_map: 'key_value_table',
    id_set: 'id_list',
    price_map: 'textarea',
    text: 'text',
  };

  /** Widgets whose control is one input a <label for> can point at. */
  const LABELLED_INPUTS = new Set<Widget>(['switch', 'number', 'duration', 'select', 'text', 'textarea']);

  const readonly = $derived(forceReadonly || d.owned_by !== undefined);
  const w = $derived<Widget>(readonly ? 'readonly' : (widget ?? DEFAULT_WIDGET[d.kind.type]));
  const modified = $derived(value !== d.default);
  const id = $derived(`setting-${d.key.replace(/[^a-z0-9]+/gi, '-')}`);
  const range = $derived(rangeText(d));
  const unitWord = $derived(UNIT_WORDS[d.unit]);
  const options = $derived(
    d.kind.type === 'choice' || d.kind.type === 'choice_set' ? d.kind.options : [],
  );
  const chosen = $derived(new Set(value.split(',').map((s) => s.trim()).filter(Boolean)));

  /** A read-only value in words: On / Off, an option's label, a number in
   *  its unit. */
  const shown = $derived.by(() => {
    if (value === '') return '—';
    if (d.kind.type === 'bool') return value === 'true' ? 'On' : 'Off';
    if (d.kind.type === 'choice') return optionLabel(d, value);
    if (d.kind.type === 'secs' || d.kind.type === 'int') return `${toDisplay(d, value)}${unitWord ? ` ${unitWord}` : ''}`;
    return value;
  });

  let busy = $state(false);
  let error = $state<string | null>(null);
  let pending = $state<string | null>(null);

  function needsConfirm(next: string): boolean {
    return d.danger.level === 'confirm' && next !== d.default;
  }

  async function commit(next: string) {
    busy = true;
    error = null;
    const r = await setFleetSetting(d.key as SettingKey, next);
    busy = false;
    if (!r.ok) error = r.error.message;
  }

  function write(next: string) {
    if (next === value) return;
    if (needsConfirm(next)) {
      pending = next;
      return;
    }
    void commit(next);
  }

  function onNumber(e: Event) {
    const r = fromDisplay(d, (e.currentTarget as HTMLInputElement).value);
    if ('error' in r) {
      error = `${d.label}: ${r.error}`;
      return;
    }
    write(r.value);
  }

  function toggleOption(opt: string) {
    const next = new Set(chosen);
    if (next.has(opt)) next.delete(opt);
    else next.add(opt);
    write(options.filter((o) => next.has(o)).join(','));
  }

  function onJson(e: Event) {
    const raw = (e.currentTarget as HTMLTextAreaElement).value.trim();
    try {
      const v: unknown = JSON.parse(raw === '' ? (d.kind.type === 'id_set' ? '[]' : '{}') : raw);
      const wantArray = d.kind.type === 'id_set';
      if (wantArray !== Array.isArray(v) || typeof v !== 'object' || v === null) {
        error = `${d.label}: must be a JSON ${wantArray ? 'array' : 'object'}`;
        return;
      }
      write(JSON.stringify(v));
    } catch {
      error = `${d.label}: not valid JSON`;
    }
  }

  // key_value_table: an alias → path map, edited as rows.
  const entries = $derived.by<[string, string][]>(() => {
    try {
      const v: unknown = JSON.parse(value || '{}');
      return v && typeof v === 'object' && !Array.isArray(v)
        ? Object.entries(v as Record<string, string>)
        : [];
    } catch {
      return [];
    }
  });
  let newKey = $state('');
  let newVal = $state('');
  function writeEntries(rows: [string, string][]) {
    write(JSON.stringify(Object.fromEntries(rows.filter(([k]) => k.trim() !== ''))));
  }

  const ids = $derived.by<number[]>(() => {
    try {
      const v: unknown = JSON.parse(value || '[]');
      return Array.isArray(v) ? (v as number[]) : [];
    } catch {
      return [];
    }
  });
</script>

<div
  class="field"
  class:modified
  class:highlighted
  data-testid={`setting-row-${d.key}`}
  data-setting-key={d.key}>
  <div class="head">
    {#if LABELLED_INPUTS.has(w)}
      <label class="label" id={`${id}-label`} for={id}>{d.label}</label>
    {:else}
      <span class="label" id={`${id}-label`}>{d.label}</span>
    {/if}
    {#if d.tags.includes('experimental')}<span class="tag">experimental</span>{/if}
    {#if d.restart === 'app'}<span class="tag" title="Read once at launch">applies after a restart</span>{/if}
    {#if d.restart === 'hooks'}<span class="tag" title="Takes effect on each host's next hook install">applies when hooks are reinstalled</span>{/if}
    {#if modified && !readonly}
      <button
        type="button"
        class="btn btn--quiet reset"
        data-testid={`setting-reset-${d.key}`}
        disabled={busy}
        title={`Reset to the default (${d.default === '' ? 'empty' : d.default})`}
        onclick={() => write(d.default)}>Reset</button
      >
    {/if}
  </div>

  <div class="control">
    {#if w === 'switch'}
      <input
        {id}
        type="checkbox"
        role="switch"
        checked={value === 'true'}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={(e) => {
          // Controlled: the box shows the stored value until the write
          // lands (or a cancelled confirmation leaves it as it was).
          const el = e.currentTarget as HTMLInputElement;
          const next = el.checked ? 'true' : 'false';
          el.checked = value === 'true';
          write(next);
        }} />
    {:else if w === 'number' || w === 'duration'}
      <input
        {id}
        class="num"
        type="number"
        min="0"
        step="any"
        value={toDisplay(d, value)}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={onNumber} />
      {#if unitWord}<span class="unit">{unitWord}</span>{/if}
    {:else if w === 'select'}
      <select
        {id}
        {value}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          const next = el.value;
          el.value = value;
          write(next);
        }}>
        {#each options as o (o)}<option value={o}>{optionLabel(d, o)}</option>{/each}
      </select>
    {:else if w === 'radio'}
      <div role="radiogroup" aria-labelledby={`${id}-label`} data-testid={id}>
        {#each options as o (o)}
          <label class="choice">
            <input
              type="radio"
              name={id}
              value={o}
              checked={value === o}
              disabled={busy}
              data-testid={`${id}-${o}`}
              onchange={(e) => {
                // Controlled, like the switch: the stored choice stays
                // checked until the write lands.
                const group = (e.currentTarget as HTMLElement).closest('[role="radiogroup"]');
                group?.querySelectorAll<HTMLInputElement>('input').forEach((r) => (r.checked = r.value === value));
                write(o);
              }} />
            {optionLabel(d, o)}
          </label>
        {/each}
      </div>
    {:else if w === 'multiselect'}
      <div class="choices" role="group" aria-labelledby={`${id}-label`} data-testid={id}>
        {#each options as o (o)}
          <label class="choice">
            <input
              type="checkbox"
              checked={chosen.has(o)}
              disabled={busy}
              data-testid={`${id}-${o}`}
              onchange={(e) => {
                (e.currentTarget as HTMLInputElement).checked = chosen.has(o);
                toggleOption(o);
              }} />
            {optionLabel(d, o)}
          </label>
        {/each}
      </div>
    {:else if w === 'text'}
      <input
        {id}
        class="text"
        type="text"
        {value}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={(e) => write((e.currentTarget as HTMLInputElement).value.trim())} />
    {:else if w === 'textarea'}
      <textarea
        {id}
        rows="3"
        {value}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={onJson}></textarea>
    {:else if w === 'key_value_table'}
      <div class="kv" data-testid={id}>
        {#each entries as [k, v], i (k)}
          <div class="kv-row">
            <span class="kv-key">{k}</span>
            <input
              type="text"
              value={v}
              disabled={busy}
              aria-label={`${d.label}: ${k}`}
              data-testid={`${id}-value-${k}`}
              onchange={(e) =>
                writeEntries(
                  entries.map(([ek, ev], j) =>
                    j === i ? [ek, (e.currentTarget as HTMLInputElement).value.trim()] : [ek, ev],
                  ),
                )} />
            <button
              type="button"
              class="btn btn--quiet"
              disabled={busy}
              aria-label={`Remove ${k}`}
              onclick={() => writeEntries(entries.filter((_, j) => j !== i))}>×</button
            >
          </div>
        {/each}
        <div class="kv-row">
          <input type="text" placeholder="host" bind:value={newKey} disabled={busy} aria-label="New key" />
          <input type="text" placeholder="path" bind:value={newVal} disabled={busy} aria-label="New value" />
          <button
            type="button"
            class="btn"
            disabled={busy || newKey.trim() === '' || newVal.trim() === ''}
            data-testid={`${id}-add`}
            onclick={() => {
              writeEntries([...entries, [newKey.trim(), newVal.trim()]]);
              newKey = '';
              newVal = '';
            }}>Add</button
          >
        </div>
      </div>
    {:else if w === 'id_list'}
      <div class="ids" data-testid={id}>
        <span>{ids.length === 0 ? 'None' : `${ids.length} selected`}</span>
        <button
          type="button"
          class="btn"
          disabled={busy || ids.length === 0}
          data-testid={`${id}-clear`}
          onclick={() => write('[]')}>Clear</button
        >
      </div>
    {:else}
      <span class="readonly" data-testid={id}>{shown}</span>
    {/if}
  </div>

  <p class="help" id={`${id}-help`}>
    {d.help}
    {#if range}<span class="range">({range})</span>{/if}
    {#if hint}<span class="hint">{hint}</span>{/if}
    {#if d.owned_by}<span class="owner">Change it with {d.owned_by}.</span>{/if}
  </p>
  {#if error}<p class="err" role="alert" data-testid={`setting-error-${d.key}`}>{error}</p>{/if}
</div>

{#if pending !== null && d.danger.level === 'confirm'}
  <ConfirmDialog
    title={d.label}
    message={d.danger.message}
    confirmLabel={d.kind.type === 'bool' ? 'Turn it on' : 'Change it'}
    danger
    confirmTestId={`setting-confirm-${d.key}`}
    onconfirm={() => {
      const next = pending;
      pending = null;
      if (next !== null) void commit(next);
    }}
    oncancel={() => (pending = null)} />
{/if}

<style>
  .field {
    border-left: 2px solid transparent;
    padding: 0.35rem 0 0.35rem 0.6rem;
    margin-left: -0.6rem;
  }
  .field.modified {
    border-left-color: var(--accent);
  }
  .field.highlighted {
    background: var(--accent-soft);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .label {
    font-size: 0.85rem;
    font-weight: 500;
  }
  .reset {
    margin-left: auto;
  }
  .control {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-top: 0.25rem;
    flex-wrap: wrap;
  }
  .num {
    width: 7rem;
  }
  .text,
  textarea {
    width: 100%;
    font-family: var(--mono);
    font-size: var(--control-font);
  }
  .unit {
    color: var(--fg-muted);
    font-size: var(--control-font);
  }
  .choices,
  [role='radiogroup'] {
    display: flex;
    gap: 0.75rem;
    flex-wrap: wrap;
  }
  .choice {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.82rem;
  }
  .kv {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    width: 100%;
  }
  .kv-row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .kv-key {
    font-family: var(--mono);
    min-width: 6rem;
  }
  .kv-row input {
    flex: 1;
  }
  .ids {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.82rem;
  }
  .readonly {
    font-family: var(--mono);
    font-size: var(--control-font);
  }
  .help {
    margin: 0.2rem 0 0;
    font-size: 0.75rem;
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .range,
  .hint,
  .owner {
    margin-left: 0.25rem;
  }
  .err {
    margin: 0.2rem 0 0;
    font-size: 0.75rem;
    color: var(--usage-crit);
  }
</style>
