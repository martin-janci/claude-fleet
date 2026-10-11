// Gap plan G7.10 (Jev N2): New session's past-work notice with a "Resume or
// start fresh" proposal. "Has previous work: Resume <name> · Proposed by
// Jev · Start fresh instead": Resume opens the existing resume flow on the
// proposed past session, "Start fresh instead" sets the proposal aside and
// records the person's pick. With no proposal the plain notice stays.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';
import { hosts } from './hosts';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import type { ResumeOrNew } from './resume_or_new';

const project = {
  project: { id: 1, owner: 'acme', repo: 'papaya-pos', base_path: '/r/pp', last_session_at: null, adopted: false, system: false },
  worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/pp', branch: 'main' }],
};

const now = Math.floor(Date.now() / 1000);
const links = [
  { id: 7, ref_key: 'PD-2412', state: 'confirmed', source: 'manual', created_at: 1, ended_at: now - 2 * 86400, snap_name: 'receipt-totals' },
  { id: 5, ref_key: 'PD-2412', state: 'confirmed', source: 'manual', created_at: 1, ended_at: now - 9 * 86400, snap_name: 'receipt-card' },
];

const jevResume: ResumeOrNew = {
  value: 'l5',
  link_id: 5,
  session_id: 55,
  name: 'receipt-card',
  source: 'jev',
  reason: 'its name, branch and ended 9 days ago',
  confidence_pct: 82,
  run_id: 3,
  unsure: false,
};

const plan = {
  key: 'PD-2412',
  live: [],
  candidates: [{ link_id: 5, host_alias: 'local', branch: 'pd-2412-receipt-card' }],
  link_id: 5,
  host_alias: 'local',
  project_id: 1,
  modes: [
    { mode: 'last', ok: true },
    { mode: 'brief', ok: true },
    { mode: 'fresh', ok: true },
  ],
  hosts: ['local'],
};

function answer(proposal: ResumeOrNew | null) {
  vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
    const args = (a as { args?: Record<string, unknown> } | undefined)?.args ?? {};
    if (cmd === 'session_work_links') return args.key === 'PD-2412' ? links : [];
    if (cmd === 'resume_or_new_propose') return proposal;
    if (cmd === 'resume_or_new_follow') return true;
    if (cmd === 'work_resume_plan') return plan;
    if (cmd === 'resume_work') return session('local', 'dev-resumed', { id: 99 });
    return null;
  });
}

function calls(cmd: string): unknown[] {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => c[1]);
}

async function settle() {
  for (let i = 0; i < 6; i++) await tick();
}

async function planPd2412(onCancel = () => {}) {
  render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel } });
  await tick();
  await fireEvent.click(screen.getByTestId('new-worktree-chip'));
  await tick();
  await fireEvent.input(screen.getByTestId('friendly-name'), { target: { value: 'PD-2412 receipts' } });
  await settle();
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  sessions.set([]);
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  localStorage.clear();
});

describe('NewSessionDialog: resume or start fresh (G7.10, Jev N2)', () => {
  it('asks for the planned key and shows the proposal with Proposed by Jev and a word, never a percentage', async () => {
    answer(jevResume);
    await planPd2412();
    expect(calls('resume_or_new_propose')).toEqual([{ args: { key: 'PD-2412' } }]);
    const past = await screen.findByTestId('work-past');
    expect(past).toHaveTextContent('has previous work: Resume receipt-card');
    const why = screen.getByTestId('resume-proposed-by');
    expect(why).toHaveTextContent('Proposed by Jev');
    expect(why).toHaveTextContent('likely');
    expect(why).not.toHaveTextContent('82');
    expect(screen.getByTestId('resume-proposed-by-change')).toHaveTextContent('Start fresh instead');
    expect(screen.queryByTestId('resume-past')).toBeNull();
  });

  it('Resume opens the resume flow on the proposed past session and records the pick', async () => {
    answer(jevResume);
    const onCancel = vi.fn();
    await planPd2412(onCancel);
    await fireEvent.click(await screen.findByTestId('resume-proposed'));
    await settle();
    expect(screen.getByTestId('resume-dialog')).toBeTruthy();
    const planArgs = calls('work_resume_plan')[0] as { args: { key: string; link_id: number } };
    expect(planArgs.args.key).toBe('PD-2412');
    expect(planArgs.args.link_id).toBe(5);
    await fireEvent.click(screen.getByTestId('resume-start'));
    await settle();
    expect(calls('resume_work')).toHaveLength(1);
    expect(calls('resume_or_new_follow')).toEqual([{ args: { key: 'PD-2412', chosen: 'l5' } }]);
    expect(onCancel).toHaveBeenCalled();
  });

  it('Start fresh instead sets the proposal aside, records new, and keeps the plain notice', async () => {
    answer(jevResume);
    await planPd2412();
    await fireEvent.click(await screen.findByTestId('resume-proposed-by-change'));
    await settle();
    expect(calls('resume_or_new_follow')).toEqual([{ args: { key: 'PD-2412', chosen: 'new' } }]);
    expect(screen.queryByTestId('resume-proposed-by')).toBeNull();
    expect(screen.queryByTestId('resume-proposed')).toBeNull();
    expect(screen.getByTestId('work-past')).toHaveTextContent('has previous work (2 sessions');
    expect(screen.getByTestId('resume-past')).toBeTruthy();
  });

  it('a rule proposal says so, and is never sent back as a follow-up', async () => {
    answer({ ...jevResume, value: 'l7', link_id: 7, name: 'receipt-totals', source: 'rule', reason: 'the only past session, ended 2 days ago', confidence_pct: null, run_id: null });
    await planPd2412();
    expect(await screen.findByTestId('work-past')).toHaveTextContent('Resume receipt-totals');
    expect(screen.getByTestId('resume-proposed-by')).toHaveTextContent('Proposed by a rule');
    await fireEvent.click(screen.getByTestId('resume-proposed-by-change'));
    await settle();
    expect(calls('resume_or_new_follow')).toEqual([]);
    expect(screen.getByTestId('resume-past')).toBeTruthy();
  });

  it('with no proposal, unsure or a weak one, the notice stays the plain one', async () => {
    for (const p of [null, { unsure: true, run_id: 4 }, { ...jevResume, confidence_pct: 30 }] as (ResumeOrNew | null)[]) {
      answer(p);
      const { unmount } = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
      await tick();
      await fireEvent.click(screen.getByTestId('new-worktree-chip'));
      await tick();
      await fireEvent.input(screen.getByTestId('friendly-name'), { target: { value: 'PD-2412 receipts' } });
      await settle();
      expect(await screen.findByTestId('work-past')).toHaveTextContent('has previous work (2 sessions');
      expect(screen.queryByTestId('resume-proposed-by')).toBeNull();
      expect(screen.getByTestId('resume-past')).toBeTruthy();
      unmount();
    }
  });

  it('a Jev "start fresh" keeps Resume and offers Resume instead', async () => {
    answer({ ...jevResume, value: 'new', link_id: null, session_id: null, name: null, reason: 'the past sessions’ names' });
    await planPd2412();
    expect(await screen.findByTestId('work-past')).toHaveTextContent('has previous work (2 sessions');
    const why = screen.getByTestId('resume-proposed-by');
    expect(why).toHaveTextContent('Proposed by Jev');
    expect(why).toHaveTextContent('start fresh');
    await fireEvent.click(screen.getByTestId('resume-proposed-by-change'));
    await settle();
    expect(screen.getByTestId('resume-dialog')).toBeTruthy();
  });
});
