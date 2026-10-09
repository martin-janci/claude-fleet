<script lang="ts">
  // A transfer's loader (redesign step 10.10), the same rule wherever one
  // runs: a known size always shows the Progress ring with how far it is, an
  // unknown one Data rain, which promises nothing about when
  // (`transfer_loader.ts`). Downloads draw their own; updates, syncs and
  // imports use this.
  import Loader from './Loader.svelte';
  import { transferLoader } from './transfer_loader';

  let {
    fraction,
    label,
    size = 16,
    testid = 'transfer-mark',
  }: {
    /** 0–1 when the size is known, null when it is not. */
    fraction: number | null;
    label: string;
    size?: number;
    testid?: string;
  } = $props();

  const l = $derived(transferLoader(fraction));
</script>

{#if l.name === 'progress-ring'}
  <span
    class="mark"
    role="progressbar"
    aria-label={label}
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuenow={Math.round(l.value * 100)}
    data-testid={testid}
    data-loader="progress-ring"
  >
    <Loader name="progress-ring" {size} value={l.value} stage={false} />
  </span>
{:else}
  <span class="mark" data-testid={testid} data-loader="data-rain">
    <Loader name="data-rain" {size} stage={false} {label} />
  </span>
{/if}

<style>
  .mark { display: inline-flex; align-items: center; flex: none; }
</style>
