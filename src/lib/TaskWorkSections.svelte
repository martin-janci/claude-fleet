<script lang="ts">
  // The task page's shared-work sections (design 2026-09-29 §4): the brief
  // (a native task's notes), subtasks (+ add, and the Start split button), agent proposals
  // (accept / reject; rejected behind a toggle), delegated jobs with their
  // result, and agent steps per session — "per the agent", never a status.
  // `part` splits them around the page's Sessions list: the work above it,
  // the steps below. All text renders as text.
  import { createWorkTask, decideWorkProposal, mergeWorkProposal } from './work';
  import { duplicateProposal, openTask, readErrorText, type TaskDetail } from './work_view';
  import ProposedBy from './ProposedBy.svelte';
  import WorkButton from './WorkButton.svelte';
  import TaskBrief from './TaskBrief.svelte';
  import type { Result } from './result';
  import { subtaskMark, subtaskProgress } from './task_detail';

  let { detail, part = 'all' }: { detail: TaskDetail; part?: 'all' | 'work' | 'steps' } = $props();

  const showWork = $derived(part !== 'steps');
  const showSteps = $derived(part !== 'work');
  // Local work goes three levels deep (epic → task → subtask); a ticket
  // takes subtasks at any depth (its tracker owns that); a bare key has no
  // item. An older hub sends no `level`: then a subtask takes none, as before.
  const LOCAL_DEPTH_MAX = 3;
  const canAddSubtask = $derived(
    detail.task.item_id != null &&
      (detail.task.kind !== 'local'
        ? !detail.task.parent_task_id
        : (detail.task.level ?? (detail.task.parent_task_id ? LOCAL_DEPTH_MAX : 1)) < LOCAL_DEPTH_MAX),
  );

  let adding = $state(false);
  let newTitle = $state('');
  let busy = $state(false);
  let showRejected = $state(false);
  let err = $state<string | null>(null);
  let addInput = $state<HTMLInputElement | null>(null);
  $effect(() => {
    if (adding) addInput?.focus();
  });

  async function run<T>(p: Promise<Result<T>>): Promise<Result<T>> {
    busy = true;
    const r = await p;
    busy = false;
    err = r.ok ? null : readErrorText(r.error);
    return r;
  }

  async function addSubtask() {
    const title = newTitle.trim();
    if (!title || busy) return;
    const r = await run(createWorkTask({ title, parent: detail.task.task_id }));
    if (!r.ok) return;
    newTitle = '';
    adding = false;
  }
</script>

