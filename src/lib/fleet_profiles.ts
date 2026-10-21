// One human, several fleet identities (multi-hub design, Phase 3b:
// docs/superpowers/specs/2026-10-17-desktop-multi-hub-design.md).
//
// The same person can be a DIFFERENT person on each hub this window shows —
// deliberately: a work identity on the company hub, a private one on the
// personal hub, each with its own GitHub token, e-mail and sign-in. This
// module says, per fleet, who this device is there:
//
//   - the hub's answer to `my_profile` (the caller's own person: name,
//     display name, e-mail, linked sign-in issuers, the NAMES of its
//     person-scope variables, its git identity) — read from that fleet's
//     own hub, routed by `fleet` like every fleet-scoped read;
//   - a hub older than `my_profile` refuses it (`E_HUB_PROTOCOL` for a
//     master token, `E_FORBIDDEN … is not a client-callable tool` for a
//     paired device, whose guard fails closed on a name it does not know);
//     then the profile is pieced together from `my_grants` (who) and `env_vars_list`
//     (the person's own variable names), with no name or sign-in;
//   - a LABEL ("Work", "Private") that is this desktop's alone, kept in its
//     own prefs keyed by fleet slot id. Nothing about one fleet — its label,
//     its profile — is ever sent to another hub.
//
// Values never cross: a variable is listed by name, and only the git
// identity's name and e-mail are shown (plain values the person set
// themselves, which `env_vars list` already shows them).
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type IpcError } from './result';
import { PRIMARY, fleets, onFleetForgotten, type FleetInfo } from './fleets';
import { readPref, writePref } from './prefs';

/** One `env_vars_list` row. A secret or a reference is write-only: a row
 *  carries `value` for a plain entry only. */
export interface EnvVarRow {
  id: number;
  /** global < host < project < person < session (the narrower wins). */
  scope: 'global' | 'host' | 'project' | 'person' | 'session';
  /** '' for global, a host alias, owner/repo, a person id, or a session id. */
  scope_key: string;
  name: string;
  /** Present for a plain, non-reference entry only. */
  value?: string;
  secret: boolean;
  is_ref: boolean;
  updated_at: number;
}

/** `my_profile`'s answer (`fleet_core::service::my_profile::MyProfile`). */
export interface MyProfile {
  person_id: number | null;
  name?: string | null;
  display_name?: string | null;
  email?: string | null;
  is_owner?: boolean;
  sso?: { issuer: string; last_login_at?: number | null }[];
  env?: { name: string; secret: boolean; is_ref: boolean }[];
  git?: { name?: string | null; email?: string | null } | null;
}

/** One fleet's profile as this window knows it. */
export interface FleetProfileState {
  profile: MyProfile | null;
  /** `hub`: `my_profile`; `fallback`: an older hub, pieced together. */
  source: 'hub' | 'fallback' | null;
  error: string | null;
  /** The fallback only: `env_vars_list` failed, so the variables (and the
   *  git identity) are unknown, not unset. */
  envError?: string | null;
}

/** The variables a profile row calls out by name when set. */
export const KEY_VARS = ['GH_TOKEN', 'GIT_AUTHOR_NAME', 'GIT_AUTHOR_EMAIL', 'JIRA_EMAIL', 'JIRA_API_TOKEN'] as const;

// ── the local label ─────────────────────────────────────────────────────

const LABELS_PREF = 'fleets.profileLabels';
const isLabels = (v: unknown): v is Record<string, string> =>
  typeof v === 'object' && v !== null && Object.values(v).every((x) => typeof x === 'string');
const LABEL_MAX = 32;

const labelsStore = writable<Record<string, string>>(readPref(LABELS_PREF, {}, isLabels));
/** Fleet slot id → the label this person gave that profile here. */
export const profileLabels = { subscribe: labelsStore.subscribe };

/** Name a fleet's profile ("Work"); an empty label goes back to the fleet's. */
export function setProfileLabel(fleet: string, label: string): void {
  const t = label.trim().slice(0, LABEL_MAX);
  labelsStore.update((m) => {
    const next = { ...m };
    if (t) next[fleet] = t;
    else delete next[fleet];
    writePref(LABELS_PREF, next);
    return next;
  });
}

/** A profile's label: the one given here, else the fleet's own label. */
export function profileLabelOf(f: Pick<FleetInfo, 'id' | 'label'>, labels: Readonly<Record<string, string>>): string {
  return labels[f.id] || f.label;
}

// ── the hub's answer ────────────────────────────────────────────────────

const profilesStore = writable<Map<string, FleetProfileState>>(new Map());
/** Fleet slot id → its profile. */
export const fleetProfiles = { subscribe: profilesStore.subscribe };

const fleetArg = (fleet: string) => (fleet === PRIMARY ? {} : { fleet });

/** A hub older than the tool refuses it: a master token reaches rmcp's
 *  unknown-tool path (`E_HUB_PROTOCOL`); a paired device is stopped first by
 *  the guard, which fails closed on a tool it cannot classify. Only that
 *  exact refusal counts — any other `E_FORBIDDEN` is a real refusal. */
export function isOlderHub(e: IpcError): boolean {
  if (e.code === 'E_HUB_PROTOCOL') return true;
  return e.code === 'E_FORBIDDEN' && (e.message ?? '').includes('my_profile is not a client-callable tool');
}

/** The fallback for a hub without `my_profile`: who (my_grants) and the
 *  person's own variable names (env_vars_list), nothing else. */
