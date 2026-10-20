<script lang="ts">
  // Control's chat goes on with the recommended option (auto_continue.ts):
  // the countdown above the box, its Send now and Cancel. The clock and the
  // send are the fleet's (`service::control_continue`), so this bar only
  // shows them: it reads the state when the chat's turn changes and every
  // few seconds, and settles the card when the person acts. Typing answers
  // the card in the person's own words, so it cancels it.
  import { onDestroy, untrack } from 'svelte';
  import { autoContinue, countdown, hubLacksAutoContinue, liveCountdown, type AutoContinueView } from './auto_continue';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';

  let {
    sessionId,
    turnSeq,
    edits,
    onSend,
  }: {
    sessionId: number;
    /** The chat's turn counter: a new turn may bring a new card. */
    turnSeq: number;
    /** Bumped on each edit the person makes in the box. An edit while the
     *  countdown runs cancels it; text already in the box does not (review
     *  w13-8: a restored draft, a ControlPlusMenu insert or a cancelled held
     *  line used to cancel every card on its first read). */
    edits: number;
    onSend: (text: string) => void;
  } = $props();

  /** How often the state is read again while the chat is open. */
  const POLL_MS = 10_000;

  let view = $state<AutoContinueView | null>(null);
  let now = $state(Date.now());
  /** The hub predates `control_auto_continue` (it runs no clock): say so
   *  instead of drawing nothing, and stop asking. */
  let unsupported = $state(false);

  // Answers can land out of order (the poll, a new turn, the post-due read,
  // Cancel, Send now): only the newest request's answer is applied, so an
  // older 'waiting' snapshot never brings back a card already settled. The
  // Result is still handed back: a stale 'take' answer still carries the
  // prompt this window must send.
  let seq = 0;
  let applied = 0;
  async function call(action: 'status' | 'cancel' | 'take', turnSeq?: number) {
    const mine = ++seq;
    const r = await autoContinue(action, turnSeq);
    if (mine < applied) return r;
    applied = mine;
    if (r.ok) view = r.value;
    else if (hubLacksAutoContinue(r.error)) {
      unsupported = true;
      view = null;
    }
    return r;
  }

  async function refresh() {
    if (unsupported) return;
    await call('status');
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
  // A boolean, not `left`: `left` changes every second, and an effect on it
  // would clear and re-arm the timeout below on each tick so it never fires.
  const overdue = $derived(left !== null && left <= 0);

  // Once due the fleet sends it; read again shortly after to clear the bar.
  $effect(() => {
    if (overdue) {
      const t = setTimeout(() => void refresh(), 3000);
      return () => clearTimeout(t);
    }
  });

  let editsSeen = untrack(() => edits);
  $effect(() => {
    const n = edits;
    if (n === editsSeen) return;
    editsSeen = n;
    if (untrack(() => live)) void cancel();
  });

  async function go() {
    if (!live) return;
    const r = await call('take', live.pick.turn_seq);
    if (r.ok) {
      // `pending` is the card only when this window settled it: a send by the
      // fleet or another window already happened.
      if (r.value.pending) onSend(r.value.pending.prompt);
      void refresh();
    }
  }

  async function cancel() {
    if (!live) return;
    await call('cancel', live.pick.turn_seq);
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
{:else if unsupported && settingBool($fleetSettings, SETTING_KEYS.controlAutoContinue)}
  <div class="auto-continue" role="note" data-testid="auto-continue-unsupported">
    <span class="line">Going on with a recommended option needs a newer hub: update the hub to use it.</span>
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
