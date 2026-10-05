<script lang="ts">
  import { applyCompletion, completions, parseQuery, type QueryVocab } from './assets_query';

  /** The workspace's token filter (spec, Query; Rulings R21): `/` focuses
   *  it (the key is the workspace's, not this component's), typing
   *  completes keys and values, Tab or Enter takes the highlighted one,
   *  Esc closes the list, then clears, then leaves. A combobox over a
   *  listbox, operable from the keyboard alone. */
  let {
    value = $bindable(''),
    vocab,
    placeholder = 'Filter · host: kind: state: layer: catalog: scope:',
    testid = 'assets-query',
    onescape,
  }: {
    value?: string;
    vocab: QueryVocab;
    placeholder?: string;
    testid?: string;
    /** Esc on an empty field with no list: give focus back to the list. */
    onescape?: () => void;
  } = $props();

  let input: HTMLInputElement | undefined = $state();
  let open = $state(false);
  let active = $state(0);
  const uid = $props.id();
  const listId = `${uid}-completions`;
  const options = $derived(open ? completions(value, vocab) : []);
  const tokens = $derived(parseQuery(value).tokens);
  const optionId = (i: number) => `${uid}-opt-${i}`;

  export function focus() {
    input?.focus();
    input?.select();
  }

  function take(i: number): boolean {
    const c = options[i];
    if (c === undefined) return false;
    value = applyCompletion(value, c);
    active = 0;
    // A key completion leaves the list open for that key's values.
    open = c.endsWith(':');
    return true;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' && !open && completions(value, vocab).length) {
      e.preventDefault();
      open = true;
      active = 0;
    } else if (e.key === 'ArrowDown' && options.length) {
      e.preventDefault();
      active = (active + 1) % options.length;
    } else if (e.key === 'ArrowUp' && options.length) {
      e.preventDefault();
      active = (active + options.length - 1) % options.length;
    } else if ((e.key === 'Tab' || e.key === 'Enter') && options.length) {
      if (take(active)) e.preventDefault();
    } else if (e.key === 'Escape') {
      // Every Esc here is the field's (R22): it never reaches the App, which
      // would close the Assets overlay. The next one, on the list, does.
      e.preventDefault();
      e.stopPropagation();
      if (options.length) open = false;
      else if (value) value = '';
      else onescape?.();
    }
  }
</script>

<div class="query" role="search">
  <input
    bind:this={input}
    bind:value
    class="field"
    type="text"
    role="combobox"
    aria-label="Filter assets"
    aria-autocomplete="list"
    aria-expanded={options.length > 0}
    aria-controls={listId}
    aria-activedescendant={options.length ? optionId(active) : undefined}
    autocomplete="off"
    spellcheck="false"
    {placeholder}
    data-testid={testid}
    oninput={() => {
      open = true;
      active = 0;
    }}
    onblur={() => (open = false)}
    {onkeydown}
  />
  {#if tokens.length}
    <span class="tokens" aria-hidden="true" data-testid={`${testid}-tokens`}>
      {#each tokens as t, i (i)}<span class="tok"><i>{t.key}:</i>{t.values.join(',')}</span>{/each}
    </span>
  {/if}
  <!-- Always in the DOM (hidden while closed), so `aria-controls` names an
       element that exists. -->
  <ul class="list" id={listId} role="listbox" aria-label="Completions" hidden={!options.length} data-testid={`${testid}-completions`}>
    {#each options as o, i (o)}
      <li
        id={optionId(i)}
        role="option"
        aria-selected={i === active}
        class:active={i === active}
        onmousedown={(e) => {
          e.preventDefault();
          take(i);
        }}
      >{o}</li>
    {/each}
  </ul>
</div>

<style>
  .query {
    position: relative; display: flex; align-items: center; gap: 6px; flex: 1; max-width: 520px;
    height: var(--control-h-lg); padding: 0 8px; border: 1px solid var(--control-border);
    border-radius: var(--radius-md); background: var(--control-bg);
  }
  .query:focus-within { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .field { flex: 1; min-width: 80px; border: 0; outline: 0; background: none; color: var(--fg); font: inherit; font-size: var(--control-font); }
  .tokens { display: flex; gap: 4px; }
  .tok { font-family: var(--mono); font-size: var(--control-font-sm); padding: 1px 6px; border-radius: var(--radius-sm); background: var(--accent-soft); color: var(--fg); white-space: nowrap; }
  .tok i { font-style: normal; color: var(--accent); }
  .list[hidden] { display: none; }
  .list {
    position: absolute; top: calc(100% + 4px); left: 0; z-index: 5; min-width: 220px; margin: 0; padding: 4px 0;
    list-style: none; border: 1px solid var(--control-border); border-radius: var(--radius-md); background: var(--bg);
  }
  .list li { padding: 3px 10px; font-family: var(--mono); font-size: 11.5px; cursor: pointer; }
  .list li.active { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
</style>
