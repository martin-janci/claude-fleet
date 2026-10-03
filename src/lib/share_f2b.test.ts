// The gates multi-user M1 task F2b added, each with the positive control that
// the owner keeps the action.
//
// Two shapes of proof, because the gates have two shapes:
//
//   - **the funnel** (`moves.ts`, `preflight.ts`, `operator.ts`): the store
//     function refuses, so no surface can get it wrong. Tested directly — no
//     render, and the assertion is simply "the command was not invoked";
//   - **the control** (HostDetail's Restore, the bulk prompt dialog, the answer
//     card, Summarise): disabled, carrying the true reason, with the handler
//     re-asking so a grant narrowed while a panel is open cannot land a write.
//
// `share_sweep.test.ts` is the third proof and the one that generalises: it
// fails for a write with no access answer in reach, which is what makes each
// `if (xBlocked !== null) return;` below load-bearing rather than decorative.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';

import { resetAccessForTests, setMyGrants } from './access';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { sessions, type SessionRow } from './sessions';
import { hosts } from './hosts';
import { ADMIN, NOW, host, session } from './hosts_fixture';
import { sharedWith } from './hosts_view';
import {
  moves,
  startMove,
  retryMove,
  cancelWait,
  resolveMoveRun,
  putRunForTest,
  resetMovesForTest,
  type MoveRun,
} from './moves';
import { requestPreflight, resetPreflightsForTest } from './preflight';
import { operatorSession, restartOperator } from './operator';
import { MOVE_STEPS } from './moveProgress';
import HostDetail from './HostDetail.svelte';
import BulkPromptDialog from './BulkPromptDialog.svelte';
import AnswerPrompt from './AnswerPrompt.svelte';
import SummarizeButton from './SummarizeButton.svelte';
import type { WorkLink } from './work';
import type { AnswerView } from './pending_input';

/** A hub this desktop is a window onto — the only mode in which anything below
 *  is blocked at all: standalone owns every row (`access.ts` rule 1). */
const REMOTE: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

/** This client is person 7; the rows below belong to person 9. */
const ME = 7;
const THEM = 9;

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> } | undefined)?.args);
}

async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async () => null);
  resetAccessForTests();
  hubStatus.set(REMOTE);
  hubConnection.set({ state: 'connected' });
  sessions.set([]);
  hosts.set([]);
});

afterEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  sessions.set([]);
});

// ── the funnel gates ──────────────────────────────────────────────────────

describe('the move lifecycle refuses at the funnel (moves.ts)', () => {
  const theirs = session('alpha', 'dev-theirs', { id: 7, owner_person_id: THEM });
  const mine = session('alpha', 'dev-mine', { id: 7, owner_person_id: ME });

  function failedRun(): MoveRun {
    return {
      sessionId: 7,
      sessionName: 'dev-theirs',
      fromHost: 'alpha',
      toHost: 'beta',
      keepSource: false,
      origin: 'local',
      steps: MOVE_STEPS.map((step) => ({ step, state: 'pending' as const, detail: null })),
      status: 'failed',
      report: null,
      error: null,
      resolveError: null,
      startedAt: Date.now(),
      settledAt: null,
      cleanTarget: false,
      forceCrossOrg: false,
      attempt: 1,
      resolving: false,
      awaitingStart: false,
      deadlineUnix: null,
      waitEnded: null,
      waitRefusal: null,
    };
  }

  beforeEach(() => {
    resetMovesForTest();
    hosts.set([host('alpha'), host('beta')]);
    // The move itself is answered with a REFUSAL: these tests only count
    // whether the command was sent, and a refusal is the one answer every path
    // through `settleMoveResult` handles without reading into a fake report.
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'move_session' || cmd === 'resolve_move') {
        throw { code: 'E_TEST', message: 'not in this test' };
      }
      return null;
    });
  });

  afterEach(() => resetMovesForTest());

  it('startMove sends nothing for a session shared at drive, and everything for the owner', () => {
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 7, level: 'drive' }]);
    startMove(theirs, 'beta', { keepSource: false });
    expect(calls('move_session')).toEqual([]);
    expect(get(moves).has(7)).toBe(false);

    // The positive control: the owner's own session still moves.
    sessions.set([mine]);
    setMyGrants(ME, []);
    startMove(mine, 'beta', { keepSource: false });
    expect(calls('move_session').length).toBe(1);
  });

  it('retryMove and resolveMoveRun refuse a watcher and serve the owner', () => {
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 7, level: 'watch' }]);
    putRunForTest(failedRun());
    retryMove(7);
    resolveMoveRun(7, 'finish');
    expect(calls('move_session')).toEqual([]);
    expect(calls('resolve_move')).toEqual([]);

    sessions.set([mine]);
    putRunForTest(failedRun());
    retryMove(7);
    expect(calls('move_session').length).toBe(1);
  });

  it('cancelWait refuses a watcher and serves the owner', () => {
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 7, level: 'watch' }]);
    putRunForTest({ ...failedRun(), status: 'waiting' });
    cancelWait(7);
    expect(calls('move_session')).toEqual([]);

    sessions.set([mine]);
    putRunForTest({ ...failedRun(), status: 'waiting' });
    cancelWait(7);
    expect(calls('move_session').length).toBe(1);
  });
});

