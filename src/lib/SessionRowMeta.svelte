<script lang="ts">
  // A Compact row's one meta line (redesign step 3.6), as the Sessions board
  // draws it: "Working · Claude Code · mac · <what it is doing>". The state
  // word is the row's attention state (step 0.4); everything else the
  // Comfortable row shows is still there on hover (chips) or in Details.
  import type { AttentionState } from './attention';
  import { sessionAgent, type SessionRow } from './sessions';
  import { AGENT_LABELS, STATE_LABELS } from './row_groups';

  let {
    sess,
    state,
    promptText,
    reason = null,
  }: {
    sess: SessionRow;
    state: AttentionState;
    promptText: string;
    /** A Blocked row's reason line (step 2.4), shown in place of the state word. */
    reason?: string | null;
  } = $props();

  const agent = $derived(sessionAgent(sess));
  const detail = $derived(sess.current_activity || promptText);
</script>

<div class="sess-meta-line" data-testid="sess-meta-line" data-state={state}>
  {#if reason}
    <!-- A reason ("Waiting for you: Allow Bash(…)?", "Paused · weekly limit")
         is the whole line, as the Sessions board draws it (UX audit L1). -->
    <span class="state reason state-{state}" data-testid="meta-state" title={reason}>{reason}</span>
  {:else}
    <span class="state state-{state}" data-testid="meta-state">{STATE_LABELS[state]}</span>
    <span class="sep" aria-hidden="true">·</span>
    <span>{AGENT_LABELS[agent] ?? agent}</span>
    <span class="sep" aria-hidden="true">·</span>
    <span data-testid="meta-host">{sess.host_alias}</span>
    {#if detail}
      <span class="sep" aria-hidden="true">·</span>
      <span class="detail" title={sess.last_prompt ?? undefined}>{detail}</span>
    {/if}
  {/if}
</div>

<style>
  .sess-meta-line {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
    padding-left: 0.85rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
  }
  .sess-meta-line > * { flex-shrink: 0; }
  .sep { opacity: 0.6; }
  .sess-meta-line > .reason { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .detail { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .state-action_required,
  .state-blocked { color: var(--status-waiting); font-weight: 600; }
  .state-failed { color: var(--status-failed); font-weight: 600; }
  .state-working { color: var(--status-working); }
  .state-done { color: var(--status-done); }
</style>
