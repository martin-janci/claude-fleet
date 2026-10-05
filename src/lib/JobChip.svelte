<script lang="ts">
  /** Background work in the footer (spec: "`JobChip` for scans and
   *  syncs"): non-modal, announced politely, with a bar when it counts. */
  let {
    label,
    done = null,
    total = null,
    testid = 'assets-job',
  }: { label: string; done?: number | null; total?: number | null; testid?: string } = $props();

  const counted = $derived(done !== null && total !== null && total > 0);
  const pct = $derived(counted ? Math.round(((done ?? 0) / (total ?? 1)) * 100) : 0);
</script>

<span class="job" role="status" aria-live="polite" data-testid={testid}>
  <span class="spin" aria-hidden="true">⟳</span>
  <span>{label}{#if counted}{' '}{done}/{total}{/if}</span>
  {#if counted}
    <span class="bar" role="progressbar" aria-label={label} aria-valuemin="0" aria-valuemax={total} aria-valuenow={done}
      ><i style:width={`${pct}%`}></i></span
    >
  {/if}
</span>

<style>
  .job { display: inline-flex; align-items: center; gap: 6px; color: var(--fg); }
  .spin { display: inline-block; animation: spin 1.2s linear infinite; }
  .bar { width: 60px; height: 4px; overflow: hidden; border-radius: 2px; background: var(--control-bg-active); }
  .bar i { display: block; height: 100%; background: var(--accent); }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
</style>
