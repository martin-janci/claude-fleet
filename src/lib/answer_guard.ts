// Two guards on answering a dialog that a press must not get wrong
// (HumanLayer review, plan v2 §3).
//
// 1. A card that just appeared or changed ignores presses for a moment: a
//    tap or a digit aimed at the card that was there before must not land
//    on its replacement (Android's obscured-touch rule, for the same reason).
// 2. Claude Code's "Yes, and don't ask again for: <rule> in <folder>" is
//    saved in that folder's settings and answers every LATER session there,
//    not only this one. It is never a one-press choice: the first press says
//    so, and only "Save rule and allow" sends the key. The session-only
//    variant ("…don't ask again this session") stays an ordinary choice.

/** How long a new or changed card ignores presses. Tests set it to 0. */
export const answerArm = { ms: 600 };

export interface PersistentRule {
  /** The rule as Claude Code shows it (`git push:*`), when the label says. */
  rule: string | null;
  /** The folder it is saved for, when the label says. */
  folder: string | null;
}

const SESSION_ONLY = /\b(?:this|the) session\b|\bduring this\b/i;
const DONT_ASK = /don['’]?t ask again/i;
const SHAPE = /don['’]?t ask again\s+for:?\s*(.+?)\s+in\s+(\S.*?)\s*$/i;

/** The rule a choice would save for every later session in a folder, or
 *  `null` when the choice ends with this session (or saves nothing). */
export function persistentRule(label: string): PersistentRule | null {
  if (!DONT_ASK.test(label) || SESSION_ONLY.test(label)) return null;
  const m = SHAPE.exec(label);
  return m ? { rule: m[1], folder: m[2] } : { rule: null, folder: null };
}
