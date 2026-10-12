<!-- A one-word state label tinted from its status token, or the neutral chip
     for hosts, PRs and filters (manual: StatusChip). One status chip per
     row; `accent` only for things Claude did for you. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { STATE_WORD, type OfState } from './status';
  import StatusDot from './StatusDot.svelte';

  let {
    state,
    label,
    children,
    testid,
  }: {
    state?: OfState | 'accent';
    /** Defaults to the state's word ("Needs you" for waiting). */
    label?: string;
    children?: Snippet;
    testid?: string;
  } = $props();

  const word = $derived(label ?? (state && state !== 'accent' ? STATE_WORD[state] : ''));
</script>

<span
  class="of-chip"
  class:waiting={state === 'waiting'}
  class:working={state === 'working'}
  class:failed={state === 'failed'}
  class:done={state === 'done'}
  class:idle={state === 'idle'}
  class:accent={state === 'accent'}
  data-testid={testid}
  >{#if state === 'working'}<StatusDot state="working" size={6} label={null} />{/if}{word}{@render children?.()}</span
>
