<script lang="ts">
  // A matrix section (Orbit Fleet 11.9): settings that are each a choice
  // set over the same options, shown as one grid — a row per option, a
  // column per setting. A tick adds or removes that option from that
  // setting's comma list, saved as it changes through the same validated
  // `set_fleet_setting` path as every other row.
  import { setFleetSetting, type SettingKey } from '../fleet_settings';
  import { optionLabel, type Descriptor } from './pages';

  let {
    descs,
    values,
    readonly = false,
  }: {
    /** The section's settings, in column order. */
    descs: Descriptor[];
    values: Record<string, string>;
    readonly?: boolean;
  } = $props();

  const options = $derived(descs[0]?.kind.type === 'choice_set' ? descs[0].kind.options : []);
  let error = $state<string | null>(null);

  const chosen = (d: Descriptor): string[] =>
    (values[d.key] ?? d.value)
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);

  async function toggle(d: Descriptor, option: string, on: boolean) {
    const now = new Set(chosen(d));
    if (on) now.add(option);
    else now.delete(option);
    const next = options.filter((o) => now.has(o)).join(',');
    error = null;
    const r = await setFleetSetting(d.key as SettingKey, next);
    if (!r.ok) error = r.error.message;
  }
</script>

<table class="matrix" data-testid="settings-matrix">
  <thead>
    <tr>
      <th scope="col"><span class="sr-only">State</span></th>
      {#each descs as d (d.key)}<th scope="col" title={d.help} data-testid={`setting-row-${d.key}`}
          >{d.label}<span class="sr-only">. {d.help}</span></th
        >{/each}
    </tr>
  </thead>
  <tbody>
    {#each options as o (o)}
      <tr>
        <th scope="row">{optionLabel(descs[0], o)}</th>
        {#each descs as d (d.key)}
          {@const on = chosen(d).includes(o)}
          <td>
            <input
              type="checkbox"
              checked={on}
              disabled={readonly || d.owned_by !== undefined}
              aria-label={`${optionLabel(descs[0], o)}: ${d.label}`}
              data-testid={`matrix-${d.key}-${o}`}
              onchange={(e) => toggle(d, o, (e.currentTarget as HTMLInputElement).checked)} />
          </td>
        {/each}
      </tr>
    {/each}
  </tbody>
</table>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  .matrix {
    border-collapse: collapse;
    width: 100%;
    font-size: 13px;
  }
  th,
  td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    text-align: center;
  }
  th[scope='row'] {
    text-align: left;
    font-weight: 400;
    color: var(--fg);
  }
  thead th {
    color: var(--fg-muted);
    font-weight: 500;
  }
  .error {
    color: var(--danger);
    margin: 6px 0 0;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
