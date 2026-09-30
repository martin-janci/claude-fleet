<script lang="ts">
  /** One dot per host in `order`. Filled = present (and, with `odd`, a
   *  half dot = present but different); ring = absent. Shape carries the
   *  state, colour only reinforces it (app.css: never colour alone). */
  let { order, present, odd = [] }: { order: string[]; present: string[]; odd?: string[] } = $props();
  const state = (h: string) => (!present.includes(h) ? 'absent' : odd.includes(h) ? 'differs' : 'present');
  const label = $derived(order.map((h) => `${h}: ${state(h)}`).join(', '));
</script>

<span class="strip" role="img" aria-label={label} title={label}>
  {#each order as h (h)}<span class="dot {state(h)}"></span>{/each}
</span>

<style>
  .strip { display: inline-flex; gap: 3px; align-items: center; }
  .dot { width: 9px; height: 9px; border-radius: 50%; box-sizing: border-box; }
  .present { background: var(--usage-ok); }
  .differs { background: linear-gradient(90deg, var(--usage-warn) 50%, transparent 50%); box-shadow: inset 0 0 0 1.5px var(--usage-warn); }
  .absent { box-shadow: inset 0 0 0 1.5px var(--control-border-strong); }
</style>
