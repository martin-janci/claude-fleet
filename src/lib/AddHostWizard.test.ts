// Orbit Fleet 4.9: the add-host wizard against a mocked backend whose
// drafts outlive the component, the way the database outlives the app.
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AddHostWizard from './AddHostWizard.svelte';
import type { HostSetup, SetupCheck } from './add_host_wizard';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;
/** The backend's `host_setups` table. */
let db: Map<string, HostSetup>;
let checksRun: string[];
let added: { alias: string; ssh_alias: string }[];

const ANSWERS: Record<string, SetupCheck> = {
  ssh: { key: 'ssh', state: 'ok', label: 'SSH as martin@mercury', detail: '18 ms' },
  tmux: { key: 'tmux', state: 'ok', label: 'tmux 3.4', detail: 'ok' },
  git: { key: 'git', state: 'ok', label: 'git 2.47 · gh signed in', detail: 'ok' },
  agent: { key: 'agent', state: 'na', label: 'fleet-agent not needed', detail: 'this app reaches the host over SSH' },
  disk: { key: 'disk', state: 'ok', label: 'Disk space for worktrees', detail: '412 GB free' },
  agents: { key: 'agents', state: 'ok', label: 'Claude Code, Codex on PATH', detail: 'Claude Code 2.1.3' },
};

beforeEach(() => {
  db = new Map();
  checksRun = [];
  added = [];
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, payload?: { args?: Record<string, unknown> }) => {
    const a = payload?.args ?? {};
    switch (cmd) {
      case 'list_host_setups':
        return [...db.values()];
      case 'discover_hosts':
        return [{ alias: 'mercury', hostname: 'mercury.lan', user: 'martin', port: null }];
      case 'save_host_setup': {
        const prev = db.get(a.ssh_alias as string);
        const row: HostSetup = {
          ssh_alias: a.ssh_alias as string,
          alias: a.alias as string,
          step: a.step as number,
          checks: prev?.checks ?? [],
          answers: (a.answers as HostSetup['answers']) ?? {},
          created_at: prev?.created_at ?? 1,
          updated_at: 2,
        };
        db.set(row.ssh_alias, row);
        return row;
      }
      case 'run_host_setup_check': {
        const key = a.key as string;
        checksRun.push(key);
        const row = db.get(a.ssh_alias as string);
        if (row) row.checks = [...row.checks.filter((c) => c.key !== key), ANSWERS[key]];
        return ANSWERS[key];
      }
      case 'discard_host_setup':
        return db.delete(a.ssh_alias as string);
      case 'probe_ssh_alias':
        return {
          reachable: true,
          claude_version: '2.1.3',
          tmux_version: '3.4',
          account: { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: null, organization_name: null, organization_uuid: null, seat_tier: null },
        };
      case 'add_host':
        added.push(a as { alias: string; ssh_alias: string });
        return { alias: a.alias, ssh_alias: a.ssh_alias, reachable: true };
      default:
        return null;
    }
  });
});

async function toCheckStep() {
  render(AddHostWizard, { props: { onClose: vi.fn() } });
  await fireEvent.click(await screen.findByTestId('wizard-host'));
  await fireEvent.click(screen.getByTestId('wizard-next'));
  await waitFor(() => expect(checksRun).toHaveLength(6));
}

describe('AddHostWizard', () => {
  it('runs every live check in order, each landing on its row', async () => {
    await toCheckStep();
    expect(checksRun).toEqual(['ssh', 'tmux', 'git', 'agent', 'disk', 'agents']);
    await waitFor(() =>
      expect(screen.getAllByTestId('wizard-check').map((r) => r.dataset.state)).toEqual(['ok', 'ok', 'ok', 'na', 'ok', 'ok']),
    );
    expect(screen.getByText('SSH as martin@mercury')).toBeTruthy();
    expect(screen.getByText('Check mercury')).toBeTruthy();
    expect((screen.getByTestId('wizard-next') as HTMLButtonElement).disabled).toBe(false);
  });

  it('stops after a failed SSH check and keeps Next closed', async () => {
    ANSWERS.ssh = { key: 'ssh', state: 'fail', label: 'SSH', detail: 'Connection timed out' };
    try {
      render(AddHostWizard, { props: { onClose: vi.fn() } });
      await fireEvent.click(await screen.findByTestId('wizard-host'));
      await fireEvent.click(screen.getByTestId('wizard-next'));
      await waitFor(() => expect(checksRun).toEqual(['ssh']));
      await waitFor(() => expect((screen.getByTestId('wizard-next') as HTMLButtonElement).disabled).toBe(true));
      expect(screen.getByText('Connection timed out')).toBeTruthy();
    } finally {
      ANSWERS.ssh = { key: 'ssh', state: 'ok', label: 'SSH as martin@mercury', detail: '18 ms' };
    }
  });

  it('resumes after a restart at the step it was left on, with its checks', async () => {
    await toCheckStep();
    // Close the app: the component goes, the database stays.
    cleanup();
    expect(db.get('mercury')?.step).toBe(2);
    checksRun = [];
    render(AddHostWizard, { props: { onClose: vi.fn() } });
    const resume = await screen.findByTestId('wizard-resume');
    expect(resume.textContent).toContain('mercury · step 2 of 5, Check the host');
    await fireEvent.click(resume);
    expect(screen.getByText('Check mercury')).toBeTruthy();
    expect(screen.getAllByTestId('wizard-check').map((r) => r.dataset.state)).toEqual(['ok', 'ok', 'ok', 'na', 'ok', 'ok']);
    expect(checksRun).toEqual([]);
  });

  it('walks to the end, adds the host and drops the draft', async () => {
    const onNewSession = vi.fn();
    const onClose = vi.fn();
    render(AddHostWizard, { props: { onClose, onNewSession } });
    await fireEvent.click(await screen.findByTestId('wizard-host'));
    await fireEvent.click(screen.getByTestId('wizard-next'));
    await waitFor(() => expect((screen.getByTestId('wizard-next') as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(screen.getByTestId('wizard-next')); // → Agents
    expect(screen.getAllByTestId('wizard-agent').filter((a) => a.classList.contains('ok')).map((a) => a.dataset.bin)).toEqual([
      'claude',
      'codex',
    ]);
    await fireEvent.click(screen.getByTestId('wizard-next')); // → Accounts
    expect((await screen.findByTestId('wizard-account')).textContent).toContain('m.janci@32bit.sk');
    await fireEvent.click(screen.getByTestId('wizard-next')); // → Done
    expect(screen.getByTestId('wizard-summary').textContent).toContain('m.janci@32bit.sk');
    await fireEvent.click(screen.getByTestId('wizard-next')); // Add mercury
    await screen.findByTestId('wizard-added');
    expect(added).toEqual([{ alias: 'mercury', ssh_alias: 'mercury' }]);
    expect(db.size).toBe(0);
    await fireEvent.click(screen.getByTestId('wizard-new-session'));
    expect(onNewSession).toHaveBeenCalledWith('mercury');
  });

  it('discards a saved draft', async () => {
    db.set('venus', { ssh_alias: 'venus', alias: 'venus', step: 3, checks: [], answers: {}, created_at: 1, updated_at: 1 });
    render(AddHostWizard, { props: { onClose: vi.fn() } });
    await fireEvent.click(await screen.findByTestId('wizard-discard'));
    await waitFor(() => expect(screen.queryByTestId('wizard-drafts')).toBeNull());
    expect(db.size).toBe(0);
  });
});
