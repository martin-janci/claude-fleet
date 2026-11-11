<script lang="ts">
  // The actions the built-in agents propose and a person answers (agents
  // design step 5): under the Inbox's Needs you, and in Automation's Agents
  // tab. Each says what it will do ("Post a review on o/r#4: request
  // changes, 2 comments", "Jira: update issue PD-2709 (status)"), which agent
  // asked and on what, shows what it will send on Show, and takes Approve or
  // Reject. One agent's proposals of the same write fold into one card with
  // Approve all / Reject all and the items under Show (2026-11-06: seven
  // identical "Call jira.jira_update_issue" cards). An approved action is
  // carried out by its agent on the next run pass
  // (`service::agents::execute_approved`); its count is part of the Needs
  // you badge (`pendingAgentCount`). What the person's approvals of the last
  // day came to ("Done: … — created task 42", "Failed: … — why") shows under
  // them (`status.recent`, review w08-3).
  import { shortAge, exactTime } from '../session_status';
  import { push, pushError } from '../toasts';
  import {
    actionDetails,
    actionOutcome,
    boldRuns,
    actionTitle,
    answerAgentAction,
    answerAgentActions,
    pendingAgentGroups,
    recentAgentOutcomes,
    type ActionAbout,
    type ActionGroup,
    type AgentAction,
  } from '../agents';

  type Details = ReturnType<typeof actionDetails>;

  // One id per call in flight (review w11-13): a single id let the first
  // call's return re-enable a second item's buttons mid-call.
  let busy = $state<ReadonlySet<number>>(new Set());
  const setBusy = (ids: number[], on: boolean) => {
    const next = new Set(busy);
    for (const id of ids) {
      if (on) next.add(id);
      else next.delete(id);
    }
    busy = next;
  };
  /** Bodies shown, by action id. */
  let shown = $state<number | null>(null);
  /** Groups opened, by key. */
  let open = $state<ReadonlySet<string>>(new Set());
  const toggle = (key: string) => {
    const next = new Set(open);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    open = next;
  };

  async function answer(a: AgentAction, approve: boolean) {
    setBusy([a.id], true);
    const r = await answerAgentAction(a.id, approve);
    setBusy([a.id], false);
    if (!r.ok) pushError(r.error, approve ? 'Approve failed' : 'Reject failed');
    else push({ kind: 'success', message: approve ? 'Approved: it runs within a minute, and what it did shows here' : 'Rejected' });
  }

  async function answerAll(g: ActionGroup, approve: boolean) {
    const ids = g.items.map((i) => i.action.id);
    setBusy(ids, true);
    const { done, error } = await answerAgentActions(ids, approve);
    setBusy(ids, false);
    if (error && !error.ok) pushError(error.error, approve ? 'Approve failed' : 'Reject failed');
    if (done > 0)
      push({
        kind: 'success',
        message: approve ? `Approved ${done}: they run within a minute, and what they did shows here` : `Rejected ${done}`,
      });
  }

  /** "jira-triage · on PD-2709 · proposed 15m": who asked, on what, when. */
  function origin(about: ActionAbout | null, withFrom: boolean): string {
    const parts: string[] = [];
    if (withFrom && about?.from) parts.push(about.from);
    if (about?.subject) parts.push(`on ${about.subject}`);
    return parts.join(' · ');
  }
</script>

