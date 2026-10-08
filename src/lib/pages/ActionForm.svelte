<script lang="ts">
  // One action's form: an input per declared param and a button labelled
  // with the action. Options come from the page (hosts, trackers), already
  // narrowed to what the record does not hold.
  import { formReady, formValues, paramValue, type ActionSpec } from './resources';

  let {
    action,
    options = () => [],
    busy = false,
    onrun,
    testid,
  }: {
    action: ActionSpec;
    /** The choices of an `options` param. */
    options?: (param: string) => { value: string; label: string }[];
    busy?: boolean;
    onrun: (params: Record<string, string>) => void;
    testid?: string;
  } = $props();

  let values = $state<Record<string, string>>({});
  const ready = $derived(formReady(action, values));

  function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!ready || busy) return;
    const sent = formValues(action, values);
    values = {};
    onrun(sent);
  }
</script>

<form class="action-form" data-testid={testid ?? `action-${action.id}`} onsubmit={submit}>
  {#each action.params as p (p.name)}
    {#if p.type === 'text'}
      <input
        type="text"
        maxlength={p.max}
        placeholder={p.placeholder}
        aria-label={p.label}
        disabled={busy}
        data-testid={`param-${action.id}-${p.name}`}
        value={values[p.name] ?? ''}
        oninput={(e) => (values = { ...values, [p.name]: (e.currentTarget as HTMLInputElement).value })} />
    {:else if p.type === 'secret'}
      <input
        type="password"
        autocomplete="off"
        aria-label={p.label}
        placeholder={p.label}
        disabled={busy}
        data-testid={`param-${action.id}-${p.name}`}
        value={values[p.name] ?? ''}
        oninput={(e) => (values = { ...values, [p.name]: (e.currentTarget as HTMLInputElement).value })} />
    {:else if p.type === 'color'}
      <input
        type="color"
        aria-label={p.label}
        disabled={busy}
        data-testid={`param-${action.id}-${p.name}`}
        value={values[p.name] ?? '#3b82f6'}
        oninput={(e) => (values = { ...values, [p.name]: (e.currentTarget as HTMLInputElement).value })} />
    {:else if p.type === 'choice'}
      <select
        aria-label={p.label}
        disabled={busy}
        data-testid={`param-${action.id}-${p.name}`}
        value={paramValue(p, values)}
        onchange={(e) => (values = { ...values, [p.name]: (e.currentTarget as HTMLSelectElement).value })}>
        {#each p.options as [v, label] (v)}<option value={v}>{label}</option>{/each}
      </select>
    {:else}
      {@const opts = options(p.name)}
      <select
        aria-label={p.label}
        disabled={busy || opts.length === 0}
        data-testid={`param-${action.id}-${p.name}`}
        value={values[p.name] ?? ''}
        onchange={(e) => (values = { ...values, [p.name]: (e.currentTarget as HTMLSelectElement).value })}>
        <option value="">{opts.length === 0 ? `No ${p.label.toLowerCase()} to add` : `${p.label}…`}</option>
        {#each opts as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
      </select>
    {/if}
  {/each}
  <button class="btn" type="submit" disabled={busy || !ready} data-testid={`run-${action.id}`}>{action.label}</button>
</form>

<style>
  .action-form {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    align-items: center;
  }
  input[type='text'],
  select {
    font: inherit;
    font-size: 11px;
    padding: 0.2rem 0.35rem;
  }
</style>
