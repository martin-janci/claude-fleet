// Gap plan G1.11: one session action registry. Fork, Rewind, Switch account,
// Change model, Copy transcript and Archive are offered by the row menu, both
// Details layouts and the ⌘K palette, and each runs the backend command its
// per-turn or bulk twin already runs.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {},
}));

import { invoke } from '@tauri-apps/api/core';
import SessionDetails from './SessionDetails.svelte';
import { hosts } from './hosts';
import { host, session } from './hosts_fixture';
import { sessions, type SessionRow } from './sessions';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests } from './access';
import { clearSelection } from './selection';
import { clearToasts, toasts } from './toasts';
import { outbox } from './outbox';
import { composerDrafts, transcriptMarkdown, type ConvTurn } from './conversation';
import { rewindChoices } from './reply_actions';
import {
  noOtherLogin,
  sessionActionRequest,
  sessionActionShown,
  sessionMenuItems,
} from './session_actions';
import { paletteCommands, runCommand } from './commands';

const mockedInvoke = invoke as unknown as ReturnType<typeof vi.fn>;

const work = { link_id: 41, item_id: 5, key: 'FL-1', title: 'Timeout shim', source: 'manual', archived_at: null };
const sess = session('mefistos', 'dev-foo', {
  id: 1,
  claude_session_id: 'c-1',
  project_id: 3,
  worktree_id: 4,
  work,
} as Partial<SessionRow>);
const noConversation = session('mefistos', 'fresh', { id: 2, claude_session_id: null });

function turn(prompt: string | null, uuid: string | null, text = 'ok'): ConvTurn {
  return {
    prompt,
    at: '2026-10-09T10:00:00Z',
    ended_at: '2026-10-09T10:01:00Z',
    items: [{ kind: 'text', text }],
    prompt_uuid: uuid,
  };
}
const conv = {
  turns: [turn('first', 'u1', 'Hello.'), turn('second line\nmore', 'u2', 'Done.'), turn('third', 'u3', 'Fine.')],
  truncated: false,
  context: null,
  events: [],
};

/** Answer `cmd` with `value`; anything else resolves `null`. */
function answer(table: Record<string, unknown>) {
  mockedInvoke.mockImplementation(async (cmd: string) => (cmd in table ? table[cmd] : null));
}
function callsOf(cmd: string): Record<string, unknown>[] {
  return mockedInvoke.mock.calls.filter((c) => c[0] === cmd).map((c) => (c[1] as { args: Record<string, unknown> }).args);
}
async function settle() {
  for (let i = 0; i < 4; i++) {
    await tick();
    await Promise.resolve();
  }
}

beforeEach(() => {
  mockedInvoke.mockReset();
  answer({});
  hosts.set([host('mefistos', { claude_profiles: [{ name: 'work', account_uuid: 'acc-2', email: 'w@example.com' }] })]);
  sessions.set([sess, noConversation]);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  sessionActionRequest.set(null);
  clearSelection();
  clearToasts();
  composerDrafts.clear();
});
afterEach(() => vi.restoreAllMocks());

describe('the registry', () => {
  it('offers Fork and Rewind only on a session with a conversation, Archive only with work linked', () => {
    expect(sessionActionShown('fork', sess)).toBe(true);
    expect(sessionActionShown('rewind', sess)).toBe(true);
    expect(sessionActionShown('fork', noConversation)).toBe(false);
    expect(sessionActionShown('copy_transcript', noConversation)).toBe(false);
    expect(sessionActionShown('archive', sess)).toBe(true);
    expect(sessionActionShown('archive', noConversation)).toBe(false);
    expect(sessionActionShown('archive', { ...sess, work: { ...work, archived_at: 1 } } as SessionRow)).toBe(false);
    expect(sessionActionShown('fork', session('m', 'sh', { kind: 'shell', claude_session_id: 'c' }))).toBe(false);
  });

  it('Switch account is disabled, with the reason, on a host with no other login', () => {
    hosts.set([host('mefistos', { claude_profiles: [] })]);
    const item = get(sessionMenuItems)(sess).find((i) => i.id === 'switch_account')!;
    expect(item.blocked).toMatch(/no other login/);
    // On a profile, the host's own login is always the other one.
    expect(noOtherLogin({ ...sess, claude_profile: 'work' }, { claude_profiles: [] })).toBeNull();
  });

  it('the row menu and Details agree on every new action (shown, disabled, reason)', async () => {
    hosts.set([host('mefistos', { claude_profiles: [] })]);
    render(SessionDetails, { props: { session: sess } });
    await tick();
    for (const i of get(sessionMenuItems)(sess)) {
      const b = screen.getByTestId(i.detailsTestId) as HTMLButtonElement;
      expect(b.disabled, i.id).toBe(i.blocked !== null);
      if (i.blocked) expect(b.title, i.id).toBe(i.blocked);
    }
  });
});