<div class="tws">
  {#if err}<p class="err" role="alert" data-testid="task-sections-error">{err}</p>{/if}

  {#if showWork}
    <TaskBrief task={detail.task} notes={detail.notes} />

    <section data-testid="task-subtasks">
      <h3>
        Subtasks <span class="n" data-testid="task-subtask-progress">{subtaskProgress(detail.subtasks) ?? 0}</span>
        {#if canAddSubtask}
          <button class="btn btn--quiet" type="button" data-testid="task-add-subtask" onclick={() => (adding = true)}>+ Add subtask</button>
        {/if}
      </h3>
      {#if adding}
        <input
          class="add"
          aria-label="Subtask title"
          placeholder="Subtask title — Enter to add, Esc to cancel"
          bind:this={addInput}
          bind:value={newTitle}
          onkeydown={(e) => {
            if (e.key === 'Enter') void addSubtask();
            if (e.key === 'Escape') adding = false;
          }}
        />
      {/if}
      {#each detail.subtasks ?? [] as s (s.item_id)}
        {@const mark = subtaskMark(s.status)}
        <div class="row" data-testid="task-subtask">
          <span class="mark mark--{s.status ?? 'todo'}" role="img" aria-label={mark.label} data-testid="task-subtask-mark">{mark.glyph}</span>
          <button class="link" type="button" onclick={() => openTask(s.task_id)}>{s.title}</button>
          {#if s.key}<span class="key">{s.key}</span>{/if}
          {#if s.origin === 'agent'}<span class="chip agent">delegated job</span>{:else if s.origin === 'proposed'}<span class="chip prop"
              >from a proposal</span
            >{/if}
          <span class="spacer"></span>
          {#if s.status === 'todo' && s.live_sessions === 0}
            <!-- Redesign 6.6: the same split button as a task row. -->
            <WorkButton task={{ ...s, sessions: [] }} />
          {:else}
            <span class="muted">{s.live_sessions > 0 ? `${s.live_sessions} live` : (s.job_state ?? s.status ?? '')}</span>
          {/if}
        </div>
      {:else}
        {#if !adding}<p class="muted">No subtasks yet.</p>{/if}
      {/each}
    </section>

    {#if (detail.proposals?.length ?? 0) > 0 || (detail.rejected_proposals?.length ?? 0) > 0}
      <section data-testid="task-proposals">
        <h3>Proposals <span class="n">{detail.proposals?.length ?? 0}</span><span class="hint">agents propose, you decide</span></h3>
        {#each detail.proposals ?? [] as p (p.item_id)}
          <div class="prop-card" data-testid="task-proposal">
            <div><strong>{p.title}</strong> {#if p.key}<span class="key">{p.key}</span>{/if}</div>
            {#if p.why}<p class="text">{p.why}</p>{/if}
            {#if p.notes}<p class="text muted">{p.notes}</p>{/if}
            {#if p.proposed_by}<p class="muted small">Proposed by {p.proposed_by}</p>{/if}
            {#if p.duplicate}
              <!-- Redesign 6.9 (K4): Jev's "may duplicate". Merge moves
                   what hangs on the proposal (its sessions' links, its
                   subtasks) to the existing task and closes the proposal;
                   Keep both accepts it. A person decides either way. -->
              <div class="dup" data-testid="task-proposal-duplicate">
                <span
                  >May duplicate <button
                    type="button"
                    class="link"
                    data-testid="task-proposal-duplicate-open"
                    onclick={() => p.duplicate && openTask(p.duplicate.task_id)}
                    >{p.duplicate.key ?? p.duplicate.title}</button
                  ></span
                >
                <ProposedBy
                  proposal={duplicateProposal(p)}
                  field="duplicate"
                  testid="task-proposal-duplicate-by"
                />
              </div>
              <div class="acts">
                <button
                  class="btn"
                  type="button"
                  data-testid="task-proposal-merge"
                  title="Move its sessions and subtasks to the existing task, then close this proposal"
                  disabled={busy}
                  onclick={() =>
                    void run(
                      p.duplicate
                        ? mergeWorkProposal(p.item_id, p.duplicate.item_id)
                        : decideWorkProposal(p.item_id, false),
                    )}>Merge</button
                >
                <button
                  class="btn"
                  type="button"
                  data-testid="task-proposal-keep-both"
                  title="Accept this proposal as a task of its own"
                  disabled={busy}
                  onclick={() => void run(decideWorkProposal(p.item_id, true))}>Keep both</button
                >
              </div>
            {:else}
            <div class="acts">
              <button
                class="btn btn--primary"
                type="button"
                data-testid="task-proposal-accept"
                disabled={busy}
                onclick={() => void run(decideWorkProposal(p.item_id, true))}>Accept</button
              >
              <button
                class="btn"
                type="button"
                data-testid="task-proposal-reject"
                disabled={busy}
                onclick={() => void run(decideWorkProposal(p.item_id, false))}>Reject</button
              >
            </div>
            {/if}
          </div>
        {/each}
        {#if (detail.rejected_proposals?.length ?? 0) > 0}
          <button
            class="btn btn--quiet"
            type="button"
            data-testid="task-rejected-toggle"
            aria-expanded={showRejected}
            onclick={() => (showRejected = !showRejected)}
            >{showRejected ? 'Hide' : 'Show'} rejected ({detail.rejected_proposals?.length})</button
          >
          {#if showRejected}
            <ul class="rejected">
              {#each detail.rejected_proposals ?? [] as r (r.item_id)}<li>{r.title}</li>{/each}
            </ul>
          {/if}
        {/if}
      </section>
    {/if}

    {#if (detail.jobs?.length ?? 0) > 0 || detail.job_result}
      <section data-testid="task-jobs">
        <h3>Delegated jobs <span class="n">{detail.jobs?.length ?? 0}</span></h3>
        {#if detail.job_result}
          <p class="text result" data-testid="task-own-job-result">{detail.job_result}</p>
        {/if}
        {#each detail.jobs ?? [] as j (j.item_id)}
          <div class="job">
            <div>
              <strong>{j.title}</strong> <span class="state state--{j.state}">{j.state}</span>{#if j.worker}<span class="muted">
                  · {j.worker}</span
                >{/if}
            </div>
            {#if j.result}<p class="text result" data-testid="task-job-result">{j.result}</p>{/if}
          </div>
        {/each}
      </section>
    {/if}
  {/if}

  {#if showSteps}
    <section data-testid="task-steps">
      <h3>Agent steps <span class="hint">from Claude Code tasks · per the agent, not proof of done</span></h3>
      {#each detail.steps ?? [] as g, i (g.claude_session_id)}
        {@const steps = g.steps ?? []}
        <details open={i === 0}>
          <summary>{g.label} <span class="muted">{steps.filter((s) => s.state === 'completed').length}/{steps.length}</span></summary>
          <ul>
            {#each steps as s, k (k)}
              <li class="step step--{s.state}" data-testid="task-step">
                <span class="m" aria-hidden="true"
                  >{s.state === 'completed' ? '✓' : s.state === 'in_progress' ? '◐' : s.state === 'cancelled' ? '×' : '○'}</span
                >{s.text}
              </li>
            {/each}
          </ul>
        </details>
      {:else}
        <p class="muted">No agent steps recorded.</p>
      {/each}
    </section>
  {/if}
</div>

<style>
  .tws {
    display: grid;
    gap: 12px;
    margin: 0.6rem 0;
  }
  h3 {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0 0 6px;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  h3 .btn {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
  }
  .n {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }
  .hint {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
  }
  .text {
    margin: 0;
    white-space: pre-wrap;
    max-width: 72ch;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 3px 0;
  }
  .spacer {
    flex: 1;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--fg);
    font: inherit;
    cursor: pointer;
    text-align: left;
  }
  .link:hover {
    text-decoration: underline;
  }
  .key {
    font-family: var(--mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .chip {
    font-size: var(--text-2xs);
    border-radius: var(--radius-pill);
    padding: 0 6px;
    white-space: nowrap;
  }
  .prop {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .agent {
    background: var(--chip-bg);
    color: var(--fg-2);
  }
  .prop-card {
    border: 1px dashed var(--border);
    border-radius: var(--radius-md);
    padding: 8px 10px;
    display: grid;
    gap: 4px;
    margin-bottom: 6px;
  }
  .acts {
    display: flex;
    gap: 6px;
  }
  .dup {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: var(--text-xs);
    color: var(--fg-2);
  }
  .dup .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .job {
    padding: 4px 0;
  }
  .result {
    border-left: 2px solid var(--control-border);
    padding-left: 8px;
  }
  .state--done {
    color: var(--usage-ok);
  }
  .state--failed,
  .state--cancelled {
    color: var(--usage-crit);
  }
  .mark {
    flex: none;
    width: 1em;
    text-align: center;
    color: var(--fg-muted);
  }
  .mark--in_progress {
    color: var(--accent);
  }
  .mark--done {
    color: var(--usage-ok);
  }
  details {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 4px 8px;
    margin-bottom: 4px;
  }
  summary {
    cursor: pointer;
  }
  ul {
    list-style: none;
    margin: 4px 0;
    padding: 0 0 0 12px;
  }
  .step {
    display: flex;
    gap: 6px;
  }
  .m {
    font-family: var(--mono);
    color: var(--fg-muted);
    width: 1em;
    flex: none;
  }
  .step--completed .m {
    color: var(--usage-ok);
  }
  .step--in_progress .m {
    color: var(--accent);
  }
  .muted {
    color: var(--fg-muted);
    margin: 0;
  }
  .small {
    font-size: var(--text-2xs);
  }
  .err {
    color: var(--usage-crit);
    margin: 0;
  }
  .add {
    width: 100%;
    margin-bottom: 4px;
  }
</style>
