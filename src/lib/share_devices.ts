// Redesign step 5.8: Share warns when the person's device is read-only.
// A readonly device token can read but never send a prompt, so a drive share
// to someone whose every paired device is readonly gives them nothing more
// than watch until they pair a full one.
import type { DeviceSummary } from './devices';

/** The warning for sharing at drive with `person`, or null when they have a
 *  full device, no device this client can see, or no name typed yet. */
export function readOnlyRecipient(person: string, devices: readonly DeviceSummary[]): string | null {
  const name = person.trim().toLowerCase();
  if (!name) return null;
  const theirs = devices.filter((d) => (d.person ?? '').toLowerCase() === name);
  // Only a full device sends prompts: an answer-only one (M15 G2.10) answers.
  if (theirs.length === 0 || theirs.some((d) => d.mode === 'full')) return null;
  const who = theirs[0].person ?? person.trim();
  return theirs.length === 1
    ? `${who}'s only device (${theirs[0].name}) is read-only, so they can watch this session but not send it prompts.`
    : `All ${theirs.length} of ${who}'s devices are read-only, so they can watch this session but not send it prompts.`;
}