describe('Details runs each action', () => {
  it('Fork… opens the Fork sheet on the whole conversation and forks with no anchor', async () => {
    answer({ rewind_conversation: { ...sess, id: 9, tmux_name: 'fork' } });
    render(SessionDetails, { props: { session: sess } });
    await fireEvent.click(screen.getByTestId('fork-from-details'));
    expect(screen.getByTestId('fork-from').textContent).toBe('The latest turn');
    expect((screen.getByTestId('fork-worktree-name') as HTMLInputElement).value).toBe('fork-of-dev-foo');
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(callsOf('rewind_conversation')).toEqual([
      { session_id: 1, mode: 'fork', anchor_uuid: null, new_worktree: 'fork-of-dev-foo' },
    ]);
  });

  it('Rewind… lists the turns it can go back to, newest first, and rewinds to the picked one', async () => {
    answer({ session_conversation: conv, rewind_conversation: sess });
    render(SessionDetails, { props: { session: sess } });
    await fireEvent.click(screen.getByTestId('rewind-from-details'));
    await settle();
    const radios = screen.getAllByTestId('rewind-turn') as HTMLInputElement[];
    // The first turn is the conversation's start: rewinding it is /clear.
    expect(radios.map((r) => r.value)).toEqual(['u3', 'u2']);
    expect(radios[0].checked).toBe(true);
    await fireEvent.click(radios[1]);
    await fireEvent.click(screen.getByTestId('rewind-confirm'));
    await settle();
    expect(callsOf('rewind_conversation')).toEqual([
      { session_id: 1, mode: 'rewind', anchor_uuid: 'u2', new_worktree: null },
    ]);
    // The prompt goes back to the composer, as the per-turn Rewind does.
    expect(composerDrafts.get(1)).toBe('second line\nmore');
    expect(screen.queryByTestId('rewind-sheet')).toBeNull();
  });

  it('Switch account… proposes the login with headroom and restarts under it', async () => {
    answer({
      check_account_headroom: {
        pause_at_pct: 90,
        chosen: null,
        over: true,
        suggestion: { profile: 'work', account_uuid: 'acc-2', used_pct: 10 },
        logins: [],
      },
      restart_session: sess,
    });
    render(SessionDetails, { props: { session: sess } });
    await fireEvent.click(screen.getByTestId('switch-account-from-details'));
    await settle();
    expect((screen.getByTestId('switch-account-pick') as HTMLSelectElement).value).toBe('work');
    await fireEvent.click(screen.getByTestId('confirm-account-switch'));
    await settle();
    expect(callsOf('restart_session')).toEqual([{ host_alias: 'mefistos', name: 'dev-foo', profile: 'work' }]);
  });

  it('Change model… sends /model through the outbox', async () => {
    const enqueue = vi.spyOn(outbox, 'enqueue').mockReturnValue('m1');
    render(SessionDetails, { props: { session: sess } });
    await fireEvent.click(screen.getByTestId('change-model-from-details'));
    const confirm = screen.getByTestId('confirm-model-change') as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    await fireEvent.change(screen.getByTestId('change-model-pick'), { target: { value: 'opus' } });
    await fireEvent.click(confirm);
    expect(enqueue).toHaveBeenCalledWith(
      { id: 1, host_alias: 'mefistos', tmux_name: 'dev-foo' },
      { kind: 'command', text: '/model opus', prefix: null },
    );
  });

  it('Copy transcript copies the conversation as Markdown', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    answer({ session_conversation: conv });
    render(SessionDetails, { props: { session: sess } });
    await fireEvent.click(screen.getByTestId('copy-transcript-from-details'));
    await settle();
    expect(callsOf('session_conversation')[0]).toMatchObject({ session_id: 1, turns: 100 });
    expect(writeText).toHaveBeenCalledWith(transcriptMarkdown(conv, 'dev-foo'));
    expect(get(toasts).some((t) => t.message === 'Copied 3 turns')).toBe(true);
  });

  it('Archive archives through tidy-up with Undo', async () => {
    answer({ tidy_apply: { results: [{ session_id: 1, ok: true }] }, unarchive_session_work: sess });
    render(SessionDetails, { props: { session: sess } });
    await fireEvent.click(screen.getByTestId('archive-from-details'));
    await settle();
    expect(callsOf('tidy_apply')).toEqual([{ items: [{ session_id: 1, action: 'archive', link_id: 41 }] }]);
    const t = get(toasts).find((x) => x.message === 'Archived 1 session');
    expect(t?.action?.label).toBe('Undo');
    t!.action!.run();
    await settle();
    expect(callsOf('unarchive_session_work').length).toBe(1);
  });

  it('the inspector offers Fork, Switch account and Archive', async () => {
    render(SessionDetails, { props: { session: sess, variant: 'inspector' } });
    await tick();
    for (const id of ['fork-from-details', 'switch-account-from-details', 'archive-from-details']) {
      expect(screen.getByTestId(id), id).toBeTruthy();
    }
  });
});

