<script lang="ts" module>
  /** The line icons the Conversation view draws. One stroke weight and one
   *  24-unit grid, `currentColor` throughout, so an icon takes its button's
   *  colour and state instead of an emoji font's (⏪ drew as an orange glyph
   *  that matched nothing next to it). */
  export type IconName =
    | 'copy'
    | 'check'
    | 'quote'
    | 'retry'
    | 'fork'
    | 'rewind'
    | 'file'
    | 'edit'
    | 'terminal'
    | 'search'
    | 'list'
    | 'tool'
    | 'circle'
    | 'circle-half'
    | 'circle-check'
    | 'inbox'
    | 'library'
    | 'key';
</script>

<script lang="ts">
  let { name, size = 16 }: { name: IconName; size?: number } = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.75"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {#if name === 'copy'}
    <rect x="9" y="9" width="12" height="12" rx="2" />
    <path d="M5 15H4.5A1.5 1.5 0 0 1 3 13.5v-9A1.5 1.5 0 0 1 4.5 3h9A1.5 1.5 0 0 1 15 4.5V5" />
  {:else if name === 'check'}
    <path d="M5 12.5l4.5 4.5L19 7.5" />
  {:else if name === 'quote'}
    <path d="M4 5v14" />
    <path d="M9 7h11M9 12h11M9 17h7" />
  {:else if name === 'retry'}
    <path d="M20 12a8 8 0 1 1-2.34-5.66" />
    <path d="M20 4v5h-5" />
  {:else if name === 'fork'}
    <circle cx="6" cy="5" r="2" />
    <circle cx="6" cy="19" r="2" />
    <circle cx="18" cy="7" r="2" />
    <path d="M6 7v10" />
    <path d="M18 9c0 4-4 5-8.5 6.5" />
  {:else if name === 'rewind'}
    <path d="M3.5 12a8.5 8.5 0 1 0 2.5-6" />
    <path d="M3 3.5V8h4.5" />
    <path d="M12 8v4.5l3 2" />
  {:else if name === 'file'}
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
    <path d="M14 3v5h5" />
  {:else if name === 'edit'}
    <path d="M4 20h4L19 9l-4-4L4 16z" />
    <path d="M13.5 6.5l4 4" />
  {:else if name === 'terminal'}
    <path d="M5 7l5 5-5 5" />
    <path d="M12 18h7" />
  {:else if name === 'search'}
    <circle cx="11" cy="11" r="6.5" />
    <path d="M16 16l4.5 4.5" />
  {:else if name === 'list'}
    <path d="M9 6h11M9 12h11M9 18h11" />
    <path d="M4 6h.01M4 12h.01M4 18h.01" />
  {:else if name === 'tool'}
    <path d="M14.5 5.5a4 4 0 0 0-5 5L4 16l4 4 5.5-5.5a4 4 0 0 0 5-5l-2.5 2.5-3-1-1-3z" />
  {:else if name === 'circle'}
    <circle cx="12" cy="12" r="8" />
  {:else if name === 'circle-half'}
    <circle cx="12" cy="12" r="8" />
    <path d="M12 4a8 8 0 0 1 0 16z" fill="currentColor" />
  {:else if name === 'circle-check'}
    <circle cx="12" cy="12" r="8" />
    <path d="M8.5 12.5l2.5 2.5 4.5-5" />
  {:else if name === 'inbox'}
    <path d="M3 12h5l2 3h4l2-3h5" /><path d="M5 5h14l2 7v6a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-6z" />
  {:else if name === 'library'}
    <rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" />
    <rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" />
  {:else if name === 'key'}
    <circle cx="8" cy="15" r="4" /><path d="m11 12 9-9M17 6l3 3" />
  {/if}
</svg>

<style>
  .icon {
    display: block;
    flex: 0 0 auto;
  }
</style>
