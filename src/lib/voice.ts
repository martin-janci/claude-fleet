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
  /** `stopped`: a capture ended; the claim may or may not still be held. */
  state: 'claimed' | 'capturing' | 'stopped' | 'released' | 'error';
  error?: string;
}

const initial = (): VoiceState => ({ sessionId: null, state: 'off', error: null, tipShown: false });

export const voiceState = writable<VoiceState>(initial());

export function resetVoiceForTest(): void {
  voiceState.set(initial());
}

/** One rule for "can this host carry a microphone": the toggle's disabled
 *  state and the follow-the-session effect both read it. */
export const voiceSupported = (transport: 'ssh' | 'agent'): boolean => transport === 'ssh';

// Every claim / release takes a ticket. A claim that resolves after a newer
// call must not flip the UI back, and if the newest call was a release it
// gives the microphone back itself.
let ticket = 0;
let lastOp: 'claim' | 'release' = 'release';
const newestIsRelease = (): boolean => lastOp === 'release';

export async function claimVoice(sessionId: number): Promise<void> {
  const mine = ++ticket;
  lastOp = 'claim';
  const r = await invokeCmd<void>('voice_claim', { sessionId });
  if (mine !== ticket) {
    if (r.ok && newestIsRelease()) void invokeCmd<void>('voice_release');
    return;
  }
  voiceState.update((s) =>
    r.ok
      ? { ...s, sessionId, state: 'claimed', error: null }
      : { ...s, sessionId, state: 'error', error: r.error.message },
  );
}

export async function releaseVoice(): Promise<void> {
  const mine = ++ticket;
  lastOp = 'release';
  await invokeCmd<void>('voice_release');
  if (mine !== ticket) return;
  voiceState.update((s) => ({ ...s, state: 'off', error: null }));
}

/** The terminal attached `sessionId` (on a host of `transport`): the claim
 *  follows it. Nothing happens while the mic is off. An agent host cannot
 *  carry one, so it is released; a failed claim is dropped, not retried. */
export function followSession(sessionId: number, transport: 'ssh' | 'agent'): void {
  const s = get(voiceState);
  if (s.state === 'off') return;
  if (!voiceSupported(transport)) {
    void releaseVoice();
  } else if (s.state === 'error') {
    // The backend may still hold the claim (a microphone that failed to open
    // leaves it registered); a release is idempotent, so always send one.
    voiceState.update((v) => ({ ...v, sessionId: null, state: 'off', error: null }));
    void releaseVoice();
  } else if (s.sessionId !== sessionId) {
    void claimVoice(sessionId);
  }
}

/** The attach of `sessionId` failed: a claim held for another session goes. */
export function abandonFollow(sessionId: number): void {
  const s = get(voiceState);
  if (s.state !== 'off' && s.sessionId !== sessionId) void releaseVoice();
}

/** Follow the backend's `voice:state`. A claim taken elsewhere or lapsed
 *  idle (a hub's 4001 / 4002, or the standalone registry's revocation)
 *  arrives as `released` with an error text: the toggle turns off but keeps
 *  the reason in its tooltip. `stopped` (a capture ended) returns to
 *  `claimed` only from `capturing`: it can arrive after a release. */
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
        case 'stopped':
          return s.state === 'capturing' ? { ...s, state: 'claimed', error: null } : s;
        default:
          return { ...s, sessionId: p.session_id, state: 'claimed', error: null };
      }
    });
  });
}
