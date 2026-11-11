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
  /** Saved in the folder's settings for later sessions (a rule), as opposed
   *  to a choice that only stops the asking (auto mode, bypass). */
  saved: boolean;
}

const SESSION_ONLY = /\b(?:this|the) session[\s.]*$|\bduring this session[\s.]*$/i;
const SAVES = /don['’]?t ask again|always allow/i;
const FOR = /don['’]?t ask again\s+for:?\s*(\S.*?)\s*$/i;
// The folder is the label's LAST " in <path>": a rule may hold " in " itself.
const IN_FOLDER = /^(.*\S)\s+in\s+((?:~|\/|[A-Za-z]:[\\/])\S*)$/;

/** What a choice does beyond answering this one request, or `null` for an
 *  ordinary choice. The hub decides it (`persists` on the option, from
 *  `pane_intel::persists_label`); the label is read only for a hub that
 *  predates the flag, and for the rule and folder to show. */
export function persistentRule(o: { label: string; persists?: boolean }): PersistentRule | null {
  const saved = SAVES.test(o.label) && !SESSION_ONLY.test(o.label);
  if (o.persists !== true && !saved) return null;
  const m = FOR.exec(o.label);
  if (!m) return { rule: null, folder: null, saved };
  const f = IN_FOLDER.exec(m[1]);
  return f ? { rule: f[1], folder: f[2], saved } : { rule: m[1], folder: null, saved };
}
