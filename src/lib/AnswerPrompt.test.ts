import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionActivity: vi.fn() };
});
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, sendPrompt: vi.fn() };
});

import AnswerPrompt from './AnswerPrompt.svelte';
import { sessionActivity, type ActivityProbe } from './conversation';
import { sendPrompt, type SessionRow } from './sessions';
import { pendingInputFor, type AnswerView, type PendingInput } from './pending_input';

const mockedAct = vi.mocked(sessionActivity);
const mockedSend = vi.mocked(sendPrompt);

const DIALOG: PendingInput = {
  kind: 'permission',
  question: 'Do you want to proceed?',
  options: [
    { n: 1, label: 'Yes', selected: true },
    { n: 2, label: "Yes, and don't ask again", selected: false },
    { n: 3, label: 'No, and tell Claude what to do differently', selected: false },
  ],
};

/** As production has it: the row carries the dialog too (the 20 s tick wrote
 *  it), which is exactly what the freshness check must not fall back on. */
function session(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 7,
    host_alias: 'local',
    tmux_name: 'dev-foo',
    claude_status: 'blocked',
    stuck_kind: null,
    pending_input: DIALOG,
    ...over,
  } as unknown as SessionRow;
}

function probe(pending: PendingInput | null): ActivityProbe {
  return {
    claude_status: 'blocked',
    current_activity: null,
    stuck_kind: null,
    waiting_for: 'permission',
    spinner: null,
    pending_input: pending,
  };
}

function view(p: PendingInput = DIALOG): AnswerView {
  const v = pendingInputFor({ rowStatus: 'blocked', rowStuck: null, rowPending: p, probe: null });
  if (!v) throw new Error('fixture must produce a view');
  return v;
}

/** Let the click's await chain (recheck → send) run to completion. */
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  vi.clearAllMocks();
  mockedAct.mockResolvedValue({ ok: true, value: probe(DIALOG) });
  mockedSend.mockResolvedValue({ ok: true, value: undefined });
});

