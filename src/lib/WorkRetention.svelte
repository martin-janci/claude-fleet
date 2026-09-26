<script lang="ts">
  // Settings → Limits → Work retention (work graph M12.3): what the GC
  // tick's retention sweep holds and would delete now (a dry run), and one
  // sweep on demand. The windows themselves are the three inputs above
  // this in SettingsDialog.
  import { onMount } from 'svelte';
  import {
    retentionStatus,
    retentionSweepNow,
    retentionLine,
    lastSweepLine,
    type RetentionStatus,
  } from './work_retention';

  let {
    now = () => Math.floor(Date.now() / 1000),
  }: {
    /** Unix seconds; injectable for tests. */
    now?: () => number;
  } = $props();

  let status = $state<RetentionStatus | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function refresh() {
    busy = true;
    error = null;
    const r = await retentionStatus();
    busy = false;
    if (r.ok) status = r.value;
    else error = r.error.message;
  }

  async function sweepNow() {
    busy = true;
    error = null;
    const r = await retentionSweepNow();
    if (!r.ok) {
      busy = false;
      error = r.error.message;
      return;
    }
    await refresh();
  }

  const pending = $derived(status ? status.tables.reduce((n, t) => n + t.would_delete, 0) : 0);

  onMount(() => void refresh());
</script>

<div class="hook-desc" data-testid="work-retention-status">
  {#if status}
    <ul>
      {#each status.tables as t (t.table)}
        <li data-testid={`work-retention-${t.table}`}>{retentionLine(t)}</li>
      {/each}
    </ul>
    <span data-testid="work-retention-last">{lastSweepLine(status.last_sweep, now())}</span>
    {#if pending > status.tick_cap}
      <span data-testid="work-retention-backlog">
        · at most {status.tick_cap} per table per sweep; the rest goes over later sweeps
      </span>
    {/if}
  {:else if !error}
    loading…
  {/if}
</div>
<div class="mcp-field">
  <button class="btn" type="button" data-testid="work-retention-preview" disabled={busy}
    onclick={() => void refresh()}>Preview</button>
  <button class="btn" type="button" data-testid="work-retention-sweep" disabled={busy || pending === 0}
    onclick={() => void sweepNow()}>Sweep now</button>
</div>
{#if error}<p class="err" role="alert" data-testid="work-retention-error">{error}</p>{/if}
