<script lang="ts">
  // Settings → Work → Usage (work graph M13.2, decision D24): how the work
  // graph is used over a window, as a read-only table of counts, and Copy
  // as text for an acceptance run's record. Counted from the store on this
  // machine; nothing is sent anywhere. Shown only where this desktop owns
  // its fleet (WorkSettings gates it): on a hub it is `fleet-hub work usage`.
  import { onMount } from 'svelte';
  import { copyText } from './clipboard';
  import { USAGE_WINDOWS, usageRows, usageText, workUsage, type UsageSummary } from './work_usage';

  let days = $state(30);
  let usage = $state<UsageSummary | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let copied = $state(false);

  async function load() {
    busy = true;
    error = null;
    copied = false;
    const r = await workUsage(days);
    busy = false;
    if (r.ok) usage = r.value;
    else error = r.error.message;
  }

  async function copy() {
    if (!usage) return;
    copied = await copyText(usageText(usage));
  }

  onMount(() => void load());
</script>

<div class="usage" data-testid="work-usage">
  <div class="head">
    <h5>Usage</h5>
    <select
      data-testid="work-usage-days"
      aria-label="Window"
      bind:value={days}
      onchange={() => void load()}
      disabled={busy}
    >
      {#each USAGE_WINDOWS as d (d)}<option value={d}>last {d} d</option>{/each}
    </select>
    <button class="btn" type="button" data-testid="work-usage-copy" disabled={!usage} onclick={() => void copy()}
      >{copied ? 'Copied' : 'Copy as text'}</button
    >
  </div>
  {#if usage}
    <table data-testid="work-usage-table">
      <tbody>
        {#each usageRows(usage) as [group, value] (group)}
          <tr data-testid="work-usage-row"><th scope="row">{group}</th><td>{value}</td></tr>
        {/each}
      </tbody>
    </table>
    {#if (usage.unrecorded ?? []).length > 0}
      <p class="hint" data-testid="work-usage-unrecorded">
        Not recorded, so not counted: {(usage.unrecorded ?? []).join('; ')}.
      </p>
    {/if}
  {:else if !error}
    <p class="hint">loading…</p>
  {/if}
  {#if error}<p class="err" role="alert" data-testid="work-usage-error">{error}</p>{/if}
</div>

<style>
  .usage {
    margin-top: 0.6rem;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  h5 {
    margin: 0;
    font-size: 0.8rem;
  }
  select {
    font: inherit;
    font-size: 0.75rem;
  }
  table {
    border-collapse: collapse;
    font-size: 0.75rem;
    margin: 0.3rem 0;
  }
  th {
    text-align: left;
    font-weight: 600;
    padding: 0.1rem 0.6rem 0.1rem 0;
    white-space: nowrap;
    vertical-align: top;
  }
  td {
    padding: 0.1rem 0;
  }
  .hint {
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .err {
    color: var(--err, #ef4444);
    font-size: 0.72rem;
  }
</style>
