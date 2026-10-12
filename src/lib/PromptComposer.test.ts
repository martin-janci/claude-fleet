import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PromptComposer from './PromptComposer.svelte';
import { sessions, type SessionRow } from './sessions';
import { hosts } from './hosts';
import { accounts } from './accounts';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';

const source: SessionRow = {
  id: 1,
  tmux_name: 'dev-source',
  host_alias: 'local',
  project_id: 1,
  worktree_id: 10,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
  kind: 'work',
  reviews_session_id: null,
  worktree_key: 'main',
  lost_at: null,
  claude_session_id: null,
  claude_status: null,
  effort_level: null,
  pr_url: null,
  current_activity: null,
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
};

const sibling: SessionRow = {
  id: 2,
  tmux_name: 'dev-sibling',
  host_alias: 'mefistos',
  project_id: 1,
  worktree_id: 10,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
  kind: 'work',
  reviews_session_id: null,
  worktree_key: 'main',
  lost_at: null,
  claude_session_id: null,
  claude_status: null,
  effort_level: null,
  pr_url: null,
  current_activity: null,
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
};

const unrelated: SessionRow = {
  id: 3,
  tmux_name: 'dev-other',
  host_alias: 'local',
  project_id: 99,
  worktree_id: 100,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
  kind: 'work',
  reviews_session_id: null,
  worktree_key: 'main',
  lost_at: null,
  claude_session_id: null,
  claude_status: null,
  effort_level: null,
  pr_url: null,
  current_activity: null,
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([source, sibling, unrelated]);
  hosts.set([]);
  accounts.set([]);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
});

afterEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
});

// send_prompt is a ROUTED_ACTION (hub.ts): a hub client with the live link
// down is blocked the same way NewSessionDialog's Create is (see
// NewSessionDialog.test.ts "Enter is gated the same as the Create button").
const REMOTE_DISCONNECTED: HubStatus = {
  remote: true,
  url: null,
  client_name: 'laptop',
  client_mode: null,
  configured_url: null,
  configured_client_name: 'laptop',
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

async function renderBlocked() {
  hubStatus.set(REMOTE_DISCONNECTED);
  hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 3, reason: 'closed' });
  render(PromptComposer, { props: { source, onClose: () => {} } });
  await tick();
}

