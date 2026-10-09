// The form kit's behaviour (Orbit Fleet step G1.2, the FormsAnatomy board),
// shared by DialogSheet, WizardDialog and FormWizard so every dialog and chat
// form answers the keyboard, shows a failure and offers Undo alike:
//
// - Enter submits a one-field form, ⌘↵ / Ctrl+Enter any form (`submitKey`,
//   through the shortcut registry's `form` scope).
// - A server error is a banner at the top of the body; a hub refusal
//   (`E_FORBIDDEN`) says so, keeps the person's input and names who can help
//   (`formFailure`).
// - A saved form closes and a toast offers Undo when the write can be put
//   back (`savedWithUndo`).
import { errorText } from '../error_copy';
import type { IpcError } from '../result';
import { matchShortcut, type KeyEventLike } from '../shortcuts';
import { push } from '../toasts';

/** The controls a person fills in, for "is this a one-field form". */
const CONTROLS =
  'input:not([type="hidden"]):not([type="button"]):not([type="submit"]), select, textarea, [role="radiogroup"]';

/** How many fields `root` holds (a radio group counts once). */
export function fieldCount(root: ParentNode | null | undefined): number {
  if (!root) return 0;
  const all = Array.from(root.querySelectorAll(CONTROLS));
  return all.filter((el) => !el.parentElement?.closest('[role="radiogroup"]')).length;
}

const NOT_TEXT = ['checkbox', 'radio', 'button', 'submit'];

/**
 * Whether `e` submits the form: ⌘↵ (Ctrl+Enter off the Mac) anywhere in it,
 * or a bare Enter in the text input of a form with one field. Enter in a
 * textarea is a new line, and on a button it presses that button.
 */
export function submitKey(
  e: KeyEventLike & { target?: EventTarget | null; isComposing?: boolean },
  fields: number,
  isMac: boolean,
): boolean {
  if (e.isComposing) return false;
  const id = matchShortcut('form', e, isMac);
  if (id === 'form.submit') return true;
  if (id !== 'form.submit-one' || fields !== 1) return false;
  const t = e.target as HTMLInputElement | null;
  return t?.tagName === 'INPUT' && !NOT_TEXT.includes(t.type);
}

/** What a form shows for a failed call. */
export interface FormFailure {
  /** `refused`: the hub said no to this person; their input is kept. */
  kind: 'refused' | 'error';
  headline: string;
  meta: string | null;
}

/**
 * The banner for `error`. A plain string is the dialog's own sentence; an
 * `IpcError` is read through `errorText`, and `E_FORBIDDEN` (the hub's
 * refusal) says the hub refused, that the input is kept, and to ask an admin.
 */
export function formFailure(error: string | IpcError | null | undefined): FormFailure | null {
  if (!error) return null;
  if (typeof error === 'string') return { kind: 'error', headline: error, meta: null };
  const why = errorText(error).replace(/\.$/, '');
  if (error.code === 'E_FORBIDDEN') {
    return {
      kind: 'refused',
      headline: `The hub refused this: ${why}.`,
      meta: 'Your changes are kept. Ask an admin of this hub for access, then try again.',
    };
  }
  return { kind: 'error', headline: `${why}.`, meta: 'Your changes are kept.' };
}

/** "Saved": the form has closed; a toast says what changed and, when the
 *  write can be put back, offers Undo. */
export function savedWithUndo(message: string, undo?: () => void | Promise<unknown>): void {
  push({
    kind: 'success',
    message,
    ...(undo ? { action: { label: 'Undo', run: () => void undo() } } : {}),
  });
}
