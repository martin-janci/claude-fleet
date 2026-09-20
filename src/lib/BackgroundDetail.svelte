<script lang="ts">
  // The Conversations tab's replaced-thread view for one background entry:
  // an agent or command this conversation launched, or a fleet task it
  // dispatched. Read-only — it renders what the transcript and the stores
  // already carry, and fetches nothing.
  import type { BackgroundEntry } from './conversation';
  import Markdown from './MarkdownView.svelte';
  import CopyButton from './CopyButton.svelte';

  let { entry, onBack }: { entry: BackgroundEntry; onBack: () => void } = $props();

  const STATUS_WORD: Record<BackgroundEntry['status'], string> = {
    running: 'running',
    done: 'done',
    failed: 'failed',
    stopped: 'stopped',
  };
  // One report is the entry's own result, already shown above; only a
  // resumed task's several are worth listing separately.
  const reports = $derived(entry.history.length > 1 ? entry.history : []);
</script>

<div class="bg-detail" data-testid="bg-detail">
  <div class="bg-head">
    <button type="button" class="linkish" data-testid="bg-detail-back" onclick={onBack}>← Back to conversation</button>
  </div>
  <h3 class="bg-title">
    <span class="bg-kind" data-testid="bg-detail-kind">{entry.kind}</span>
    <span class="bg-label" data-testid="bg-detail-label">{entry.label}</span>
    <span class="bg-status" data-status={entry.status}>{STATUS_WORD[entry.status]}</span>
  </h3>

  {#if entry.error}
    <p class="bg-error" data-testid="bg-detail-error">{entry.error}</p>
  {:else if entry.result}
    <div class="bg-result" data-testid="bg-detail-result"><Markdown source={entry.result} /></div>
  {:else}
    <p class="muted" data-testid="bg-detail-empty">This background task has not reported back yet.</p>
  {/if}

  {#if reports.length > 0}
    <h4 class="bg-sub">Reports ({reports.length})</h4>
    {#each reports as r, i (i)}
      <div class="bg-report" data-testid="bg-detail-report">
        <div class="bg-report-head">
          {#if r.at}<time datetime={r.at}>{new Date(r.at).toLocaleTimeString()}</time>{/if}
          {#if r.status}<span class="bg-report-status">{r.status}</span>{/if}
        </div>
        {#if r.result}<Markdown source={r.result} />{/if}
      </div>
    {/each}
  {/if}

  {#if entry.outputFile}
    <div class="bg-output" data-testid="bg-detail-output">
      <span class="bg-output-label">Full output on the host</span>
      <code>{entry.outputFile}</code>
      <CopyButton text={entry.outputFile} label="Copy path" />
    </div>
  {/if}
</div>

<style>
  .bg-detail {
    padding: 0.6rem 0.9rem 1.2rem;
    overflow-y: auto;
    min-height: 0;
  }
  .bg-head {
    margin-bottom: 0.5rem;
  }
  .bg-title {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin: 0 0 0.6rem;
    font-size: 0.95rem;
    min-width: 0;
  }
  .bg-kind {
    flex: 0 0 auto;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .bg-label {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .bg-status {
    flex: 0 0 auto;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .bg-status[data-status='failed'] {
    color: var(--usage-crit);
  }
  .bg-status[data-status='stopped'] {
    color: var(--usage-warn);
  }
  .bg-error {
    color: var(--usage-crit);
    overflow-wrap: anywhere;
  }
  .bg-sub {
    margin: 1rem 0 0.4rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
  .bg-report {
    border-left: 2px solid var(--border);
    padding-left: 0.6rem;
    margin-bottom: 0.8rem;
  }
  .bg-report-head {
    display: flex;
    gap: 0.4rem;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .bg-output {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    margin-top: 1rem;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .bg-output code {
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--fg-muted);
  }
  .linkish {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
</style>
