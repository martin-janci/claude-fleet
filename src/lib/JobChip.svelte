<script lang="ts">
  /** Background work in the footer (spec: "`JobChip` for scans and
   *  syncs"): non-modal, with a bar when it counts. Visual only: the
   *  footer's persistent `role=status` region announces the job, so a chip
   *  that appears and disappears is never announced twice. Its mark is
   *  the kit's 16 px Orbit (review r12): it waits 400 ms, so a quick scan
   *  never flashes, and follows the app's Motion setting. */
  import Loader from './Loader.svelte';
  import TransferMark from './TransferMark.svelte';

  let {
    label,
    done = null,
    total = null,
    transfer = false,
    testid = 'assets-job',
  }: {
    label: string;
    done?: number | null;
    total?: number | null;
    /** A sync to the hosts (step 10.10): the Progress ring once its size is
     *  known, Data rain until then, in place of the Orbit. */
    transfer?: boolean;
    testid?: string;
  } = $props();

  const counted = $derived(done !== null && total !== null && total > 0);
  const pct = $derived(counted ? Math.round(((done ?? 0) / (total ?? 1)) * 100) : 0);
</script>

<span class="job" data-testid={testid}>
  {#if transfer}
    <TransferMark fraction={counted ? (done ?? 0) / (total ?? 1) : null} {label} testid="assets-job-mark" />
  {:else}
    <Loader size={16} testid="assets-job-mark" />
  {/if}
  <span>{label}{#if counted}{' '}{done}/{total}{/if}</span>
  {#if counted}
    <span class="bar" role="progressbar" aria-label={label} aria-valuemin="0" aria-valuemax={total} aria-valuenow={done}
      ><i style:width={`${pct}%`}></i></span
    >
  {/if}
</span>

<style>
  .job { display: inline-flex; align-items: center; gap: 6px; color: var(--fg); }
  .bar { width: 60px; height: 4px; overflow: hidden; border-radius: var(--radius-xs); background: var(--control-bg-active); }
  .bar i { display: block; height: 100%; background: var(--accent); }
</style>
