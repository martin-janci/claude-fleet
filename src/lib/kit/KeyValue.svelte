<!-- Facts for the inspector and detail panes (manual: KeyValue): an 84 px
     muted label, then the value. Long mono values wrap anywhere; numbers use
     tabular figures; no buttons in a value. -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Fact {
    label: string;
    value?: string;
    mono?: boolean;
    tnum?: boolean;
    /** A richer value: a status dot, a link, a Meter. */
    content?: Snippet;
    /** The value's test id. */
    testid?: string;
  }

  let { items, testid }: { items: Fact[]; testid?: string } = $props();
</script>

<dl class="of of-kv" data-testid={testid}>
  {#each items as f, i (i)}
    <dt>{f.label}</dt>
    <dd
      class:mono={f.mono}
      class:tnum={f.tnum}
      style:overflow-wrap={f.mono ? 'anywhere' : undefined}
      data-testid={f.testid}
    >
      {#if f.content}{@render f.content()}{:else}{f.value ?? ''}{/if}
    </dd>
  {/each}
</dl>
