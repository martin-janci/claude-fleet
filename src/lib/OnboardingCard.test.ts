import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import OnboardingCard from './OnboardingCard.svelte';
import { hosts, type HostRow } from './hosts';
import { projects, type ProjectTreeRow } from './projects';
import { sessions, type SessionRow } from './sessions';
import { uiLayout } from './prefs';
import { selectedSession } from './selection';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const started = { id: 31, tmux_name: 'dev-acme-papaya-pos-1', friendly_name: null, host_alias: 'mercury', kind: 'work', claude_status: null } as SessionRow;

/** Picks `v` for `name`, as chips or as a dropdown, whichever it drew. */
async function choose(name: string, v: string) {
  const chip = screen.queryByTestId(`form-field-${name}-${v}`);
  if (chip) await fireEvent.click(chip);
  else await fireEvent.change(screen.getByTestId(`form-field-${name}`), { target: { value: v } });
}

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => (cmd === 'new_session' ? started : cmd === 'tunnel_status' ? [] : null));
  hosts.set([{ alias: 'mercury', hidden: false } as HostRow]);
  projects.set([{ project: { id: 4, owner: 'acme', repo: 'papaya-pos' }, worktrees: [] } as unknown as ProjectTreeRow]);
  sessions.set([]);
});
afterEach(() => uiLayout.set('classic'));

// Redesign step 10.12: Get started's first session runs on the New session wizard.
describe('Get started › Create first session', () => {
  it('opens the New session wizard in the New layout and selects what it started', async () => {
    uiLayout.set('new');
    const onnewsession = vi.fn();
    render(OnboardingCard, { props: { onaddhost: vi.fn(), onnewsession } });
    await fireEvent.click(screen.getByText('Create first session'));
    expect(onnewsession).not.toHaveBeenCalled();
    expect(await screen.findByTestId('wizard-new_session')).toBeInTheDocument();
    await choose('project', '4');
    await choose('host', 'mercury');
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(screen.queryByTestId('wizard-new_session')).toBeNull());
    expect(inv).toHaveBeenCalledWith('new_session', expect.objectContaining({ args: expect.objectContaining({ project_id: 4, host_alias: 'mercury' }) }));
    expect(get(selectedSession)?.id).toBe(31);
  });

  it('keeps the project picker in the Classic layout', async () => {
    const onnewsession = vi.fn();
    render(OnboardingCard, { props: { onaddhost: vi.fn(), onnewsession } });
    await fireEvent.click(screen.getByText('Create first session'));
    expect(onnewsession).toHaveBeenCalled();
    expect(screen.queryByTestId('wizard-new_session')).toBeNull();
  });
});
