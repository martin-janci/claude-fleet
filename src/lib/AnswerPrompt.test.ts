import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionActivity: vi.fn() };
});
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, answerDialog: vi.fn() };
});

import AnswerPrompt from './AnswerPrompt.svelte';
import { expectAccessible } from './a11y_check';
import { sessionActivity, type ActivityProbe } from './conversation';
import { answerDialog, type SessionRow } from './sessions';
import { pendingInputFor, type AnswerView, type PendingInput } from './pending_input';

const mockedAct = vi.mocked(sessionActivity);
const mockedSend = vi.mocked(answerDialog);

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

/** A multi-select box's state, as a screen reader hears it. */
const ticked = (o: HTMLElement) => (o.textContent ?? '').includes('ticked: ') && !(o.textContent ?? '').includes('not ticked: ');

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
    expect(screen.getByTestId('answer-card').textContent).toContain('Do you want to proceed?');
    const opts = screen.getAllByTestId('answer-option');
    expect(opts.map((o) => o.textContent?.trim()[0])).toEqual(['1', '2', '3']);
    expect(opts[0].textContent).toContain('Yes');
  });

  it('flags an option that stops Claude asking again', () => {
    // Not a confirm step — the labels are Claude's own words and a dialog
    // about a dialog is noise — but the one option that changes future
    // behaviour should not look like the two that do not.
    render(AnswerPrompt, { session: session(), view: view() });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts[1].title).toContain('this also stops Claude asking again');
    expect(opts[0].title).not.toContain('stops Claude asking again');
  });

  it('re-reads the pane and sends that option key when the dialog is unchanged', async () => {
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.click(screen.getAllByTestId('answer-option')[1]);
    await settle();

    expect(mockedAct).toHaveBeenCalledWith(7);
    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', '2', expect.anything());
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

    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', 'Escape', expect.anything());
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

describe('AnswerPrompt, multi-select', () => {
  const MULTI: PendingInput = {
    kind: 'input',
    question: 'Which features do you want to enable?',
    multi: true,
    options: [
      { n: 1, label: 'Auth', selected: true },
      { n: 2, label: 'Logging', selected: false, checked: true },
      { n: 3, label: 'Metrics', selected: false },
      { n: 4, label: 'Type something', selected: false },
    ],
  };

  beforeEach(() => {
    mockedAct.mockResolvedValue({ ok: true, value: probe(MULTI) });
  });

  it('shows each box ticked as the pane has it', () => {
    render(AnswerPrompt, { session: session({ pending_input: MULTI }), view: view(MULTI) });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts.map((o) => ticked(o))).toEqual([false, true, false, false]);
  });

  it('toggles a box and keeps the choices up for the next one', async () => {
    // A digit only toggles on a multi-select. Reporting it as "Sent" and
    // hiding the choices left the user with one tick and no way to finish.
    render(AnswerPrompt, { session: session({ pending_input: MULTI }), view: view(MULTI) });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(mockedSend).toHaveBeenLastCalledWith('local', 'dev-foo', '1', expect.anything());
    expect(screen.queryByTestId('answer-sent')).toBeNull();
    expect(ticked(screen.getAllByTestId('answer-option')[0])).toBe(true);

    await fireEvent.click(screen.getAllByTestId('answer-option')[2]);
    await settle();
    expect(mockedSend).toHaveBeenLastCalledWith('local', 'dev-foo', '3', expect.anything());
    expect(screen.queryByTestId('answer-stale')).toBeNull();
  });

  it('still toggles once the pane shows the earlier tick', async () => {
    // The pane after one toggle: same question, one more box ticked. That
    // is the same dialog, not "the dialog changed — nothing was sent".
    const afterOne: PendingInput = {
      ...MULTI,
      options: MULTI.options.map((o) => (o.n === 1 ? { ...o, checked: true } : o)),
    };
    mockedAct.mockResolvedValue({ ok: true, value: probe(afterOne) });
    render(AnswerPrompt, { session: session({ pending_input: MULTI }), view: view(MULTI) });
    await fireEvent.click(screen.getAllByTestId('answer-option')[2]);
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', '3', expect.anything());
    expect(screen.queryByTestId('answer-stale')).toBeNull();
  });

  it('continues with Tab, which keeps the ticks and moves on', async () => {
    render(AnswerPrompt, { session: session({ pending_input: MULTI }), view: view(MULTI) });
    await fireEvent.click(screen.getByTestId('answer-continue'));
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', 'Tab', expect.anything());
    expect(screen.getByTestId('answer-sent').textContent).toContain('Logging');
  });

  it('leaves the free-text row to the terminal', async () => {
    render(AnswerPrompt, { session: session({ pending_input: MULTI }), view: view(MULTI) });
    const free = screen.getAllByTestId('answer-option')[3] as HTMLButtonElement;
    expect(free.disabled).toBe(true);
    await fireEvent.click(free);
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
  });

  it('toggles nothing while the cursor is in the free-text box', async () => {
    // There a digit is typed into the box instead of toggling anything.
    const typing: PendingInput = {
      ...MULTI,
      options: MULTI.options.map((o) => ({ ...o, selected: o.n === 4 })),
    };
    render(AnswerPrompt, { session: session({ pending_input: typing }), view: view(typing) });
    const first = screen.getAllByTestId('answer-option')[0] as HTMLButtonElement;
    expect(first.disabled).toBe(true);
    expect(first.getAttribute('title')).toMatch(/Type something/);
    expect((screen.getByTestId('answer-continue') as HTMLButtonElement).disabled).toBe(false);
  });

  it('has no Continue on a single-select dialog', () => {
    render(AnswerPrompt, { session: session(), view: view() });
    expect(screen.queryByTestId('answer-continue')).toBeNull();
  });
});

describe('AnswerPrompt 1–9 (redesign step 3.8)', () => {
  it('a digit answers the Conversation card when no field has focus', async () => {
    render(AnswerPrompt, { session: session(), view: view() });
    await fireEvent.keyDown(document.body, { key: '2' });
    await settle();
    expect(mockedSend).toHaveBeenCalledTimes(1);
    expect(mockedSend.mock.calls[0][2]).toBe('2');
    expect(screen.getByTestId('answer-sent').textContent).toContain("Yes, and don't ask again");
  });

  it('a digit typed in a field, with a modifier, or past the options does nothing', async () => {
    render(AnswerPrompt, { session: session(), view: view() });
    const input = document.createElement('input');
    document.body.appendChild(input);
    await fireEvent.keyDown(input, { key: '1' });
    await fireEvent.keyDown(document.body, { key: '1', metaKey: true });
    await fireEvent.keyDown(document.body, { key: '7' });
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
    input.remove();
  });

  it('the compact sidebar card takes no digits', async () => {
    render(AnswerPrompt, { session: session(), view: view(), compact: true });
    await fireEvent.keyDown(document.body, { key: '1' });
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
  });
});

describe('AnswerPrompt quick answer (redesign step 10.9)', () => {
  const QUESTION: PendingInput = {
    kind: 'input',
    question: 'Which test runner?',
    options: [
      { n: 1, label: 'Jest', selected: true },
      { n: 2, label: 'Vitest', selected: false },
      { n: 3, label: 'Push the branch first', selected: false },
    ],
  };
  const jev = (value: string, confidence_pct = 80) => ({
    proposals: [{ feature: 'quick_answer', value, source: 'jev' as const, confidence_pct }],
  });

  beforeEach(() => {
    mockedAct.mockResolvedValue({ ok: true, value: probe(QUESTION) });
  });

  const shown = () => screen.getAllByTestId('answer-option').map((b) => b.textContent);

  it('moves the likely option first, numbers the shown order, and a digit still sends its own key', async () => {
    render(AnswerPrompt, { session: session({ pending_input: QUESTION, ...jev('o2') }), view: view(QUESTION) });
    expect(shown()).toEqual(['1Vitest', '2Jest', '3Push the branch first']);
    expect(screen.getByTestId('answer-proposed')).toHaveTextContent('Proposed by Jev');
    await fireEvent.keyDown(document.body, { key: '1' });
    await settle();
    expect(mockedSend.mock.calls[0][2]).toBe('2');
  });

  it('never moves a risky option, a weak or unsure proposal, or anything on a permission', () => {
    for (const p of [jev('o3'), jev('o2', 30), jev('unsure')]) {
      const { unmount } = render(AnswerPrompt, { session: session({ pending_input: QUESTION, ...p }), view: view(QUESTION) });
      expect(shown()).toEqual(['1Jest', '2Vitest', '3Push the branch first']);
      expect(screen.queryByTestId('answer-proposed')).toBeNull();
      unmount();
    }
    render(AnswerPrompt, { session: session(jev('o2')), view: view() });
    expect(shown()[0]).toBe('1Yes');
  });

  it('Keep the order puts the options back for this question', async () => {
    render(AnswerPrompt, { session: session({ pending_input: QUESTION, ...jev('o2') }), view: view(QUESTION) });
    await fireEvent.click(screen.getByTestId('answer-proposed-change'));
    expect(shown()).toEqual(['1Jest', '2Vitest', '3Push the branch first']);
  });

  it('the New layout’s card shows the likely answer first as its primary, numbered as shown', async () => {
    render(AnswerPrompt, { session: session({ pending_input: QUESTION, ...jev('o2') }), view: view(QUESTION) });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts.map((b) => b.textContent?.replace(/\s+/g, ''))).toEqual(['1Vitest', '2Jest', '3Pushthebranchfirst']);
    expect(screen.getByTestId('answer-proposed')).toHaveTextContent('Proposed by Jev');
    await fireEvent.keyDown(document.body, { key: '1' });
    await settle();
    expect(mockedSend.mock.calls[0][2]).toBe('2');
  });

  // F10: a question that names a push (or a merge, a deploy…) is a risky
  // step, whatever its options say: "Yes, go ahead" is the push.
  const PUSH: PendingInput = {
    kind: 'input',
    question: 'Push the 3 commits to origin/main now?',
    options: [
      { n: 1, label: 'Not yet', selected: true },
      { n: 2, label: 'Yes, go ahead', selected: false },
    ],
  };

  it('never reorders a question that names a risky action, nor draws Jev’s pick primary', () => {
    render(AnswerPrompt, { session: session({ pending_input: PUSH, ...jev('o2', 95) }), view: view(PUSH) });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts.map((b) => b.textContent?.replace(/\s+/g, ''))).toEqual(['1Notyet', '2Yes,goahead']);
    expect(opts.some((b) => b.classList.contains('primary'))).toBe(false);
    expect(screen.queryByTestId('answer-proposed')).toBeNull();
  });

  it('may move an affirmative option first on a safe question, but never draws it primary', () => {
    const SAFE: PendingInput = {
      kind: 'input',
      question: 'Shall I add tests for the parser?',
      options: [
        { n: 1, label: 'Not now', selected: true },
        { n: 2, label: 'Yes, go ahead', selected: false },
      ],
    };
    render(AnswerPrompt, { session: session({ pending_input: SAFE, ...jev('o2') }), view: view(SAFE) });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts[0].textContent).toContain('Yes, go ahead');
    expect(opts.some((b) => b.classList.contains('primary'))).toBe(false);
    expect(screen.getByTestId('answer-proposed')).toHaveTextContent('Proposed by Jev');
  });

  // F11: the sidebar's compact card has no room to say who moved what.
  it('never reorders in compact mode, where no "Proposed by Jev" or "Keep the order" shows', async () => {
    render(AnswerPrompt, {
      session: session({ pending_input: QUESTION, ...jev('o2') }),
      view: view(QUESTION),
      compact: true,
    });
    const opts = screen.getAllByTestId('answer-option');
    expect(opts.map((b) => b.textContent?.replace(/\s+/g, ''))).toEqual(['1Jest', '2Vitest', '3Pushthebranchfirst']);
    expect(opts.some((b) => b.classList.contains('primary'))).toBe(false);
  });

  // F12: a proposal that arrives after the card is drawn must not change
  // what a digit means.
  it('a proposal that arrives after the card was drawn does not reorder it', async () => {
    const { rerender } = render(AnswerPrompt, { session: session({ pending_input: QUESTION }), view: view(QUESTION) });
    expect(shown()).toEqual(['1Jest', '2Vitest', '3Push the branch first']);
    await rerender({ session: session({ pending_input: QUESTION, ...jev('o2') }), view: view(QUESTION) });
    expect(shown()).toEqual(['1Jest', '2Vitest', '3Push the branch first']);
    await fireEvent.keyDown(document.body, { key: '1' });
    await settle();
    expect(mockedSend.mock.calls[0][2]).toBe('1');
  });

  it('a new question with its proposal already there is reordered as usual', async () => {
    const OTHER: PendingInput = { ...QUESTION, question: 'Which formatter?', options: [
      { n: 1, label: 'Prettier', selected: true },
      { n: 2, label: 'Biome', selected: false },
    ] };
    const { rerender } = render(AnswerPrompt, { session: session({ pending_input: QUESTION }), view: view(QUESTION) });
    await rerender({ session: session({ pending_input: OTHER, ...jev('o2') }), view: view(OTHER) });
    expect(shown()).toEqual(['1Biome', '2Prettier']);
  });
});

