// Redesign step 5.8: Share warns when the person's device is read-only.
// A readonly device token can read but never send a prompt, so a drive share
// to someone whose every paired device is readonly gives them nothing more
// than watch until they pair a full one.
//
// G7.11 (Sharing and presence board): "Peter can only read until you trust
// his iPhone · Trust now". Trusting here makes the device `full`
// (`update_device` mode), which is what lets it send prompts; it stays as
// trusted or untrusted for settings as it was.
import type { DeviceSummary } from './devices';
import { invokeCmd, type Result } from './result';

/** The warning for sharing at drive with `person`, or null when they have a
 *  full device, no device this client can see, or no name typed yet. */
export function readOnlyRecipient(person: string, devices: readonly DeviceSummary[]): string | null {
  return limitedRecipient(person, devices, 'drive')?.message ?? null;
}

/** What a share at `level` cannot reach on `person`'s devices: the line to
 *  show and the devices "Trust now" would make full. Null when one of their
 *  devices can act at that level, none is visible, or no name is typed. */
export function limitedRecipient(
  person: string,
  devices: readonly DeviceSummary[],
  level: 'watch' | 'answer' | 'drive' | string,
): { message: string; devices: DeviceSummary[] } | null {
  const name = person.trim().toLowerCase();
  if (!name || (level !== 'drive' && level !== 'answer')) return null;
  const theirs = devices.filter((d) => (d.person ?? '').toLowerCase() === name);
  if (theirs.length === 0) return null;
  // Only a full device sends prompts; an answer-only one (M15 G2.10) answers.
  const able = (d: DeviceSummary) => d.mode === 'full' || (level === 'answer' && d.mode === 'answer');
  if (theirs.some(able)) return null;
  const who = theirs[0].person ?? person.trim();
  const what = level === 'drive' ? 'not send it prompts' : 'not answer its questions';
  const message =
    theirs.length === 1
      ? `${who} can only read until you trust their ${theirs[0].name}: it is read-only, so they can watch this session but ${what}.`
      : `${who} can only read until you trust one of their ${theirs.length} devices: all are read-only, so they can watch this session but ${what}.`;
  return { message, devices: theirs };
}

/** "Trust now": make `device` a full device, so it can send prompts. */
export function trustDeviceForShare(device: string): Promise<Result<DeviceSummary>> {
  return invokeCmd<DeviceSummary>('update_device', { args: { device, mode: 'full' } });
}
