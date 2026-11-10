<script lang="ts">
  // Two buttons under a session's PR result (plan v2 R10): send the failing
  // checks, or the unresolved review comments, to the session's agent as one
  // prompt (`pr_feedback.ts`). The prompt goes through `queue_prompt`, so it
  // waits for the turn to end instead of typing into one.
  //
  // A button sends once per PR head: after a send it says so and stays off
  // until a newer push, so a double click is one prompt. What was sent is
  // kept per session in `prFeedbackSends`, keyed before the await, so a
  // reply that lands after a switch to another session marks the right one.
  import { queuePrompt, sessionAgent, type SessionRow } from './sessions';
  import { agentLabel } from './conversation';
  import { failingChecksPrompt, prFeedbackKey, prFeedbackSends, reviewCommentsPrompt, type PrFeedbackKind } from './pr_feedback';
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

  const blocked = $derived(
    session.fleet
      ? 'This session is on another hub: send it from there'
      : (hubActionBlocked('queue_prompt', $hubStatus, $hubConnection) ?? $sessionIdBlocked(session.id, 'queue_prompt')),
  );

  const keyOf = (kind: PrFeedbackKind) => prFeedbackKey(session.id, kind, ev?.head_oid);
  const checksKey = $derived(keyOf('checks'));
  const reviewKey = $derived(keyOf('review'));
  /** A send in flight for this session. */
  const busy = $derived(
    Object.entries($prFeedbackSends).some(([k, v]) => v === null && k.startsWith(`${session.id}|`)),
  );
  const doneChecks = $derived($prFeedbackSends[checksKey] ?? undefined);
  const doneReview = $derived($prFeedbackSends[reviewKey] ?? undefined);
  /** The last failed send's words, by session. */
  let errors = $state<Record<number, string>>({});
  const error = $derived(errors[session.id] ?? null);

  async function send(kind: PrFeedbackKind) {
    // Everything the reply is filed under is read now, before the await.
    const key = keyOf(kind);
    const sid = session.id;
    if (busy || blocked !== null || prUrl === null || key in $prFeedbackSends) return;
    prFeedbackSends.update((m) => ({ ...m, [key]: null }));
    errors = { ...errors, [sid]: '' };
    const prompt = kind === 'checks' ? failingChecksPrompt(prUrl, failing, failingTotal) : reviewCommentsPrompt(prUrl);
    const r = await queuePrompt(sid, prompt);
    if (r.ok) {
      const at = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      prFeedbackSends.update((m) => ({ ...m, [key]: r.value.delivered ? `Sent ${at}` : `Waits for the turn to end · ${at}` }));
    } else {
      prFeedbackSends.update((m) => {
        const rest = { ...m };
        delete rest[key];
        return rest;
      });
      errors = { ...errors, [sid]: errorText(r.error) };
    }
  }
</script>

{#if prUrl && open && agent !== 'shell'}
  <div class="pr-feedback" role="group" aria-label="Send PR feedback to {name}" data-testid="pr-feedback">
    {#if failingTotal > 0}
      <button
        type="button"
        class="btn btn--chip"
        data-testid="pr-feedback-checks"
        disabled={busy || blocked !== null || doneChecks !== undefined}
        title={blocked ?? `One prompt naming the ${failingTotal} failing check${failingTotal === 1 ? '' : 's'}; ${name} reads the logs itself`}
        onclick={() => void send('checks')}>Send failing checks to {name}</button
      >
      {#if doneChecks}<span class="note" role="status" data-testid="pr-feedback-checks-sent">{doneChecks}</span>{/if}
    {/if}
    <button
      type="button"
      class="btn btn--chip"
      data-testid="pr-feedback-review"
      disabled={busy || blocked !== null || doneReview !== undefined}
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
