import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('./result', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./result')>();
  return { ...actual, invokeCmd: vi.fn() };
});
vi.mock('./sessions', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./sessions')>();
  return { ...actual, mergeSession: vi.fn() };
});
import { invokeCmd, type Result } from './result';
import { mergeSession, type SessionRow } from './sessions';
import {
  moveSession,
  previewMove,
  cancelMoveWait,
  type MoveReport,
  type MovePreview,
  type MoveWaiting,
  type MoveWaitCancelled,
} from './moveSession';

const invoked = invokeCmd as ReturnType<typeof vi.fn>;
const merged = mergeSession as ReturnType<typeof vi.fn>;

/** What `invokeCmd` resolves to on success / on failure — it never rejects,
 *  it always answers with a `Result`. Mirrors `moves.test.ts`'s helpers. */
function ok<T>(value: T): Result<T> {
  return { ok: true, value };
}
function err<T = never>(code: string, message: string, details?: unknown): Result<T> {
  return { ok: false, error: { code, message, details } };
}

const row = (over: Partial<SessionRow>): SessionRow =>
  ({
    id: 5, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: 10,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: '550e8400-e29b-41d4-a716-446655440000', claude_status: null,
    effort_level: null, pr_url: null, current_activity: null, friendly_name: null,
    safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null,
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null,
    stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null,
    parent_session_id: null, tags: [], model: null, context_tokens: null,
    context_window: null, context_source: null, context_at: null, context_stale: false,
    tmux_pane_id: null, ...over,
  }) as SessionRow;

const target = row({ id: 6, host_alias: 'beta', parent_session_id: 5 });

const reportFixture: MoveReport = {
  source_session_id: 5,
  target_session_id: 6,
  from_host: 'mefistos',
  to_host: 'beta',
  tmux_name: 'dev-foo',
  claude_session_id: '550e8400-e29b-41d4-a716-446655440000',
  branch: 'feat',
  target_cwd: '/r/.claude/worktrees/feat',
  transcript_bytes: 10,
  source_killed: true,
  warnings: [],
  carried: {
    commits: 0,
    bundle_bytes: 0,
    dirty_entries: [],
    ignored_carried: [],
    ignored_left_behind: [],
    target_seeded: 'existing',
    session_state: { carried: [], kept_target: [], left_behind: [] },
    memory: { carried: [], kept_target: [], identical: 0, index_lines_added: 0, left_behind: [] },
  },
  target,
};

const previewFixture: MovePreview = {
  session_id: 7,
  from_host: 'mefistos',
  to_host: 'beta',
  branch: 'feat',
  source_cwd: '/r/.claude/worktrees/feat',
  unpushed_commits: 2,
  commits_ahead: 1,
  dirty: [{ status: 'M', path: 'src/lib/moveSession.ts' }],
  ignored_carried: [{ path: '.env.local', bytes: 42 }],
  ignored_left_behind: [{ path: 'huge.bin', bytes: 99, reason: 'over_cap' }],
  transcript_bytes: 1024,
  session_state_files: 3,
  session_state_bytes: 512,
  memory_files: 1,
  memory_bytes: 128,
  target_path: '/home/beta/.claude/worktrees/feat',
  target: { state: 'clean', head: 'abc123' },
  unknowns: ['the bundle size is decided by snapshotting the source worktree'],
};

const waitingFixture: MoveWaiting = {
  session_id: 7,
  to_host: 'beta',
  deadline_unix: 2_000_000_000,
};

const waitCancelledFixture: MoveWaitCancelled = {
  session_id: 7,
  was_waiting: true,
};

beforeEach(() => {
  invoked.mockReset();
  merged.mockReset();
});

describe('moveSession force_cross_org', () => {
  // Work graph M5: a move whose live links would cross the org boundary on
  // the target is refused (E_FORBIDDEN, `cross_org: true`) unless forced.
  // The flag travels only when set, so an older hub sees the same
  // arguments it always did.
  it('sends force_cross_org only when asked', async () => {
    invoked.mockResolvedValue(ok({ kind: 'moved', ...reportFixture }));
    await moveSession(7, 'beta');
    expect(invoked.mock.calls[0][1].args).not.toHaveProperty('force_cross_org');
    await moveSession(7, 'beta', { forceCrossOrg: true });
    expect(invoked.mock.calls[1][1].args).toMatchObject({ force_cross_org: true });
  });

  it('passes the cross-org refusal through untouched', async () => {
    const details = { cross_org: true, work_org_id: 1, session_org_id: 2 };
    invoked.mockResolvedValueOnce(err('E_FORBIDDEN', 'crosses', details));
    const r = await moveSession(7, 'beta');
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toEqual({ code: 'E_FORBIDDEN', message: 'crosses', details });
    expect(merged).not.toHaveBeenCalled();
  });
});

