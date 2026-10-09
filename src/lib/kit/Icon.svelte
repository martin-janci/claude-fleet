<!-- The manual's 16-unit, 1.5 px line icon (icons.ts), in currentColor. -->
<script lang="ts">
  import { OF_ICONS, type OfIconName } from './icons';

  let { name, size = 16, label }: { name: OfIconName; size?: number; label?: string } = $props();
</script>

<svg
  class="of-ico"
  width={size}
  height={size}
  viewBox="0 0 16 16"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
  data-icon={name}
>
  {#each OF_ICONS[name] as s, i (i)}
    {#if s.kind === 'path'}<path d={s.d} fill={s.fill ? 'currentColor' : undefined} stroke={s.fill ? 'none' : undefined} />{:else if s.kind === 'circle'}<circle cx={s.cx} cy={s.cy} r={s.r} />{:else}<rect
        x={s.x}
        y={s.y}
        width={s.width}
        height={s.height}
        rx={s.rx}
      />{/if}
  {/each}
</svg>
