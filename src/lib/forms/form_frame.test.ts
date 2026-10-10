// The form kit's behaviour (G1.2, FormsAnatomy): which keys submit, how a
// failure reads, and the Undo toast after a save.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { fieldCount, formFailure, savedWithUndo, submitHint, submitKey } from './form_frame';
import { clearToasts, runToastAction, toasts } from '../toasts';
import { findConflicts } from '../shortcuts';

function key(key: string, mods: { meta?: boolean; ctrl?: boolean; shift?: boolean } = {}, target?: Element) {
  return {
    key,
    metaKey: !!mods.meta,
    ctrlKey: !!mods.ctrl,
    altKey: false,
    shiftKey: !!mods.shift,
    target: target ?? null,
  };
}

function el(html: string): HTMLElement {
  const d = document.createElement('div');
  d.innerHTML = html;
  return d;
}

beforeEach(() => clearToasts());

describe('submitKey', () => {
  const input = el('<input type="text">').firstElementChild!;
  const area = el('<textarea></textarea>').firstElementChild!;
  const box = el('<input type="checkbox">').firstElementChild!;

  it('⌘↵ on the Mac and Ctrl+Enter elsewhere submit any form, from any field', () => {
    expect(submitKey(key('Enter', { meta: true }, area), 4, true)).toBe(true);
    expect(submitKey(key('Enter', { ctrl: true }, area), 4, false)).toBe(true);
    // Each platform's own chord only: Ctrl+Enter on the Mac is not it.
    expect(submitKey(key('Enter', { ctrl: true }, area), 4, true)).toBe(false);
    expect(submitKey(key('Enter', { shift: true, ctrl: true }, area), 4, false)).toBe(false);
  });

  it('a bare Enter submits only a one-field form, and only from its text input', () => {
    expect(submitKey(key('Enter', {}, input), 1, true)).toBe(true);
    expect(submitKey(key('Enter', {}, input), 2, true)).toBe(false);
    expect(submitKey(key('Enter', {}, area), 1, true)).toBe(false);
    expect(submitKey(key('Enter', {}, box), 1, true)).toBe(false);
    expect(submitKey(key('Enter', { shift: true }, input), 1, true)).toBe(false);
  });

  it('never submits while an input method is composing', () => {
    expect(submitKey({ ...key('Enter', {}, input), isComposing: true }, 1, true)).toBe(false);
  });

  it('the chords are free: no conflict in the registry', () => {
    expect(findConflicts().filter((c) => c.a.startsWith('form.') || c.b.startsWith('form.'))).toEqual([]);
  });
});

describe('fieldCount', () => {
  it('counts each control once, a radio group as one, and no buttons', () => {
    expect(fieldCount(el('<input type="text"><button>x</button><input type="hidden">'))).toBe(1);
    expect(fieldCount(el('<input><textarea></textarea><select></select>'))).toBe(3);
    expect(fieldCount(el('<div role="radiogroup"><input type="radio"><input type="radio"></div>'))).toBe(1);
    expect(fieldCount(null)).toBe(0);
  });
});

describe('formFailure', () => {
  it("a dialog's own sentence is shown as it is", () => {
    expect(formFailure('Pick a host.')).toEqual({ kind: 'error', headline: 'Pick a host.', meta: null });
    expect(formFailure(null)).toBeNull();
  });

  it('a hub refusal says so, keeps the input and asks an admin', () => {
    const f = formFailure({ code: 'E_FORBIDDEN', message: 'you are a Viewer in 32bit.' });
    expect(f?.kind).toBe('refused');
    expect(f?.headline).toBe('The hub refused this: you are a Viewer in 32bit.');
    expect(f?.meta).toMatch(/Your changes are kept\. Ask an admin/);
  });

  it('any other failure reads for people and keeps the input', () => {
    const f = formFailure({ code: 'E_HUB_UNREACHABLE', message: 'error sending request (os error 111)' });
    expect(f).toEqual({ kind: 'error', headline: "Couldn't reach the hub.", meta: 'Your changes are kept.' });
  });
});

describe('savedWithUndo', () => {
  it('toasts the save, and Undo runs the put-back', () => {
    const undo = vi.fn();
    savedWithUndo('Task saved.', undo);
    const [t] = get(toasts);
    expect(t.message).toBe('Task saved.');
    expect(t.action?.label).toBe('Undo');
    runToastAction(t.id);
    expect(undo).toHaveBeenCalledOnce();
  });

  it('offers no Undo when the save cannot be put back', () => {
    savedWithUndo('Saved.');
    expect(get(toasts)[0].action).toBeNull();
  });
});

describe('submitHint (G7.4)', () => {
  it('reads ⌘↵ on the Mac and Ctrl+Enter elsewhere', () => {
    expect(submitHint(true)).toEqual({ label: '⌘↵', aria: 'Meta+Enter' });
    expect(submitHint(false)).toEqual({ label: 'Ctrl+Enter', aria: 'Control+Enter' });
  });
});
