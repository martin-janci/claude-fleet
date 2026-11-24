<script lang="ts">
  // Control's chat goes on with the recommended option (auto_continue.ts):
  // the countdown above the box, its Send now and Cancel. The clock and the
  // send are the fleet's (`service::control_continue`), so this bar only
  // shows them: it reads the state when the chat's turn changes and every
  // few seconds, and settles the card when the person acts. Typing answers
  // the card in the person's own words, so it cancels it.
  import { onDestroy, untrack } from 'svelte';
  import { autoContinue, countdown, hubLacksAutoContinue, spokenCountdown, liveCountdown, type AutoContinueView } from './auto_continue';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';
  import { windowHidden } from './window_hidden';
  import { hubClockSkew } from './hub_connection';

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
  // `due_at` is the hub's time, so the countdown runs on the hub's clock
  // (review w15-3): this machine's plus the difference its `ready` showed.
  const hubNow = () => Date.now() + $hubClockSkew * 1000;
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

  // Not while the window is hidden: a desktop left minimised with Control's
  // chat open asked every 10 s all night (review w19-4). Once when it shows
  // again, if a read was skipped.
  let missed = false;
  const poll = setInterval(() => {
    if (windowHidden()) missed = true;
    else void refresh();
  }, POLL_MS);
  const shown = () => {
    if (missed && !windowHidden()) {
      missed = false;
      void refresh();
    }
  };
  if (typeof document !== 'undefined') document.addEventListener('visibilitychange', shown);
  onDestroy(() => {
    clearInterval(poll);
    if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', shown);
  });

  const live = $derived(liveCountdown(view, sessionId));
  // The clock ticks only while a countdown shows.
  const ticking = $derived(live !== null);
  $effect(() => {
    if (!ticking) return;
    now = hubNow();
    const timer = setInterval(() => (now = hubNow()), 1000);
    return () => clearInterval(timer);
  });
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
  <div class="auto-continue" data-testid="auto-continue">
    <!-- The live region holds words that change once a minute; the ticking
         clock beside it is hidden from screen readers (review w19-1). -->
    <span class="sr-only" role="status" data-testid="auto-continue-spoken"
      >Going on with {live.pick.label} in {spokenCountdown(left)}.</span
    >
    <span class="line" aria-hidden="true"
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
