// Voice relay F1: this computer's microphone for `/voice` in one session.
// The backend (src-tauri `voice_claim` / `voice_release`) owns the capture;
// this store only mirrors its `voice:state` events for the 🎤 toggle.
import { writable, get } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invokeCmd } from './result';

export interface VoiceState {
  sessionId: number | null;
  state: 'off' | 'claimed' | 'capturing' | 'error';
  error: string | null;
  /** The `/voice` tip stays until the first real recording. */
  tipShown: boolean;
}

interface VoiceStatePayload {
  session_id: number;
  state: 'claimed' | 'capturing' | 'released' | 'error';
  error?: string;
}

const initial = (): VoiceState => ({ sessionId: null, state: 'off', error: null, tipShown: false });

export const voiceState = writable<VoiceState>(initial());

export function resetVoiceForTest(): void {
  voiceState.set(initial());
}

export async function claimVoice(sessionId: number): Promise<void> {
  const r = await invokeCmd<void>('voice_claim', { sessionId });
  voiceState.update((s) =>
    r.ok
      ? { ...s, sessionId, state: 'claimed', error: null }
      : { ...s, sessionId, state: 'error', error: r.error.message },
  );
}

export async function releaseVoice(): Promise<void> {
  await invokeCmd<void>('voice_release');
  voiceState.update((s) => ({ ...s, state: 'off', error: null }));
}

/** Follow the backend's `voice:state`. A hub 4001 arrives as `released` with
 *  an error text: the toggle turns off but keeps the reason in its tooltip. */
export async function startVoiceEvents(): Promise<UnlistenFn> {
  return listen<VoiceStatePayload>('voice:state', (e) => {
    const p = e.payload;
    voiceState.update((s) => {
      // A stale event for a session we no longer hold must not flip the toggle.
      if (s.sessionId !== null && p.session_id !== s.sessionId) return s;
      switch (p.state) {
        case 'released':
          return { ...s, sessionId: p.session_id, state: 'off', error: p.error ?? null };
        case 'error':
          return { ...s, sessionId: p.session_id, state: 'error', error: p.error ?? 'Microphone error' };
        case 'capturing':
          return { ...s, sessionId: p.session_id, state: 'capturing', error: null, tipShown: true };
        default:
          return { ...s, sessionId: p.session_id, state: 'claimed', error: null };
      }
    });
  });
}

export const voiceIsOn = (): boolean => get(voiceState).state !== 'off';
