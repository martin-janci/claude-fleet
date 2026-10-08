<script lang="ts" module>
  export type Shown =
    | { kind: 'output'; title: string; output: string; exit_code: number; truncated: boolean }
    | { kind: 'image'; title: string; caption: string; mime: string; data: string };
</script>

<script lang="ts">
  // A record action's answer (result views `output` and `image`): a debug
  // device's logs or an install's output as text, or its screenshot. Shown
  // above the record until dismissed; nothing in it changes anything.
  import { copyText } from '../clipboard';

  let { shown, onclose }: { shown: Shown; onclose: () => void } = $props();
</script>

<section class="result" data-testid="action-result" aria-label={shown.title}>
  <header>
    <h5>{shown.title}</h5>
    <div class="row">
      {#if shown.kind === 'output'}
        <button type="button" class="btn btn--quiet" data-testid="action-result-copy" onclick={() => void copyText(shown.output)}
          >Copy</button>
      {/if}
      <button type="button" class="btn btn--quiet" data-testid="action-result-close" onclick={onclose}>Done</button>
    </div>
  </header>
  {#if shown.kind === 'output'}
    {#if shown.exit_code !== 0}
      <p class="meta" data-testid="action-result-exit">Ended with exit code {shown.exit_code}.</p>
    {/if}
    {#if shown.output.trim()}
      <pre data-testid="action-result-output">{shown.output}</pre>
    {:else}
      <p class="meta" data-testid="action-result-empty">It printed nothing.</p>
    {/if}
    {#if shown.truncated}
      <p class="meta" data-testid="action-result-truncated">Cut to the last part; the rest was longer than fleet keeps.</p>
    {/if}
  {:else}
    <img data-testid="action-result-image" src={`data:${shown.mime};base64,${shown.data}`} alt={shown.caption || shown.title} />
    {#if shown.caption}<p class="meta">{shown.caption}</p>{/if}
  {/if}
</section>

<style>
  .result {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.6rem 0.75rem;
    margin-bottom: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  h5 {
    margin: 0;
    font-size: 0.95rem;
  }
  .row {
    display: flex;
    gap: 0.5rem;
  }
  .meta {
    margin: 0;
    font-size: 0.8rem;
    color: var(--fg-muted);
    line-height: 1.4;
  }
  pre {
    margin: 0;
    max-height: 22rem;
    overflow: auto;
    font-size: 0.75rem;
    line-height: 1.35;
    white-space: pre-wrap;
    word-break: break-all;
  }
  img {
    max-width: 100%;
    max-height: 32rem;
    align-self: flex-start;
    border-radius: var(--radius-sm);
  }
</style>
