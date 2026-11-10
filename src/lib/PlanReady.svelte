<script lang="ts">
  // The "Plan ready" card (docs/plan-first.md): the agent has said its plan
  // is ready and named the file, and that file is among the session's
  // changes. The card names the file, shows it rendered on request, and
  // offers the two answers: approve (one prompt, through `queue_prompt`) or
  // ask for changes (the composer, with the file named). Nothing advances by
  // itself.
  //
  // The session's changes are read once the agent has said so and its turn
  // has ended, never while it works. The card is about what the agent said
  // last: the next prompt, whoever sends it, takes it away.
  import MarkdownView from './MarkdownView.svelte';
  import { agentLabel } from './conversation';
  import { errorText } from './error_copy';
  import { repoChanges, repoFile } from './files';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { planApprovedPrompt, planChangesPrefix, planFile, planSettled, settlePlan, type PlanSaid } from './plan_first';
  import { queuePrompt, sessionAgent, type ClaudeStatus, type SessionRow } from './sessions';
  import { sessionIdBlocked } from './share';

  let {
    session,
    sKey,
    status,
    said,
    onAskChanges,
  }: {
    session: SessionRow;
    /** The panel's key for the session (an extra fleet's ids never collide). */
    sKey: number;
    status: ClaudeStatus | null;
    /** The agent's "Plan ready", when that is what it said last (`planSaid`). */
    said: PlanSaid | null;
    /** Put words in the composer; absent where there is none. */
    onAskChanges?: (prefix: string) => void;
  } = $props();

  const agent = $derived(sessionAgent(session));
  const name = $derived(agent === 'claude' ? 'Claude' : agentLabel(agent));
  const blocked = $derived(
    session.fleet
      ? 'This session is on another hub: answer it from there'
      : (hubActionBlocked('queue_prompt', $hubStatus, $hubConnection) ?? $sessionIdBlocked(session.id, 'queue_prompt')),
  );

  // By value: the panel hands over a new object each time it reads the
  // conversation or the row changes, and neither may read the changes again.
  const reply = $derived(said?.text ?? '');
  const saidAt = $derived(said?.at ?? '');
  const key = $derived(sKey);
  const sessionId = $derived(session.id);
  const elsewhere = $derived(Boolean(session.fleet));
  const working = $derived(status === 'working');

  let path = $state<string | null>(null);
  let settled = $state(0); // bumped after approve/dismiss so `pending` re-reads storage
  let text = $state<string | null>(null);
  let open = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let sentNote = $state<string | null>(null);

  const pending = $derived.by(() => {
    void settled;
    return path !== null && !planSettled(sKey, path, saidAt);
  });

  // Another session, or the conversation moved on: what was shown, opened or
  // sent belonged to the last announcement.
  $effect(() => {
    void key;
    void saidAt;
    path = null;
    text = null;
    open = false;
    sentNote = null;
    error = null;
  });

  $effect(() => {
    const k = key;
    const words = reply;
    // Read with the reset above, so a card it took away is looked for again.
    void saidAt;
    if (elsewhere || words === '' || working) return;
    void look(k, sessionId, words);
  });

  async function look(k: number, id: number, words: string) {
    const r = await repoChanges(id);
    if (k !== sKey || words !== reply) return;
    path = r.ok ? planFile(r.value, words) : null;
  }

  async function toggle() {
    open = !open;
    if (!open || text !== null || path === null) return;
    const p = path;
    const r = await repoFile(session.id, p);
    if (p !== path) return;
    if (r.ok && !r.value.binary) text = r.value.content + (r.value.truncated ? '\n\n… (cut short)' : '');
    else error = r.ok ? 'The plan file cannot be shown here.' : errorText(r.error);
  }

  async function approve() {
    if (busy || blocked !== null || path === null) return;
    busy = true;
    error = null;
    const p = path;
    const at = saidAt;
    const r = await queuePrompt(session.id, planApprovedPrompt(p));
    busy = false;
    if (!r.ok) {
      error = errorText(r.error);
      return;
    }
    settlePlan(sKey, p, at);
    settled += 1;
    sentNote = r.value.delivered ? `Approved: ${name} is starting.` : `Approved: ${name} starts when its turn ends.`;
  }

  function askChanges() {
    if (path === null) return;
    onAskChanges?.(planChangesPrefix(path));
  }

  function dismiss() {
    if (path === null) return;
    settlePlan(sKey, path, saidAt);
    settled += 1;
  }
</script>

{#if pending && path}
  <div class="plan-ready" data-testid="plan-ready" role="group" aria-label="Plan ready">
    <div class="head">
      <strong>Plan ready</strong>
      <span class="path" data-testid="plan-ready-path" title={path}>{path}</span>
      <button type="button" class="x" data-testid="plan-ready-dismiss" aria-label="Hide this card" title="Hide this card" onclick={dismiss}
        >×</button
      >
    </div>
    <p class="why">{name} wrote a plan and is waiting. Nothing starts until you approve it.</p>
    <div class="acts">
      <button type="button" class="btn btn--chip" data-testid="plan-ready-open" aria-expanded={open} onclick={() => void toggle()}
        >{open ? 'Hide plan' : 'Read plan'}</button
      >
      <button
        type="button"
        class="btn btn--chip"
        data-testid="plan-ready-approve"
        disabled={busy || blocked !== null}
        title={blocked ?? `Tells ${name} the plan is approved and to start`}
        onclick={() => void approve()}>{busy ? 'Sending…' : 'Approve and start'}</button
      >
      {#if onAskChanges}
        <button
          type="button"
          class="btn btn--chip"
          data-testid="plan-ready-changes"
          disabled={blocked !== null}
          title={blocked ?? 'Write what to change; the work does not start'}
          onclick={askChanges}>Ask for changes…</button
        >
      {/if}
    </div>
    {#if error}<p class="err" role="alert" data-testid="plan-ready-error">{error}</p>{/if}
    {#if open}
      <div class="plan" data-testid="plan-ready-text">
        {#if text === null && !error}<span class="why">Reading…</span>{:else if text !== null}<MarkdownView source={text} />{/if}
      </div>
    {/if}
  </div>
{:else if sentNote}
  <p class="sent" role="status" data-testid="plan-ready-sent">{sentNote}</p>
{/if}

<style>
  .plan-ready {
    border: 1px solid var(--border);
    border-left: 3px solid var(--status-waiting);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    padding: 8px 10px;
    margin: 6px 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .head { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
  .path { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
  .x { border: 0; background: none; color: var(--fg-muted); cursor: pointer; font-size: var(--text-md); line-height: 1; padding: 0 4px; }
  .why, .sent { color: var(--fg-muted); font-size: var(--text-xs); margin: 0; }
  .sent { margin: 6px 0; }
  .acts { display: flex; flex-wrap: wrap; gap: 6px; }
  .err { color: var(--status-failed); font-size: var(--text-xs); margin: 0; }
  .plan { max-height: 50vh; overflow: auto; border-top: 1px solid var(--border); padding-top: 6px; }
</style>
