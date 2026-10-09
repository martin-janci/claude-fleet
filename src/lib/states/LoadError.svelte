<script lang="ts">
  // The states kit's failed load (review round 13): a read that failed is
  // said as a failure, never as an empty list, in a sentence with Retry as
  // the next step. The code and the backend's own words sit under Details.
  import type { IpcError } from '../result';
  import { errorDetail, errorSentence } from '../error_copy';

  let {
    title,
    error,
    onretry = null,
    retrying = false,
    testid = 'load-error',
  }: {
    /** What could not be read: "Couldn't load the library". */
    title: string;
    error: Pick<IpcError, 'code' | 'message'>;
    onretry?: (() => void) | null;
    retrying?: boolean;
    testid?: string;
  } = $props();
</script>

<div class="load-error" role="alert" data-testid={testid}>
  <p class="title">{title}</p>
  <p class="body" data-testid="{testid}-text">{errorSentence(error)}</p>
  <details class="details">
    <summary>Details</summary>
    <code data-testid="{testid}-code">{errorDetail(error)}</code>
  </details>
  {#if onretry}
    <div class="actions">
      <button type="button" class="btn" disabled={retrying} data-testid="{testid}-retry" onclick={onretry}
        >{retrying ? 'Trying…' : 'Retry'}</button
      >
    </div>
  {/if}
</div>

<style>
  .load-error {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.75rem;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .title {
    margin: 0;
    color: var(--fg);
    font-weight: 500;
  }
  .body {
    margin: 0;
  }
  .details {
    font-size: var(--text-2xs);
  }
  .details summary {
    cursor: pointer;
  }
  .details code {
    display: block;
    margin-top: 0.15rem;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    gap: var(--space-1);
  }
</style>
