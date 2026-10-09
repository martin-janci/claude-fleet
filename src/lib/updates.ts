// This desktop's own update (update-channel design S7, §11).
//
// The backend decides and verifies (`update_check`: the hub when paired, the
// published channel when standalone) and installs (`update_install`: only
// the target it verified itself, never a URL from here). This module asks
// at start and then on the decision's own interval, and turns the answer
// into the one banner the window shows.
import { writable, get } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import { invokeCmd, type Result } from './result';

/** Mirror of `self_update::DesktopUpdate`. */
export interface DesktopUpdate {
  status: string;
  mode: string;
  source: string;
  installed: string;
  reason: string;
  version: string | null;
  mandatory: boolean;
  deadline: string | null;
  /** `in_place` (Restart to update), `download` (a person installs it), `none`. */
  install: 'in_place' | 'download' | 'none';
  download_url: string | null;
  notes_url: string | null;
  next_check_secs: number;
}

export const desktopUpdate = writable<DesktopUpdate | null>(null);
/** `downloaded / total` bytes while installing, else null. */
export const updateProgress = writable<{ downloaded: number; total: number | null } | null>(null);
export const updateError = writable<string | null>(null);

export interface Banner {
  tone: 'info' | 'warn';
  text: string;
  /** `restart` (install in place), `download` (open the link), or none. */
  action: 'restart' | 'download' | null;
  dismissible: boolean;
}

/**
 * PURE: what the window says about an update, or null for nothing.
 *
 * `update_available` is an offer the person may dismiss for the session;
 * `update_required` (this hub refuses this build, or a mandatory release is
 * past its deadline) is not dismissible. `client_too_new` names the hub as
 * the thing to update. Everything else (`up_to_date`, `hold`, `unknown`)
 * says nothing.
 */
export function bannerFor(u: DesktopUpdate | null, dismissed: string | null): Banner | null {
  if (!u) return null;
  const action = u.install === 'in_place' ? 'restart' : u.install === 'download' ? 'download' : null;
  const how = action === 'restart' ? ' — restart to update' : action === 'download' ? ' — download it to update' : '';
  switch (u.status) {
    case 'update_available':
      if (!u.version || dismissed === u.version) return null;
      return {
        tone: u.mandatory ? 'warn' : 'info',
        text: u.mandatory
          ? `claude-fleet ${u.version} is required${u.deadline ? ` by ${u.deadline.slice(0, 10)}` : ''}${how}.`
          : `claude-fleet ${u.version} is available${how}.`,
        action,
        dismissible: !u.mandatory,
      };
    case 'update_required':
      return {
        tone: 'warn',
        text: u.version
          ? `Update required: ${u.reason} claude-fleet ${u.version}${how}.`
          : `Update required: ${u.reason} No release this platform can install fits yet.`,
        action: u.version ? action : null,
        dismissible: false,
      };
    case 'client_too_new':
      return { tone: 'warn', text: u.reason, action: null, dismissible: false };
    default:
      return null;
  }
}

export async function checkForUpdate(): Promise<Result<DesktopUpdate>> {
  const r = await invokeCmd<DesktopUpdate>('update_check');
  if (r.ok) {
    desktopUpdate.set(r.value);
    updateError.set(null);
  }
  return r;
}

export async function installUpdate(): Promise<Result<void>> {
  updateError.set(null);
  updateProgress.set({ downloaded: 0, total: null });
  const r = await invokeCmd<void>('update_install');
  // On success the app restarts and nothing after this runs.
  if (!r.ok) {
    updateProgress.set(null);
    updateError.set(r.error.message);
  }
  return r;
}

/** Between checks: the decision's own interval, within an hour and a day. */
export function nextCheckMs(u: DesktopUpdate | null): number {
  const secs = u?.next_check_secs ?? 21600;
  return Math.min(Math.max(secs, 3600), 86400) * 1000;
}

/**
 * Check after start-up has settled, then on the decision's interval.
 * `automatic` installs the first available in-place update it finds right
 * after a launch — the quiet point, before the person has started work in
 * this window. Returns a stop function.
 */
export function startUpdateChecks(firstDelayMs = 15_000): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;
  let first = true;
  const unlisten = listen<{ downloaded: number; total: number | null }>('update:progress', (e) =>
    updateProgress.set(e.payload),
  );
  const run = async () => {
    if (stopped) return;
    const r = await checkForUpdate();
    const u = r.ok ? r.value : get(desktopUpdate);
    if (first && r.ok && u && u.mode === 'automatic' && u.status === 'update_available' && u.install === 'in_place') {
      void installUpdate();
    }
    first = false;
    if (!stopped) timer = setTimeout(run, nextCheckMs(u));
  };
  timer = setTimeout(run, firstDelayMs);
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    void unlisten.then((f) => f());
  };
}
