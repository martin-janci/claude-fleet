// Settings › Company (Organisations, Devices, People, Groups): what THIS
// device may change on one fleet, and the resource pages that follow.
//
// The hub's `org_admin` tool (`crates/fleet-core/src/mcp/tools/fleet.rs`,
// `service/org_admin.rs`) decides two things, and this module reads the hub's
// own answers to both rather than the fleet-wide `settingsWritable`:
//   - WHAT the device administers (`org_admin::authority_for`): the whole
//     company from a fleet admin's device bound to no org — exactly the
//     callers `setting_proposals` (`Access::PersonDevice`) answers — or one
//     org from the device of that org's admin, fenced to it. A device with
//     neither gets `E_FORBIDDEN` from `list_devices`.
//   - WHETHER it may change anything (`org_admin_writer`): a trusted `full`
//     device, which `list_devices` reports on its own row (`this_device`).
// The hub stays the judge of each write (an org admin's rules, a person's
// name, the device in hand); a gate here only keeps the page from offering
// what the hub would refuse whole, and keeps its read-only sentence true.
import { PRIMARY } from './fleets';
import { errorText } from './error_copy';
import { invokeCmd, type IpcError, type Result } from './result';
import type { DeviceSummary } from './devices';
import type { ResourceRecord, ResourceType } from './pages/resources';

/** What this device may do on one fleet's Company pages. */
export interface CompanyAccess {
  /** What it administers: the whole company, one org, or nothing. */
  admin: 'fleet' | 'org' | null;
  /** It is a trusted full device: its administration may change things. */
  write: boolean;
  /** A read that did not get through (unreachable, timed out): the access
   *  is unknown, not "none". */
  error?: string;
}

/** Everything: this app owns its fleet (standalone, its own master). */
export const OWN_COMPANY: CompanyAccess = { admin: 'fleet', write: true };
/** Nothing known yet. */
export const NO_COMPANY: CompanyAccess = { admin: null, write: false };

/** The Settings pages this module gates, by page id. */
export const COMPANY_PAGES: readonly string[] = ['settings.orgs', 'settings.devices', 'settings.people', 'settings.groups'];

/**
 * The access from the hub's two answers: whether `setting_proposals` was
 * served (a fleet admin's unbound device) and `list_devices` (null when it
 * refused: the device administers nothing).
 */
export function companyAccessFrom(fleetAdmin: boolean, devices: DeviceSummary[] | null): CompanyAccess {
  if (devices === null) return NO_COMPANY;
  const me = devices.find((d) => d.this_device);
  return { admin: fleetAdmin ? 'fleet' : 'org', write: !!me && me.trusted && me.mode === 'full' };
}

/** Refusals that ARE the hub's answer (a hub older than the command, or one
 *  that will not tell this device); anything else did not get through. */
const ANSWERED_NO = new Set(['E_HUB_PROTOCOL', 'E_INVALID', 'E_FORBIDDEN']);
const unread = (r: Result<unknown>): IpcError | null => (!r.ok && !ANSWERED_NO.has(r.error.code) ? r.error : null);

/**
 * Read one fleet's Company access: the primary when `fleet` is the primary
 * slot (or null), else that extra fleet, routed by `fleet`. A refusal counts
 * as "no"; a read that did not get through is "no" too, but carries `error`.
 */
export async function loadCompanyAccess(fleet: string | null): Promise<CompanyAccess> {
  const args = fleet && fleet !== PRIMARY ? { fleet } : {};
  const [pending, devices] = await Promise.all([
    invokeCmd<{ can_write: boolean }>('setting_proposals', args),
    invokeCmd<DeviceSummary[]>('list_devices', args),
  ]);
  const access = companyAccessFrom(pending.ok, devices.ok && Array.isArray(devices.value) ? devices.value : null);
  const failed = unread(devices) ?? unread(pending);
  return failed ? { ...access, error: errorText(failed) } : access;
}

/** Whether this device may change one listed record of `resource`. */
export function companyRecordWritable(access: CompanyAccess, resourceId: string, record: ResourceRecord): boolean {
  if (!access.write || !access.admin) return false;
  if (access.admin === 'fleet') return true;
  // An org admin: their own org (the hub lists each org with the caller's
  // role), the devices the hub lists them (their org's), no person's name.
  if (resourceId === 'org') return record.my_role === 'admin';
  if (resourceId === 'device') return true;
  return false;
}

/**
 * A Company resource as this device may use it: no Add and no Remove where
 * the hub would refuse them whole — adding or removing an org, adding or
 * disabling a person are a fleet admin's; pairing a device needs a trusted
 * device that administers something.
 */
export function companyResourceFor(resource: ResourceType, access: CompanyAccess): ResourceType {
  const writes = access.write && access.admin !== null;
  const fleetWide = writes && access.admin === 'fleet';
  if (resource.id === 'device') return writes ? resource : { ...resource, create: undefined, delete: undefined };
  if (resource.id === 'org' || resource.id === 'person')
    return fleetWide ? resource : { ...resource, create: undefined, delete: undefined };
  return resource;
}

/**
 * Why some or all of a Company page cannot be changed from here, in a
 * person's words, or null when nothing is held back. `where` names the hub
 * and `clientName` the device, for the operator's one command.
 */
export function companyReadonlyReason(access: CompanyAccess, where: string, clientName: string | null): string | null {
  if (access.error) return `Could not read what this device may change on ${where}: ${access.error}. Until then nothing here can be changed. Try again.`;
  if (!access.admin)
    return `This device administers no organisation on ${where}, so it reads what it is shown and changes nothing here. A fleet admin’s device bound to no org, or an org admin’s device, does.`;
  if (!access.write)
    return `This device may read the company’s orgs, devices and people on ${where}; to change them, its operator trusts it (a full device): fleet-hub client trust ${clientName ?? '<name>'}.`;
  if (access.admin === 'org')
    return `As an org admin on ${where}, this device changes its own org, its members and their devices. Adding or removing orgs, their rules, people’s names and disabling someone stay a fleet admin’s.`;
  return null;
}
