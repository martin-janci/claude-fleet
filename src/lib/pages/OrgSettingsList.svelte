<script lang="ts">
  // An org's own settings (org administration phase C; resource field kind
  // `settings`): each per-org setting either inherits the fleet's value or
  // takes the org's own. An inherited one shows the fleet's value and
  // "Set for this org"; an own one is the ordinary settings row, written
  // through `onset`, with "Inherit" to take the fleet's again.
  import FieldRow from './FieldRow.svelte';
  import { valueInWords } from './review';
  import { homeOf, pagesBundle } from './pages';
  import { openSettingsAt } from '../app_views';
  import type { OrgSettingRow } from '../orgs';
  import type { Result } from '../result';

  let {
    rows,
    readonly = false,
    busy = false,
    onset,
  }: {
    rows: OrgSettingRow[];
    readonly?: boolean;
    busy?: boolean;
    /** Write the org's own value, or `null` to inherit. */
    onset: (key: string, value: string | null) => Promise<Result<unknown>>;
  } = $props();

  // M15 G7.14, "Set a value": an org value by key, for a setting the rows
  // above leave inherited, with the hub's own value and the setting's page
  // beside it. Only a key an org may set is offered; the hub refuses others.
  let key = $state('');
  let value = $state('');
  const picked = $derived(rows.find((r) => r.setting.key === key.trim()) ?? null);
  const home = $derived(picked ? homeOf($pagesBundle.pages, picked.setting.key) : null);

  async function setValue() {
    if (!picked || value.trim() === '') return;
    const r = await onset(picked.setting.key, value.trim());
    if (r.ok) {
      key = '';
      value = '';
    }
  }
</script>

<ul class="org-settings" data-testid="org-settings">
  {#each rows as r (r.setting.key)}
    <li data-testid={`org-setting-${r.setting.key}`}>
      {#if r.own !== undefined}
        <FieldRow d={{ ...r.setting, value: r.own, modified: false }} value={r.own} {readonly} save={(v) => onset(r.setting.key, v)} />
        <p class="inherit">
          <span>Fleet: {valueInWords(r.setting, r.setting.value)}</span>
          {#if !readonly}
            <button
              type="button"
              class="btn btn--quiet"
              disabled={busy}
              data-testid={`org-setting-inherit-${r.setting.key}`}
              onclick={() => void onset(r.setting.key, null)}>Inherit</button
            >
          {/if}
        </p>
      {:else}
        <div class="inherited">
          <span class="label">{r.setting.label}</span>
          <span class="value" data-testid={`org-setting-fleet-${r.setting.key}`}
            >{valueInWords(r.setting, r.setting.value)} (the fleet's)</span
          >
          {#if !readonly}
            <button
              type="button"
              class="btn"
              disabled={busy}
              data-testid={`org-setting-own-${r.setting.key}`}
              onclick={() => void onset(r.setting.key, r.setting.value)}>Set for this org</button
            >
          {/if}
        </div>
        <p class="help">{r.setting.help}</p>
      {/if}
    </li>
  {:else}
    <li class="none">None</li>
  {/each}
</ul>

{#if !readonly && rows.length > 0}
  <form
    class="set-value"
    data-testid="org-setting-set"
    onsubmit={(e) => {
      e.preventDefault();
      void setValue();
    }}>
    <span class="label">Set a value</span>
    <span class="help">For a setting by its key.</span>
    <div class="row">
      <input
        type="text"
        list="org-setting-keys"
        maxlength="128"
        placeholder="sessions.max_parallel"
        aria-label="Key"
        data-testid="org-setting-set-key"
        disabled={busy}
        bind:value={key} />
      <datalist id="org-setting-keys">
        {#each rows as r (r.setting.key)}<option value={r.setting.key}>{r.setting.label}</option>{/each}
      </datalist>
      <input
        type="text"
        maxlength="4096"
        aria-label="Value"
        data-testid="org-setting-set-value"
        disabled={busy || !picked}
        bind:value />
      <button type="submit" class="btn" disabled={busy || !picked || value.trim() === ''} data-testid="org-setting-set-go"
        >Set</button>
    </div>
    {#if picked}
      <span class="help" data-testid="org-setting-set-default"
        >Hub default: {valueInWords(picked.setting, picked.setting.value)}{#if home}
          ·
          <button
            type="button"
            class="link"
            data-testid="org-setting-set-reference"
            onclick={() => openSettingsAt(home.page, picked.setting.key)}>Settings reference</button
          >{/if}</span>
    {:else if key.trim() !== ''}
      <span class="help" data-testid="org-setting-set-unknown">Not a setting an org may set: it is one value for the whole fleet.</span>
    {/if}
  </form>
{/if}

<style>
  .org-settings {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    width: 100%;
  }
  .inherited {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .label {
    font-size: var(--text-xs);
    font-weight: 500;
  }
  .value,
  .inherit span,
  .none {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .inherit {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
  }
  .set-value {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin-top: 0.6rem;
    padding-top: 0.5rem;
    border-top: 1px dashed var(--border);
  }
  .set-value .row {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .set-value input {
    font: inherit;
    font-size: var(--text-2xs);
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .help {
    margin: 0.2rem 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    line-height: 1.4;
  }
</style>
