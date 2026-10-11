<script lang="ts">
  // Control's chat goes on with the recommended option (auto_continue.ts):
  // the countdown above the box, its Send now and Cancel, and the send when
  // the clock runs out. The panel says when the clock may run (`ready`: the
  // agent quiet, the box writable) and whether the person is typing.
  import { onDestroy } from 'svelte';
  import { fleetSettings, settingBool, settingInt, SETTING_KEYS } from './fleet_settings';
  import { clockStart, countdown, isSettled, pauseClock, settle, type RecommendedPick } from './auto_continue';

  let {
    sessionId,
    pick,
    ready,
    typing,
    onSend,
  }: {
    sessionId: number;
    pick: RecommendedPick | null;
    ready: boolean;
    typing: boolean;
    onSend: (text: string) => void;
  } = $props();

  const enabled = $derived(settingBool($fleetSettings, SETTING_KEYS.controlAutoContinue));
  const waitMs = $derived(Math.max(1, settingInt($fleetSettings, SETTING_KEYS.controlAutoContinueMinutes) || 10) * 60_000);

  let now = $state(Date.now());
  // Bumped by settle(), which lives outside Svelte's reach.
  let settledTick = $state(0);
  const live = $derived(enabled && pick !== null && settledTick >= 0 && !isSettled(sessionId, pick.key));

  // Typing answers the card in the person's own words: its clock stops for
  // good. A busy agent or a blocked box only pauses it.
  $effect(() => {
    if (!live || !pick) return;
    if (typing) {
      settle(sessionId, pick.key);
      settledTick++;
    } else if (!ready) {
      pauseClock(sessionId, pick.key);
    }
  });

  const start = $derived(live && ready && !typing && pick ? clockStart(sessionId, pick.key, now) : null);
  const left = $derived(start === null ? null : start + waitMs - now);

  const timer = setInterval(() => (now = Date.now()), 1000);
  onDestroy(() => clearInterval(timer));

  $effect(() => {
    if (left === null || left > 0 || !pick) return;
    go();
  });

  function go() {
    if (!pick) return;
    settle(sessionId, pick.key);
    settledTick++;
    onSend(pick.option.prompt);
  }

  function cancel() {
    if (!pick) return;
    settle(sessionId, pick.key);
    settledTick++;
  }
</script>

{#if left !== null && pick}
  <div class="auto-continue" role="status" data-testid="auto-continue">
    <span class="line"
      >Going on with <strong>{pick.option.label}</strong> in <span class="clock" data-testid="auto-continue-clock"
        >{countdown(left)}</span
      ></span
    >
    <button type="button" class="btn btn--chip" data-testid="auto-continue-now" onclick={go}>Send now</button>
    <button type="button" class="btn btn--quiet" data-testid="auto-continue-cancel" onclick={cancel}>Cancel</button>
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
