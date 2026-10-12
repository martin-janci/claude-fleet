// Resume, in the three places that offer it — the Work view's task detail, the
// sidebar's past-work Resume ▾, and the dialog behind it.
//
// `resume_work` is `share.ts`'s `own` tier (F2c): it re-opens a past session's
// Claude conversation, which is a take-over of somebody's transcript, exactly
// what `rewind_conversation` is `own` for. Before F2c these three were gated on
// `hubActionBlocked('start_work', …)` or on nothing at all — the hub half, which
// answers only "is the link up".
//
// Every gate here has the owner's positive control beside it, and the re-ask
// (open → revoke → act) is tested on its own, because that is the case a
// control-only gate passes and a person hits.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';

import WorkTaskDetail from './WorkTaskDetail.svelte';
import ResumeButton from './ResumeButton.svelte';
import ResumeDialog from './ResumeDialog.svelte';
import { sessions } from './sessions';
import { clearSelection } from './selection';
import { session } from './hosts_fixture';
import { link, task } from './work_view_fixture';
import { selectedTaskId, workTreeMeta, type TaskDetail } from './work_view';
import { resetAccessForTests, setMyGrants } from './access';
import { hubStatus, STANDALONE, type HubStatus } from './hub';

/** The ended link's conversation, on both the snapshot and the session row. */
const PAST_CONV = 'conv-old-1';
import { hubConnection } from './hub_connection';
import { UNKNOWN_SESSION_REASON } from './share';
import type { WorkLink } from './work';

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

/** Session 7 is live and mine; session 11 is the PAST one a resume re-opens,
 *  and it belongs to someone else until a test says otherwise. */
const rows = () => [
  session('mefistos', 'api', { id: 7, owner_person_id: 7 }),
  session('mefistos', 'old-1', { id: 11, owner_person_id: 9, claude_session_id: PAST_CONV }),
];

const detail = (): TaskDetail => ({
  task: task({
    sessions: [
      link({ link_id: 42, session_id: 7, primary: true }),
      link({
        link_id: 41,
        session_id: 11,
        name: 'old-1',
        state: 'ended',
        primary: false,
        ended_at: 1789995000,
      }),
    ],
  }),
  aliases: [],
  rules: [],
});

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  clearSelection();
  selectedTaskId.set(null);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  sessions.set(rows());
  workTreeMeta.set({ orgs: [], trackers: [], groups: [] });
  handlers = {
    work_task: () => detail(),
    work_rules: () => [],
    resume_work: () => session('mefistos', 'resumed', { id: 50 }),
  };
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const h = handlers[cmd];
    return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
  });
});

// ── 2. WorkTaskDetail's Continue ──────────────────────────────────────────

/** The task page's ▾ Continue item (WorkButton, redesign 6.6). */
async function continueItem(): Promise<HTMLButtonElement> {
  if (!screen.queryByTestId('work-button-menu-list')) {
    await fireEvent.click(screen.getAllByTestId('work-button-menu')[0]);
    await flush();
  }
  return screen.getAllByTestId('work-button-continue')[0] as HTMLButtonElement;
}

describe('Work view → task → Continue', () => {
  it('a standalone desktop continues its own past work', async () => {
    // The positive control. Rule 1: this process IS the fleet.
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    const btn = await continueItem();
    expect(btn.disabled).toBe(false);
    await fireEvent.click(btn);
    await flush();
    expect(calls('resume_work')).toHaveLength(1);
  });

  it('the owner of the past session keeps Continue on a paired desktop', async () => {
    sessions.set([
      session('mefistos', 'api', { id: 7, owner_person_id: 7 }),
      session('mefistos', 'old-1', { id: 11, owner_person_id: 7, claude_session_id: PAST_CONV }),
    ]);
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, []);
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect((await continueItem()).disabled).toBe(false);
    await fireEvent.click(await continueItem());
    await flush();
    expect(calls('resume_work')).toHaveLength(1);
  });

  it('a drive grantee may NOT continue someone else’s conversation', async () => {
    // `drive` is "make this machine do work", not "take the transcript".
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, [{ session_id: 11, level: 'drive' }]);
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    const btn = await continueItem();
    expect(btn.disabled).toBe(true);
    expect(btn.title).toContain('owner');
  });

  it('a revoke while the page is open refuses Continue', async () => {
    // Open it under a grant that allows the write, narrow the grant while the
    // page is on screen, then act. A `grant:changed` moves no field of this
    // task, so nothing in the page re-reads: the ▾ item must ask for itself.
    sessions.set([
      session('mefistos', 'api', { id: 7, owner_person_id: 7 }),
      session('mefistos', 'old-1', { id: 11, owner_person_id: 7, claude_session_id: PAST_CONV }),
    ]);
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, []);
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect((await continueItem()).disabled).toBe(false);

    // The past session changes hands.
    sessions.set([
      session('mefistos', 'api', { id: 7, owner_person_id: 7 }),
      session('mefistos', 'old-1', { id: 11, owner_person_id: 9, claude_session_id: PAST_CONV }),
    ]);
    await flush();

    // The ▾ item reads the access live: it is refused before the click, and
    // a click sends nothing.
    const item = await continueItem();
    expect(item.disabled).toBe(true);
    expect(item.title).toContain('not shared with you');
    await fireEvent.click(item);
    await flush();
    expect(calls('resume_work')).toHaveLength(0);
  });
});

// ── 3. the sidebar's Resume ▾ ─────────────────────────────────────────────

