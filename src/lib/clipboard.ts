// Pure helpers for terminal copy/paste. No DOM or Tauri deps so they can be
// unit-tested directly and reused by both the Cmd+V and drag-drop paths.

/** Bracketed-paste markers (DEC mode 2004). */
const PASTE_START = '\x1b[200~';
const PASTE_END = '\x1b[201~';

/** Trim trailing whitespace from each line of a selection. Terminal rows are
 *  space-padded to the full width, so a raw selection carries a block of
 *  trailing spaces; real terminals strip them on copy. Leading/interior
 *  whitespace is preserved. */
export function trimSelectionText(raw: string): string {
  return raw
    .split('\n')
    .map((line) => line.replace(/[ \t]+$/, ''))
    .join('\n');
}

/** Remove any embedded paste-end marker from text about to be pasted, so a
 *  malicious/odd clipboard payload can't prematurely close the bracket. */
export function sanitizePaste(text: string): string {
  return text.split(PASTE_END).join('');
}

/** Wrap text in bracketed-paste markers when the remote app requested mode
 *  2004; otherwise return it unchanged. */
export function framePaste(text: string, bracketed: boolean): string {
  return bracketed ? `${PASTE_START}${text}${PASTE_END}` : text;
}
