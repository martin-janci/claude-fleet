// Copy / paste for TerminalView (moved out of TerminalView.svelte, F5b). The
// component's reactive state stays in the component; this module reads and
// writes it through `ClipboardHost`.
import { sanitizePaste, framePaste } from './clipboard';
import { nativeWriteText, nativeReadText } from './clipboard_native';
import type { Screen } from './ansi';
import type { CellPos } from './terminal_selection';

export interface ClipboardHost {
  ptyOpen(): boolean;
  screen(): Screen | null;
  selAnchor(): CellPos | null;
  selFocus(): CellPos | null;
  setOpenError(message: string): void;
  writePty(data: string): void;
  bumpDrain(): void;
}

/** Build the prompt text for a set of uploaded remote paths: space-joined,
 *  POSIX single-quoted (embedded quotes escaped as '\'') when a path
 *  contains whitespace or a quote, trailing space so the user can keep
 *  typing. */
export function pathsToPasteText(paths: string[]): string {
  return (
    paths
      .map((p) => (/[\s']/.test(p) ? `'${p.replace(/'/g, "'\\''")}'` : p))
      .join(' ') + ' '
  );
}

export function createTerminalClipboard(host: ClipboardHost) {
  /** Send text to the PTY as a paste: strip any embedded paste-end marker,
   *  then frame in bracketed-paste markers if the app requested mode 2004.
   *  Shared by Cmd+V and the drag-drop path. */
  function sendPaste(text: string) {
    if (!host.ptyOpen()) return;
    const clean = sanitizePaste(text);
    if (clean === '') return;
    const framed = framePaste(clean, host.screen()?.bracketedPaste ?? false);
    host.writePty(framed);
    host.bumpDrain();
  }

  /** Copy the current selection to the native clipboard. No-op if empty. */
  async function copySelection() {
    const screen = host.screen();
    const selAnchor = host.selAnchor();
    const selFocus = host.selFocus();
    if (!screen || !selAnchor || !selFocus) return;
    const text = screen.selectionText(selAnchor, selFocus);
    if (text === '') return;
    const r = await nativeWriteText(text);
    if (!r.ok) host.setOpenError(`Copy failed: ${r.error.message}`);
  }

  /** Paste the native clipboard into the PTY (bracketed-paste framing happens
   *  in sendPaste). Shared by Cmd+V and the context-menu Paste item. */
  async function pasteFromClipboard() {
    const r = await nativeReadText();
    if (r.ok) sendPaste(r.value);
    else host.setOpenError(`Paste failed: ${r.error.message}`);
  }

  return { sendPaste, copySelection, pasteFromClipboard };
}
