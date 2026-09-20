<script lang="ts">
  // The Conversations tab's replaced-thread view for one background entry:
  // an agent or command this conversation launched, or a fleet task it
  // dispatched. Read-only — it renders what the transcript and the stores
  // already carry, and fetches nothing.
  import { formatDuration, type BackgroundEntry, type BackgroundReport } from './conversation';
  import Markdown from './MarkdownView.svelte';
  import CopyButton from './CopyButton.svelte';

  let {
    entry,
    onBack,
    onOpenSession,
  }: {
    entry: BackgroundEntry;
    onBack: () => void;
    /** Switch the app to the entry's worker session, when it has one. */
    onOpenSession?: (id: number) => void;
  } = $props();

  const STATUS_WORD: Record<BackgroundEntry['status'], string> = {
    running: 'running',
    done: 'done',
    failed: 'failed',
    stopped: 'stopped',
    idle: 'idle',
  };
  // One report is the entry's own result, already shown above; only a
  // resumed task's several are worth listing separately.
  const reports = $derived(entry.history.length > 1 ? entry.history : []);

  /** The last report to carry a non-null `k` — the same last-non-null-wins
   *  rule `transcriptBackground` uses for `result` and the output file. */
  function newest<K extends keyof BackgroundReport>(k: K): BackgroundReport[K] | null {
    for (let i = entry.history.length - 1; i >= 0; i--) {
      const v = entry.history[i][k];
      if (v !== null) return v;
    }
    return null;
  }

  // A background `Bash` or `Monitor` reports a `<summary>` carrying its exit
  // code and no `<result>` at all. That IS its report, so showing it beats
  // telling the reader nothing came back one click after a row that said it
  // had finished.
  const summary = $derived(entry.error === null && entry.result === null ? newest('summary') : null);
  const nothing = $derived(entry.error === null && entry.result === null && entry.history.length === 0);

  // Launch → newest report. Either end missing means we do not know, and a
  // made-up duration is worse than none.
  const duration = $derived.by(() => {
    const end = newest('at');
    if (!entry.at || !end) return null;
    const ms = Date.parse(end) - Date.parse(entry.at);
    if (!Number.isFinite(ms) || ms < 0) return null;
    return formatDuration(ms);
  });
</script>

<div class="bg-detail" data-testid="bg-detail">
  <div class="bg-head">
    <button type="button" class="linkish" data-testid="bg-detail-back" onclick={onBack}>← Back to conversation</button>
  </div>
  <h3 class="bg-title">
    <span class="bg-kind" data-testid="bg-detail-kind">{entry.kind}</span>
    <span class="bg-label" data-testid="bg-detail-label">{entry.label}</span>
    <span class="bg-status" data-status={entry.status} data-testid="bg-detail-status">{STATUS_WORD[entry.status]}</span>
    {#if duration}<span class="bg-dur" data-testid="bg-detail-duration">{duration}</span>{/if}
  </h3>

  {#if entry.sessionId !== null && onOpenSession}
    {@const workerId = entry.sessionId}
    <p class="bg-worker">
      <button type="button" class="linkish" data-testid="bg-detail-open-session" onclick={() => onOpenSession(workerId)}
        >Open the worker session →</button
      >
    </p>
  {/if}

  {#if entry.error}
    <p class="bg-error" data-testid="bg-detail-error">{entry.error}</p>
  {:else if entry.result}
    <div class="bg-result" data-testid="bg-detail-result"><Markdown source={entry.result} /></div>
  {:else if summary}
    <p class="bg-summary" data-testid="bg-detail-summary">{summary}</p>
  {:else if nothing}
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
        {#if r.summary}<p class="bg-summary">{r.summary}</p>{/if}
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
  .bg-dur {
    flex: 0 0 auto;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .bg-worker {
    margin: 0 0 0.6rem;
    font-size: 0.8rem;
  }
  .bg-summary {
    margin: 0 0 0.4rem;
    overflow-wrap: anywhere;
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
