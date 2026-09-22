import { describe, expect, it } from 'vitest';
import type { ActivityProbe } from './conversation';
import {
  ANSWER_MAX_DIGIT,
  answerFingerprint,
  answerKeyFor,
  pendingInputFor,
  type PendingInput,
} from './pending_input';

const dialog: PendingInput = {
  kind: 'permission',
  question: 'Do you want to proceed?',
  options: [
    { n: 1, label: 'Yes', selected: true },
    { n: 2, label: "Yes, and don't ask again", selected: false },
    { n: 3, label: 'No, and tell Claude what to do differently', selected: false },
  ],
};

function probe(over: Partial<ActivityProbe> = {}): ActivityProbe {
  return {
    claude_status: 'blocked',
    current_activity: null,
    stuck_kind: null,
    waiting_for: 'permission',
    spinner: null,
    pending_input: null,
    ...over,
  };
}

describe('pendingInputFor', () => {
  it('draws the row dialog when no fresh probe has arrived yet', () => {
    const v = pendingInputFor({
      rowStatus: 'blocked',
      rowStuck: null,
      rowPending: dialog,
      probe: null,
    });
    expect(v?.question).toBe('Do you want to proceed?');
    expect(v?.options.map((o) => o.n)).toEqual([1, 2, 3]);
    expect(v?.live).toBe(false);
  });

  it('prefers the probe dialog over the row, and marks it live', () => {
    const fresher: PendingInput = {
      kind: 'input',
      question: 'Which branch?',
      options: [{ n: 1, label: 'main', selected: true }],
    };
    const v = pendingInputFor({
      rowStatus: 'blocked',
      rowStuck: null,
      rowPending: dialog,
      probe: probe({ waiting_for: 'input', pending_input: fresher }),
    });
    expect(v?.question).toBe('Which branch?');
    expect(v?.kind).toBe('input');
    expect(v?.live).toBe(true);
  });

  it('clears the card when a fresh probe sees no dialog, however stale the row', () => {
    // The row is written by the 20 s tick; the probe is seconds old. Someone
    // answering in the terminal must not leave buttons behind that would
    // send a keystroke into whatever came next.
    expect(
      pendingInputFor({
        rowStatus: 'blocked',
        rowStuck: null,
        rowPending: dialog,
        probe: probe({ pending_input: null }),
      }),
    ).toBeNull();
  });

  it('shows nothing when the session is not blocked', () => {
    expect(
      pendingInputFor({
        rowStatus: 'working',
        rowStuck: null,
        rowPending: dialog,
        probe: null,
      }),
    ).toBeNull();
  });

  it('shows nothing while the session is stuck', () => {
    // Stuck outranks a dialog everywhere else in the app (the Press Enter
    // chip and the stuck chip own that state), so it does here too.
    expect(
      pendingInputFor({
        rowStatus: 'blocked',
        rowStuck: 'press_enter',
        rowPending: dialog,
        probe: null,
      }),
    ).toBeNull();
  });

  it('shows nothing for a dialog with no options', () => {
    expect(
      pendingInputFor({
        rowStatus: 'blocked',
        rowStuck: null,
        rowPending: { kind: 'permission', question: 'Proceed?', options: [] },
        probe: null,
      }),
    ).toBeNull();
  });

  it('gives each option the key that answers it, and none past the digits', () => {
    const many: PendingInput = {
      kind: 'input',
      question: null,
      options: [
        { n: 1, label: 'one', selected: false },
        { n: 9, label: 'nine', selected: false },
        { n: 10, label: 'ten', selected: false },
      ],
    };
    const v = pendingInputFor({
      rowStatus: 'blocked',
      rowStuck: null,
      rowPending: many,
      probe: null,
    });
    expect(v?.options.map((o) => o.key)).toEqual(['1', '9', null]);
  });
});

describe('answerKeyFor', () => {
  it('is the digit for 1 through 9', () => {
    expect(answerKeyFor(1)).toBe('1');
    expect(answerKeyFor(ANSWER_MAX_DIGIT)).toBe('9');
  });

  it('is null past the digits, where the REPL has no single keystroke', () => {
    // A dialog may carry up to 16 options; pressing "1" for option 10 would
    // silently answer option 1.
    expect(answerKeyFor(10)).toBeNull();
    expect(answerKeyFor(0)).toBeNull();
    expect(answerKeyFor(1.5)).toBeNull();
  });
});

describe('answerFingerprint', () => {
  it('is stable for the same dialog read twice', () => {
    expect(answerFingerprint(dialog)).toBe(answerFingerprint({ ...dialog }));
  });

  it('changes when an option label changes', () => {
    const edited: PendingInput = {
      ...dialog,
      options: [{ ...dialog.options[0], label: 'Yes, obviously' }, ...dialog.options.slice(1)],
    };
    expect(answerFingerprint(edited)).not.toBe(answerFingerprint(dialog));
  });

  it('changes when the question changes', () => {
    expect(answerFingerprint({ ...dialog, question: 'Delete everything?' })).not.toBe(
      answerFingerprint(dialog),
    );
  });

  it('ignores which option is currently highlighted', () => {
    // Arrow keys move the ❯ glyph without changing the question being asked;
    // that must not read as "the dialog changed under you".
    const moved: PendingInput = {
      ...dialog,
      options: dialog.options.map((o) => ({ ...o, selected: o.n === 2 })),
    };
    expect(answerFingerprint(moved)).toBe(answerFingerprint(dialog));
  });
});
