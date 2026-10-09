<script lang="ts" module>
  import type { Component } from 'svelte';

  export type Loader = () => Promise<{ default: Component<any> }>;

  // One import per loader for the app's life: a view opened twice is fetched
  // once, and a second mount renders on its first frame.
  const loaded = new WeakMap<Loader, Component<any>>();
  const pending = new WeakMap<Loader, Promise<Component<any>>>();

  export function preload(load: Loader): Promise<Component<any>> {
    let p = pending.get(load);
    if (!p) {
      p = load().then((m) => {
        loaded.set(load, m.default);
        return m.default;
      });
      pending.set(load, p);
    }
    return p;
  }
</script>

<script lang="ts">
  // A view that is not on the first screen (Hosts, Toolkit, Accounts,
  // Control, Automation, the board), in its own chunk so launch parses only
  // what it draws (review r16 D8). Props pass through unchanged.
  let { load, ...props }: { load: Loader; [key: string]: unknown } = $props();

  // A loader already resolved renders on the first frame; otherwise the
  // effect fills it in. Keyed on `load`, so a different view swaps cleanly.
  let fetched = $state<{ load: Loader; view: Component<any> } | null>(null);
  const View = $derived(loaded.get(load) ?? (fetched?.load === load ? fetched.view : null));

  $effect(() => {
    const want = load;
    if (loaded.has(want)) return;
    let live = true;
    void preload(want).then((view) => {
      if (live) fetched = { load: want, view };
    });
    return () => {
      live = false;
    };
  });
</script>

{#if View}
  <View {...props} />
{/if}
