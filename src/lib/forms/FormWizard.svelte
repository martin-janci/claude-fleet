<script lang="ts">
  // One chat form, step by step. The field markup is FlowView's; what is
  // asked follows the answers (form_model.ts). A secret is cleared once
  // sent and never prefilled.
  import { untrack } from 'svelte';
  import { stepProblems, visibleSteps } from './form_model';
  import type { FieldProblem, FormField, FormSpec, Values } from './forms';

  let {
    spec,
    busy = false,
    disabled = false,
    serverProblems = [],
    onsubmit,
    onunplaced,
  }: {
    spec: FormSpec;
    busy?: boolean;
    disabled?: boolean;
    serverProblems?: FieldProblem[];
    onsubmit: (values: Values) => void;
    /** Server problems whose field is on no visible step (nowhere to show them). */
    onunplaced?: (problems: FieldProblem[]) => void;
  } = $props();
  const uid = $props.id();

  function defaults(s: FormSpec): Values {
    const out: Values = {};
    for (const step of s.steps)
      for (const f of step.fields) if (f.type !== 'secret' && f.value !== undefined) out[f.name] = f.value;
    return out;
  }

  // The spec of one form never changes under the wizard (FormCard unmounts the
  // wizard while it loads another form), so the starting values are read once on purpose.
  // svelte-ignore state_referenced_locally
  let values = $state<Values>(defaults(spec));
  let index = $state(0);
  const steps = $derived(visibleSteps(spec, values));
  const step = $derived(steps[Math.min(index, steps.length - 1)]);
  const last = $derived(index >= steps.length - 1);
  const ready = $derived(stepProblems(spec, Math.min(index, steps.length - 1), values).length === 0);
  const problemOf = (name: string) => serverProblems.find((p) => p.field === name)?.problem ?? null;

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

  /** Only what a visible field holds is sent: a hidden step's values stay
   *  behind, as the backend would drop them anyway. */
  function submit() {
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
</script>

<div class="wizard">
  {#if steps.length > 1}
    <div class="count" data-testid="form-step-count">Step {index + 1} of {steps.length}</div>
  {/if}
  {#if step}
    <h6 data-testid="form-step-title">{step.title}</h6>
    {#if step.intro}<p class="intro">{step.intro}</p>{/if}
    {#each step.fields as f (f.name)}
      <div class="field">
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
          {#each f.options ?? [] as [v, l] (v)}
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
              {l}
            </label>
          {/each}
        {:else}
          <label for={`${uid}-${f.name}`}>{f.label}{f.required ? ' *' : ''}</label>
          {#if f.type === 'select'}
            <select
              id={`${uid}-${f.name}`}
              data-testid={`form-field-${f.name}`}
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
  {/if}
  <div class="row">
    {#if index > 0}
      <button type="button" data-testid="form-back" disabled={busy} onclick={() => (index -= 1)}>Back</button>
    {/if}
    {#if last}
      <button type="button" class="primary" data-testid="form-submit" disabled={off || !ready} onclick={submit}>
        {spec.submit ?? 'Submit'}
      </button>
    {:else}
      <button type="button" class="primary" data-testid="form-next" disabled={off || !ready} onclick={() => (index += 1)}>Next</button>
    {/if}
  </div>
</div>

<style>
  .wizard { display: flex; flex-direction: column; gap: 0.6rem; }
  .count { font-size: 11px; color: var(--fg-muted); }
  h6 { margin: 0; font-size: 0.9rem; }
  .intro { margin: 0; font-size: 0.8rem; color: var(--fg-muted); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  label, .label { font-size: 0.82rem; }
  .check { display: flex; gap: 0.35rem; align-items: flex-start; }
  input:not([type='checkbox']), select, textarea { font: inherit; font-size: 0.82rem; padding: 0.25rem 0.4rem; }
  .help { font-size: 11px; color: var(--fg-muted); }
  .err { font-size: 11px; color: var(--usage-crit); }
  .row { display: flex; gap: 0.4rem; justify-content: flex-end; }
</style>
