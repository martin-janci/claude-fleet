// Settings › Environment: what THIS device may change on one fleet, and the
// resource page that follows from it (session env, person scope).
//
// The hub's `env_vars` tool (`crates/fleet-core/src/mcp/tools/env_vars.rs`)
// draws two lines, and this module mirrors them rather than the fleet-wide
// `settingsWritable`:
//   - the `person` scope is each person's own: any paired device that speaks
//     for a person lists, sets and unsets its own person's variables, trusted
//     or not;
//   - the shared scopes (global, host, project, session) need a fleet admin's
//     trusted full device — the same trust `setting_proposals.can_write`
//     reports.
// The hub stays the judge: a gate here only keeps the page from offering
// what the hub would refuse, and keeps its read-only sentence true.
import { PRIMARY } from './fleets';
import { errorText } from './error_copy';
import { invokeCmd, type IpcError, type Result } from './result';
import type { ResourceRecord, ResourceType } from './pages/resources';

/** What this device may change on one fleet's Environment page. */
export interface EnvAccess {
  /** Set and remove its own person's variables (scope `person`). */
  person: boolean;
  /** Set and remove the global, host and project ones. */
  shared: boolean;
  /** Set when a read failed without the hub answering (unreachable, timed
   *  out): the access is unknown, not "none", so the page says the read
   *  failed rather than asking the operator for trust. */
  error?: string;
}

/** Everything: this app owns its fleet (standalone, its own master). */
export const OWN_FLEET: EnvAccess = { person: true, shared: true };
/** Nothing known yet, or a fleet that answered neither read. */
export const NO_ACCESS: EnvAccess = { person: false, shared: false };

/**
 * The access a paired device has, from the two answers the hub gives it:
 * `my_grants.person_id` (who the connection speaks for) and
 * `setting_proposals.can_write` (the hub's operator trusts it).
 */
export function envAccessFrom(personId: number | null | undefined, canWrite: boolean): EnvAccess {
  return { person: typeof personId === 'number', shared: canWrite };
}

/** Refusals that ARE the hub's answer: a hub older than the command
 *  (`E_HUB_PROTOCOL`, `E_INVALID`) or one that will not tell this device
 *  (`E_FORBIDDEN`). Any other failure is a read that did not get through. */
const ANSWERED_NO = new Set(['E_HUB_PROTOCOL', 'E_INVALID', 'E_FORBIDDEN']);

const unread = (r: Result<unknown>): IpcError | null => (!r.ok && !ANSWERED_NO.has(r.error.code) ? r.error : null);

/**
 * Read one fleet's access: the primary when `fleet` is the primary slot (or
 * null), else that extra fleet, routed by `fleet` like every fleet-scoped
 * command. A refusal counts as "no": a hub older than either command gets
 * the read-only page, never a form it would refuse. A read that did not get
 * through (the hub unreachable, timed out) is "no" too, but carries `error`.
 */
export async function loadEnvAccess(fleet: string | null): Promise<EnvAccess> {
  const args = fleet && fleet !== PRIMARY ? { fleet } : {};
  const [who, pending] = await Promise.all([
    invokeCmd<{ person_id: number | null }>('my_grants', args),
    invokeCmd<{ can_write: boolean }>('setting_proposals', args),
  ]);
  const access = envAccessFrom(who.ok ? (who.value?.person_id ?? null) : null, pending.ok && pending.value?.can_write === true);
  const failed = unread(who) ?? unread(pending);
  return failed ? { ...access, error: errorText(failed) } : access;
}

/** Whether this device may change (remove) one listed variable. */
export function envRecordWritable(access: EnvAccess, record: ResourceRecord): boolean {
  return record.scope === 'person' ? access.person : access.shared;
}

/**
 * The Environment resource as this device may use it: Add offers only the
 * scopes it may write ("Me (my sessions)" alone for a person's untrusted
 * device, which then needs no "Host or owner/repo"), and with neither scope
 * there is no Add and no Remove at all.
 */
