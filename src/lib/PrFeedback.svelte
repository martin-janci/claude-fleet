<script lang="ts">
  // Two buttons under a session's PR result (plan v2 R10): send the failing
  // checks, or the unresolved review comments, to the session's agent as one
  // prompt (`pr_feedback.ts`). The prompt goes through `queue_prompt`, so it
  // waits for the turn to end instead of typing into one.
  //
  // A button sends once per PR reading: after a send it says so and stays
  // off until the next reading, so a double click is one prompt.
  import { queuePrompt, sessionAgent, type SessionRow } from './sessions';
  import { agentLabel } from './conversation';
  import { failingChecksPrompt, reviewCommentsPrompt } from './pr_feedback';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';
  import { errorText } from './error_copy';

  let { session }: { session: SessionRow } = $props();

  const ev = $derived(session.pr_evidence ?? null);
  const prUrl = $derived(session.pr_url ?? null);
  const agent = $derived(sessionAgent(session));
  const name = $derived(agent === 'claude' ? 'Claude' : agentLabel(agent));
  const open = $derived(ev !== null && (ev.state === undefined || ev.state === 'OPEN'));
  const failing = $derived(ev?.checks.failing ?? []);
  const failingTotal = $derived(ev?.checks.failing_total ?? 0);
  /** One reading of the PR: a send is spent until the next one. */
  const reading = $derived(`${session.pr_checked_at ?? ''}|${ev?.head_oid ?? ''}`);

  const blocked = $derived(
    session.fleet
      ? 'This session is on another hub: send it from there'
      : (hubActionBlocked('queue_prompt', $hubStatus, $hubConnection) ?? $sessionIdBlocked(session.id, 'queue_prompt')),
  );

  let busy = $state<'checks' | 'review' | null>(null);
  let sent = $state<Record<string, string>>({});
  let error = $state<string | null>(null);
  const doneChecks = $derived(sent[`checks|${reading}`]);
  const doneReview = $derived(sent[`review|${reading}`]);

  async function send(kind: 'checks' | 'review') {
    if (busy !== null || blocked !== null || prUrl === null || sent[`${kind}|${reading}`]) return;
    busy = kind;
    error = null;
    const prompt = kind === 'checks' ? failingChecksPrompt(prUrl, failing, failingTotal) : reviewCommentsPrompt(prUrl);
    const r = await queuePrompt(session.id, prompt);
    busy = null;
    if (r.ok) {
      const at = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      sent = { ...sent, [`${kind}|${reading}`]: r.value.delivered ? `Sent ${at}` : `Waits for the turn to end · ${at}` };
    } else error = errorText(r.error);
  }
</script>

{#if prUrl && open && agent !== 'shell'}
  <div class="pr-feedback" role="group" aria-label="Send PR feedback to {name}" data-testid="pr-feedback">
    {#if failingTotal > 0}
      <button
        type="button"
        class="btn btn--chip"
        data-testid="pr-feedback-checks"
        disabled={busy !== null || blocked !== null || doneChecks !== undefined}
        title={blocked ?? `One prompt naming the ${failingTotal} failing check${failingTotal === 1 ? '' : 's'}; ${name} reads the logs itself`}
        onclick={() => void send('checks')}>Send failing checks to {name}</button
      >
      {#if doneChecks}<span class="note" role="status" data-testid="pr-feedback-checks-sent">{doneChecks}</span>{/if}
    {/if}
    <button
      type="button"
      class="btn btn--chip"
      data-testid="pr-feedback-review"
      disabled={busy !== null || blocked !== null || doneReview !== undefined}
      title={blocked ?? `${name} reads the unresolved review comments with gh and answers each one`}
      onclick={() => void send('review')}>Send review comments to {name}</button
    >
    {#if doneReview}<span class="note" role="status" data-testid="pr-feedback-review-sent">{doneReview}</span>{/if}
    {#if error}<span class="err" role="status" data-testid="pr-feedback-error">{error}</span>{/if}
  </div>
{/if}

<style>
  .pr-feedback {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    margin-top: 0.35rem;
  }
  .note {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .err {
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
