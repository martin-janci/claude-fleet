// Control (redesign step 9.1): the New layout's agent column, its tabs and
// the Views panel beside the chat, through the axe and audit checks.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => { throw { code: 'E_TEST', message: 'no backend' }; }) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import ControlView from './ControlView.svelte';
import { controlViews, defaultLayout } from './control_views';
import { get } from 'svelte/store';
import { controlTab } from './control';
import { sessions } from './sessions';
import { operatorState, operatorSession } from './operator';
import { session } from './hosts_fixture';
import { expectAccessible } from './a11y_check';

const asking = session('mac', 'asking', { claude_status: 'blocked', last_prompt: 'Fix the login bug' });

beforeEach(() => {
  controlViews.set(defaultLayout());
  controlTab.set('chat');
  sessions.set([asking, session('mac', 'busy', { claude_status: 'working' })]);
  operatorState.set('ready');
  operatorSession.set(session('local', 'fleet-operator', { id: 7 }));
});
afterEach(() => {
  sessions.set([]);
  operatorSession.set(null);
});

describe('ControlView', () => {
  it('Chat and Today, with the Views panel open, is accessible', async () => {
    const { container } = render(ControlView, { isMac: false });
    expect(screen.getByTestId('control-views')).toBeTruthy();
    await expectAccessible(container);
    await fireEvent.click(screen.getByTestId('control-tab-today'));
    for (let i = 0; i < 5; i++) await tick();
    await expectAccessible(container);
  });

  it('Overview opens the Today briefing beside the chat, even when the panel was closed (UX audit C3)', async () => {
    controlViews.set({ ...defaultLayout(), open: false, hidden: ['today'] });
    render(ControlView, { isMac: false });
    await fireEvent.click(screen.getByTestId('control-overview'));
    const l = get(controlViews);
    expect(l.open).toBe(true);
    expect(l.active).toBe('today');
    expect(l.hidden).not.toContain('today');
    expect(screen.getByTestId('control-overview').getAttribute('aria-pressed')).toBe('true');
  });
});
