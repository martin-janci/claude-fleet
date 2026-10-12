<!-- A small count beside a label (a section, a tab, the Filters button).
     The number rolls when it changes at Full motion (G4.9). -->
<script lang="ts">
  import { countRoll } from '../motion_catalog';

  let { n, label }: { n: number; label?: string } = $props();
</script>

<span class="of-count tnum" aria-label={label}><span class="roll" use:countRoll={n}>{n}</span></span>

<style>
  .roll {
    display: inline-block;
  }
</style>