describe('AnswerPrompt', () => {
  it('shows the question and one button per option', () => {
    render(AnswerPrompt, { session: session(), view: view() });
    expect(screen.getByTestId('answer-question').textContent).toContain('Do you want to proceed?');
    const opts = screen.getAllByTestId('answer-option');
    expect(opts.map((o) => o.getAttribute('data-n'))).toEqual(['1', '2', '3']);
    expect(opts[0].textContent).toContain('Yes');
  });

  it('marks the option the pane has highlighted', () => {
    render(AnswerPrompt, { session: session(), view: view() });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts[0].getAttribute('data-selected')).toBe('true');
    expect(opts[1].getAttribute('data-selected')).toBeNull();
  });

  it('flags an option that stops Claude asking again', () => {
    // Not a confirm step — the labels are Claude's own words and a dialog
    // about a dialog is noise — but the one option that changes future
    // behaviour should not look like the two that do not.
    render(AnswerPrompt, { session: session(), view: view() });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts[1].getAttribute('data-sticky')).toBe('true');
    expect(opts[0].getAttribute('data-sticky')).toBeNull();
  });

  it('re-reads the pane and sends that option key when the dialog is unchanged', async () => {
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[1]);
    await settle();

    expect(mockedAct).toHaveBeenCalledWith(7);
    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', '', { keys: '2' });
  });

  it('sends nothing when the pane is now showing a different dialog', async () => {
    const moved: PendingInput = {
      kind: 'permission',
      question: 'Do you want to delete notes.md?',
      options: [{ n: 1, label: 'Yes', selected: true }],
    };
    mockedAct.mockResolvedValue({ ok: true, value: probe(moved) });
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();

    expect(mockedSend).not.toHaveBeenCalled();
    expect(screen.getByTestId('answer-stale').textContent).toMatch(/changed/i);
  });

  it('sends nothing when the dialog is already gone', async () => {
    mockedAct.mockResolvedValue({ ok: true, value: probe(null) });
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();

    expect(mockedSend).not.toHaveBeenCalled();
    expect(screen.getByTestId('answer-stale')).toBeTruthy();
  });

  it('sends nothing when the pane cannot be read at all', async () => {
    // No reading is not the same as "unchanged": without proof the dialog is
    // still there, a keystroke is a guess.
    mockedAct.mockResolvedValue({ ok: false, error: { code: 'E_SSH', message: 'host offline' } });
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();

    expect(mockedSend).not.toHaveBeenCalled();
    expect(screen.getByTestId('answer-error').textContent).toContain('host offline');
  });

  it('sends nothing when the probe comes back empty', async () => {
    // A reading that is not a reading must not fall through to the row: the
    // row is the up-to-a-tick-stale value the freshness check exists to
    // distrust, so "no probe" has to refuse exactly like "cannot read".
    mockedAct.mockResolvedValue({ ok: true, value: null as unknown as ActivityProbe });
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();

    expect(mockedSend).not.toHaveBeenCalled();
    expect(screen.getByTestId('answer-stale')).toBeTruthy();
  });

  it('shows why a send failed', async () => {
    mockedSend.mockResolvedValue({ ok: false, error: { code: 'E_TMUX', message: 'no such pane' } });
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();

    expect(screen.getByTestId('answer-error').textContent).toContain('no such pane');
  });

  it('answers at most once per click', async () => {
    render(AnswerPrompt, { session: session(), view: view() });
    const opts = screen.getAllByTestId('answer-option');
    await fireEvent.click(opts[0]);
    await fireEvent.click(opts[1]);
    await settle();

    expect(mockedSend).toHaveBeenCalledTimes(1);
  });

  it('offers no key for an option past the digits', async () => {
    const many: PendingInput = {
      kind: 'input',
      question: 'Pick one',
      options: Array.from({ length: 10 }, (_, i) => ({ n: i + 1, label: `opt ${i + 1}`, selected: false })),
    };
    render(AnswerPrompt, { session: session(), view: view(many) });
    const tenth = screen.getAllByTestId('answer-option')[9];
    expect((tenth as HTMLButtonElement).disabled).toBe(true);
    expect(tenth.getAttribute('title')).toMatch(/terminal/i);

    await fireEvent.click(tenth);
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
  });

  it('dismisses the dialog with Escape', async () => {
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getByTestId('answer-esc'));
    await settle();

    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', '', { keys: 'Escape' });
  });

  it('opens the terminal when asked', async () => {
    const onOpenTerminal = vi.fn();
    render(AnswerPrompt, { session: session(), view: view(), onOpenTerminal });
    await fireEvent.click(screen.getByTestId('answer-open-terminal'));
    expect(onOpenTerminal).toHaveBeenCalled();
  });

  it('reports what it pressed and stops offering the choices', async () => {
    // The sidebar row has no probe, so its card would otherwise sit there
    // with live buttons until the next 20 s tick — long enough to invite a
    // second press at a dialog that is already answered.
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[1]);
    await settle();

    expect(screen.getByTestId('answer-sent').textContent).toContain("Yes, and don't ask again");
    expect(screen.queryAllByTestId('answer-option')).toHaveLength(0);
  });

  it('lets you choose again when the keystroke did not take', async () => {
    // The card cannot know a dialog ignored the key: the same question is
    // still on the pane either way. Without a way back, a key that did not
    // register leaves a blocked session with no buttons at all.
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(screen.queryAllByTestId('answer-option')).toHaveLength(0);

    await fireEvent.click(screen.getByTestId('answer-again'));
    expect(screen.getAllByTestId('answer-option')).toHaveLength(3);

    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(mockedSend).toHaveBeenCalledTimes(2);
  });

  it('offers the choices again when the pane asks something new', async () => {
    const { rerender } = render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(screen.queryAllByTestId('answer-option')).toHaveLength(0);

    const next: PendingInput = {
      kind: 'permission',
      question: 'Do you want to create notes.md?',
      options: [{ n: 1, label: 'Yes', selected: true }],
    };
    await rerender({ session: session(), view: view(next) });

    expect(screen.queryByTestId('answer-sent')).toBeNull();
    expect(screen.getAllByTestId('answer-option')).toHaveLength(1);
  });

  it('keeps its clicks to itself', async () => {
    // In the sidebar the card sits inside a row that is itself a button:
    // clicking a choice must answer the question, not also select the
    // session (or, in select mode, tick its bulk checkbox).
    const outer = vi.fn();
    document.body.addEventListener('click', outer);
    try {
      render(AnswerPrompt, { session: session(), view: view() });
      await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
      await fireEvent.click(screen.getByTestId('answer-esc'));
      await settle();
      expect(outer).not.toHaveBeenCalled();
    } finally {
      document.body.removeEventListener('click', outer);
    }
  });

  it('compact mode keeps the options and drops the secondary actions', async () => {
    // The sidebar row is one line of chrome: the numbered choices are the
    // whole point there, Escape and Open terminal belong to the full card.
    render(AnswerPrompt, { session: session(), view: view(), compact: true });
    expect(screen.getAllByTestId('answer-option')).toHaveLength(3);
    expect(screen.queryByTestId('answer-esc')).toBeNull();
    expect(screen.queryByTestId('answer-open-terminal')).toBeNull();
  });
});
