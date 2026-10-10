<script lang="ts">
  // One action's form: an input per declared param and a button labelled
  // with the action. Options come from the page (hosts, trackers), already
  // narrowed to what the record does not hold.
  import Loader from '../Loader.svelte';
  import { formReady, formValues, paramValue, previewAction, type ActionSpec } from './resources';

  let {
    action,
    options = () => [],
    busy = false,
    onrun,
    argsFor,
    previewDelayMs = 300,
    testid,
  }: {
    action: ActionSpec;
    /** The choices of an `options` param, and the values a `suggest` param
     *  offers. */
    options?: (param: string) => { value: string; label: string }[];
    busy?: boolean;
    onrun: (params: Record<string, string>) => void;
    /** The command's arguments for these values, for an action with a
     *  `preview` (M15 G2.10: an org rule's live impact). */
    argsFor?: (params: Record<string, string>) => Record<string, unknown>;
    previewDelayMs?: number;
    testid?: string;
  } = $props();

  let values = $state<Record<string, string>>({});
  const ready = $derived(formReady(action, values));

  // The preview runs by itself a moment after each edit, once the form is
  // filled in; a slower answer to an older edit is dropped.
  let preview = $state<string | null>(null);
  let previewSeq = 0;
  $effect(() => {
    const want = action.preview && argsFor && ready ? formValues(action, values) : null;
    const seq = ++previewSeq;
    if (!want) {
      preview = null;
      return;
    }
    const t = setTimeout(async () => {
      const line = await previewAction(action, argsFor!(want));
      if (seq === previewSeq) preview = line;
    }, previewDelayMs);
    return () => clearTimeout(t);
  });

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
    {:else if p.type === 'suggest'}
      {@const opts = options(p.name)}
      <input
        type="text"
        maxlength={p.max}
        placeholder={p.placeholder}
        aria-label={p.label}
        list={`suggest-${action.id}-${p.name}`}
        disabled={busy}
        data-testid={`param-${action.id}-${p.name}`}
        value={values[p.name] ?? ''}
        oninput={(e) => (values = { ...values, [p.name]: (e.currentTarget as HTMLInputElement).value })} />
      <datalist id={`suggest-${action.id}-${p.name}`} data-testid={`suggest-${action.id}-${p.name}`}>
        {#each opts as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
      </datalist>
    {:else if p.type === 'toggle'}
      <label class="toggle">
        <input
          type="checkbox"
          role="switch"
          disabled={busy}
          data-testid={`param-${action.id}-${p.name}`}
          checked={paramValue(p, values) === 'true'}
          onchange={(e) => (values = { ...values, [p.name]: String((e.currentTarget as HTMLInputElement).checked) })} />
        {p.label}
      </label>
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
  {#if preview}
    <p class="preview" aria-live="polite" data-testid={`preview-${action.id}`}>{preview}</p>
  {/if}
  {#if busy && action.busy === 'counter-orbit'}
    <!-- 11.12: two hubs exchanging keys. -->
    <span class="busy" data-testid={`busy-${action.id}`}>
      <Loader name="counter-orbit" size={32} label="Talking to the other hub" />
      <span>Talking to the other hub…</span>
    </span>
  {/if}
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
    font-size: var(--text-2xs);
    padding: 0.2rem 0.35rem;
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: var(--text-2xs);
  }
  .preview {
    flex-basis: 100%;
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .busy {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
