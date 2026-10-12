<script lang="ts">
  // Layout L3: a wizard whose steps the backend decides. It shows the step
  // it is given — a title, a sentence, the fields — and posts their values;
  // the answer is the next step, the same step with an error, or done. A
  // secret field is never prefilled and is cleared once sent.
  import { onDestroy, onMount } from 'svelte';
  import { flowBack, flowCancel, flowStart, flowSubmit, type FlowStep } from './flows';

  let {
    flow,
    prefill = {},
    ondone,
    oncancel,
  }: {
    flow: string;
    prefill?: Record<string, string>;
    ondone: (message: string, recordId: number | null) => void;
    oncancel: () => void;
  } = $props();

  let step = $state<FlowStep | null>(null);
  let values = $state<Record<string, string>>({});
  let busy = $state(false);
  let error = $state<string | null>(null);
  let finished = false;

  function show(s: FlowStep) {
    step = s;
    error = s.error ?? null;
    // Start from what the backend says; a secret always starts empty.
    values = Object.fromEntries(s.fields.map((f) => [f.name, f.type === 'secret' ? '' : f.value]));
  }

  onMount(async () => {
    busy = true;
    const r = await flowStart(flow, prefill);
    busy = false;
    if (r.ok) show(r.value);
    else error = r.error.message;
  });

  onDestroy(() => {
    if (step && !finished) void flowCancel(step.flow_id);
  });

  const ready = $derived(
    !!step && step.fields.every((f) => !f.required || f.type === 'bool' || (values[f.name] ?? '').trim() !== ''),
  );

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!step || !ready || busy) return;
    busy = true;
    const sent = { ...values };
    // The secret leaves this component with the request.
    for (const f of step.fields) if (f.type === 'secret') values[f.name] = '';
    const r = await flowSubmit(step.flow_id, sent);
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    if (r.value.state === 'done') {
      finished = true;
      ondone(r.value.message, r.value.record_id ?? null);
      return;
    }
    show(r.value);
  }

  async function back() {
    if (!step) return;
    busy = true;
    const r = await flowBack(step.flow_id);
    busy = false;
    if (r.ok) show(r.value);
    else error = r.error.message;
  }
</script>

<form class="flow" data-testid={`flow-${flow}`} onsubmit={submit}>
  {#if step}
    <h5 data-testid="flow-title">{step.title}</h5>
    {#if step.intro}<p class="intro" data-testid="flow-intro">{step.intro}</p>{/if}
    {#each step.fields as f (f.name)}
      <div class="field">
        {#if f.type === 'bool'}
          <label class="check">
            <input
              type="checkbox"
              data-testid={`flow-field-${f.name}`}
              checked={values[f.name] === 'true'}
              disabled={busy}
              onchange={(e) => (values = { ...values, [f.name]: (e.currentTarget as HTMLInputElement).checked ? 'true' : 'false' })} />
            {f.label}
          </label>
        {:else}
          <label for={`flow-${f.name}`}>{f.label}</label>
          {#if f.type === 'select'}
            <select
              id={`flow-${f.name}`}
              data-testid={`flow-field-${f.name}`}
              value={values[f.name] ?? ''}
              disabled={busy}
              onchange={(e) => (values = { ...values, [f.name]: (e.currentTarget as HTMLSelectElement).value })}>
              {#each f.options as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
          {:else if f.type === 'textarea'}
            <textarea
              id={`flow-${f.name}`}
              rows="3"
              spellcheck="false"
              data-testid={`flow-field-${f.name}`}
              value={values[f.name] ?? ''}
              disabled={busy}
              oninput={(e) => (values = { ...values, [f.name]: (e.currentTarget as HTMLTextAreaElement).value })}></textarea>
          {:else}
            <input
              id={`flow-${f.name}`}
              type={f.type === 'secret' ? 'password' : 'text'}
              autocomplete="off"
              spellcheck="false"
              placeholder={f.type === 'text' ? f.placeholder : ''}
              data-testid={`flow-field-${f.name}`}
              value={values[f.name] ?? ''}
              disabled={busy}
              oninput={(e) => (values = { ...values, [f.name]: (e.currentTarget as HTMLInputElement).value })} />
          {/if}
        {/if}
        {#if f.help}<span class="help">{f.help}</span>{/if}
      </div>
    {/each}
  {/if}
  {#if error}<p class="err" role="alert" data-testid="flow-error">{error}</p>{/if}
  <div class="row">
    {#if step?.back}
      <button type="button" class="btn btn--quiet" disabled={busy} data-testid="flow-back" onclick={() => void back()}>Back</button>
    {/if}
    <button type="button" class="btn btn--quiet" data-testid="flow-cancel" onclick={oncancel}>Cancel</button>
    {#if step}
      <button type="submit" class="btn btn--primary" disabled={!ready || busy} data-testid="flow-submit"
        >{busy ? 'Working…' : step.submit}</button
      >
    {/if}
  </div>
</form>

<style>
  .flow {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    max-width: 32rem;
  }
  h5 {
    margin: 0;
    font-size: var(--text-sm);
  }
  .intro {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    word-break: break-all;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  label {
    font-size: var(--text-2xs);
  }
  .check {
    display: flex;
    gap: 0.35rem;
    align-items: flex-start;
  }
  input:not([type='checkbox']),
  select,
  textarea {
    font: inherit;
    font-size: var(--text-2xs);
    padding: 0.25rem 0.4rem;
  }
  textarea {
    font-family: var(--mono);
  }
  .help {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .err {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--usage-crit);
  }
  .row {
    display: flex;
    gap: 0.4rem;
    justify-content: flex-end;
  }
</style>
