// Redesign step 3.12 (K1): the project a start lands in carries the shared
// "Proposed by …" chip (ProposedBy), on the start popover and the New session
// dialog, each with Change.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import StartPopover from './StartPopover.svelte';
import NewSessionDialog from './NewSessionDialog.svelte';
import { switcherRequest } from './switcher_request';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import type { StartPreview } from './start_preview';

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

const preview: StartPreview = {
  key: 'ABC-12',
  title: 'Login',
  item_id: 12,
  plan: null,
  missing: 'project',
  projects: [
    { id: 3, owner: 'acme', repo: 'api' },
    { id: 4, owner: 'acme', repo: 'web' },
  ],
  hosts: [{ alias: 'mefistos', reachable: true }],
  conflicts: [],
  suggested_project: { project_id: 4, confidence_pct: 88, run_id: 5 },
} as StartPreview;

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  // The popover reads the preview back on every choice, also from a
  // debounce that outlives its test.
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    if (cmd !== 'preview_start_work') return null;
    const a = (raw as { args: { project_id?: number } }).args;
    return a.project_id == null
      ? { ...preview, suggested_project: null }
      : { ...preview, missing: null, suggested_project: null, plan: { key: 'ABC-12', title: 'Login', item_id: 12, project_id: a.project_id, host_alias: 'mefistos', branch: 'abc-12-login', name: 'ABC-12 Login' } };
  });
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  switcherRequest.set(null);
  localStorage.clear();
});

describe('the start popover', () => {
  const props = {
    base: { item_id: 12, with_brief: true },
    preview,
    heading: 'Start ABC-12',
    onclose: () => {},
    onstarted: () => {},
    debounceMs: 0,
  };

  it('New: the shared chip, and Change empties the repository', async () => {
    render(StartPopover, props);
    await flush();
    const chip = screen.getByTestId('start-popover-suggested');
    expect(chip.querySelector('.pill')?.textContent).toBe('Proposed by Jev');
    expect(chip.textContent).toContain('likely');
    expect(screen.getByTestId('start-popover-project')).toHaveClass('ai-pre');
    await fireEvent.click(screen.getByTestId('start-popover-suggested-change'));
    await flush();
    expect((screen.getByTestId('start-popover-project') as HTMLSelectElement).value).toBe('');
    expect(screen.getByTestId('start-popover-project')).not.toHaveClass('ai-pre');
    expect(screen.queryByTestId('start-popover-suggested')).toBeNull();
  });

  it('below the floor nothing is pre-selected and no chip shows', async () => {
    render(StartPopover, { ...props, preview: { ...preview, suggested_project: { project_id: 4, confidence_pct: 30 } } });
    await flush();
    expect((screen.getByTestId('start-popover-project') as HTMLSelectElement).value).toBe('');
    expect(screen.queryByTestId('start-popover-suggested')).toBeNull();
  });
});

describe('review r20-sweep: the start popover', () => {
  it('closed mid-debounce, it reads no preview', async () => {
    const { unmount } = render(StartPopover, {
      base: { item_id: 12, with_brief: true },
      preview: { ...preview, suggested_project: null },
      heading: 'Start ABC-12',
      onclose: () => {},
      onstarted: () => {},
      debounceMs: 30,
    });
    await flush();
    vi.mocked(invoke).mockClear();
    await fireEvent.change(screen.getByTestId('start-popover-project'), { target: { value: '3' } });
    unmount();
    await new Promise((r) => setTimeout(r, 60));
    expect(vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'preview_start_work')).toHaveLength(0);
  });
});

describe('the New session dialog', () => {
  const project = {
    project: { id: 2, owner: 'acme', repo: 'papaya-pos', base_path: '/r/pp', last_session_at: null, adopted: false, system: false },
    worktrees: [],
  };
  const ticket = { id: 101, tracker_id: 1, source: 'jira', key: 'ABC-1', title: 'Login', status_category: 'todo', status_name: 'To Do', created_at: 1, updated_at: 1 };
  const jev = { value: '2', source: 'jev' as const, confidence_pct: 88 };

  it('New: the chip, and Change re-opens the picker for the same ticket', async () => {
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, ticket: ticket as never, proposal: jev, onCreate: () => {}, onCancel } });
    await flush();
    expect(screen.getByTestId('new-session-proposed').textContent).toContain('Proposed by Jev');
    await fireEvent.click(screen.getByTestId('new-session-proposed-change'));
    expect(onCancel).toHaveBeenCalled();
    expect(get(switcherRequest)?.ticket?.key).toBe('ABC-1');
  });

  it('a rule says so', async () => {
    const rule = { value: '2', source: 'rule' as const, reason: 'ABC work runs here' };
    render(NewSessionDialog, { props: { project, proposal: rule, onCreate: () => {}, onCancel: () => {} } });
    await flush();
    const chip = screen.getByTestId('new-session-proposed');
    expect(chip.textContent).toContain('Proposed by a rule');
    expect(chip.textContent).toContain('ABC work runs here');
  });

  it('no proposal: no chip', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await flush();
    expect(screen.queryByTestId('new-session-proposed')).toBeNull();
  });
});
