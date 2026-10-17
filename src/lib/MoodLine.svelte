<script lang="ts">
  // The fleet's mood as a 2px line along the top of the window (dynamic
  // theme, the HUD look's opt-in glow line): the accent while all is calm,
  // the working blue while sessions run, amber when one needs you, red when
  // the hub is lost. The same state the tray icon wears (tray_state.ts).
  // Off unless Settings › Appearance turns it on; decoration only, so it is
  // hidden from assistive tech (the status bar and Inbox carry the facts).
  import { moodLine } from './theme';
  import { trayState } from './tray_state';
</script>

{#if $moodLine}
  <div class="mood-line" data-mood={$trayState} data-testid="mood-line" aria-hidden="true"></div>
{/if}

<style>
  .mood-line {
    position: fixed;
    inset: 0 0 auto 0;
    height: 2px;
    z-index: 1000;
    pointer-events: none;
    color: var(--accent);
    background: currentColor;
    box-shadow: 0 0 12px 1px currentColor;
    opacity: 0.9;
    transition: color var(--dur-slow) ease;
  }
  .mood-line[data-mood='working'] {
    color: var(--status-working);
  }
  .mood-line[data-mood='needs_you'] {
    color: var(--status-waiting);
    animation: mood-breathe var(--loop-slow) ease-in-out infinite alternate;
  }
  .mood-line[data-mood='lost'] {
    color: var(--status-failed);
  }
  @keyframes mood-breathe {
    from {
      opacity: 0.55;
    }
    to {
      opacity: 1;
    }
  }
  :global(:root[data-motion='reduced']) .mood-line,
  :global(:root[data-motion='off']) .mood-line {
    animation: none;
  }
</style>
