import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ReviewDialog from './ReviewDialog.svelte';
import { DEFAULT_REVIEW_PROMPT, type SessionRow } from './sessions';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';
import { expectAccessible } from './a11y_check';

const source: SessionRow = {
  id: 1, tmux_name: 'dev-source', host_alias: 'local',
  project_id: 1, worktree_id: 10, created_at: 1, last_activity_at: 1,
  status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null,
  worktree_key: 'main', lost_at: null,
  claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null,
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
};

beforeEach(() => { (mockedInvoke as ReturnType<typeof vi.fn>).mockReset(); });

describe('ReviewDialog', () => {
  it('prefills the default multipass prompt', async () => {
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    const ta = screen.getByTestId('review-textarea') as HTMLTextAreaElement;
    expect(ta.value).toBe(DEFAULT_REVIEW_PROMPT);
  });

  it('Start review calls spawn_review with source id + prompt', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'spawn_review') return { ...source, id: 2, tmux_name: 'dev-source--review-abc', kind: 'review', reviews_session_id: 1 };
      return null;
    });
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('review-start'));
    for (let i = 0; i < 6; i++) await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'spawn_review');
    expect(call).toBeDefined();
    const payload = call![1] as { args: { source_session_id: number; prompt: string } };
    expect(payload.args.source_session_id).toBe(1);
    expect(payload.args.prompt).toContain('Pass 1');
    expect('agent' in payload.args).toBe(false);
  });

  it('Codex as the reviewer sends agent codex; Claude Code sends none (M15 G7.12)', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'spawn_review') return { ...source, id: 2, tmux_name: 'dev-source--review-abc', kind: 'review', reviews_session_id: 1 };
      if (cmd === 'assets_inventory')
        return [
          { host_alias: 'local', kind: 'skill', harness: 'claude', name: 'pr-review', state: 'ok' },
          { host_alias: 'local', kind: 'skill', harness: 'codex', name: 'codex-review', state: 'ok' },
        ];
      return null;
    });
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    for (let i = 0; i < 4; i++) await tick();
    const skillOpts = () => Array.from((screen.getByTestId('review-skill') as HTMLSelectElement).options, (o) => o.value);
    expect(skillOpts()).toContain('pr-review');
    await fireEvent.change(screen.getByTestId('review-agent'), { target: { value: 'codex' } });
    await tick();
    expect(skillOpts()).toEqual(['', 'codex-review']);
    await fireEvent.click(screen.getByTestId('review-start'));
    for (let i = 0; i < 6; i++) await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'spawn_review');
    expect((call![1] as { args: { agent?: string } }).args.agent).toBe('codex');
  });

  it('Start is disabled when prompt is emptied', async () => {
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    const ta = screen.getByTestId('review-textarea') as HTMLTextAreaElement;
    await fireEvent.input(ta, { target: { value: '   ' } });
    await tick();
    expect((screen.getByTestId('review-start') as HTMLButtonElement).disabled).toBe(true);
  });
});

// ── Multi-user M1 (F2a): the dialog re-asks on confirm ──────────────────────
//
// `SessionDetails`' Review… button composes both halves; this dialog asked the
// hub's alone, so a reason that arrived WHILE it was open — a revoke, a
// narrowed grant — never reached Start review. `spawn_review` is `own` in
// `share.ts::SESSION_TIER`: it starts a session in the owner's worktree with a
// terminal of its own.
describe('ReviewDialog access gate (multi-user M1)', () => {
  const paired: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };
  const theirs = { ...source, visibility: 'private', owner_person_id: 42 } as SessionRow;
  const mine = { ...source, visibility: 'private', owner_person_id: 7 } as SessionRow;
  const start = () => screen.getByTestId('review-start') as HTMLButtonElement;
  const spawns = () =>
    (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'spawn_review');

  beforeEach(() => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue({ ...source, id: 2 });
    hubStatus.set(paired);
    hubConnection.set({ state: 'connected' });
    resetAccessForTests();
  });

  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    resetAccessForTests();
  });

  it('the owner still starts a review on a paired desktop', async () => {
    setMyGrants(7, []);
    render(ReviewDialog, { props: { source: mine, onClose: () => {} } });
    await tick();
    expect(start().disabled).toBe(false);
    expect(screen.queryByTestId('review-blocked')).toBeNull();
    await fireEvent.click(start());
    for (let i = 0; i < 6; i++) await tick();
    expect(spawns()).toHaveLength(1);
  });

  for (const level of ['watch', 'drive'] as const) {
    it(`a ${level} grantee cannot start one, and the dialog says so in text`, async () => {
      setMyGrants(7, [{ session_id: 1, level }]);
      render(ReviewDialog, { props: { source: theirs, onClose: () => {} } });
      await tick();
      expect(start().disabled).toBe(true);
      expect(screen.getByTestId('review-blocked').textContent).toMatch(
        /only the session’s owner/i,
      );
      await fireEvent.click(start());
      for (let i = 0; i < 6; i++) await tick();
      expect(spawns()).toHaveLength(0);
    });
  }

  it('a revoke arriving while the dialog is open reaches Start review', async () => {
    // The point of the fix: the opener was gated, the dialog was not, and a
    // grant can move between the click that opened it and the click that
    // starts. No row event is involved — only the grant map, which is why the
    // dialog has to read it through the STORE.
    setMyGrants(7, [{ session_id: 1, level: 'drive' }]);
    render(ReviewDialog, { props: { source: theirs, onClose: () => {} } });
    await tick();
    expect(start().disabled).toBe(true);
    // And the sentence changes with the state rather than going stale.
    expect(screen.getByTestId('review-blocked').textContent).toMatch(/only the session’s owner/i);
    applyGrantChanges([{ session_id: 1, person_id: 7, level: null }]);
    await tick();
    expect(start().disabled).toBe(true);
    expect(screen.getByTestId('review-blocked').textContent).toMatch(/belongs to someone else/i);
    await fireEvent.click(start());
    for (let i = 0; i < 6; i++) await tick();
    expect(spawns()).toHaveLength(0);
  });

  it('standalone is untouched: no grants, Start review stays live', async () => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    render(ReviewDialog, { props: { source: theirs, onClose: () => {} } });
    await tick();
    expect(start().disabled).toBe(false);
  });
});

describe('ReviewDialog accessibility (7.2)', () => {
  it('passes the axe and audit checks', async () => {
    const { container } = render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    await expectAccessible(container);
  });
});