describe('PromptComposer', () => {
  it('defaults to showing related targets only', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    // sibling is related; unrelated must not appear by default
    expect(screen.queryByText('dev-sibling')).toBeInTheDocument();
    expect(screen.queryByText('dev-other')).toBeNull();
  });

  it('toggling Show all fleet expands the targets list', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const toggle = screen.getByTestId('show-all-fleet') as HTMLInputElement;
    await fireEvent.click(toggle);
    await tick();
    expect(screen.queryByText('dev-other')).toBeInTheDocument();
  });

  it('Send is disabled until prompt + at least one target are set', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const send = screen.getByTestId('composer-send') as HTMLButtonElement;
    // aria-disabled, not the disabled attribute: the button stays focusable
    // so a keyboard/screen-reader user can still reach the blocking reason.
    expect(send.getAttribute('aria-disabled')).toBe('true'); // prompt is empty
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'hello' } });
    await tick();
    // sibling is auto-checked by default → send is now enabled
    expect((screen.getByTestId('composer-send') as HTMLButtonElement).getAttribute('aria-disabled')).toBe('false');
  });

  it('clicking Send calls send_prompt for each checked target', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'send_prompt') return null;
      return null;
    });
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'echo hi' } });
    await tick();
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    const sendCalls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'send_prompt');
    expect(sendCalls).toHaveLength(1);
    const [, payload] = sendCalls[0] as [string, { args: { host_alias: string; tmux_name: string; prompt: string } }];
    expect(payload.args.host_alias).toBe('mefistos');
    expect(payload.args.tmux_name).toBe('dev-sibling');
    expect(payload.args.prompt).toBe('echo hi');
  });

  it('fires all sends concurrently, not sequentially', async () => {
    const callTimes: number[] = [];
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'send_prompt') {
        callTimes.push(performance.now());
        await new Promise((r) => setTimeout(r, 50));
        return null;
      }
      return null;
    });
    // Pre-populate 3 sibling targets with the same project_id+worktree_id as source.
    const sibs = [2, 3, 4].map((id) => ({
      id,
      tmux_name: `dev-sib-${id}`,
      host_alias: 'mefistos',
      project_id: 1,
      worktree_id: 10,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
      account_uuid: null,
      kind: 'work',
      reviews_session_id: null,
      worktree_key: 'main',
      lost_at: null,
      claude_session_id: null,
      claude_status: null,
      effort_level: null,
      pr_url: null,
      current_activity: null,
      friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    }));
    sessions.set([source, ...sibs]);
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'hello' } });
    await tick();
    await fireEvent.click(screen.getByTestId('composer-send'));
    // Wait a tick or two for the parallel awaits to resolve.
    for (let i = 0; i < 12; i++) await tick();
    expect(callTimes).toHaveLength(3);
    // All three should have fired within a small window of each other (parallel).
    const span = Math.max(...callTimes) - Math.min(...callTimes);
    expect(span).toBeLessThan(20);
  });

  it('states why send is blocked, in text, not only in a tooltip', async () => {
    // Render with a hub status that blocks send_prompt, the way the file's
    // other hub-blocked case does.
    await renderBlocked();
    const send = screen.getByTestId('composer-send');
    expect(send.getAttribute('aria-disabled')).toBe('true');
    const id = send.getAttribute('aria-describedby');
    expect(id).toBeTruthy();
    expect(document.getElementById(id!)?.textContent).toContain('hub');
  });
});

