<script lang="ts">
  import type { SessionRow } from './sessions';
  import { voiceState, claimVoice, releaseVoice, voiceSupported } from './voice';
  let { session, transport }: { session: SessionRow; transport: 'ssh' | 'agent' } = $props();
  const mine = $derived($voiceState.sessionId === session.id);
  const on = $derived(mine && ($voiceState.state === 'claimed' || $voiceState.state === 'capturing'));
  const title = $derived(
    !voiceSupported(transport)
      ? 'Voice needs an SSH host (agent hosts come later)'
      : mine && $voiceState.error
        ? $voiceState.error
        : on
          ? 'Microphone on for this session — click to turn off'
          : "Use this computer's microphone for /voice in this session",
  );
  async function toggle() {
    if (on) await releaseVoice();
    else await claimVoice(session.id);
  }
</script>

<button class="mic" class:on data-testid="mic-toggle" disabled={!voiceSupported(transport)} {title} onclick={toggle}>
  🎤{#if mine && $voiceState.state === 'capturing'}<span class="live" data-testid="mic-live"></span>{/if}
</button>
{#if on && !$voiceState.tipShown}
  <span class="tip" data-testid="mic-tip">Run <code>/voice</code> in the session, then hold space.</span>
{/if}

<style>
  .mic { background: none; border: 1px solid transparent; border-radius: 4px; cursor: pointer; opacity: 0.5; position: relative; }
  .mic.on { opacity: 1; border-color: var(--accent, #6aa0ff); }
  .mic:disabled { cursor: not-allowed; opacity: 0.25; }
  .live { position: absolute; top: 1px; right: 1px; width: 6px; height: 6px; border-radius: 50%; background: #e5484d; }
  .tip { font-size: 11px; opacity: 0.8; margin-left: 4px; }
</style>
