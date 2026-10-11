// Control's chat goes on with the recommended option (auto_continue.ts).
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import AutoContinueBar from './AutoContinueBar.svelte';
import { countdown, recommendedPick, resetAutoContinue, STALE_MS } from './auto_continue';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import type { Conversation } from './conversation';

const NOW = Date.parse('2026-10-11T12:00:00Z');

const card = (options: object[], extra: object = {}) =>
  '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', kind: 'choices', options, ...extra }) + '\n```';

const conv = (text: string, endedAt = '2026-10-11T11:59:00Z'): Conversation => ({
  turns: [{ prompt: 'What next?', at: endedAt, ended_at: endedAt, items: [{ kind: 'text', text }] }],
  truncated: false,
  context: null,
  events: [],
});

const OPEN_PR = { label: 'Open a PR', prompt: 'Open a draft PR', recommended: true };
const WAIT = { label: 'Wait', prompt: 'Wait for CI' };

describe('recommendedPick', () => {
  it('picks the recommended option of the card the reply ends on', () => {
    const p = recommendedPick(conv(`Two ways on.\n\n${card([WAIT, OPEN_PR])}`), NOW);
    expect(p?.option.prompt).toBe('Open a draft PR');
  });

  it('picks nothing without a recommended option, a card, or a reply', () => {
    expect(recommendedPick(conv(card([WAIT])), NOW)).toBeNull();
    expect(recommendedPick(conv('Just text.'), NOW)).toBeNull();
    expect(recommendedPick(null, NOW)).toBeNull();
  });

  it('never picks a risky option or one under a risky question', () => {
    const push = { label: 'Push it', prompt: 'Push the branch to origin', recommended: true };
    expect(recommendedPick(conv(card([push, WAIT])), NOW)).toBeNull();
    expect(recommendedPick(conv(card([OPEN_PR, WAIT], { question: 'Force push to main?' })), NOW)).toBeNull();
  });

  it('leaves a card from a reply more than a day old alone', () => {
    const old = new Date(NOW - STALE_MS - 1000).toISOString();
    expect(recommendedPick(conv(card([OPEN_PR]), old), NOW)).toBeNull();
  });

  it('reads only the last card of the reply', () => {
    const text = `${card([OPEN_PR])}\n\nOr rather:\n\n${card([WAIT])}`;
    expect(recommendedPick(conv(text), NOW)).toBeNull();
  });
});

describe('countdown', () => {
  it('is m:ss, never negative', () => {
    expect(countdown(9 * 60_000 + 41_000)).toBe('9:41');
    expect(countdown(500)).toBe('0:01');
    expect(countdown(-5)).toBe('0:00');
  });
});

describe('AutoContinueBar', () => {
  const pick = recommendedPick(conv(card([OPEN_PR, WAIT])), NOW)!;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    resetAutoContinue();
    fleetSettings.set({ ...SETTING_DEFAULTS, 'control.auto_continue_minutes': '2' });
  });
  afterEach(() => vi.useRealTimers());

  it('sends the recommended option once the wait runs out', async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, pick, ready: true, typing: false, onSend });
    expect(screen.getByTestId('auto-continue-clock').textContent).toBe('2:00');
    await vi.advanceTimersByTimeAsync(119_000);
    expect(onSend).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(onSend).toHaveBeenCalledExactlyOnceWith('Open a draft PR');
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });

  it('Cancel stops it for that card', async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, pick, ready: true, typing: false, onSend });
    await fireEvent.click(screen.getByTestId('auto-continue-cancel'));
    await vi.advanceTimersByTimeAsync(300_000);
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });

  it('Send now sends at once', async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, pick, ready: true, typing: false, onSend });
    await fireEvent.click(screen.getByTestId('auto-continue-now'));
    expect(onSend).toHaveBeenCalledExactlyOnceWith('Open a draft PR');
  });

  it('typing stops it, and a busy agent holds the clock', async () => {
    const onSend = vi.fn();
    const busy = render(AutoContinueBar, { sessionId: 7, pick, ready: false, typing: false, onSend });
    await vi.advanceTimersByTimeAsync(300_000);
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.queryByTestId('auto-continue')).toBeNull();
    busy.unmount();

    render(AutoContinueBar, { sessionId: 8, pick, ready: true, typing: true, onSend });
    await vi.advanceTimersByTimeAsync(300_000);
    expect(onSend).not.toHaveBeenCalled();
  });

  it('does nothing when the setting is off', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'control.auto_continue': 'false' });
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, pick, ready: true, typing: false, onSend });
    await vi.advanceTimersByTimeAsync(3_600_000);
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });
});
