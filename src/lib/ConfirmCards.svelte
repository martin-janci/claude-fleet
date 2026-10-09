<!--
  Confirms as cards in the transcript (Orbit Fleet redesign step 9.2): the
  control-API calls the operator is waiting on — its starts and kills always
  need a person (M9.7) — shown where the conversation is, as the manual's
  QuestionCard, instead of a dialog over the whole window. Mounted inside an
  agent transcript; while one is mounted the dialog
  leaves the operator's requests to it (`confirms.ts`).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import QuestionCard from './kit/QuestionCard.svelte';
  import { answerConfirm, answering, cardConfirms, hostConfirmCards } from './confirms';
  import { shortAge } from './session_status';
  import type { ConfirmRequest } from './mcp';

  let { mac = false }: { mac?: boolean } = $props();

  onMount(() => hostConfirmCards());

  // A ticking clock for "asked 2m ago"; a minute is the finest it says.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  onMount(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });

  /** The operator's call in words, the tool name kept for the exact detail. */
  const WORDS: Record<string, string> = {
    new_session: 'Start a session',
    kill_session: 'Kill a session',
    restart_session: 'Restart a session',
    fork_session: 'Fork a session',
    rewind_session: 'Rewind a session',
    broadcast_prompt: 'Send a prompt to several sessions',
    delete_worktree: 'Delete a worktree',
    set_clipboard: 'Set the clipboard',
  };

  function question(r: ConfirmRequest): string {
    return `${WORDS[r.tool] ?? `Run ${r.tool}`}?`;
  }

  function detail(r: ConfirmRequest): string {
    return r.summary ? `${r.tool} ${r.summary}` : r.tool;
  }
</script>

{#if $cardConfirms.length > 0}
  <div class="confirm-cards" data-testid="confirm-cards">
    {#each $cardConfirms as r (r.nonce)}
      {@const busy = $answering.has(r.nonce)}
      <QuestionCard
        question={question(r)}
        age={r.asked_at > 0 ? `asked ${shortAge(r.asked_at, nowSec)} ago` : undefined}
        detail={detail(r)}
        label="The agent needs your OK"
        testid="confirm-card"
        {mac}
        answers={[
          { label: 'Approve', onselect: () => void answerConfirm(r.nonce, true), disabled: busy, testid: 'confirm-card-approve' },
          { label: 'Deny', onselect: () => void answerConfirm(r.nonce, false), disabled: busy, testid: 'confirm-card-deny' },
        ]}
      >
        <p class="why">The agent's starts and kills always wait for you. Nothing runs until you approve.</p>
      </QuestionCard>
    {/each}
  </div>
{/if}

<style>
  .confirm-cards {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .why {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
</style>
