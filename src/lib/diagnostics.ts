// Settings → Diagnostics: copy a redacted plain-text bundle (version, schema,
// hosts, tunnels, MCP state, session counts, last log lines) for bug reports,
// and open the log folder. The bundle is built and redacted on the backend
// (`service::diagnostics`); it never contains a token.
import { invokeCmd, type Result } from './result';
import { nativeWriteText } from './clipboard_native';
import { push, pushError } from './toasts';

/** Mirrors the backend `DiagnosticsBundle` (`service/diagnostics.rs`). */
export interface DiagnosticsBundle {
  /** The redacted report, ready to paste into an issue. */
  text: string;
  /** Directory holding the rotated log files. */
  log_dir: string;
  /** The log file currently being written, or null before the first line. */
  log_file: string | null;
}

export async function collectDiagnostics(): Promise<Result<DiagnosticsBundle>> {
  return invokeCmd<DiagnosticsBundle>('collect_diagnostics');
}

/** Open the log folder in the OS file manager; resolves to its path. */
export async function openLogFolder(): Promise<Result<string>> {
  return invokeCmd<string>('open_log_folder');
}

/**
 * Collect the bundle and put it on the clipboard, reporting the outcome as a
 * toast. Returns the bundle on success (so the caller can show the log
 * folder path), or null when collecting or copying failed.
 */
export async function copyDiagnostics(): Promise<DiagnosticsBundle | null> {
  const r = await collectDiagnostics();
  if (!r.ok) {
    pushError(r.error, 'Collect diagnostics failed');
    return null;
  }
  const w = await nativeWriteText(r.value.text);
  if (!w.ok) {
    pushError(w.error, 'Copy diagnostics failed');
    return null;
  }
  push({ kind: 'success', message: 'Diagnostics copied to clipboard' });
  return r.value;
}
