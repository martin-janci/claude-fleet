<script lang="ts">
  import Icon from './kit/Icon.svelte';
  import type { SessionRow } from './sessions';
  import { voiceState, claimVoice, releaseVoice, voiceSupported } from './voice';
  import { uiLayout } from './prefs';
  import Loader from './Loader.svelte';
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
  <Icon name="mic" size={14} />{#if mine && $voiceState.state === 'capturing'}<span class="live" data-testid="mic-live"></span>{/if}
</button>
{#if $uiLayout === 'new' && mine && $voiceState.state === 'capturing'}
  <!-- Redesign 5.14 (LoadersInFlows, Chat · Voice input): the Sonar
       follows the mic level while it records… -->
  <span class="voice-loader" data-testid="mic-listening" role="status" style:--level={$voiceState.level ?? 0}
    ><span class="sonar"><Loader name="sonar" size={24} /></span>Listening</span
  >
{:else if $uiLayout === 'new' && mine && $voiceState.transcribing}
  <!-- …and turns into a Dot wave while Claude Code transcribes. -->
  <span class="voice-loader" data-testid="mic-transcribing" role="status"
    ><Loader name="dot-wave" size={24} />Transcribing</span
  >
{/if}
{#if on && !$voiceState.tipShown}
  <span class="tip" data-testid="mic-tip">Run <code>/voice</code> in the session, then hold space.</span>
{/if}

<style>
  .mic { background: none; border: 1px solid transparent; border-radius: var(--radius-sm); cursor: pointer; opacity: 0.5; position: relative; }
  .mic.on { opacity: 1; border-color: var(--accent); }
  .mic:disabled { cursor: not-allowed; opacity: 0.25; }
  .live { position: absolute; top: 1px; right: 1px; width: 6px; height: 6px; border-radius: 50%; background: var(--status-failed); }
  .tip { font-size: var(--text-2xs); opacity: 0.8; margin-left: 4px; }
  .voice-loader { display: inline-flex; align-items: center; gap: 4px; margin-left: 4px; font-size: var(--text-2xs); color: var(--fg-muted); }
  .sonar { display: inline-flex; transform: scale(calc(0.75 + var(--level, 0) * 0.5)); transition: transform var(--dur-fast) linear; }
</style>
