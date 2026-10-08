<!-- The one control for every action (manual: Button). One primary per
     screen or card; `sm` only inside a row; `lg` for dialog primaries, the
     composer send and Start. A label that opens a dialog ends with "…".
     `busy` shows the 12 px Comet and the -ing label ("Starting…"). -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Kbd from './Kbd.svelte';
  import Loader from '../Loader.svelte';

  type Variant = 'default' | 'quiet' | 'primary' | 'danger' | 'danger-fill';

  let {
    variant = 'default',
    size = 'md',
    icon = false,
    kbd,
    mac,
    busy = false,
    busyLabel,
    disabled = false,
    type = 'button',
    title,
    label,
    onclick,
    testid,
    children,
  }: {
    variant?: Variant;
    size?: 'sm' | 'md' | 'lg';
    /** Square icon-only button; give it `label`. */
    icon?: boolean;
    /** A leading key hint (the 1/2/3 of a question card), as the Mac chord. */
    kbd?: string;
    mac?: boolean;
    busy?: boolean;
    busyLabel?: string;
    disabled?: boolean;
    type?: 'button' | 'submit';
    title?: string;
    /** Accessible name when the content is not text. */
    label?: string;
    onclick?: (e: MouseEvent) => void;
    testid?: string;
    children?: Snippet;
  } = $props();
</script>

<button
  class="of-btn"
  class:quiet={variant === 'quiet'}
  class:primary={variant === 'primary'}
  class:danger={variant === 'danger'}
  class:danger-fill={variant === 'danger-fill'}
  class:sm={size === 'sm'}
  class:lg={size === 'lg'}
  class:icon
  {type}
  {title}
  aria-label={label}
  aria-busy={busy ? 'true' : undefined}
  disabled={disabled || busy}
  {onclick}
  data-testid={testid}
>
  {#if busy}<Loader name="comet" size={12} />{busyLabel}{:else}{#if kbd}<Kbd chord={kbd} {mac} />{/if}{@render children?.()}{/if}
</button>