describe('a preview is a write too (preflight.ts)', () => {
  beforeEach(() => {
    resetPreflightsForTest();
    // As above: the preview is answered with a refusal, which `preflight.ts`
    // stores as `refused`. These tests count the call, not the answer.
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'move_session') throw { code: 'E_TEST', message: 'not in this test' };
      return null;
    });
  });
  afterEach(() => resetPreflightsForTest());

  it('requestPreflight asks for nothing on a session shared at drive', async () => {
    sessions.set([session('alpha', 'dev-theirs', { id: 7, owner_person_id: THEM })]);
    setMyGrants(ME, [{ session_id: 7, level: 'drive' }]);
    requestPreflight(7, 'beta');
    await new Promise((r) => setTimeout(r, 400));
    expect(calls('move_session')).toEqual([]);
  });

  it('…and asks for the owner’s own session', async () => {
    sessions.set([session('alpha', 'dev-mine', { id: 7, owner_person_id: ME })]);
    setMyGrants(ME, []);
    requestPreflight(7, 'beta');
    await new Promise((r) => setTimeout(r, 400));
    expect(calls('move_session').length).toBe(1);
  });
});

describe('the agent’s restart is restart_session (operator.ts)', () => {
  afterEach(() => operatorSession.set(null));

  it('restartOperator refuses when the operator session is not this client’s', async () => {
    const theirs = session('local', 'fleet-agent', { id: 3, owner_person_id: THEM });
    sessions.set([theirs]);
    operatorSession.set(theirs);
    setMyGrants(ME, [{ session_id: 3, level: 'drive' }]);
    await restartOperator();
    expect(calls('restart_session')).toEqual([]);
  });

  it('…and restarts the one this client owns', async () => {
    const mine = session('local', 'fleet-agent', { id: 3, owner_person_id: ME });
    sessions.set([mine]);
    operatorSession.set(mine);
    setMyGrants(ME, []);
    // `restartOperator` refreshes the status afterwards; answer it.
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'operator_status' ? { session: mine, host: 'local', ready: true, blocked: null } : null,
    );
    await restartOperator();
    expect(calls('restart_session').length).toBe(1);
  });
});

// ── HostDetail: "Restore n lost sessions…" ────────────────────────────────

describe('HostDetail: restoring a host’s lost sessions is the own tier', () => {
  const lost = (id: number, name: string, owner: number) =>
    session('mefistos', name, {
      id,
      owner_person_id: owner,
      status: 'ghost',
      lost_at: NOW - 100,
      claude_session_id: `conv-${id}`,
    });

  function mount(hostSessions: SessionRow[]) {
    const h = host('mefistos', { account_uuid: ADMIN.uuid });
    render(HostDetail, {
      props: {
        host: h,
        account: ADMIN,
        snapshot: null,
        sharedWith: sharedWith(h, [h]),
        hostSessions,
        token: null,
        tokensLoaded: true,
        hook: { state: 'seen' as const, lastAt: NOW - 300 },
        attention: null,
        now: NOW,
        locale: 'en-GB',
        timeZone: 'UTC',
        editingNickname: false,
        oneditstart: vi.fn(),
        oneditdone: vi.fn(),
        onreprobe: vi.fn(),
        onrefreshusage: vi.fn(),
      },
    });
  }

  it('is disabled, with the reason, when every lost row on the host is someone else’s', async () => {
    const rows = [lost(1, 'dev-a', THEM), lost(2, 'dev-b', THEM)];
    sessions.set(rows);
    setMyGrants(ME, [{ session_id: 1, level: 'drive' }]);
    mount(rows);
    await flush();
    const btn = screen.getByTestId('restore-lost') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.title).toMatch(/only the session’s owner|not yours/i);
    await fireEvent.click(btn);
    await flush();
    expect(calls('restore_host_sessions')).toEqual([]);
  });

  it('the owner keeps it — and a mixed host says how many are left out', async () => {
    const rows = [lost(1, 'dev-mine', ME), lost(2, 'dev-theirs', THEM)];
    sessions.set(rows);
    setMyGrants(ME, [{ session_id: 2, level: 'drive' }]);
    mount(rows);
    await flush();
    const btn = screen.getByTestId('restore-lost') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    expect(btn.textContent).toContain('1 not yours');
    expect(btn.textContent).toContain('Restore 1 lost session');
  });

  it('the confirm sends only the rows this client owns, and the dialog says so', async () => {
    const rows = [lost(1, 'dev-mine', ME), lost(2, 'dev-theirs', THEM)];
    sessions.set(rows);
    setMyGrants(ME, [{ session_id: 2, level: 'drive' }]);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'restore_host_sessions') {
        return {
          plan: [
            { session_id: 1, tmux_name: 'dev-mine', friendly_name: null, cwd: null, action: 'restore', reason: null },
            { session_id: 2, tmux_name: 'dev-theirs', friendly_name: null, cwd: null, action: 'restore', reason: null },
          ],
          results: [{ session_id: 1, tmux_name: 'dev-mine', ok: true, error: null }],
        };
      }
      return null;
    });
    mount(rows);
    await flush();
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await flush();
    // The backend plans for the HOST; the dialog narrows it again.
    expect(screen.getByTestId('restore-not-mine').textContent).toMatch(/1 of these belong to someone else/);
    await fireEvent.click(screen.getByTestId('confirm-restore'));
    await flush();
    const sent = calls('restore_host_sessions').filter((a) => Array.isArray(a?.session_ids));
    expect(sent).toHaveLength(1);
    expect(sent[0]!.session_ids).toEqual([1]);
  });
});

