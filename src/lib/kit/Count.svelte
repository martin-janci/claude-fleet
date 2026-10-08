<!-- A small count beside a label (a section, a tab, the Filters button). -->
<script lang="ts">
  let { n, label }: { n: number; label?: string } = $props();
</script>

<span class="of-count tnum" aria-label={label}>{n}</span>