describe('the ⌘K palette', () => {
  it('lists the session actions this person may run and hands them to Details', async () => {
    const cmds = paletteCommands({ selected: sess, sessionView: 'conversation' });
    const ids = cmds.filter((c) => c.section === 'This session').map((c) => c.id);
    for (const a of ['fork', 'rewind', 'switch_account', 'change_model', 'copy_transcript', 'archive']) {
      expect(ids, a).toContain(`session.action.${a}`);
    }
    expect(cmds.find((c) => c.id === 'session.action.change_model')?.synonyms).toContain('model');
    await runCommand('session.action.fork', { selected: sess, sessionView: 'conversation' });
    expect(get(sessionActionRequest)).toMatchObject({ sessionId: 1, action: 'fork' });

    render(SessionDetails, { props: { session: sess } });
    await waitFor(() => expect(screen.getByTestId('fork-sheet')).toBeTruthy());
  });

  it('leaves out an action the session may not run now', () => {
    hosts.set([host('mefistos', { claude_profiles: [] })]);
    const ids = paletteCommands({ selected: sess, sessionView: 'conversation' }).map((c) => c.id);
    expect(ids).not.toContain('session.action.switch_account');
    expect(ids).toContain('session.action.fork');
  });
});

describe('pure helpers', () => {
  it('rewindChoices: the turns whose reply offers Rewind, newest first', () => {
    expect(rewindChoices(conv.turns, false).map((c) => c.anchor)).toEqual(['u3', 'u2']);
    // Older turns were dropped: index 0 is not the conversation's start.
    expect(rewindChoices(conv.turns, true).map((c) => c.anchor)).toEqual(['u3', 'u2', 'u1']);
    expect(rewindChoices([turn('a', 'u1'), turn(null, null), turn('b', 'u3')], false).map((c) => c.anchor)).toEqual(['u3']);
  });

  it('transcriptMarkdown: prompts and replies, tools as one line, older turns noted', () => {
    const md = transcriptMarkdown(
      {
        truncated: true,
        turns: [
          {
            ...turn('fix it', 'u1', 'Looking.'),
            items: [
              { kind: 'text', text: 'Looking.' },
              { kind: 'tool', summary: '', id: 't', name: 'Read', target: 'src/a.ts', at: null, ended_at: null, done: true },
              { kind: 'text', text: 'Fixed.' },
            ],
          },
        ],
      },
      'dev-foo',
    );
    expect(md).toBe(
      '# dev-foo\n\n_Older turns are not included._\n\n## You\n\nfix it\n\n## Claude\n\nLooking.\n- Read: src/a.ts\n\nFixed.\n',
    );
  });
});
