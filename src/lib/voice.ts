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
  /** How loud the last recorded chunk was, 0–1, while capturing (redesign
   *  5.14: the Sonar follows it). Absent from a backend that predates it. */
  level?: number;
  /** A recording just ended: Claude Code is turning it into text. Set on
   *  `stopped` from `capturing`, cleared by the next event or after
   *  {@link TRANSCRIBE_SHOW_MS}: the session reports no end of its own. */
  transcribing?: boolean;
}

/** How long the Dot wave stays after a recording ends, at most. */
export const TRANSCRIBE_SHOW_MS = 4_000;

export interface VoiceStatePayload {
  session_id: number;
  /** `stopped`: a capture ended; the claim may or may not still be held. */
  state: 'claimed' | 'capturing' | 'stopped' | 'released' | 'error' | 'level';
  error?: string;
  /** `level` only: 0–1. */
  level?: number;
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
  return listen<VoiceStatePayload>('voice:state', (e) => applyVoiceEvent(e.payload));
}

let transcribeTimer: ReturnType<typeof setTimeout> | undefined;

/** One `voice:state` payload applied to the store (exported for tests). */
export function applyVoiceEvent(p: VoiceStatePayload): void {
  if (p.state !== 'level') {
    clearTimeout(transcribeTimer);
    transcribeTimer = undefined;
  }
  voiceState.update((s) => {
    // A stale event for a session we no longer hold must not flip the toggle.
    if (s.sessionId !== null && p.session_id !== s.sessionId) return s;
    const base = { ...s, level: undefined, transcribing: false };
    switch (p.state) {
      case 'level':
        // A level only moves the Sonar; it never claims or flips anything.
        return s.state === 'capturing' ? { ...s, level: Math.min(1, Math.max(0, p.level ?? 0)) } : s;
      case 'released':
        return { ...base, sessionId: p.session_id, state: 'off', error: p.error ?? null };
      case 'error':
        return { ...base, sessionId: p.session_id, state: 'error', error: p.error ?? 'Microphone error' };
      case 'capturing':
        return { ...base, sessionId: p.session_id, state: 'capturing', error: null, tipShown: true, level: 0 };
      case 'stopped':
        if (s.state !== 'capturing') return s;
        transcribeTimer = setTimeout(() => {
          transcribeTimer = undefined;
          voiceState.update((v) => ({ ...v, transcribing: false }));
        }, TRANSCRIBE_SHOW_MS);
        return { ...base, state: 'claimed', error: null, transcribing: true };
      default:
        return { ...base, sessionId: p.session_id, state: 'claimed', error: null };
    }
  });
}
