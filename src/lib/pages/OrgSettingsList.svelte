<script lang="ts">
  // An org's own settings (org administration phase C; resource field kind
  // `settings`): each per-org setting either inherits the fleet's value or
  // takes the org's own. An inherited one shows the fleet's value and
  // "Set for this org"; an own one is the ordinary settings row, written
  // through `onset`, with "Inherit" to take the fleet's again.
  import FieldRow from './FieldRow.svelte';
  import { valueInWords } from './review';
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
    font-size: 0.85rem;
    font-weight: 500;
  }
  .value,
  .inherit span,
  .none {
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
  .inherit {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
  }
  .help {
    margin: 0.2rem 0 0;
    font-size: 11px;
    color: var(--fg-muted);
    line-height: 1.4;
  }
</style>