// ── Multi-user M1 (F2a): the fan-out is narrowed PER TARGET ─────────────────
//
// This sheet was one of the task's two blockers. It gated on the hub half alone
// and fanned `send_prompt` out over `$sessions.filter((s) => s.id !== source.id)`
// — every session in the fleet with "Show all fleet" ticked. Its entry button in
// `SessionDetails` is access-gated, so a watcher cannot open it for a shared
// row; the TARGET list inside it was not, so anyone could prompt anyone's
// session from it. The fix is per-target narrowing with `bulkTargets`, not one
// answer for the whole sheet.
describe('PromptComposer target narrowing (multi-user M1)', () => {
  /** A paired desktop with a live link: `hubActionBlocked` answers null, so the
   *  only thing left to gate is who this client is on each target. */
  const paired = () => {
    hubStatus.set({
      ...REMOTE_DISCONNECTED,
      url: 'https://fleet.example.com',
      configured_url: 'https://fleet.example.com',
    });
    hubConnection.set({ state: 'connected' });
  };
  const owned = (row: SessionRow, person: number | null): SessionRow => ({
    ...row,
    visibility: person === null ? 'unclaimed' : 'private',
    owner_person_id: person,
  });
  const sendCalls = () =>
    (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'send_prompt');

  async function open() {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    await fireEvent.input(screen.getByTestId('composer-textarea'), {
      target: { value: 'echo hi' },
    });
    await tick();
  }

  it('the owner keeps every target, checked and sendable', async () => {
    // The positive control: without it, a narrowing that dropped everything
    // would satisfy every assertion below.
    sessions.set([owned(source, 1), owned(sibling, 1), owned(unrelated, 1)]);
    paired();
    setMyGrants(1, []);
    await open();
    const cb = screen.getByTestId('target-checkbox-2') as HTMLInputElement;
    expect(cb.disabled).toBe(false);
    expect(cb.checked).toBe(true);
    expect(screen.queryByTestId('target-not-mine-2')).toBeNull();
    expect(screen.getByTestId('composer-send').getAttribute('aria-disabled')).toBe('false');
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    expect(sendCalls()).toHaveLength(1);
  });

  it('a target shared at watch is not checkable, not pre-checked, and not sent to', async () => {
    sessions.set([owned(source, 1), owned(sibling, 42), owned(unrelated, 42)]);
    paired();
    setMyGrants(1, [{ session_id: 2, level: 'watch' }]);
    await open();
    const cb = screen.getByTestId('target-checkbox-2') as HTMLInputElement;
    expect(cb.disabled).toBe(true);
    expect(cb.checked).toBe(false);
    expect(screen.getByTestId('target-not-mine-2').title).toMatch(/needs drive/i);
    // Nothing left to send to, so Send stays off even with a prompt typed.
    expect(screen.getByTestId('composer-send').getAttribute('aria-disabled')).toBe('true');
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    expect(sendCalls()).toHaveLength(0);
  });

  it('a drive grantee keeps the target: drive IS permission to prompt', async () => {
    // The counter-test to the one above. Putting `send_prompt` out of a
    // driver's reach would make the whole drive level meaningless.
    sessions.set([owned(source, 1), owned(sibling, 42), owned(unrelated, 42)]);
    paired();
    setMyGrants(1, [{ session_id: 2, level: 'drive' }]);
    await open();
    expect((screen.getByTestId('target-checkbox-2') as HTMLInputElement).disabled).toBe(false);
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    expect(sendCalls()).toHaveLength(1);
  });

  it('Show all fleet does not widen past the grants: only the drivable ones are sent to', async () => {
    // The blocker as a behaviour: "Show all fleet" is the toggle that turned
    // this sheet into a fleet-wide prompt gun.
    const mine = { ...unrelated, id: 3, tmux_name: 'dev-mine' };
    const theirs = { ...unrelated, id: 4, tmux_name: 'dev-theirs', host_alias: 'elsewhere' };
    sessions.set([owned(source, 1), owned(mine, 1), owned(theirs, 42)]);
    paired();
    setMyGrants(1, [{ session_id: 4, level: 'watch' }]);
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('show-all-fleet'));
    await tick();
    await fireEvent.input(screen.getByTestId('composer-textarea'), { target: { value: 'ping' } });
    await tick();
    // Both are listed; only the owner's own can be ticked.
    expect((screen.getByTestId('target-checkbox-3') as HTMLInputElement).disabled).toBe(false);
    expect((screen.getByTestId('target-checkbox-4') as HTMLInputElement).disabled).toBe(true);
    await fireEvent.click(screen.getByTestId('target-checkbox-3'));
    await tick();
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    expect(sendCalls()).toHaveLength(1);
    const [, payload] = sendCalls()[0] as [string, { args: { tmux_name: string } }];
    expect(payload.args.tmux_name).toBe('dev-mine');
  });

  it('a narrow arriving while the sheet is open takes the target away, with no row event', async () => {
    sessions.set([owned(source, 1), owned(sibling, 42), owned(unrelated, 42)]);
    paired();
    setMyGrants(1, [{ session_id: 2, level: 'drive' }]);
    await open();
    expect((screen.getByTestId('target-checkbox-2') as HTMLInputElement).disabled).toBe(false);
    applyGrantChanges([{ session_id: 2, person_id: 1, level: 'watch' }]);
    await tick();
    expect((screen.getByTestId('target-checkbox-2') as HTMLInputElement).disabled).toBe(true);
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    expect(sendCalls()).toHaveLength(0);
  });

  it('standalone is untouched: no grants, every target sendable', async () => {
    // `access.ts` rule 1 — a single-user install needs no `my_grants` answer.
    sessions.set([source, sibling, unrelated]);
    await open();
    expect((screen.getByTestId('target-checkbox-2') as HTMLInputElement).disabled).toBe(false);
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    expect(sendCalls()).toHaveLength(1);
  });
});
