// Whose version is on the screen.
//
// The footer's one version line used to read `v0.4.5 · db: ok · schema 90`
// in both modes, and in remote mode every number in it was the HUB's:
// `health_check` routes to the hub's `fleet_health` (see `ipc.ts`), so the
// version belonged to the machine at the other end of the wire while looking
// exactly like this app's. The hub badge beside it was supposed to carry that
// distinction, which is a lot to ask of a badge that says only *which* hub.
//
// So this module holds the other half: this window's OWN version, and the
// pure rule that labels both — `app 0.4.5 · hub 0.4.6 · db: ok · schema 90`.
import { writable } from 'svelte/store';
import { getVersion } from '@tauri-apps/api/app';
import type { Health } from './ipc';

/**
 * This app's version, or null until it is read (and if the read fails).
 *
 * Deliberately NOT a field of `Health` or `HubStatus`: both of those answer
 * questions about the fleet, and in remote mode `Health` is the hub's answer
 * — the very confusion this exists to end. Tauri's `getVersion()` reads
 * `src-tauri/tauri.conf.json`, which `scripts/check-version-consistency.sh`
 * holds byte-identical to `src-tauri/Cargo.toml`, i.e. to the
 * `CARGO_PKG_VERSION` that `fleet_core::app_version::set` records and
 * `fleet_health` reports. One string, two readers, kept equal by CI.
 */
export const appVersion = writable<string | null>(null);

/**
 * Read this app's version once, at startup.
 *
 * Never throws and never toasts: a version is for a footer and a bug report,
 * and `versionLine` below says less rather than guessing when it is missing.
 */
export async function loadAppVersion(): Promise<void> {
  try {
    const v = (await getVersion()).trim();
    if (v !== '') appVersion.set(v);
  } catch {
    // Leave it null. The footer then names the hub's version only, which is
    // still unambiguous — it is labelled `hub`.
  }
}

/** The footer's version text and the sentence behind it. */
export interface VersionLine {
  text: string;
  title: string;
}

/** What `db:` says about whichever database the line is describing. */
function dbWord(ready: boolean): string {
  return ready ? 'db: ok' : 'db: fail';
}

/**
 * PURE: the footer's version line.
 *
 * `health` is the fleet in front of the reader — this app's own numbers
 * standalone, the hub's over the wire — so the labelling turns on `remote`
 * and on nothing else:
 *
 * - standalone, one owner: `app 0.4.5 · db: ok · schema 90`. `health.version`
 *   *is* this app's version here, so it stands in when {@link appVersion} has
 *   not been read yet and nothing reads as unknown.
 * - paired: `app 0.4.5 · hub 0.4.6 · db: ok · schema 90`, where the database
 *   and the schema are the hub's, like the version beside them.
 *
 * With no health at all the line is just this app's version, which is the one
 * fact still true when a hub cannot be reached.
 */
export function versionLine(v: {
  app: string | null;
  health: Pick<Health, 'version' | 'db_ready' | 'schema_version'> | null;
  remote: boolean;
  hubUrl: string | null;
}): VersionLine {
  const { app, health, remote } = v;
  const hub = v.hubUrl ?? 'the hub';

  if (!health) {
    return {
      text: app ? `app ${app}` : '',
      title: app
        ? `This app's version. Nothing is being said about a fleet: there is no health to report.`
        : '',
    };
  }

  if (!remote) {
    // Standalone the app IS the fleet, so one version covers both and
    // `health.version` is as good a source for it as Tauri's.
    const mine = app ?? health.version;
    return {
      text: `app ${mine} · ${dbWord(health.db_ready)} · schema ${health.schema_version}`,
      title: `This app owns the fleet: version ${mine}, its own database (schema ${health.schema_version}). No hub.`,
    };
  }

  const parts = [
    ...(app ? [`app ${app}`] : []),
    `hub ${health.version}`,
    dbWord(health.db_ready),
    `schema ${health.schema_version}`,
  ];
  // Say which is which, then say what a difference between them means —
  // because the obvious reading of two unequal numbers is "something is
  // broken", and what actually decides compatibility is the wire contract
  // (`hub_connection.ts`, which has its own banner when that is the problem).
  const whose = app
    ? `app ${app} is this window; hub ${health.version} is ${hub}, whose database (schema ${health.schema_version}) these are.`
    : `hub ${health.version} is ${hub}, whose database (schema ${health.schema_version}) these are. This app's own version could not be read.`;
  const drift =
    app && app !== health.version
      ? ' The two differ, which is allowed: the hub and its clients are released separately, and compatibility is decided by the wire contract rather than by matching versions.'
      : '';
  return { text: parts.join(' · '), title: whose + drift };
}
