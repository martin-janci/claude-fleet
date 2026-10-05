import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  voiceState,
  resetVoiceForTest,
  claimVoice,
  releaseVoice,
  followSession,
  abandonFollow,
  startVoiceEvents,
} from './voice';

const calls = () => vi.mocked(invoke).mock.calls.map((c) => c[0]);
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  resetVoiceForTest();
});

describe('followSession (the terminal follows the attached session)', () => {
  it('does nothing while the mic is off', () => {
    followSession(6, 'ssh');
    expect(invoke).not.toHaveBeenCalled();
  });
  it('re-claims for the newly attached session', async () => {
    await claimVoice(5);
    followSession(6, 'ssh');
    await flush();
    expect(invoke).toHaveBeenLastCalledWith('voice_claim', { sessionId: 6 });
    expect(get(voiceState)).toMatchObject({ sessionId: 6, state: 'claimed' });
  });
  it('releases on an agent-host session instead of claiming', async () => {
    await claimVoice(5);
    followSession(6, 'agent');
    await flush();
    expect(invoke).toHaveBeenLastCalledWith('voice_release', undefined);
    expect(get(voiceState).state).toBe('off');
  });
  it('does not retry after a failed claim; it goes off and gives the backend claim back', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'nope' });
    await claimVoice(5);
    expect(get(voiceState).state).toBe('error');
    followSession(6, 'ssh');
    await flush();
    // An error (a microphone that failed to open) can leave the standalone
    // claim registered; the release is idempotent, so it is always sent.
    expect(calls()).toEqual(['voice_claim', 'voice_release']);
    expect(get(voiceState)).toMatchObject({ state: 'off', sessionId: null, error: null });
  });
  it('a failed attach gives up a claim held for another session', async () => {
    await claimVoice(5);
    abandonFollow(6);
    await flush();
    expect(invoke).toHaveBeenLastCalledWith('voice_release', undefined);
    expect(get(voiceState).state).toBe('off');
  });
});

describe('claim/release ordering', () => {
  it('a claim that resolves after a release does not turn the mic back on, and gives it back', async () => {
    let done!: () => void;
    vi.mocked(invoke).mockImplementationOnce(() => new Promise<undefined>((r) => (done = () => r(undefined))));
    const c = claimVoice(5);
    await releaseVoice();
    done();
    await c;
    await flush();
    expect(get(voiceState).state).toBe('off');
    expect(calls()).toEqual(['voice_claim', 'voice_release', 'voice_release']);
  });
});

describe('voice:state events', () => {
  type Payload = { session_id: number; state: string; error?: string };
  async function events(): Promise<(p: Payload) => void> {
    vi.mocked(listen).mockClear();
    await startVoiceEvents();
    const handler = vi.mocked(listen).mock.calls[0][1] as (e: { payload: Payload }) => void;
    return (payload) => handler({ payload });
  }

  it('a capture that stops returns to claimed', async () => {
    const send = await events();
    await claimVoice(5);
    send({ session_id: 5, state: 'capturing' });
    expect(get(voiceState).state).toBe('capturing');
    send({ session_id: 5, state: 'stopped' });
    expect(get(voiceState).state).toBe('claimed');
  });
  it('a stop that arrives after turning the mic off does not turn it back on', async () => {
    const send = await events();
    await claimVoice(5);
    send({ session_id: 5, state: 'capturing' });
    await releaseVoice();
    send({ session_id: 5, state: 'released' });
    send({ session_id: 5, state: 'stopped' });
    expect(get(voiceState).state).toBe('off');
  });
  it('a stop while merely claimed changes nothing', async () => {
    const send = await events();
    await claimVoice(5);
    send({ session_id: 5, state: 'stopped' });
    expect(get(voiceState)).toMatchObject({ state: 'claimed', sessionId: 5 });
  });
  it('an idle claim that lapsed goes off and says why', async () => {
    const send = await events();
    await claimVoice(5);
    send({ session_id: 5, state: 'released', error: 'microphone idle — turn 🎤 on again' });
    expect(get(voiceState)).toMatchObject({ state: 'off', error: 'microphone idle — turn 🎤 on again' });
  });
  it('a claim taken by another device goes off and says why', async () => {
    const send = await events();
    await claimVoice(5);
    send({ session_id: 5, state: 'released', error: 'microphone claimed elsewhere' });
    expect(get(voiceState)).toMatchObject({ state: 'off', error: 'microphone claimed elsewhere' });
  });
});
