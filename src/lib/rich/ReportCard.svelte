<script lang="ts">
  // A task report as a card: the outcome, the summary, then what needs a
  // person first (blockers, warnings), then tests and follow-ups. It is what
  // the worker said, not proof (report.rs): the card says "reported".
  import type { TaskReport } from '../rich_blocks';
  import { insertIntoComposer } from '../conversation';
  import Badge from '../Badge.svelte';
  import StatusChip from '../kit/StatusChip.svelte';
  import { outcomeLabel, outcomeState } from '../mission_triage';
  import CopyButton from '../CopyButton.svelte';
  import Markdown from '../MarkdownView.svelte';

  let {
    report,
    raw,
    marker = null,
    title,
    sessionId = null,
  }: { report: TaskReport; raw: string; marker?: string | null; title?: string; sessionId?: number | null } = $props();

</script>

<section class="card rich-card {report.outcome}" data-testid="rich-report" aria-label="Task report">
  <header>
    <strong>{title ?? 'Task report'}</strong>
    <StatusChip state={outcomeState(report.outcome)} label={outcomeLabel(report.outcome)} testid="rich-report-outcome" />
    {#if report.confidence}<Badge label={`confidence ${report.confidence}`} tone="muted" testid="rich-report-confidence" />{/if}
  </header>
  {#if report.summary}
    <div class="summary"><Markdown source={report.summary} /></div>
  {/if}
  {#if report.blockers.length}
    <div class="group crit" data-testid="rich-report-blockers">
      <h6>Blockers</h6>
      <ul>{#each report.blockers as b, k (k)}<li>{b}</li>{/each}</ul>
    </div>
  {/if}
  {#if report.warnings.length}
    <div class="group warn" data-testid="rich-report-warnings">
      <h6>Warnings</h6>
      <ul>{#each report.warnings as w, k (k)}<li>{w}</li>{/each}</ul>
    </div>
  {/if}
  {#if report.tests_run.length}
    <div class="group" data-testid="rich-report-tests">
      <h6>Tests run</h6>
      <ul class="mono">{#each report.tests_run as t, k (k)}<li>{t}</li>{/each}</ul>
    </div>
  {/if}
  {#if report.followups.length}
    <div class="group" data-testid="rich-report-followups">
      <h6>Follow-ups</h6>
      <ul class="followups">
        {#each report.followups as f, k (k)}
          <li>
            <span>{f}</span>
            {#if sessionId !== null}
              <button
                type="button"
                class="ghost"
                data-testid="rich-report-followup"
                title="Put this follow-up in the composer"
                onclick={() => sessionId !== null && insertIntoComposer(sessionId, f)}>Ask</button
              >
            {/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}
  <details class="raw">
    <summary>{marker ? `Reported after ${marker}` : 'Source'}</summary>
    <div class="raw-bar"><CopyButton text={raw} label="Copy report JSON" /></div>
    <pre>{raw}</pre>
  </details>
</section>

<style>
  .card {
    --tone: var(--fg-muted);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--tone);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--tone) 6%, var(--bg-pane));
  }
  .done { --tone: var(--usage-ok); }
  .partial { --tone: var(--usage-warn); }
  .blocked,
  .failed { --tone: var(--usage-crit); }
  header {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    align-items: center;
  }
  .summary { font-size: 0.92em; }
  h6 {
    margin: 0 0 0.2rem;
    font-size: var(--text-2xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .group.crit h6 { color: var(--usage-crit); }
  .group.warn h6 { color: var(--usage-warn); }
  ul {
    margin: 0;
    padding-left: 1.2em;
    font-size: 0.88em;
  }
  li { margin: 0.1em 0; }
  .mono { font-family: var(--mono); font-size: 0.8em; }
  .followups li > span { margin-right: 0.4em; }
  .ghost {
    background: none;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--control-fg-quiet);
    font-size: var(--text-2xs);
    padding: 0 0.4rem;
    cursor: pointer;
  }
  .ghost:hover { color: var(--fg); }
  .raw summary {
    cursor: pointer;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    font-family: var(--mono);
  }
  .raw-bar { display: flex; justify-content: flex-end; }
  .raw pre {
    margin: 0.2rem 0 0;
    max-height: 18rem;
    overflow: auto;
    font-size: var(--text-2xs);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
