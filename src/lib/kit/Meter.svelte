<!-- A 4 px bar for context, quota and spend (manual: Meter), always beside
     the number it shows: `label` is that number in words. Level ok
     (status-done), warn (status-waiting) or crit (status-failed). -->
<script lang="ts">
  let {
    value,
    level = 'ok',
    label,
    testid,
  }: { value: number; level?: 'ok' | 'warn' | 'crit'; label: string; testid?: string } = $props();

  const pct = $derived(Math.round(Math.min(1, Math.max(0, value)) * 100));
</script>

<div
  class="of-meter"
  class:warn={level === 'warn'}
  class:crit={level === 'crit'}
  role="meter"
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={pct}
  aria-label={label}
  data-testid={testid}
>
  <span style:width="{pct}%"></span>
</div>
