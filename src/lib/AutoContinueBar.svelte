<script lang="ts">
  // Control's chat goes on with the recommended option (auto_continue.ts):
  // the countdown above the box, its Send now and Cancel. The clock and the
  // send are the fleet's (`service::control_continue`), so this bar only
  // shows them: it reads the state when the chat's turn changes and every
  // few seconds, and settles the card when the person acts. Typing answers
  // the card in the person's own words, so it cancels it.
  import { onDestroy } from 'svelte';
  import { autoContinue, countdown, liveCountdown, type AutoContinueView } from './auto_continue';

  let {
    sessionId,
    turnSeq,
    typing,
    onSend,
  }: {
    sessionId: number;
    /** The chat's turn counter: a new turn may bring a new card. */
    turnSeq: number;
    typing: boolean;
    onSend: (text: string) => void;
  } = $props();

  /** How often the state is read again while the chat is open. */
  const POLL_MS = 10_000;

  let view = $state<AutoContinueView | null>(null);
  let now = $state(Date.now());

  async function refresh() {
    const r = await autoContinue('status');
    if (r.ok) view = r.value;
  }

  $effect(() => {
    void turnSeq;
    void refresh();
  });

  const timer = setInterval(() => (now = Date.now()), 1000);
  const poll = setInterval(() => void refresh(), POLL_MS);
  onDestroy(() => {
    clearInterval(timer);
    clearInterval(poll);
  });

  const live = $derived(liveCountdown(view, sessionId));
  const left = $derived(live ? live.dueAt * 1000 - now : null);

  // Once due the fleet sends it; read again shortly after to clear the bar.
  $effect(() => {
    if (left !== null && left <= 0) {
      const t = setTimeout(() => void refresh(), 3000);
      return () => clearTimeout(t);
    }
  });

  $effect(() => {
    if (typing && live) void cancel();
  });

  async function go() {
    if (!live) return;
    const r = await autoContinue('take', live.pick.turn_seq);
    if (r.ok) {
      view = r.value;
      // `pending` is the card only when this window settled it: a send by the
      // fleet or another window already happened.
      if (r.value.pending) onSend(r.value.pending.prompt);
      void refresh();
    }
  }

  async function cancel() {
    if (!live) return;
    const r = await autoContinue('cancel', live.pick.turn_seq);
    if (r.ok) view = r.value;
  }
</script>

{#if left !== null && live}
  <div class="auto-continue" role="status" data-testid="auto-continue">
    <span class="line"
      >Going on with <strong>{live.pick.label}</strong> in <span class="clock" data-testid="auto-continue-clock"
        >{countdown(left)}</span
      ></span
    >
    <button type="button" class="btn btn--chip" data-testid="auto-continue-now" onclick={() => void go()}>Send now</button>
    <button type="button" class="btn btn--quiet" data-testid="auto-continue-cancel" onclick={() => void cancel()}>Cancel</button>
  </div>
{/if}

<style>
  .auto-continue {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-top: 1px solid var(--border);
    font-size: var(--text-sm);
    color: var(--fg-muted);
  }
  .line {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .clock {
    font-variant-numeric: tabular-nums;
    color: var(--fg);
  }
</style>