// ── BulkPromptDialog ──────────────────────────────────────────────────────

describe('BulkPromptDialog narrows its own fan-out', () => {
  const run = (id: number, owner: number) =>
    session('mefistos', `dev-${id}`, { id, owner_person_id: owner, status: 'running' });

  it('sends only to the sessions this client may drive, and says how many it left out', async () => {
    const targets = [run(1, ME), run(2, THEM)];
    sessions.set(targets);
    setMyGrants(ME, [{ session_id: 2, level: 'watch' }]);
    render(BulkPromptDialog, { props: { targets, onClose: vi.fn() } });
    await flush();
    expect(screen.getByTestId('bulk-prompt-not-mine').textContent).toMatch(/1 of the selected/);
    expect(screen.getByTestId('bulk-not-mine-2')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('bulk-prompt-textarea'), { target: { value: 'ship it' } });
    await flush();
    await fireEvent.click(screen.getByTestId('bulk-prompt-send'));
    await waitFor(() => expect(calls('send_prompt').length).toBe(1));
    expect(calls('send_prompt')[0]!.tmux_name).toBe('dev-1');
  });

  it('refuses to send at all when no selected session is this client’s', async () => {
    const targets = [run(1, THEM), run(2, THEM)];
    sessions.set(targets);
    setMyGrants(ME, [
      { session_id: 1, level: 'watch' },
      { session_id: 2, level: 'watch' },
    ]);
    render(BulkPromptDialog, { props: { targets, onClose: vi.fn() } });
    await flush();
    await fireEvent.input(screen.getByTestId('bulk-prompt-textarea'), { target: { value: 'ship it' } });
    await flush();
    expect((screen.getByTestId('bulk-prompt-send') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByTestId('bulk-prompt-send'));
    await flush();
    expect(calls('send_prompt')).toEqual([]);
  });

  it('the owner keeps the whole selection', async () => {
    const targets = [run(1, ME), run(2, ME)];
    sessions.set(targets);
    setMyGrants(ME, []);
    render(BulkPromptDialog, { props: { targets, onClose: vi.fn() } });
    await flush();
    expect(screen.queryByTestId('bulk-prompt-not-mine')).toBeNull();
    await fireEvent.input(screen.getByTestId('bulk-prompt-textarea'), { target: { value: 'ship it' } });
    await flush();
    expect((screen.getByTestId('bulk-prompt-send') as HTMLButtonElement).disabled).toBe(false);
  });
});

// ── AnswerPrompt ──────────────────────────────────────────────────────────

describe('the answer card is a pane write', () => {
  const view: AnswerView = {
    live: true,
    kind: 'permission',
    question: 'Allow the edit?',
    options: [
      { n: 1, label: 'Yes', key: '1', selected: false },
      { n: 2, label: 'No', key: '2', selected: false },
    ],
  } as AnswerView;

  it('a watcher gets disabled choices with the reason, and pressing one sends nothing', async () => {
    const theirs = session('mefistos', 'dev-theirs', { id: 7, owner_person_id: THEM, claude_status: 'blocked' });
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 7, level: 'watch' }]);
    render(AnswerPrompt, { props: { session: theirs, view } });
    await flush();
    const options = screen.getAllByTestId('answer-option') as HTMLButtonElement[];
    expect(options.length).toBe(2);
    for (const o of options) {
      expect(o.disabled).toBe(true);
      expect(o.title).toMatch(/watch is read-only/i);
    }
    await fireEvent.click(options[0]);
    await flush();
    expect(calls('send_prompt')).toEqual([]);
    // Not even the pane re-read the card does before it answers.
    expect(calls('session_activity')).toEqual([]);
  });

  it('a driver keeps them (it is a prompt, not a disposal)', async () => {
    const theirs = session('mefistos', 'dev-theirs', { id: 7, owner_person_id: THEM, claude_status: 'blocked' });
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 7, level: 'drive' }]);
    render(AnswerPrompt, { props: { session: theirs, view } });
    await flush();
    for (const o of screen.getAllByTestId('answer-option') as HTMLButtonElement[]) {
      expect(o.disabled).toBe(false);
    }
  });
});

