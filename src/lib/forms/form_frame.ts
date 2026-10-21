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
import { matchShortcut, shortcutLabel, type KeyEventLike } from '../shortcuts';
import { push } from '../toasts';
import { detectMac } from '../terminal_keys';

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

/** The submit key a form's verb shows beside its name ("Create ⌘↵"),
 *  and the same chord for `aria-keyshortcuts`. Drawn by CSS from
 *  `data-shortcut`, so the button's own name stays the verb alone. */
export function submitHint(isMac: boolean): { label: string; aria: string } {
  const label = shortcutLabel('form.submit', isMac);
  return {
    label: isMac ? label.replace(/Enter$/, '↵') : label,
    aria: isMac ? 'Meta+Enter' : 'Control+Enter',
  };
}

/** `submitHint` for this machine, for a dialog that uses `formKeys`. */
export function platformSubmitHint(): { label: string; aria: string } {
  return submitHint(detectMac(typeof navigator === 'undefined' ? undefined : navigator));
}

export interface FormKeysOptions {
  /** What the primary verb does. Left out on a `<form>`: its submit
   *  button's form is submitted, unless that button is disabled. */
  submit?: () => void;
  /** False while the verb may not run (the primary button is disabled). */
  enabled?: boolean;
}

/**
 * The form rule for a dialog that is not a DialogSheet (review w18-6):
 * ⌘↵ / Ctrl+Enter submits from any field, a bare Enter only in a one-field
 * form (`submitKey`), and a bare Enter in one text field of a several-field
 * `<form>` no longer submits it natively. Put it on the element holding the
 * fields (the `<form>` itself when there is one).
 */
export function formKeys(node: HTMLElement, options: FormKeysOptions = {}) {
  let opts = options;
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  function run() {
    if (opts.enabled === false) return;
    if (opts.submit) {
      opts.submit();
      return;
    }
    if (!(node instanceof HTMLFormElement)) return;
    const btn = node.querySelector<HTMLButtonElement>('button[type="submit"]');
    if (btn?.disabled) return;
    node.requestSubmit(btn ?? undefined);
  }
  function onkeydown(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    if (submitKey(e, fieldCount(node), isMac)) {
      e.preventDefault();
      run();
      return;
    }
    // A several-field form: Enter in a text field is not a submit.
    const t = e.target as HTMLInputElement | null;
    if (
      e.key === 'Enter' &&
      !e.metaKey &&
      !e.ctrlKey &&
      !e.altKey &&
      t?.tagName === 'INPUT' &&
      !NOT_TEXT.includes(t.type) &&
      t.form != null
    ) {
      e.preventDefault();
    }
  }
  node.addEventListener('keydown', onkeydown);
  return {
    update(next: FormKeysOptions = {}) {
      opts = next;
    },
    destroy() {
      node.removeEventListener('keydown', onkeydown);
    },
  };
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
