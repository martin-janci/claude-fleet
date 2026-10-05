import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));
import { invoke } from '@tauri-apps/api/core';
import MicToggle from './MicToggle.svelte';
import { voiceState, resetVoiceForTest } from './voice';

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