{#snippet text(value: string)}{#each boldRuns(value) as r, i (i)}{#if r.bold}<strong>{r.text}</strong>{:else}{r.text}{/if}{/each}{/snippet}

{#snippet buttons(a: AgentAction, details: Details)}
  <span class="buttons">
    <button type="button" class="btn btn--chip" data-testid="agent-action-approve" disabled={busy.has(a.id)} onclick={() => answer(a, true)}
      >Approve</button
    >
    <button type="button" class="btn btn--chip" data-testid="agent-action-reject" disabled={busy.has(a.id)} onclick={() => answer(a, false)}
      >Reject</button
    >
    {#if details}
      <button type="button" class="btn btn--chip" data-testid="agent-action-show" aria-expanded={shown === a.id} onclick={() => (shown = shown === a.id ? null : a.id)}
        >{shown === a.id ? 'Hide details' : 'Details'}</button
      >
    {/if}
  </span>
  {#if details && shown === a.id}
    <div class="body" data-testid="agent-action-body">
      {#if details.tool}<div class="tool">{details.tool}</div>{/if}
      {#if details.fields.length === 1 && !details.fields[0].label}
        <div class="value">{@render text(details.fields[0].value)}</div>
      {:else}
        <dl class="fields">
          {#each details.fields as f, i (i)}
            <dt>{f.label}</dt>
            {#if f.code}<dd><pre class="code">{f.value}</pre></dd>{:else}<dd class="value">{@render text(f.value)}</dd>{/if}
          {/each}
        </dl>
      {/if}
    </div>
  {/if}
{/snippet}

{#if $pendingAgentGroups.length > 0}
  <div class="actions-list" data-testid="agent-actions" role="group" aria-label="Agent actions waiting for you">
    {#each $pendingAgentGroups as g (g.key)}
      {#if g.items.length === 1}
        {@const a = g.items[0].action}
        {@const about = g.items[0].about}
        {@const from = origin(about, true)}
        <div class="item" data-testid="agent-action">
          <span class="dot" aria-label="Waiting for you"></span>
          <div class="text">
            <span class="name">{actionTitle(a, about)} <span class="chip">agent</span></span>
            <span class="line" title={exactTime(a.created_at)}>{from ? `${from} · ` : ''}proposed {shortAge(a.created_at)}</span>
            {@render buttons(a, actionDetails(a))}
          </div>
        </div>
      {:else}
        {@const newest = g.items[0].action}
        {@const isOpen = open.has(g.key)}
        {@const ids = g.items.map((i) => i.action.id)}
        {@const anyBusy = ids.some((id) => busy.has(id))}
        <div class="item" data-testid="agent-action-group">
          <span class="dot" aria-label="Waiting for you"></span>
          <div class="text">
            <span class="name">{g.items.length} proposals: {g.what} <span class="chip">agent</span></span>
            <span class="line" title={exactTime(newest.created_at)}
              >{g.from ? `from ${g.from} · ` : ''}newest {shortAge(newest.created_at)}</span
            >
            <span class="buttons">
              <button type="button" class="btn btn--chip" data-testid="agent-group-approve" disabled={anyBusy} onclick={() => answerAll(g, true)}
                >Approve all {g.items.length}</button
              >
              <button type="button" class="btn btn--chip" data-testid="agent-group-reject" disabled={anyBusy} onclick={() => answerAll(g, false)}
                >Reject all</button
              >
              <button type="button" class="btn btn--chip" data-testid="agent-group-show" aria-expanded={isOpen} onclick={() => toggle(g.key)}
                >{isOpen ? 'Hide' : 'Show each'}</button
              >
            </span>
            {#if isOpen}
              <ul class="group-items" data-testid="agent-group-items">
                {#each g.items as it (it.action.id)}
                  {@const on = origin(it.about, false)}
                  <li class="sub" data-testid="agent-action">
                    <span class="name">{actionTitle(it.action, it.about)}</span>
                    <span class="line" title={exactTime(it.action.created_at)}>{on ? `${on} · ` : ''}proposed {shortAge(it.action.created_at)}</span>
                    {@render buttons(it.action, actionDetails(it.action))}
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        </div>
      {/if}
    {/each}
  </div>
{/if}
{#if $recentAgentOutcomes.length > 0}
  <div class="actions-list" data-testid="agent-outcomes" role="group" aria-label="What your approvals did">
    {#each $recentAgentOutcomes as a (a.id)}
      <div class="item" data-testid="agent-outcome">
        <span class="dot" class:failed={a.state === 'failed'} class:done={a.state !== 'failed'} aria-hidden="true"></span>
        <div class="text">
          <span class="outcome">{actionOutcome(a)}</span>
          {#if a.executed_at}<span class="line" title={exactTime(a.executed_at)}>{shortAge(a.executed_at)}</span>{/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .actions-list { display: flex; flex-direction: column; gap: 2px; padding: 4px 8px; }
  .item { display: flex; gap: 8px; align-items: flex-start; padding: 6px 4px; border-radius: var(--radius-md); }
  .item:hover { background: var(--bg-hover); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--status-waiting); margin-top: 5px; flex: none; }
  .dot.done { background: var(--status-done); }
  .dot.failed { background: var(--status-failed); }
  .outcome { color: var(--fg); overflow-wrap: anywhere; font-size: var(--text-xs); }
  .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .name { color: var(--fg); font-weight: 500; overflow-wrap: anywhere; }
  .chip { font-size: var(--text-2xs); color: var(--fg-muted); border: 1px solid var(--border); border-radius: var(--radius-pill); padding: 0 6px; font-weight: 400; }
  .line { color: var(--fg-muted); font-size: var(--text-xs); overflow-wrap: anywhere; }
  .buttons { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
  /* What it will send, readable before Approve: labelled fields, line breaks
     kept, the body font (2026-11-11: the raw escaped JSON could not be read). */
  .body { container-type: inline-size; max-height: 360px; overflow: auto; font-size: var(--text-sm); line-height: 1.5; background: var(--bg-sunk); border-radius: var(--radius-md); padding: 8px 10px; margin: 4px 0 0; color: var(--fg); }
  .tool { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--fg-muted); margin-bottom: 6px; }
  .fields { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 4px 12px; margin: 0; }
  .fields dt { color: var(--fg-muted); font-size: var(--text-xs); padding-top: 2px; }
  .fields dd { margin: 0; min-width: 0; }
  .value { white-space: pre-wrap; overflow-wrap: anywhere; }
  .code { margin: 0; white-space: pre-wrap; font-size: var(--text-xs); overflow-wrap: anywhere; }
  @container (max-width: 420px) { .fields { grid-template-columns: minmax(0, 1fr); } .fields dd { margin-bottom: 6px; } }
  .group-items { list-style: none; margin: 6px 0 0; padding: 0 0 0 8px; border-left: 2px solid var(--border); display: flex; flex-direction: column; gap: 8px; }
  .sub { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .sub .name { font-weight: 400; }
</style>