describe('AnswerPrompt: the command being approved', () => {
  it('a press is refused when the pane now asks about a different command', async () => {
    // Same question, same options; only the tool call differs. The press
    // meant for `rm -rf build` must not approve `rm -rf ~`.
    const v = view({ ...DIALOG, detail: 'Bash(rm -rf build)' });
    mockedAct.mockResolvedValue({ ok: true, value: probe({ ...DIALOG, detail: 'Bash(rm -rf ~)' }) });
    render(AnswerPrompt, { session: session(), view: v });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
  });
});

describe('AnswerPrompt: the backend re-checks the dialog with the key', () => {
  it('sends the dialog it answers, and reads E_CONFLICT as a moved dialog', async () => {
    mockedSend.mockResolvedValueOnce({ ok: false, error: { code: 'E_CONFLICT', message: 'The dialog changed — nothing was sent.' } });
    render(AnswerPrompt, { session: session(), view: view(DIALOG) });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', '1', {
      kind: DIALOG.kind,
      question: DIALOG.question,
      options: DIALOG.options.map((o) => ({ n: o.n, label: o.label })),
      detail: null,
      selected: DIALOG.options.find((o) => o.selected)?.n ?? null,
    });
    expect(screen.queryByTestId('answer-sent')).toBeNull();
  });
});

