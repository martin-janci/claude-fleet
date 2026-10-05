import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
import { invoke } from '@tauri-apps/api/core';
import { voiceState, resetVoiceForTest, claimVoice, releaseVoice, followSession, abandonFollow } from './voice';

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
  it('does not retry after a failed claim; it goes off', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'nope' });
    await claimVoice(5);
    expect(get(voiceState).state).toBe('error');
    followSession(6, 'ssh');
    await flush();
    expect(calls()).toEqual(['voice_claim']);
    expect(get(voiceState)).toMatchObject({ state: 'off', sessionId: null });
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
