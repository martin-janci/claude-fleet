<!-- The Agents tab's Debug section (`service::agents::debug`): per rule,
     whether it polls and why not, its next poll, its last poll error and its
     jobs by state; a job opens whole, its proposed actions included. Loaded
     when opened; nothing here changes a rule or answers an action. -->
<script lang="ts">
  import Button from '../kit/Button.svelte';
  import StatusDot from '../kit/StatusDot.svelte';
  import { errorText } from '../error_copy';
  import type { IpcError } from '../result';
  import { debugDot } from '../routines';
  import { actionTitle, agentJob, explainAgents, jobCountsLine, ruleLine, type AgentJobDetail, type AgentsExplain } from '../agents';

  let { clock }: { clock: (s: number) => string } = $props();

  let open = $state(false);
  let explain = $state<AgentsExplain | null>(null);
  let job = $state<AgentJobDetail | null>(null);
  let error = $state<IpcError | null>(null);

  async function load() {
    const r = await explainAgents();
    if (r.ok) {
      explain = r.value;
      error = null;
    } else {
      error = r.error;
    }
  }

  async function openJob(id: number) {
    const r = await agentJob(id);
    if (r.ok) {
      job = r.value;
      error = null;
    } else {
      error = r.error;
    }
  }
</script>

<details
  class="debug"
  data-testid="agents-debug"
  bind:open
  ontoggle={() => {
    if (open && !explain) void load();
  }}
>
  <summary>Debug <span class="meta">why a rule finds or runs nothing</span></summary>
  <div class="bar">
    {#if explain}
      <span class="meta" data-testid="agents-debug-settings">
        {explain.automation_paused ? 'Pause all is on · ' : ''}polls every {explain.poll_interval_secs || '—'} s · {explain.max_concurrent} at once · {explain.max_retries} retries
      </span>
    {/if}
    <span class="grow"></span>
    <Button variant="quiet" size="sm" testid="agents-debug-refresh" onclick={() => void load()}>Refresh</Button>
  </div>
  {#if error}<p class="err" role="alert">{errorText(error)}</p>{/if}
  {#if explain}
    {#each explain.loops as l (l.name)}
      {#if l.result === 'error'}<p class="err" data-testid="agents-debug-loop-error">{l.label}: {l.last_error}</p>{/if}
    {/each}
    <ul class="list" data-testid="agents-debug-rules">
      {#each explain.rules as r (r.rule.id)}
        <li data-testid="agents-debug-rule">
          <StatusDot state={debugDot(r.state)} label={null} />
          <span class="body">
            <strong>{r.summary}</strong>
            <span class="meta">
              rule {r.rule.id} · {ruleLine({ ...r.rule, active: r.active })} · {jobCountsLine(r.jobs)}{r.polled_at ? ` · polled ${clock(r.polled_at)}` : ''}{r.next_poll_at ? ` · next ${clock(r.next_poll_at)}` : ''}
            </span>
            {#if r.poll_error}<span class="bad">Poll failed {clock(r.poll_error.at)}: {r.poll_error.message}</span>{/if}
            {#if r.last_failed}
              <button type="button" class="link" data-testid="agents-debug-last-failed" onclick={() => void openJob(r.last_failed!.id)}
                >Last failed: {r.last_failed.subject} · {r.last_failed.error ?? 'no message'}</button
              >
            {/if}
            {#if r.next_job}
              <button type="button" class="link" onclick={() => void openJob(r.next_job!.id)}>Next: {r.next_job.subject}</button>
            {/if}
          </span>
        </li>
      {:else}
        <li class="none meta">No agent has a rule.</li>
      {/each}
    </ul>
  {/if}
  {#if job}
    <section class="job" data-testid="agents-debug-job">
      <strong>Job {job.job.id} · {job.job.subject}</strong>
      <p class="meta">{job.summary} · trigger {job.job.trigger} · {job.job.attempts} attempt(s)</p>
      {#each job.actions as a (a.id)}
        <p class="meta">{actionTitle(a)} · {a.state}{a.error ? `: ${a.error}` : ''}</p>
      {/each}
      {#if job.job.result}<pre class="mono">{job.job.result}</pre>{/if}
    </section>
  {/if}
</details>

<style>
  .debug { margin-top: var(--space-3); }
  summary { cursor: pointer; font-size: var(--text-sm); font-weight: 500; }
  .bar { display: flex; align-items: center; gap: var(--space-2); margin: var(--space-2) 0; }
  .grow { flex: 1 1 auto; }
  .meta { font-size: var(--text-xs); color: var(--fg-muted); margin: 0; }
  .err, .bad { color: var(--status-failed); font-size: var(--text-xs); margin: 0; }
  .list { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border); border-radius: var(--radius-md); }
  .list li { display: grid; grid-template-columns: 16px minmax(0, 1fr); gap: 10px; align-items: start; padding: 8px 12px; font-size: var(--text-sm); }
  .list li + li { border-top: 1px solid var(--border); }
  .list li.none { display: block; }
  .body { display: flex; flex-direction: column; gap: 2px; min-width: 0; overflow-wrap: anywhere; }
  .link { border: 0; padding: 0; background: none; color: var(--accent); font: inherit; font-size: var(--text-xs); text-align: left; cursor: pointer; }
  .job { margin-top: var(--space-2); padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); }
  .mono { font-family: var(--font-mono); font-size: var(--text-xs); white-space: pre-wrap; overflow-wrap: anywhere; margin: 6px 0 0; }
</style>
