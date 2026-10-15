<script lang="ts">
  // One panel for a task's result (design 2026-10-13, D6): the verdict and
  // what was done first, then what speaks for it, then the detail folded.
  // The report is what the worker said ("reported"); the gate, the changes
  // and the PRs are what fleet read itself. A follow-up becomes a task here
  // instead of only filling the composer.
  import {
    createFollowupTask,
    headline,
    validationLines,
    type ResultPr,
    type TaskResultView,
    type ValidationState,
  } from './task_result';
  import { setWorkStatus } from './work';
  import { insertIntoComposer } from './conversation';
  import { openExternal } from './open_external';
  import type { OfState } from './kit/status';
  import Badge from './Badge.svelte';
  import StatusChip from './kit/StatusChip.svelte';
  import Markdown from './MarkdownView.svelte';

  let {
    result,
    title,
    sessionId = null,
    canMarkDone = false,
    onOpenTask,
    onChanged,
  }: {
    result: TaskResultView;
    title: string;
    sessionId?: number | null;
    /** The parent's call: a local item that is not done yet. */
    canMarkDone?: boolean;
    onOpenTask?: (id: number) => void;
    onChanged?: () => void;
  } = $props();

  const TONE_STATE: Record<'success' | 'warning' | 'danger', OfState> = {
    success: 'done',
    warning: 'waiting',
    danger: 'failed',
  };
  const MARK: Record<ValidationState, string> = {
    pass: 'passed',
    fail: 'failed',
    pending: 'pending',
    reported: 'reported',
  };

  const head = $derived(headline(result));
  const report = $derived(result.report);
  const lines = $derived(validationLines(result));
  const unmet = $derived(result.gate.checks.filter((c) => c.state !== 'pass'));
  const ev = $derived(result.evidence);
  const commits = $derived(ev?.commits ?? []);
  const files = $derived(ev?.files ?? []);
  const moreCommits = $derived(Math.max(0, (ev?.commits_total ?? 0) - commits.length));
  const moreFiles = $derived(Math.max(0, (ev?.files_total ?? 0) - files.length));

  // Tasks made from this panel, by follow-up index, until the parent's
  // reload answers them in `result.followups` itself.
  let created = $state<Record<number, number>>({});
  let busy = $state<number | null>(null);
  let rowError = $state<Record<number, string>>({});
  const followups = $derived(
    result.followups.map((f, i) => ({ text: f.text, item_id: f.item_id ?? created[i] ?? null })),
  );

  let ownTitle = $state('');
  let ownBusy = $state(false);
  let ownError = $state<string | null>(null);
  let ownCreated = $state<{ id: number; title: string }[]>([]);

  let marking = $state(false);
  let markError = $state<string | null>(null);

  async function createAt(index: number) {
    if (busy !== null) return;
    busy = index;
    const r = await createFollowupTask(result.task_id, { index });
    busy = null;
    if (r.ok) {
      created = { ...created, [index]: r.value.id };
      rowError = { ...rowError, [index]: '' };
      onChanged?.();
    } else rowError = { ...rowError, [index]: r.error.message };
  }

  async function createOwn() {
    const t = ownTitle.trim();
    if (!t || ownBusy) return;
    ownBusy = true;
    const r = await createFollowupTask(result.task_id, { title: t });
    ownBusy = false;
    if (r.ok) {
      const { id, title: made } = r.value;
      if (!ownCreated.some((c) => c.id === id)) ownCreated = [...ownCreated, { id, title: made }];
      ownTitle = '';
      ownError = null;
      onChanged?.();
    } else ownError = r.error.message;
  }

  async function markDone() {
    if (marking) return;
    marking = true;
    const r = await setWorkStatus(result.item_id, 'done');
    marking = false;
    if (r.ok) {
      markError = null;
      onChanged?.();
    } else markError = r.error.message;
  }

  function prLabel(pr: ResultPr): string {
    const parts = [pr.number != null ? `#${pr.number}` : 'PR'];
    if (pr.state) parts.push(pr.state);
    if (pr.ci_status) parts.push(`CI ${pr.ci_status}`);
    if (pr.review_decision) parts.push(`review ${pr.review_decision.toLowerCase().replaceAll('_', ' ')}`);
    return parts.join(' · ');
  }
</script>