const pastLink = (over: Partial<WorkLink> = {}): WorkLink =>
  ({
    id: 3,
    state: 'confirmed',
    source: 'manual',
    created_at: 1,
    ended_at: 1789995000,
    snap_host: 'mefistos',
    snap_tmux: 'old-1',
    snap_name: 'old-1',
    // The conversations the session had when the link ended. Since F2d
    // `work.ts::linkSessionId` will not resolve a snapshot on the pane name
    // alone — a tmux name is reused, so `(snap_host, snap_tmux)` can name a
    // DIFFERENT session that inherited it — and corroborates on this instead.
    snap_claude_ids: JSON.stringify([PAST_CONV]),
    ...over,
  }) as WorkLink;

describe('Resume ▾ on a past-work row', () => {
  it('a standalone desktop resumes', async () => {
    render(ResumeButton, { workKey: 'ABC-12', link: pastLink() });
    await tick();
    const btn = screen.getByTestId('resume-quick') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
  });

  it('resolves the past link’s snapshot and refuses someone else’s', async () => {
    // The sidebar's past groups come from the hub's ORG-scoped `recent_ended`
    // read, so a past link there is not necessarily this person's.
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeButton, { workKey: 'ABC-12', link: pastLink() });
    await tick();
    const btn = screen.getByTestId('resume-quick') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.title).toContain('not shared with you');
    await fireEvent.click(btn);
    await flush();
    expect(calls('work_resume_plan')).toHaveLength(0);
  });

  it('…and keeps it for the owner of that past session', async () => {
    sessions.set([session('mefistos', 'old-1', { id: 11, owner_person_id: 7, claude_session_id: PAST_CONV })]);
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeButton, { workKey: 'ABC-12', link: pastLink() });
    await tick();
    expect((screen.getByTestId('resume-quick') as HTMLButtonElement).disabled).toBe(false);
  });

  it('a link whose row this client cannot see fails closed', async () => {
    // Not "no row, nothing to refuse": the hub fences rows off the stream.
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeButton, { workKey: 'ABC-12', link: pastLink({ snap_tmux: 'vanished' }) });
    await tick();
    const btn = screen.getByTestId('resume-quick') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.title).toBe(UNKNOWN_SESSION_REASON);
  });

  it('re-asks at the click: the row outlives the revoke', async () => {
    sessions.set([session('mefistos', 'old-1', { id: 11, owner_person_id: 7, claude_session_id: PAST_CONV })]);
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeButton, { workKey: 'ABC-12', link: pastLink() });
    await tick();
    expect((screen.getByTestId('resume-quick') as HTMLButtonElement).disabled).toBe(false);

    sessions.set([session('mefistos', 'old-1', { id: 11, owner_person_id: 9, claude_session_id: PAST_CONV })]);
    await tick();
    await fireEvent.click(screen.getByTestId('resume-quick'));
    await flush();
    expect(calls('work_resume_plan')).toHaveLength(0);
    expect(calls('resume_work')).toHaveLength(0);
  });
});

// ── 3b. the dialog behind Resume ▾ ────────────────────────────────────────

const RESUME_PLAN = {
  key: 'ABC-12',
  title: 'Login fails',
  live: [],
  candidates: [{ link_id: 5, host_alias: 'mefistos' }],
  link_id: 5,
  host_alias: 'mefistos',
  project_id: 1,
  branch: 'abc-12',
  worktree: 'abc-12',
  worktree_present: true,
  modes: [
    { mode: 'last', ok: true },
    { mode: 'brief', ok: true },
    { mode: 'fresh', ok: true },
  ],
  hosts: ['mefistos'],
};

describe('ResumeDialog', () => {
  beforeEach(() => {
    handlers.work_resume_plan = () => RESUME_PLAN;
  });

  it('the owner starts the resume', async () => {
    sessions.set([session('mefistos', 'old-1', { id: 11, owner_person_id: 7, claude_session_id: PAST_CONV })]);
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeDialog, { props: { workKey: 'ABC-12', sessionId: 11, onclose: () => {} } });
    await flush();
    const start = screen.getByTestId('resume-start') as HTMLButtonElement;
    expect(start.disabled).toBe(false);
    await fireEvent.click(start);
    await flush();
    expect(calls('resume_work')).toHaveLength(1);
  });

  it('every mode is refused on someone else’s past session, fresh included', async () => {
    // `fresh` carries nothing from the transcript, but it still starts in the
    // source's own worktree and branch: all three modes act on that session.
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeDialog, { props: { workKey: 'ABC-12', sessionId: 11, onclose: () => {} } });
    await flush();
    await fireEvent.click(screen.getByTestId('resume-mode-fresh'));
    await flush();
    const start = screen.getByTestId('resume-start') as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    expect(start.title).toContain('not shared with you');
  });

  it('re-asks at Start: the dialog outlives the revoke', async () => {
    sessions.set([session('mefistos', 'old-1', { id: 11, owner_person_id: 7, claude_session_id: PAST_CONV })]);
    hubStatus.set(remote);
    setMyGrants(7, []);
    render(ResumeDialog, { props: { workKey: 'ABC-12', sessionId: 11, onclose: () => {} } });
    await flush();
    expect((screen.getByTestId('resume-start') as HTMLButtonElement).disabled).toBe(false);

    sessions.set([session('mefistos', 'old-1', { id: 11, owner_person_id: 9, claude_session_id: PAST_CONV })]);
    await flush();
    await fireEvent.click(screen.getByTestId('resume-start'));
    await flush();
    expect(calls('resume_work')).toHaveLength(0);
    expect(screen.getByTestId('resume-error').textContent).toContain('not shared with you');
  });
});
