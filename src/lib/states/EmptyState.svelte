<script lang="ts">
  // The states kit's one empty state: what happened, in a sentence, and the
  // next step as buttons. `calm` is "nothing needs you" (quiet, not a void),
  // `first` a first run with nothing set up yet, `none` a search or filter
  // that matched nothing, which always offers a way out.
  import type { Snippet } from 'svelte';
  import type { StateAction } from './states';

  let {
    title,
    body = null,
    kind = 'calm',
    actions = [],
    testid = 'empty-state',
    children,
  }: {
    title: string;
    body?: string | null;
    kind?: 'calm' | 'first' | 'none';
    actions?: StateAction[];
    testid?: string;
    children?: Snippet;
  } = $props();
</script>

<div class="empty-state {kind}" data-testid={testid} data-kind={kind}>
  <p class="title">{title}</p>
  {#if body}<p class="body">{body}</p>{/if}
  {@render children?.()}
  {#if actions.length}
    <div class="actions">
      {#each actions as a (a.label)}
        <button type="button" class="btn" class:btn--primary={a.primary} data-testid={a.testid} onclick={a.onclick}>{a.label}</button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .empty-state {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 1rem 0.75rem;
    color: var(--fg-muted);
    font-size: 0.85rem;
  }
  .empty-state.first {
    align-items: flex-start;
    padding: 1.5rem 1rem;
  }
  .title {
    margin: 0;
    color: var(--fg);
    font-weight: 500;
  }
  .body {
    margin: 0;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.3rem;
  }
</style>
