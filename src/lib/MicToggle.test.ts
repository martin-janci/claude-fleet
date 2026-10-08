import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { uiLayout } from './prefs';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));
import { invoke } from '@tauri-apps/api/core';
import MicToggle from './MicToggle.svelte';
import { voiceState, resetVoiceForTest, applyVoiceEvent, TRANSCRIBE_SHOW_MS } from './voice';

const sess = { id: 5, host_alias: 'alpha', tmux_name: 's' } as any;

beforeEach(() => { vi.mocked(invoke).mockReset(); vi.mocked(invoke).mockResolvedValue(undefined); resetVoiceForTest(); });

describe('MicToggle', () => {
  it('is disabled on an agent host', () => {
    render(MicToggle, { props: { session: sess, transport: 'agent' } });
    expect((screen.getByTestId('mic-toggle') as HTMLButtonElement).disabled).toBe(true);
  });
  it('claims the microphone for the session and shows the tip once', async () => {
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    expect(invoke).toHaveBeenCalledWith('voice_claim', { sessionId: 5 });
    expect(get(voiceState)).toMatchObject({ sessionId: 5, state: 'claimed' });
    expect(screen.getByTestId('mic-tip').textContent).toContain('/voice');
  });
  it('a second click releases', async () => {
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    expect(invoke).toHaveBeenLastCalledWith('voice_release', undefined);
    expect(get(voiceState).state).toBe('off');
  });
  it('shows the error from a refused claim', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'turn on Settings → Limits → Voice first' });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    await fireEvent.click(screen.getByTestId('mic-toggle'));
    expect(screen.getByTestId('mic-toggle').getAttribute('title')).toContain('Settings → Limits → Voice');
  });
  it('shows a red dot while capturing', () => {
    voiceState.set({ sessionId: 5, state: 'capturing', error: null, tipShown: true });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    expect(screen.getByTestId('mic-live')).toBeTruthy();
  });
  it('keeps why the mic went off when another desktop took it', async () => {
    voiceState.set({ sessionId: 5, state: 'off', error: 'microphone claimed elsewhere', tipShown: true });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    expect(screen.getByTestId('mic-toggle').getAttribute('title')).toContain('claimed elsewhere');
  });
});

describe('MicToggle loaders (redesign 5.14, New layout)', () => {
  afterEach(() => { uiLayout.set('classic'); vi.useRealTimers(); });

  it('a Sonar follows the mic level while it records, then a Dot wave while it transcribes', async () => {
    vi.useFakeTimers();
    uiLayout.set('new');
    voiceState.set({ sessionId: 5, state: 'claimed', error: null, tipShown: true });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    applyVoiceEvent({ session_id: 5, state: 'capturing' });
    applyVoiceEvent({ session_id: 5, state: 'level', level: 0.6 });
    await tick();
    const listening = screen.getByTestId('mic-listening');
    expect(listening.textContent).toContain('Listening');
    expect(listening.style.getPropertyValue('--level')).toBe('0.6');
    applyVoiceEvent({ session_id: 5, state: 'stopped' });
    await tick();
    expect(screen.queryByTestId('mic-listening')).toBeNull();
    expect(screen.getByTestId('mic-transcribing').textContent).toContain('Transcribing');
    vi.advanceTimersByTime(TRANSCRIBE_SHOW_MS);
    await tick();
    expect(screen.queryByTestId('mic-transcribing')).toBeNull();
  });

  it('a level never claims the mic or reaches a session it is not recording', () => {
    applyVoiceEvent({ session_id: 5, state: 'level', level: 0.9 });
    expect(get(voiceState)).toMatchObject({ state: 'off', sessionId: null });
    voiceState.set({ sessionId: 5, state: 'claimed', error: null, tipShown: true });
    applyVoiceEvent({ session_id: 5, state: 'level', level: 0.9 });
    expect(get(voiceState).level).toBeUndefined();
    applyVoiceEvent({ session_id: 6, state: 'capturing' });
    expect(get(voiceState).state).toBe('claimed');
  });

  it('Classic shows neither', () => {
    voiceState.set({ sessionId: 5, state: 'capturing', error: null, tipShown: true, level: 0.5 });
    render(MicToggle, { props: { session: sess, transport: 'ssh' } });
    expect(screen.queryByTestId('mic-listening')).toBeNull();
  });
});