describe('previewMove', () => {
  it('sends dry_run and returns the preview', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    const r = await previewMove(7, 'beta');
    expect(invoked.mock.calls[0][0]).toBe('move_session');
    expect(invoked.mock.calls[0][1].args).toMatchObject({
      session_id: 7,
      target_host_alias: 'beta',
      dry_run: true,
    });
    expect(r.ok && r.value.to_host).toBe('beta');
  });

  // spec §4: a busy source previews with a "will wait" line instead of
  // stopping at the refusal, so the preview must ask the same question the
  // Transfer button will actually send.
  it('sends when: idle, so the preview matches what Transfer will do', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    await previewMove(7, 'beta');
    expect(invoked.mock.calls[0][1].args).toMatchObject({ when: 'idle' });
  });

  it('refuses to treat a real move as a preview', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    const r = await previewMove(7, 'beta');
    expect(r.ok).toBe(false);
  });

  it('passes the refusal through untouched', async () => {
    invoked.mockResolvedValueOnce(err('E_MOVE_MIDOP', 'mid-merge'));
    const r = await previewMove(7, 'beta');
    expect(!r.ok && r.error.code).toBe('E_MOVE_MIDOP');
  });

  it('never merges anything into the sessions store', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    await previewMove(7, 'beta');
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    await previewMove(7, 'beta');
    expect(merged).not.toHaveBeenCalled();
  });
});

describe('moveSession', () => {
  it('never sends dry_run and refuses a preview answer', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    const r = await moveSession(7, 'beta');
    expect(invoked.mock.calls[0][1].args.dry_run).toBe(false);
    expect(r.ok).toBe(false);
  });

  it('still hands back the report of a real move', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    const r = await moveSession(7, 'beta');
    expect(r.ok && r.value.kind === 'moved' && r.value.target_session_id).toBe(
      reportFixture.target_session_id,
    );
  });

  it('merges the target row of a moved outcome, and only that', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    await moveSession(7, 'beta');
    expect(merged).toHaveBeenCalledTimes(1);
    expect(merged).toHaveBeenCalledWith(target);
  });

  // A preview's `target` is a `TargetState`, not a session row: merging it
  // would put garbage into the sessions store.
  it('merges nothing on a preview answer', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    await moveSession(7, 'beta');
    expect(merged).not.toHaveBeenCalled();
  });

  it('merges nothing on an error', async () => {
    invoked.mockResolvedValueOnce(err('E_MOVE_MIDOP', 'mid-merge'));
    await moveSession(7, 'beta');
    expect(merged).not.toHaveBeenCalled();
  });

  // A real transfer defaults to `when: idle` — an idle source moves at once,
  // a busy one yields a pending wait, instead of the old flat refusal.
  it('sends when: idle by default', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    await moveSession(7, 'beta');
    expect(invoked.mock.calls[0][1].args).toMatchObject({ when: 'idle' });
  });

  it('sends when: now when asked for it', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    await moveSession(7, 'beta', { when: 'now' });
    expect(invoked.mock.calls[0][1].args).toMatchObject({ when: 'now' });
  });

  it('accepts a waiting outcome and merges nothing', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'waiting', ...waitingFixture }));
    const r = await moveSession(7, 'beta');
    expect(r.ok).toBe(true);
    expect(r.ok && r.value.kind).toBe('waiting');
    expect(r.ok && (r.value as MoveWaiting).deadline_unix).toBe(waitingFixture.deadline_unix);
    expect(merged).not.toHaveBeenCalled();
  });

  // `moveSession` never asks for `when: cancel` — a `wait_cancelled` answer
  // here would mean the backend and this wrapper disagree about the call it
  // just made, so it is refused like any other wrong-shaped answer.
  it('refuses a wait_cancelled answer', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'wait_cancelled', ...waitCancelledFixture }));
    const r = await moveSession(7, 'beta');
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error.code).toBe('E_PARSE');
  });
});

describe('cancelMoveWait', () => {
  it('sends the session id, the target host, and when: cancel', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'wait_cancelled', ...waitCancelledFixture }));
    await cancelMoveWait(7, 'beta');
    expect(invoked.mock.calls[0][0]).toBe('move_session');
    expect(invoked.mock.calls[0][1].args).toMatchObject({
      session_id: 7,
      target_host_alias: 'beta',
      when: 'cancel',
    });
  });

  it('narrows to wait_cancelled', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'wait_cancelled', ...waitCancelledFixture }));
    const r = await cancelMoveWait(7, 'beta');
    expect(r.ok && r.value).toEqual(waitCancelledFixture);
  });

  it('reports was_waiting: false when there was nothing pending to cancel', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'wait_cancelled', session_id: 7, was_waiting: false }));
    const r = await cancelMoveWait(7, 'beta');
    expect(r.ok && (r.value as MoveWaitCancelled).was_waiting).toBe(false);
  });

  it('refuses a moved answer', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    const r = await cancelMoveWait(7, 'beta');
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error.code).toBe('E_PARSE');
  });

  it('refuses a waiting answer', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'waiting', ...waitingFixture }));
    const r = await cancelMoveWait(7, 'beta');
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error.code).toBe('E_PARSE');
  });

  it('passes a refusal through untouched', async () => {
    invoked.mockResolvedValueOnce(err('E_MOVE_MIDOP', 'mid-merge'));
    const r = await cancelMoveWait(7, 'beta');
    expect(!r.ok && r.error.code).toBe('E_MOVE_MIDOP');
  });
});
