<script lang="ts">
  // One chat form, step by step. The field markup is FlowView's; what is
  // asked follows the answers (form_model.ts). A secret is cleared once
  // sent and never prefilled. A choice of up to nine options is drawn as
  // numbered options; when it is the step's only one, 1–9 pick from it
  // (step 10.1), as on the Conversation's question card.
  import { untrack } from 'svelte';
  import { destination } from '../destination';
  import { matchShortcut } from '../shortcuts';
  import { detectMac, isEditable } from '../terminal_keys';
  import { startingValues, stepProblems, visibleSteps } from './form_model';
  import type { FieldProblem, FormField, FormProposal, FormSpec, Values } from './forms';
  import ProposedBy from '../ProposedBy.svelte';
  import { neverDecidesField, preselect } from '../ai_proposal';
  import { QUICK_ANSWER, risky } from '../quick_answer';
  import Loader from '../Loader.svelte';
  import { fieldCount, submitKey } from './form_frame';

  let {
    spec,
    busy = false,
    disabled = false,
    serverProblems = [],
    proposal = null,
    initial = {},
    sending = 'Sending…',
    buttonLoader = true,
    ownDefaults = false,
    onsubmit,
    onunplaced,
    oncancel,
  }: {
    spec: FormSpec;
    /** Jev's likely option for one choice (step 10.9): shown first and
     *  pre-selected when the field is empty; never a risky option. */
    proposal?: FormProposal | null;
    busy?: boolean;
    disabled?: boolean;
    serverProblems?: FieldProblem[];
    /** Starting values over the spec's own defaults (a wizard opened with
     *  what the screen already knows). Read once, as the defaults are. */
    initial?: Values;
    /** The last button while `busy`, after a Comet (step 10.12). */
    sending?: string;
    /** The Comet in the submit button while sending; off when the host
     *  draws the flow's own loader (one loader per screen, review r12). */
    buttonLoader?: boolean;
    /** The spec is the app's own (a wizard), so its defaults stand. An
     *  agent's form starts with no risky default (`startingValues`). */
    ownDefaults?: boolean;
    onsubmit: (values: Values) => void;
    /** Server problems whose field is on no visible step (nowhere to show them). */
    onunplaced?: (problems: FieldProblem[]) => void;
    /** A quiet Cancel at the row's start (a wizard in a dialog). */
    oncancel?: () => void;
  } = $props();
  const uid = $props.id();

  // The spec of one form never changes under the wizard (FormCard unmounts the
  // wizard while it loads another form), so the starting values are read once on purpose.
  // svelte-ignore state_referenced_locally
  const started: Values = { ...startingValues(spec, !ownDefaults), ...initial };
  let values = $state<Values>({ ...started });
  let index = $state(0);
  const steps = $derived(visibleSteps(spec, values));
  const step = $derived(steps[Math.min(index, steps.length - 1)]);
  const last = $derived(index >= steps.length - 1);
  const localProblems = $derived(stepProblems(spec, Math.min(index, steps.length - 1), values));
  const ready = $derived(localProblems.length === 0);

  // Checked on blur and on submit, never on each key (FormsAnatomy): a
  // field's own problem shows once the person has left it, or once they
  // tried to go on with the step unfinished.
  let touched = $state<Record<string, true>>({});
  let tried = $state(false);
  function leave(name: string) {
    if (!touched[name]) touched = { ...touched, [name]: true };
  }
  const problemOf = (name: string) =>
    serverProblems.find((p) => p.field === name)?.problem ??
    (touched[name] || tried ? (localProblems.find((p) => p.field === name)?.problem ?? null) : null);
  /** Why Next / the last button is off, said under it. */
  const why = $derived.by(() => {
    const p = localProblems[0];
    if (!p) return null;
    const label = step?.fields.find((f) => f.name === p.field)?.label ?? p.field;
    return `${label.replace(/[\s:*]+$/, '')} ${p.problem}.`;
  });

  /** Whether anything differs from where the form started. */
  export function isDirty(): boolean {
    const keys = new Set([...Object.keys(values), ...Object.keys(started)]);
    for (const k of keys) {
      const a = values[k];
      const b = started[k];
      const blank = (v: unknown) => v === undefined || v === '' || (Array.isArray(v) && v.length === 0);
      if (blank(a) && blank(b)) continue;
      if (JSON.stringify(a) !== JSON.stringify(b)) return true;
    }
    return false;
  }

  // A problem on another step is invisible where the user stands: go to the
  // first visible step that holds one. Only a new problem list moves the
  // wizard, never the user's own typing (hence untrack).
  $effect(() => {
    const probs = serverProblems;
    if (probs.length === 0) return;
    untrack(() => {
      const names = new Set(probs.map((p) => p.field));
      const at = steps.findIndex((s) => s.fields.some((f) => names.has(f.name)));
      if (at >= 0) index = at;
      else onunplaced?.(probs);
    });
  });

  function set(name: string, v: unknown) {
    values = { ...values, [name]: v };
  }

  // Whether `busy` is this wizard's own submit (the card is also busy while
  // it declines, and that is not "Sending…").
  let sent = $state(false);
  $effect(() => {
    if (!busy) sent = false;
  });

  function next() {
    index += 1;
    tried = false;
  }

  // Enter in a one-field step and ⌘↵ / Ctrl+Enter anywhere go on: Next, or
  // the last button. An unfinished step shows its problems instead.
  let stepEl: HTMLDivElement | undefined = $state();
  function onFormKeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || off) return;
    if (!submitKey(e, fieldCount(stepEl), isMac)) return;
    e.preventDefault();
    if (!ready) tried = true;
    else if (last) submit();
    else next();
  }

  /** Only what a visible field holds is sent: a hidden step's values stay
   *  behind, as the backend would drop them anyway. */
  function submit() {
    sent = true;
    const shown = new Set(steps.flatMap((s) => s.fields.map((f) => f.name)));
    const out: Values = {};
    for (const [k, v] of Object.entries(values)) if (shown.has(k)) out[k] = v;
    onsubmit(out);
  }

  export function clearSecrets() {
    const next = { ...values };
    for (const s of spec.steps) for (const f of s.fields) if (f.type === 'secret') delete next[f.name];
    values = next;
  }

  const off = $derived(busy || disabled);
  const str = (f: FormField) => (typeof values[f.name] === 'string' ? (values[f.name] as string) : '');

  // J5 quick answer: the proposed option goes first and is pre-selected when
  // nothing is chosen yet; "Change" puts the order back and clears it.
  let dismissed = $state(false);
  const proposed = $derived.by(() => {
    if (dismissed || !proposal || preselect(QUICK_ANSWER, proposal) === null) return null;
    const f = spec.steps.flatMap((s) => s.fields).find((x) => x.name === proposal.field);
    if (!f || neverDecidesField(f)) return null;
    const opt = f.type === 'select' ? f.options?.find(([v]) => v === proposal.value) : undefined;
    return opt && !risky(opt[1]) && !risky(opt[0]) ? proposal : null;
  });
  $effect(() => {
    const p = proposed;
    if (!p) return;
    untrack(() => {
      if (values[p.field] === undefined) set(p.field, p.value);
    });
  });
  /** A field's options in the order shown: the proposed one first. */
  function optionsOf(f: FormField): [string, string][] {
    const all = f.options ?? [];
    if (!proposed || proposed.field !== f.name) return all;
    const at = all.findIndex(([v]) => v === proposed.value);
    return at < 0 ? all : [all[at], ...all.slice(0, at), ...all.slice(at + 1)];
  }
  function dismissProposal(f: FormField) {
    if (proposed && values[f.name] === proposed.value) set(f.name, undefined);
    dismissed = true;
  }

  /** A choice short enough to number: a select or multiselect of 1–9 options. */
  const numbered = (f: FormField) =>
    (f.type === 'select' || f.type === 'multiselect') && (f.options?.length ?? 0) > 0 && (f.options?.length ?? 0) <= 9;
  /** The field 1–9 answer: the step's only numbered choice, or none. */
  const keyField = $derived.by(() => {
    const choices = step?.fields.filter(numbered) ?? [];
    return choices.length === 1 ? choices[0] : null;
  });

  function pick(f: FormField, v: string) {
    if (f.type === 'multiselect') {
      const cur = Array.isArray(values[f.name]) ? (values[f.name] as string[]) : [];
      set(f.name, cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v]);
    } else {
      // Picking the chosen option of an optional choice clears it, as the
      // dropdown's empty entry did.
      set(f.name, values[f.name] === v && !f.required ? undefined : v);
    }
  }

  // 1–9: only while the session view shows, no field, terminal or dialog has
  // the keyboard, and no question card takes the digits first.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  function onWindowKeydown(e: KeyboardEvent) {
    const f = keyField;
    if (!f || off || e.defaultPrevented || $destination !== 'session') return;
    if (matchShortcut('form-card', e, isMac) !== 'form-card.option') return;
    // A key sent to the window itself has no element to ask.
    const target = e.target instanceof HTMLElement ? e.target : null;
    if (isEditable(target) || target?.dataset?.imeProxy !== undefined || target?.closest?.('dialog')) return;
    if (document.querySelector('[data-testid="answer-card"]:not(.compact)')) return;
    const o = optionsOf(f)[Number(e.key) - 1];
    if (!o) return;
    e.preventDefault();
    pick(f, o[0]);
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="wizard" onkeydown={onFormKeydown}>
  {#if steps.length > 1}
    <div class="count" data-testid="form-step-count">Step {index + 1} of {steps.length}</div>
  {/if}
  {#if step}
    <h6 data-testid="form-step-title">{step.title}</h6>
    {#if step.intro}<p class="intro">{step.intro}</p>{/if}
    <div class="fields" bind:this={stepEl}>
    {#each step.fields as f (f.name)}
      <div class="field" onfocusout={() => leave(f.name)}>
        {#if f.type === 'bool'}
          <label class="check">
            <input
              type="checkbox"
              data-testid={`form-field-${f.name}`}
              checked={values[f.name] === true}
              disabled={off}
              onchange={(e) => set(f.name, (e.currentTarget as HTMLInputElement).checked)} />
            {f.label}
          </label>
        {:else if f.type === 'multiselect'}
          <span class="label">{f.label}</span>
          {#each f.options ?? [] as [v, l], i (v)}
            <label class="check">
              <input
                type="checkbox"
                data-testid={`form-field-${f.name}-${v}`}
                checked={Array.isArray(values[f.name]) && (values[f.name] as string[]).includes(v)}
                disabled={off}
                onchange={(e) => {
                  const cur = Array.isArray(values[f.name]) ? (values[f.name] as string[]) : [];
                  set(f.name, (e.currentTarget as HTMLInputElement).checked ? [...cur, v] : cur.filter((x) => x !== v));
                }} />
              {#if keyField === f}<span class="kbd" aria-hidden="true">{i + 1}</span>{/if}
              {l}
            </label>
          {/each}
        {:else if f.type === 'select' && numbered(f)}
          <span class="label" id={`${uid}-${f.name}`}>{f.label}{f.required ? ' *' : ''}</span>
          <div class="options" role="radiogroup" aria-labelledby={`${uid}-${f.name}`}>
            {#each optionsOf(f) as [v, l], i (v)}
              <button
                type="button"
                role="radio"
                class="opt"
                class:on={values[f.name] === v}
                class:ai-pre={proposed?.field === f.name && proposed.value === v && values[f.name] === v}
                aria-checked={values[f.name] === v}
                data-testid={`form-field-${f.name}-${v}`}
                disabled={off}
                onclick={() => pick(f, v)}>
                {#if keyField === f}<span class="kbd" aria-hidden="true">{i + 1}</span>{/if}
                {l}
              </button>
            {/each}
          </div>
          {#if proposed?.field === f.name}
            <ProposedBy
              proposal={proposed}
              field={QUICK_ANSWER}
              testid={`form-proposed-${f.name}`}
              onchange={() => dismissProposal(f)} />
          {/if}
        {:else}
          <label for={`${uid}-${f.name}`}>{f.label}{f.required ? ' *' : ''}</label>
          {#if f.type === 'select'}
            <select
              id={`${uid}-${f.name}`}
              data-testid={`form-field-${f.name}`}
              class:ai-pre={proposed?.field === f.name && values[f.name] === proposed.value}
              value={str(f)}
              disabled={off}
              onchange={(e) => set(f.name, (e.currentTarget as HTMLSelectElement).value || undefined)}>
              <option value="">—</option>
              {#each f.options ?? [] as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
          {:else if f.type === 'textarea'}
            <textarea
              id={`${uid}-${f.name}`}
              rows="3"
              placeholder={f.placeholder ?? ''}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={off}
              oninput={(e) => set(f.name, (e.currentTarget as HTMLTextAreaElement).value)}></textarea>
          {:else if f.type === 'number'}
            <input
              id={`${uid}-${f.name}`}
              type="number"
              min={f.min}
              max={f.max}
              step={f.integer ? 1 : 'any'}
              data-testid={`form-field-${f.name}`}
              value={typeof values[f.name] === 'number' ? values[f.name] : ''}
              disabled={off}
              oninput={(e) => {
                const raw = (e.currentTarget as HTMLInputElement).value;
                set(f.name, raw === '' ? undefined : Number(raw));
              }} />
          {:else}
            <input
              id={`${uid}-${f.name}`}
              type={f.type === 'secret' ? 'password' : 'text'}
              autocomplete="off"
              spellcheck="false"
              placeholder={f.placeholder ?? ''}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={off}
              oninput={(e) => set(f.name, (e.currentTarget as HTMLInputElement).value)} />
          {/if}
        {/if}
        {#if f.help}<span class="help">{f.help}</span>{/if}
        {#if problemOf(f.name)}<span class="err" data-testid={`form-problem-${f.name}`}>{problemOf(f.name)}</span>{/if}
      </div>
    {/each}
    </div>
  {/if}
  <div class="row">
    {#if oncancel}
      <button type="button" class="cancel" data-testid="form-cancel" disabled={busy} onclick={oncancel}>Cancel</button>
    {/if}
    {#if index > 0}
      <button type="button" data-testid="form-back" disabled={busy} onclick={() => ((index -= 1), (tried = false))}>Back</button>
    {/if}
    {#if last}
      <button type="button" class="primary" data-testid="form-submit" disabled={off || !ready} onclick={submit}>
        {#if busy && sent}{#if buttonLoader}<Loader name="comet" size={12} class="btn-loader" />{/if}{sending}{:else}{spec.submit ?? 'Submit'}{/if}
      </button>
    {:else}
      <button type="button" class="primary" data-testid="form-next" disabled={off || !ready} onclick={next}>Next</button>
    {/if}
  </div>
  {#if why && !off}<p class="why" data-testid="form-submit-why">{why}</p>{/if}
</div>

<style>
  .wizard { display: flex; flex-direction: column; gap: 0.6rem; }
  .count { font-size: var(--text-2xs); color: var(--fg-muted); }
  h6 { margin: 0; font-size: var(--text-sm); }
  .intro { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .fields { display: flex; flex-direction: column; gap: 0.6rem; }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .why { margin: -0.3rem 0 0; text-align: right; font-size: var(--text-2xs); color: var(--fg-muted); }
  label, .label { font-size: var(--text-2xs); }
  .check { display: flex; gap: 0.35rem; align-items: flex-start; }
  input:not([type='checkbox']), select, textarea { font: inherit; font-size: var(--text-2xs); padding: 0.25rem 0.4rem; }
  .help { font-size: var(--text-2xs); color: var(--fg-muted); }
  .err { font-size: var(--text-2xs); color: var(--usage-crit); }
  .row { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .row .cancel { margin-right: auto; }
  .options { display: flex; flex-direction: column; gap: 0.15rem; }
  .opt { display: flex; gap: 0.45rem; align-items: center; text-align: left; font: inherit; font-size: var(--text-2xs); padding: 0.25rem 0.4rem; border: 1px solid transparent; border-radius: var(--radius-sm); background: none; color: inherit; cursor: pointer; }
  .opt:hover:not(:disabled) { background: var(--bg-hover); }
  .opt.on { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .kbd { font-size: var(--text-2xs); line-height: 1; padding: 0.1rem 0.3rem; border: 1px solid var(--border); border-radius: var(--radius-xs); color: var(--fg-muted); }
</style>
