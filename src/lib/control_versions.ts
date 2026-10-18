// Which versions Control is running against (review 2026-10-17, F0.2). On
// Oct 15 we guessed at the hub's version for hours; Control's header and the
// desktop's /health now say it: the hub's, this desktop's and the phone's
// (when a phone has reported one), and flag a hub older than this desktop —
// a fix the desktop shows may not be on the hub that runs Control yet.
import { get, writable } from 'svelte/store';
import { appVersion, loadAppVersion } from './app_version';
import { hubStatus } from './hub';
import { healthCheck } from './ipc';
import { compareVersions } from './hosts_view';
import { invokeCmd } from './result';

export interface ControlVersions {
  /** The hub's (`health.version`); this app's own when standalone. */
  hub: string | null;
  desktop: string | null;
  /** The newest a paired phone reported (`list_update_targets`), if any. */
  phone: string | null;
  /** Paired with a hub, rather than this desktop owning the fleet. */
  remote: boolean;
}

export const controlVersions = writable<ControlVersions>({ hub: null, desktop: null, phone: null, remote: false });

export interface VersionsSummary {
  text: string;
  title: string;
  /** The hub is older than this desktop. */
  hubBehind: boolean;
}

/** PURE: the header's line and its hover sentence. */
export function versionsSummary(v: ControlVersions): VersionsSummary | null {
  const parts: string[] = [];
  if (v.remote && v.hub) parts.push(`hub ${v.hub}`);
  if (v.desktop) parts.push(`desktop ${v.desktop}`);
  else if (!v.remote && v.hub) parts.push(`desktop ${v.hub}`);
  if (v.phone) parts.push(`phone ${v.phone}`);
  if (parts.length === 0) return null;
  const hubBehind = v.remote && !!v.hub && !!v.desktop && compareVersions(v.hub, v.desktop) < 0;
  const text = parts.join(' · ');
  const title = hubBehind
    ? `${text}. The hub that runs Control is older than this desktop: a fix this desktop has may not be on the hub yet. Update the hub.`
    : v.remote
      ? `${text}. Control runs on the hub.`
      : `${text}. This desktop runs the fleet and Control.`;
  return { text: hubBehind ? `${text} · hub behind` : text, title, hubBehind };
}

interface TargetRow {
  part: string;
  version: string;
  reported_at: number;
}

/** PURE: the newest version any phone reported. */
export function newestPhone(rows: readonly TargetRow[]): string | null {
  let best: string | null = null;
  for (const r of rows) {
    if (r.part !== 'Phone' || !r.version) continue;
    if (best === null || compareVersions(r.version, best) > 0) best = r.version;
  }
  return best;
}

/** Read the three versions; a failed read leaves that one unknown. */
export async function loadControlVersions(): Promise<void> {
  try {
    await loadAppVersion();
    const remote = get(hubStatus).remote;
    const h = await healthCheck();
    const t = await invokeCmd<TargetRow[]>('list_update_targets');
    controlVersions.set({
      hub: h.ok && h.value ? (h.value.version ?? null) : null,
      desktop: get(appVersion),
      phone: t.ok && Array.isArray(t.value) ? newestPhone(t.value) : null,
      remote,
    });
  } catch {
    // Versions are a courtesy: an unreadable one stays unknown.
  }
}
