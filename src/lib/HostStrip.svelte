<script lang="ts">
  import { DOT_LABEL, type DotState } from './assets_visual';

  /** One dot per host in `order`, shape + colour + words (app.css: never
   *  colour alone). Identity rows pass `present`/`odd`: filled = present,
   *  half = differs, ring = absent. Asset rows pass `states` (Assets M5,
   *  R27): in sync, differs, missing (ring), not here (dash), stale scan
   *  (hatched), blocked (cross); a host `states` does not name is `na`. */
  let {
    order,
    present = [],
    odd = [],
    states,
  }: { order: string[]; present?: string[]; odd?: string[]; states?: Record<string, DotState> } = $props();

  const stateOf = (h: string): DotState =>
    states ? (states[h] ?? 'na') : !present.includes(h) ? 'absent' : odd.includes(h) ? 'differs' : 'present';
  const label = $derived(order.map((h) => `${h}: ${DOT_LABEL[stateOf(h)]}`).join(', '));
</script>

<span class="strip" role="img" aria-label={label} title={label}>
  {#each order as h (h)}<span class="dot {stateOf(h)}"></span>{/each}
</span>

<style>
  .strip { display: inline-flex; gap: 3px; align-items: center; }
  .dot { width: 9px; height: 9px; border-radius: 50%; box-sizing: border-box; position: relative; }
  .present, .in_sync { background: var(--usage-ok); }
  .differs { background: linear-gradient(90deg, var(--usage-warn) 50%, transparent 50%); box-shadow: inset 0 0 0 1.5px var(--usage-warn); }
  .absent, .missing { box-shadow: inset 0 0 0 1.5px var(--control-border-strong); }
  .na { width: 6px; height: 2px; margin: 0 1.5px; border-radius: 1px; background: var(--border); }
  .stale {
    background: repeating-linear-gradient(135deg, var(--control-border-strong) 0 1.5px, transparent 1.5px 3.5px);
    box-shadow: inset 0 0 0 1px var(--control-border-strong);
  }
  .blocked::before, .blocked::after {
    content: '';
    position: absolute;
    left: 3.5px;
    top: -1px;
    width: 2px;
    height: 11px;
    border-radius: 1px;
    background: var(--usage-crit);
    transform: rotate(45deg);
  }
  .blocked::after { transform: rotate(-45deg); }
</style>