describe('AnswerPrompt in the New layout (redesign 5.9)', () => {
  it('the New layout draws the kit card, with the command, on a row as in the Conversation', async () => {
    const v = { ...view({ ...DIALOG, detail: 'Bash(git push)' }) };
    // The pane still shows the same command: the stale check compares it.
    mockedAct.mockResolvedValue({ ok: true, value: probe({ ...DIALOG, detail: 'Bash(git push)' }) });
    const onAnswered = vi.fn();
    render(AnswerPrompt, { session: session(), view: v, compact: true, onAnswered });
    const card = screen.getByTestId('answer-card');
    expect(card.querySelector('.of-question.compact')).toBeTruthy();
    expect(screen.getByTestId('question-detail').textContent).toBe('Bash(git push)');
    // A row has nowhere to type: no own-words button there.
    expect(screen.queryByTestId('question-own-words')).toBeNull();
    await fireEvent.click(screen.getAllByTestId('answer-option')[2]);
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'dev-foo', '3', expect.anything());
    expect(onAnswered).toHaveBeenCalledWith('No, and tell Claude what to do differently');
    expect(screen.getByTestId('answer-sent').textContent).toContain('No, and tell Claude');
  });

  it('a refused send moves nobody on', async () => {
    mockedAct.mockResolvedValue({ ok: true, value: probe(null) });
    const onAnswered = vi.fn();
    render(AnswerPrompt, { session: session(), view: view(), onAnswered });
    await fireEvent.click(screen.getAllByTestId('answer-option')[0]);
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
    expect(onAnswered).not.toHaveBeenCalled();
  });

  it('is accessible', async () => {
    const { container } = render(AnswerPrompt, { session: session(), view: view({ ...DIALOG, detail: 'Bash(git push)' }) });
    await expectAccessible(container);
  });
});