<section class="card rich-card {head.tone}" data-testid="task-result" aria-label="Task result">
  <header>
    <StatusChip state={TONE_STATE[head.tone]} label={head.label} testid="task-result-headline" />
    <strong>{title}</strong>
    <Badge label="reported" tone="muted" title="What the session said; the checks, changes and PRs are what fleet read itself" />
    {#if report.confidence}<Badge label={`confidence ${report.confidence}`} tone="muted" />{/if}
  </header>

  {#if report.summary}
    <div class="summary" data-testid="task-result-summary"><Markdown source={report.summary} /></div>
  {/if}
  {#if report.done.length}
    <div class="group" data-testid="task-result-done">
      <h6>What changed</h6>
      <ul>{#each report.done as d, k (k)}<li>{d}</li>{/each}</ul>
    </div>
  {/if}

  {#if lines.length}
    <div class="group" data-testid="task-result-validation">
      <h6>Validation</h6>
      <ul class="plain">
        {#each lines as l, k (k)}
          <li class="row"><span class="mark {l.state}">{MARK[l.state]}</span><span class="mono">{l.label}</span></li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if result.gate.state !== 'passed'}
    <div class="group {result.gate.state === 'blocked' ? 'crit' : 'warn'}" data-testid="task-result-gate">
      <h6>{result.gate.state === 'blocked' ? 'Blocks completion' : 'Not verified yet'}</h6>
      <ul>
        {#each unmet as c, k (k)}
          <li>
            <span class="mark {c.state === 'fail' ? 'fail' : 'pending'}">{c.state === 'fail' ? 'failed' : 'pending'}</span>
            {c.line}{#if c.detail}<span class="muted"> — {c.detail}</span>{/if}
          </li>
        {/each}
        {#each result.gate.open_children as c (c.id)}
          <li>
            <span class="mark pending">open subtask</span>
            {#if onOpenTask}
              <button type="button" class="linkish" onclick={() => onOpenTask?.(c.id)}>{c.title}</button>
            {:else}{c.title}{/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if result.prs.length}
    <div class="prs" data-testid="task-result-prs">
      {#each result.prs as pr (pr.url)}
        <button type="button" class="ghost" title={pr.url} onclick={() => void openExternal(pr.url)}>{prLabel(pr)}</button>
      {/each}
    </div>
  {/if}

  {#if ev}
    <details data-testid="task-result-changes">
      <summary
        >Changes: {ev.commits_total ?? commits.length} commit{(ev.commits_total ?? commits.length) === 1 ? '' : 's'},
        {ev.files_total ?? files.length} file{(ev.files_total ?? files.length) === 1 ? '' : 's'}</summary
      >
      {#if ev.error}<p class="err">Git could not be read: {ev.error}</p>{/if}
      {#if commits.length}
        <ul class="plain mono">
          {#each commits as c (c.sha)}<li><span class="muted">{c.sha.slice(0, 7)}</span> {c.subject}</li>{/each}
          {#if moreCommits}<li class="muted">{moreCommits} more</li>{/if}
        </ul>
      {/if}
      {#if files.length}
        <ul class="plain mono">
          {#each files as f (f.path)}
            <li>
              {f.path}
              {#if f.added != null || f.removed != null}<span class="muted">+{f.added ?? 0} −{f.removed ?? 0}</span>{/if}
            </li>
          {/each}
          {#if moreFiles}<li class="muted">{moreFiles} more</li>{/if}
        </ul>
      {/if}
      {#if ev.uncommitted}
        <p class="warn-line">{ev.uncommitted} uncommitted file{ev.uncommitted === 1 ? '' : 's'} in the checkout</p>
      {/if}
    </details>
  {/if}

  {#if report.decisions.length}
    <details data-testid="task-result-decisions">
      <summary>Decisions ({report.decisions.length})</summary>
      <ul>{#each report.decisions as d, k (k)}<li>{d}</li>{/each}</ul>
    </details>
  {/if}
  {#if report.remaining.length}
    <details data-testid="task-result-remaining">
      <summary>Left ({report.remaining.length})</summary>
      <ul>{#each report.remaining as d, k (k)}<li>{d}</li>{/each}</ul>
    </details>
  {/if}
  {#if report.blockers.length}
    <details class="crit" data-testid="task-result-blockers">
      <summary>Blockers ({report.blockers.length})</summary>
      <ul>{#each report.blockers as d, k (k)}<li>{d}</li>{/each}</ul>
    </details>
  {/if}
  {#if report.warnings.length}
    <details class="warn" data-testid="task-result-warnings">
      <summary>Warnings ({report.warnings.length})</summary>
      <ul>{#each report.warnings as d, k (k)}<li>{d}</li>{/each}</ul>
    </details>
  {/if}

  <div class="group" data-testid="task-result-followups">
    <h6>Follow-ups</h6>
    {#if followups.length || ownCreated.length}
      <ul class="plain">
        {#each followups as f, i (i)}
          <li class="row">
            <span class="grow">{f.text}</span>
            {#if f.item_id !== null}
              <button
                type="button"
                class="ghost"
                data-testid={`task-result-followup-open-${f.item_id}`}
                onclick={() => f.item_id !== null && onOpenTask?.(f.item_id)}>Open task</button
              >
            {:else}
              <button
                type="button"
                class="ghost"
                data-testid="task-result-followup-create"
                disabled={busy !== null}
                title="Make this follow-up a task next to this one"
                onclick={() => void createAt(i)}>{busy === i ? 'Creating…' : 'Create task'}</button
              >
            {/if}
          </li>
          {#if rowError[i]}<li class="err" data-testid="task-result-followup-error">{rowError[i]}</li>{/if}
        {/each}
        {#each ownCreated as c (c.id)}
          <li class="row">
            <span class="grow">{c.title}</span>
            <button
              type="button"
              class="ghost"
              data-testid={`task-result-followup-open-${c.id}`}
              onclick={() => onOpenTask?.(c.id)}>Open task</button
            >
          </li>
        {/each}
      </ul>
    {/if}
    <form
      class="row own"
      onsubmit={(e) => {
        e.preventDefault();
        void createOwn();
      }}
    >
      <input
        type="text"
        class="grow"
        placeholder="Another follow-up…"
        aria-label="Follow-up title"
        maxlength="200"
        data-testid="task-result-followup-title"
        bind:value={ownTitle}
      />
      <button type="submit" class="ghost" data-testid="task-result-followup-own" disabled={ownBusy || !ownTitle.trim()}
        >{ownBusy ? 'Creating…' : 'Create follow-up'}</button
      >
    </form>
    {#if ownError}<p class="err" data-testid="task-result-followup-own-error">{ownError}</p>{/if}
  </div>

  {#if sessionId !== null || canMarkDone}
    <div class="actions">
      {#if sessionId !== null}
        <button
          type="button"
          class="ghost"
          data-testid="task-result-ask"
          title="Put a question about this result in the composer"
          onclick={() => sessionId !== null && insertIntoComposer(sessionId, 'About the result of this task: ')}
          >Ask about result</button
        >
      {/if}
      {#if canMarkDone}
        <button
          type="button"
          class="ghost"
          data-testid="task-result-mark-done"
          disabled={marking}
          title={result.gate.state === 'passed'
            ? 'Set this task to done'
            : 'Set this task to done although the checks above are not all passed'}
          onclick={() => void markDone()}>{marking ? 'Marking…' : 'Mark done'}</button
        >
      {/if}
      {#if markError}<span class="err" data-testid="task-result-mark-done-error">{markError}</span>{/if}
    </div>
  {/if}
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
  .success { --tone: var(--usage-ok); }
  .warning { --tone: var(--usage-warn); }
  .danger { --tone: var(--usage-crit); }
  header,
  .prs,
  .actions {
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
  .group.crit h6,
  details.crit > summary { color: var(--usage-crit); }
  .group.warn h6,
  details.warn > summary { color: var(--usage-warn); }
  ul {
    margin: 0;
    padding-left: 1.2em;
    font-size: 0.88em;
  }
  ul.plain {
    list-style: none;
    padding-left: 0;
  }
  li { margin: 0.1em 0; }
  .row {
    display: flex;
    gap: 0.45rem;
    align-items: baseline;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .mono { font-family: var(--mono); font-size: 0.8em; word-break: break-word; }
  .muted { color: var(--fg-muted); }
  .mark {
    flex: none;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .mark.pass { color: var(--usage-ok); }
  .mark.fail { color: var(--usage-crit); }
  .mark.pending { color: var(--usage-warn); }
  details > summary {
    cursor: pointer;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  details > ul { margin-top: 0.2rem; }
  .own { margin-top: 0.3rem; }
  input {
    background: var(--bg);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-size: 0.88em;
    padding: 0.1rem 0.4rem;
  }
  .ghost {
    background: none;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--control-fg-quiet);
    font-size: var(--text-2xs);
    padding: 0 0.4rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .ghost:hover:not(:disabled) { color: var(--fg); }
  .ghost:disabled { opacity: 0.55; cursor: default; }
  .linkish {
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }
  .err,
  .warn-line {
    margin: 0.2rem 0 0;
    font-size: 0.85em;
  }
  .err { color: var(--usage-crit); list-style: none; }
  .warn-line { color: var(--usage-warn); }
</style>