export function envResourceFor(resource: ResourceType, access: EnvAccess): ResourceType {
  if (!access.person && !access.shared) return { ...resource, create: undefined, delete: undefined };
  const c = resource.create;
  if (!c || (access.person && access.shared)) return resource;
  const params = c.params
    .map((p) =>
      p.name === 'scope' && p.type === 'choice'
        ? { ...p, options: p.options.filter(([v]) => (v === 'person' ? access.person : access.shared)) }
        : p,
    )
    // The person scope's key is always the caller's own: nothing to ask.
    .filter((p) => !(p.name === 'scope_key' && !access.shared));
  const bind = c.bind.filter(([arg]) => !(arg === 'scope_key' && !access.shared));
  return { ...resource, create: { ...c, params, bind } };
}

/**
 * Why some or all of the page cannot be changed from here, in a person's
 * words, or null when nothing is held back. `where` names the hub and
 * `clientName` the device, for the operator's one command.
 */
export function envReadonlyReason(access: EnvAccess, where: string, clientName: string | null): string | null {
  if (access.person && access.shared) return null;
  if (access.error) {
    const until = access.person ? 'Until then only your own variables (Me) can be changed here.' : 'Until then nothing here can be changed.';
    return `Could not read what this device may change on ${where}: ${access.error}. ${until} Try again.`;
  }
  const trust = `fleet-hub client trust ${clientName ?? '<name>'}`;
  if (access.person)
    return `Your own variables (Me) are yours to change from this device. The global, host and project ones are ${where}’s: to change them here, its operator trusts this device: ${trust}.`;
  if (access.shared)
    return `This device speaks for no person on ${where}, so it has no variables of its own there; it changes the shared ones.`;
  return `This device speaks for no person on ${where}, so it has no variables of its own there, and changing the shared ones needs its operator to trust it: ${trust}.`;
}

/** The person-scope variables the Git & tokens form writes. */
export interface GitIdentity {
  name: string;
  email: string;
  ghToken: string;
  jiraEmail: string;
  jiraToken: string;
}

/**
 * The `env_var_set` calls one Git & tokens submit makes, in order: the name
 * and email as both author and committer, then each token that was typed
 * (secret). A blank field sets nothing — it never clears what is there.
 */
export function gitIdentityWrites(g: GitIdentity): { name: string; value: string; secret: boolean }[] {
  const out: { name: string; value: string; secret: boolean }[] = [];
  const name = g.name.trim();
  const email = g.email.trim();
  if (name) out.push({ name: 'GIT_AUTHOR_NAME', value: name, secret: false }, { name: 'GIT_COMMITTER_NAME', value: name, secret: false });
  if (email)
    out.push({ name: 'GIT_AUTHOR_EMAIL', value: email, secret: false }, { name: 'GIT_COMMITTER_EMAIL', value: email, secret: false });
  if (g.ghToken.trim()) out.push({ name: 'GH_TOKEN', value: g.ghToken.trim(), secret: true });
  if (g.jiraEmail.trim()) out.push({ name: 'JIRA_EMAIL', value: g.jiraEmail.trim(), secret: false });
  if (g.jiraToken.trim()) out.push({ name: 'JIRA_API_TOKEN', value: g.jiraToken.trim(), secret: true });
  return out;
}

/**
 * Write `writes` as this device's own person variables on `fleet`, one
 * `env_var_set` each, stopping at the first refusal. Answers how many were
 * set and the refusal, if any.
 */
export async function setPersonVars(
  fleet: string | null,
  writes: { name: string; value: string; secret: boolean }[],
): Promise<{ set: number; error: string | null }> {
  const where = fleet && fleet !== PRIMARY ? { fleet } : {};
  let set = 0;
  for (const w of writes) {
    const r = await invokeCmd('env_var_set', {
      args: { scope: 'person', scope_key: null, name: w.name, value: w.value, secret: w.secret },
      ...where,
    });
    if (!r.ok) return { set, error: r.error.message };
    set += 1;
  }
  return { set, error: null };
}
