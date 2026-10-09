<script lang="ts">
  // One setting on a generated page. Everything shown comes from the
  // registry's descriptor — label, help, bounds, unit, danger, restart,
  // owner — so a page spec only says *where* the setting goes. Writes go
  // through `set_fleet_setting` (the same validated path as before); the
  // backend's E_INVALID message is what the row shows when it refuses.
  //
  // G1.5: on a page with a draft (`drafts`), a typed value — a number, a
  // duration, a choice, a line of text — is staged and written by the
  // page's Save bar; a switch or other toggle still saves at once and offers
  // Undo in a toast. Under the help, a scope pill says where the value lives
  // and, when it differs from the default, what it was changed from.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { setFleetSetting, type SettingKey } from '../fleet_settings';
  import type { Result } from '../result';
  import {
    fromDisplay,
    optionLabel,
    rangeText,
    toDisplay,
    UNIT_WORDS,
    type Descriptor,
    type Widget,
  } from './pages';
  import { ago } from './resources';
  import { scopeWords } from './pages';
  import type { SettingDrafts } from './drafts.svelte';
  import { push } from '../toasts';
  import {
    decideProposals,
    settingHistory,
    valueInWords,
    whoWords,
    type SettingAudit,
    type SettingProposal,
  } from './review';

  let {
    d,
    value,
    widget,
    hint,
    highlighted = false,
    readonly: forceReadonly = false,
    proposal,
    save,
    drafts,
    remote = false,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    d: Descriptor;
    value: string;
    widget?: Widget;
    hint?: string;
    highlighted?: boolean;
    /** Show without editing (a hub client, where the backend refuses). */
    readonly?: boolean;
    /** An agent's pending proposal for this setting (P5): shown as a
     *  suggestion the person applies or rejects, never applied by itself. */
    proposal?: SettingProposal;
    /** Write somewhere else than the fleet's setting (an org's own value,
     *  org administration phase C). The row then has no History or Reset:
     *  its owner offers Inherit instead. */
    save?: (next: string) => Promise<Result<unknown>>;
    /** The page's unsaved typed values (G1.5): a typed widget stages into
     *  it instead of writing. Absent: every change writes at once. */
    drafts?: SettingDrafts;
    /** A paired desktop: the value lives on the hub (the scope pill). */
    remote?: boolean;
    now?: () => number;
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
    time_range: 'text',
  };

  /** Widgets whose control is one input a <label for> can point at. */
  const LABELLED_INPUTS = new Set<Widget>(['switch', 'number', 'duration', 'select', 'text', 'textarea']);

  const readonly = $derived(forceReadonly || d.owned_by !== undefined);
  // UX audit S2: a row is name and help on the left, the control on the
  // right (Settings board); these widgets need the row's width instead.
  const WIDE = new Set<Widget>(['text', 'textarea', 'key_value_table', 'multiselect']);
  const w = $derived<Widget>(readonly ? 'readonly' : (widget ?? DEFAULT_WIDGET[d.kind.type]));
  const modified = $derived(value !== d.default);
  /** Widgets that stage into the page's draft; the rest are toggles that
   *  write at once with Undo. */
  const BATCHED = new Set<Widget>(['number', 'duration', 'select', 'text', 'textarea', 'key_value_table']);
  const batched = $derived(drafts !== undefined && !save && BATCHED.has(w));
  /** What the control shows: the staged value while there is one. */
  const current = $derived(batched && drafts ? drafts.valueOf(d.key, value) : value);
  const staged = $derived(batched && drafts !== undefined && drafts.has(d.key));
  const scope = $derived(save ? null : scopeWords(d, remote));
  const draftError = $derived(drafts?.errors[d.key] ?? null);
  const id = $derived(`setting-${d.key.replace(/[^a-z0-9]+/gi, '-')}`);
  const range = $derived(rangeText(d));
  const unitWord = $derived(UNIT_WORDS[d.unit]);
  const options = $derived(
    d.kind.type === 'choice' || d.kind.type === 'choice_set' ? d.kind.options : [],
  );
  const chosen = $derived(new Set(current.split(',').map((s) => s.trim()).filter(Boolean)));

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

  async function commit(next: string): Promise<boolean> {
    busy = true;
    error = null;
    const r = save ? await save(next) : await setFleetSetting(d.key as SettingKey, next);
    busy = false;
    if (!r.ok) error = r.error.message;
    return r.ok;
  }

  /** Write a toggle at once, then offer to put the old value back. */
  async function commitWithUndo(next: string) {
    const before = value;
    if (!(await commit(next))) return;
    push({
      kind: 'success',
      message: `${d.label}: ${valueInWords(d, next)}`,
      action: { label: 'Undo', run: () => void commit(before) },
    });
  }

  /** Stage a typed value, or write a toggle. */
  function apply(next: string) {
    if (batched && drafts) {
      error = null;
      drafts.stage(d.key, next, value);
    } else void commitWithUndo(next);
  }

  function write(next: string) {
    if (next === current) return;
    if (needsConfirm(next)) {
      pending = next;
      return;
    }
    apply(next);
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
      const v: unknown = JSON.parse(current || '{}');
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

  // P5: the agent's suggestion, and who changed this setting before.
  let whyOpen = $state(false);
  async function decideSuggestion(apply: boolean) {
    if (!proposal) return;
    busy = true;
    error = null;
    const r = await decideProposals(apply ? [proposal.id] : [], apply ? [] : [proposal.id]);
    busy = false;
    if (!r.ok) error = r.error.message;
    else if (r.value.failed.length) error = r.value.failed[0].error;
  }

  let history = $state<SettingAudit[] | null>(null);
  let historyOpen = $state(false);
  async function toggleHistory() {
    historyOpen = !historyOpen;
    if (!historyOpen) return;
    const r = await settingHistory(d.key);
    if (r.ok) history = r.value;
    else error = r.error.message;
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
  class:wide={WIDE.has(w)}
  class:modified
  class:staged
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
    {#if !readonly && !save && d.owned_by === undefined}
      <button
        type="button"
        class="btn btn--quiet history-btn"
        aria-expanded={historyOpen}
        data-testid={`setting-history-${d.key}`}
        onclick={() => void toggleHistory()}>History</button
      >
    {/if}
    {#if staged}<span class="tag unsaved" data-testid={`setting-unsaved-${d.key}`}>not saved</span>{/if}
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
        value={toDisplay(d, current)}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={onNumber} />
      {#if unitWord}<span class="unit">{unitWord}</span>{/if}
    {:else if w === 'select'}
      <select
        {id}
        value={current}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          const next = el.value;
          el.value = current;
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
        value={current}
        disabled={busy}
        aria-describedby={`${id}-help`}
        data-testid={id}
        onchange={(e) => write((e.currentTarget as HTMLInputElement).value.trim())} />
    {:else if w === 'textarea'}
      <textarea
        {id}
        rows="3"
        value={current}
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

  {#if proposal && !readonly}
    <div class="suggestion" data-testid={`setting-suggestion-${d.key}`}>
      <span class="chip">✦ suggested by {whoWords(proposal.source, proposal.source_detail)}</span>
      <span class="proposed" data-testid={`setting-suggestion-value-${d.key}`}>{valueInWords(d, proposal.value)}</span>
      <button
        type="button"
        class="btn"
        disabled={busy}
        data-testid={`setting-suggestion-apply-${d.key}`}
        aria-label={`Apply the suggested ${d.label}`}
        onclick={() => void decideSuggestion(true)}>✓ Apply</button
      >
      <button
        type="button"
        class="btn btn--quiet"
        disabled={busy}
        data-testid={`setting-suggestion-reject-${d.key}`}
        aria-label={`Reject the suggested ${d.label}`}
        onclick={() => void decideSuggestion(false)}>✗ Not this</button
      >
      {#if proposal.why}
        <button type="button" class="btn btn--quiet" aria-expanded={whyOpen} onclick={() => (whyOpen = !whyOpen)}
          >Why?</button
        >
        {#if whyOpen}<p class="why" data-testid={`setting-suggestion-why-${d.key}`}>“{proposal.why}”</p>{/if}
      {/if}
    </div>
  {/if}

  <p class="help" id={`${id}-help`}>
    {d.help}
    {#if range}<span class="range">({range})</span>{/if}
    {#if hint}<span class="hint">{hint}</span>{/if}
    {#if d.owned_by}<span class="owner">Change it with {d.owned_by}.</span>{/if}
  </p>
  {#if scope}
    <p class="scope" data-testid={`setting-scope-${d.key}`}>
      <span class="pill" title={scope.title} data-testid={`setting-scope-pill-${d.key}`}>{scope.pill}</span>
      {#if scope.note}<span>{scope.note}</span>{/if}
      {#if modified}
        {#if scope.note}<span aria-hidden="true">·</span>{/if}
        <span data-testid={`setting-changed-${d.key}`}>changed from {valueInWords(d, d.default)}</span>
        {#if !readonly}
          <span aria-hidden="true">·</span>
          <button
            type="button"
            class="reset"
            data-testid={`setting-reset-${d.key}`}
            disabled={busy}
            onclick={() => write(d.default)}>Reset</button
          >
        {/if}
      {:else if !scope.note}
        <span>default</span>
      {/if}
    </p>
  {/if}
  {#if error ?? draftError}<p class="err" role="alert" data-testid={`setting-error-${d.key}`}>{error ?? draftError}</p>{/if}
  {#if historyOpen}
    <div class="history" data-testid={`setting-history-list-${d.key}`}>
      {#if history === null}
        loading…
      {:else if history.length === 0}
        No changes recorded yet.
      {:else}
        <ul>
          {#each history as h (h.id)}
            <li>
              {ago(h.at, now())} · {h.before == null ? `default (${valueInWords(d, d.default)})` : valueInWords(d, h.before)}
              → {valueInWords(d, h.after)} · {whoWords(h.actor, h.actor_detail)}{#if h.proposal_id}, applying proposal #{h.proposal_id}{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
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
      if (next !== null) apply(next);
    }}
    oncancel={() => (pending = null)} />
{/if}

<style>
  .field {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: var(--space-4);
    align-items: center;
    border-left: 2px solid transparent;
    border-bottom: 1px solid var(--border);
    padding: var(--space-3) 0 var(--space-3) 0.6rem;
    margin-left: -0.6rem;
  }
  .field > * { grid-column: 1 / -1; }
  .field:not(.wide) > .head { grid-column: 1; grid-row: 1; }
  .field:not(.wide) > .help { grid-column: 1; grid-row: 2; }
  .field:not(.wide) > .control {
    grid-column: 2;
    grid-row: 1 / span 3;
    justify-self: end;
    margin-top: 0;
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
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .history-btn {
    margin-left: auto;
  }
  .history-btn {
    font-size: var(--text-2xs);
  }
  .control {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-top: 0.25rem;
    flex-wrap: wrap;
  }
  .control input:not([type='checkbox']):not([type='radio']),
  .control select,
  .control textarea {
    min-height: var(--control-h);
    padding: 0 var(--control-px);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font-size: var(--control-font);
  }
  .control textarea { padding: 4px var(--control-px); }
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
    font-size: var(--text-2xs);
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
    font-size: var(--text-2xs);
  }
  .readonly {
    font-family: var(--mono);
    font-size: var(--control-font);
  }
  .help {
    margin: 0.15rem 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .field.staged {
    border-left-color: var(--accent);
    border-left-style: dashed;
  }
  .unsaved {
    color: var(--fg-muted);
  }
  .scope {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.3rem;
    margin: 0.15rem 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .field:not(.wide) > .scope { grid-column: 1; grid-row: 3; }
  .pill {
    padding: 0 0.35rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-sunk);
    color: var(--fg-2);
  }
  .scope .reset {
    min-block-size: var(--control-h);
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .scope .reset:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .range,
  .hint,
  .owner {
    margin-left: 0.25rem;
  }
  .err {
    margin: 0.2rem 0 0;
    font-size: var(--text-2xs);
    color: var(--usage-crit);
  }
  .suggestion {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    margin-top: 0.3rem;
    padding: 0.3rem 0.5rem;
    border: 1px dashed var(--accent);
    border-radius: var(--radius-sm);
    font-size: var(--text-2xs);
  }
  .suggestion .chip {
    color: var(--accent);
  }
  .suggestion .proposed {
    font-weight: 600;
    opacity: 0.85;
  }
  .suggestion .why {
    flex-basis: 100%;
    margin: 0;
    color: var(--fg-muted);
  }
  .history {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin-top: 0.25rem;
  }
  .history ul {
    margin: 0;
    padding-left: 1rem;
  }
</style>
