<!-- The Orbit Fleet mark (manual: OrbitMark), drawn on the 108-unit grid
     from brand-ink, brand-light and brand-amber. 22 px in the header, 16 px
     in the status bar and rows, 28 px beside a page title, 72+ on empty and
     error screens. -->
<script lang="ts">
  let { size = 22, label = 'Orbit Fleet' }: { size?: number; label?: string | null } = $props();
</script>

<svg
  class="of-mark"
  width={size}
  height={size}
  viewBox="0 0 108 108"
  role={label ? 'img' : undefined}
  aria-label={label ?? undefined}
  aria-hidden={label ? undefined : 'true'}
>
  <rect width="108" height="108" rx="24" class="t" />
  <circle cx="54" cy="54" r="24" class="r" />
  <circle cx="54" cy="54" r="10" class="h" />
  <circle cx="54" cy="30" r="7.5" class="h" />
  <circle cx="74.78" cy="66" r="7.5" class="a" />
  <circle cx="33.22" cy="66" r="7.5" class="h" />
</svg>