export function profileFromParts(personId: number | null, rows: readonly EnvVarRow[]): MyProfile {
  if (personId == null) return { person_id: null };
  const mine = rows.filter((r) => r.scope === 'person' && r.scope_key === String(personId));
  const plain = (n: string) => {
    const r = mine.find((x) => x.name === n && !x.secret && !x.is_ref);
    return r?.value ?? null;
  };
  const git = { name: plain('GIT_AUTHOR_NAME'), email: plain('GIT_AUTHOR_EMAIL') };
  return {
    person_id: personId,
    env: mine.map((r) => ({ name: r.name, secret: r.secret, is_ref: r.is_ref })),
    git: git.name || git.email ? git : null,
  };
}

const errText = (e: IpcError) => e.message || e.code;

/** Fleet slot id → the newest read's number: a read applies its answer only
 *  while it is still the newest for its fleet, so a slow stale answer never
 *  overwrites a newer one, nor brings back a forgotten fleet's entry. */
const loadSeq = new Map<string, number>();

/** Read one fleet's profile from its own hub. */
export async function loadProfile(fleet: string): Promise<FleetProfileState> {
  const seq = (loadSeq.get(fleet) ?? 0) + 1;
  loadSeq.set(fleet, seq);
  const r = await invokeCmd<MyProfile>('my_profile', fleetArg(fleet));
  let state: FleetProfileState;
  if (r.ok && r.value && typeof r.value === 'object') {
    state = { profile: r.value, source: 'hub', error: null };
  } else if (!r.ok && isOlderHub(r.error)) {
    const [who, env] = await Promise.all([
      invokeCmd<{ person_id: number | null }>('my_grants', fleetArg(fleet)),
      invokeCmd<EnvVarRow[]>('env_vars_list', fleetArg(fleet)),
    ]);
    if (!who.ok) state = { profile: null, source: null, error: errText(who.error) };
    else {
      const rows = env.ok && Array.isArray(env.value) ? env.value : null;
      const profile = profileFromParts(who.value?.person_id ?? null, rows ?? []);
      // Unknown is not unset: without the rows, say nothing about variables.
      if (!rows) delete profile.env;
      state = {
        profile,
        source: 'fallback',
        error: null,
        envError: rows ? null : env.ok ? 'no answer' : errText(env.error),
      };
    }
  } else {
    state = { profile: null, source: null, error: r.ok ? 'no answer' : errText(r.error) };
  }
  if (loadSeq.get(fleet) === seq) profilesStore.update((m) => new Map(m).set(fleet, state));
  return state;
}

/** Forget a removed fleet's label and profile: slot ids are reused by label,
 *  so another hub added later under the same slot must not inherit them. */
export function forgetProfile(fleet: string): void {
  loadSeq.set(fleet, (loadSeq.get(fleet) ?? 0) + 1);
  profilesStore.update((m) => {
    if (!m.has(fleet)) return m;
    const next = new Map(m);
    next.delete(fleet);
    return next;
  });
  if (fleet in get(labelsStore)) setProfileLabel(fleet, '');
}
onFleetForgotten(forgetProfile);

/** Read every usable fleet's profile (Settings, the switcher). */
export async function loadProfiles(): Promise<void> {
  const list = get(fleets).filter((f) => !f.unavailable && !f.duplicate_of);
  await Promise.all(list.map((f) => loadProfile(f.id)));
}

// ── in words ────────────────────────────────────────────────────────────

/** Who this device is on that fleet: `Martin Janči (martin)`, `person #4`,
 *  or null when it speaks for nobody there. */
export function profilePerson(p: MyProfile | null | undefined): string | null {
  if (!p || p.person_id == null) return null;
  const shown = p.display_name || p.name;
  if (!shown) return `person #${p.person_id}`;
  return p.display_name && p.name && p.display_name !== p.name ? `${p.display_name} (${p.name})` : shown;
}

/** A sign-in issuer as a short host: `sso.papayapos.dev/realms/company`. */
export function issuerShort(issuer: string): string {
  try {
    const u = new URL(issuer);
    return `${u.host}${u.pathname === '/' ? '' : u.pathname}`;
  } catch {
    return issuer;
  }
}

/** The one line a chip's tooltip and New session show:
 *  `Work · Martin · martin@papayapos.dev`. */
export function profileLine(label: string, p: MyProfile | null | undefined): string {
  const parts = [label];
  const who = profilePerson(p);
  if (who) parts.push(who);
  const mail = p?.email || p?.git?.email;
  if (mail) parts.push(mail);
  if (p && p.person_id == null) parts.push('no person on this hub');
  return parts.join(' · ');
}

/** The key variables, set or not, then any other set name. */
export function envChecklist(p: MyProfile | null | undefined): { name: string; set: boolean; secret: boolean }[] {
  const env = p?.env ?? [];
  const key = KEY_VARS.map((name) => {
    const e = env.find((x) => x.name === name);
    return { name, set: !!e, secret: !!e && (e.secret || e.is_ref) };
  });
  const rest = env
    .filter((e) => !(KEY_VARS as readonly string[]).includes(e.name))
    .map((e) => ({ name: e.name, set: true, secret: e.secret || e.is_ref }));
  return [...key, ...rest];
}

/** Per fleet: its label and one-line profile, for the switcher's chips and
 *  New session's "Runs as". A failed read says so rather than showing the
 *  bare label as if it were the whole answer. */
export const profileLines = derived([fleets, profileLabels, fleetProfiles], ([$fleets, $labels, $profiles]) => {
  const out = new Map<string, { label: string; line: string; custom: boolean }>();
  for (const f of $fleets) {
    const label = profileLabelOf(f, $labels);
    const st = $profiles.get(f.id);
    const line = st?.error ? `${label} · could not read the profile` : profileLine(label, st?.profile);
    out.set(f.id, { label, line, custom: !!$labels[f.id] });
  }
  return out;
});

/** Test seam. */
export function _resetProfilesForTest(): void {
  profilesStore.set(new Map());
  labelsStore.set({});
  loadSeq.clear();
}
