<script lang="ts">
  // An error boundary for one overlay view (Hosts). A throw while the view
  // renders, or in one of its effects, used to leave an empty or dead panel
  // with nothing to say why; here it shows the error with Copy and Retry,
  // and reports it through the hub error channel like any unhandled error.
  import type { Snippet } from 'svelte';
  import { reportError } from './error_report';
  import { copyText } from './clipboard';

  let {
    name,
    children,
  }: {
    /** The view's name: the heading, and `frontend:<name>` in the report. */
    name: string;
    children: Snippet;
  } = $props();

  let copied = $state(false);

  function describe(error: unknown): string {
    if (error instanceof Error) return `${error.name}: ${error.message}`;
    return String(error);
  }

  function details(error: unknown): string {
    const stack = error instanceof Error && error.stack ? `\n${error.stack}` : '';
    return `${name} view failed: ${describe(error)}${stack}`;
  }

  function onerror(error: unknown) {
    copied = false;
    const stack = error instanceof Error && error.stack ? error.stack.slice(0, 2000) : undefined;
    reportError(`frontend:${name.toLowerCase()}`, describe(error), null, { stack });
  }

  async function copy(error: unknown) {
    copied = await copyText(details(error));
  }
</script>

<svelte:boundary {onerror}>
  {@render children()}
  {#snippet failed(error, reset)}
    <section class="view-failed" role="alert" data-testid="view-failed">
      <h2>{name} could not be shown</h2>
      <p class="msg" data-testid="view-failed-message">{describe(error)}</p>
      <div class="actions">
        <button type="button" data-testid="view-failed-copy" onclick={() => void copy(error)}
          >{copied ? 'Copied' : 'Copy details'}</button
        >
        <button
          type="button"
          data-testid="view-failed-retry"
          onclick={() => {
            copied = false;
            reset();
          }}>Retry</button
        >
      </div>
    </section>
  {/snippet}
</svelte:boundary>

<style>
  .view-failed {
    height: 100%;
    box-sizing: border-box;
    padding: 24px;
    background: var(--bg);
    color: var(--fg);
    overflow: auto;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 15px;
  }
  .msg {
    font-family: var(--mono, monospace);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
    margin: 0 0 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
</style>