// ── SummarizeButton ───────────────────────────────────────────────────────

describe('Summarise fails closed when the link cannot name its owner', () => {
  const pastLink = (over: Partial<WorkLink> = {}): WorkLink =>
    ({
      id: 4,
      state: 'confirmed',
      source: 'manual',
      is_primary: true,
      created_at: 1,
      ended_at: 2,
      snap_host: 'mefistos',
      snap_tmux: 'dev-gone',
      // The conversations the session had when the link ended. Since F2d the
      // snapshot's pane name alone does not identify a session — a tmux name is
      // reused — so this is what makes the row below the SAME session rather
      // than a namesake (`work.ts::linkSessionId`).
      snap_claude_ids: JSON.stringify(['conv-9']),
      resumable: true,
      ...over,
    }) as WorkLink;

  it('a paired desktop with no row for the link refuses, and says why', async () => {
    sessions.set([]);
    setMyGrants(ME, []);
    render(SummarizeButton, { props: { workKey: 'ABC-1', link: pastLink() } });
    await flush();
    const btn = screen.getByTestId('summarize-button') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    // The ONE sentence, `share.ts::UNKNOWN_SESSION_REASON`: F2d replaced this
    // button's own hand-written copy of it with the shared predicate.
    expect(btn.title).toMatch(/cannot see the session this would act on/i);
    await fireEvent.click(btn);
    await flush();
    expect(calls('summarize_past_work')).toEqual([]);
  });

  it('a standalone desktop owns every row, so nothing is withheld', async () => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    sessions.set([]);
    render(SummarizeButton, { props: { workKey: 'ABC-1', link: pastLink() } });
    await flush();
    expect((screen.getByTestId('summarize-button') as HTMLButtonElement).disabled).toBe(false);
  });

  it('a link whose session IS still listed, and corroborated, is judged on that row', async () => {
    const theirs = session('mefistos', 'dev-gone', {
      id: 9,
      owner_person_id: THEM,
      claude_session_id: 'conv-9',
    });
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 9, level: 'drive' }]);
    render(SummarizeButton, { props: { workKey: 'ABC-1', link: pastLink() } });
    await flush();
    const btn = screen.getByTestId('summarize-button') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.title).toMatch(/only the session’s owner/i);
  });

  it('a NAMESAKE row — same pane name, different session — is not judged on at all', async () => {
    // F2d. The row holds the pane name the link snapshotted, but its
    // conversation is not one the link names, so it is a session that merely
    // INHERITED the name. Judging the link on it would answer `own` for this
    // person's own fresh session and hand over somebody else's transcript.
    const namesake = session('mefistos', 'dev-gone', {
      id: 11,
      owner_person_id: ME,
      claude_session_id: 'conv-brand-new',
    });
    sessions.set([namesake]);
    setMyGrants(ME, []);
    render(SummarizeButton, { props: { workKey: 'ABC-1', link: pastLink() } });
    await flush();
    const btn = screen.getByTestId('summarize-button') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.title).toMatch(/cannot see the session this would act on/i);
    await fireEvent.click(btn);
    await flush();
    expect(calls('summarize_past_work')).toEqual([]);
  });

  it('…and the owner of the corroborated row keeps Summarise', async () => {
    // The positive control: the whole point is that this still works.
    const mine = session('mefistos', 'dev-gone', {
      id: 12,
      owner_person_id: ME,
      claude_session_id: 'conv-9',
    });
    sessions.set([mine]);
    setMyGrants(ME, []);
    render(SummarizeButton, { props: { workKey: 'ABC-1', link: pastLink() } });
    await flush();
    expect((screen.getByTestId('summarize-button') as HTMLButtonElement).disabled).toBe(false);
  });
});
