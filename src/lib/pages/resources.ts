// Resources on generated pages (declarative pages P4): the types
// `crates/fleet-core/src/pages/resources.rs` serialises, and the pure
// helpers a `master_detail` page uses — read a field, encode an edit, build
// an action's arguments, say a sub-item, list the badges. Actions run the
// existing desktop commands they name, so every hub verdict applies as is.
import { invokeCmd, type Result } from '../result';
import { loadOrgs, ruleChip, type OrgRuleRow } from '../orgs';
import { loadTrackers } from '../trackers';
import { loadCatalogStatuses } from '../assets_workspace';
import { loadDevices } from '../devices';

export type Bind =
  | { from: 'record'; name: string }
  | { from: 'item' }
  | { from: 'item_field'; name: string }
  | { from: 'param'; name: string }
  | { from: 'null' }
  | { from: 'true' }
  | { from: 'false' };

export type OptionSource = 'hosts' | 'trackers' | 'orgs' | 'devices' | 'catalogs';

export type ParamSpec = { name: string; label: string; required: boolean } & (
  | { type: 'text'; max: number; placeholder: string }
  | { type: 'color' }
  | { type: 'secret' }
  | { type: 'options'; source: OptionSource }
  | { type: 'choice'; options: [string, string][] }
);

/** How an action's answer is shown (`ResultView`). */
export type ResultView = 'pairing' | 'output' | 'image';

export interface ActionSpec {
  id: string;
  label: string;
  command: string;
  envelope: 'args';
  bind: [string, Bind][];
  params: ParamSpec[];
  confirm?: string;
  /** Only for records whose `variant_by` field is one of these. */
  variants?: string[];
  /** The command answers `{ ok, error? }`. */
  report: boolean;
  /** Its answer is shown by this formatter. */
  result?: ResultView;
  /** The loader beside the form while it runs. */
  busy?: 'counter-orbit';
}

export type ItemLabel =
  | { type: 'plain' }
  | { type: 'field'; field: string }
  | { type: 'org_rule' }
  | { type: 'device' }
  | { type: 'member' }
  | { type: 'admin_need' }
  | { type: 'person_spend' };

/** A tile's line under its value (`Sub`). */
export type Sub = { type: 'budget'; field: string } | { type: 'count'; field: string; text: string };

export type Badge =
  | { when: 'true'; text: string }
  | { when: 'false'; text: string }
  | { when: 'set'; text: string }
  | { when: 'label' };

export type FieldKind =
  | { type: 'text'; max: number }
  | { type: 'color' }
  | { type: 'bool'; on_off: boolean; default: boolean }
  | { type: 'inherit' }
  | { type: 'choice'; options: [string, string][] }
  | { type: 'time' }
  | { type: 'count' }
  | { type: 'money' }
  | { type: 'money_series' }
  | { type: 'sync'; unit: string }
  | { type: 'settings'; set: ActionSpec }
  | { type: 'items'; item_label: ItemLabel; remove?: ActionSpec; add: ActionSpec[] };

export type FieldSpec = {
  id: string;
  label: string;
  help: string;
  edit?: string;
  badge?: Badge;
  confirm?: string;
  /** The value lives at this path inside the record's `edit` object. */
  merge_path?: string[];
  /** Its line under the value in a `tiles` section. */
  sub?: Sub;
} & FieldKind;

export interface ResourceType {
  id: string;
  label: string;
  plural: string;
  help: string;
  list: string;
  id_field: string;
  title_field: string;
  color_field?: string;
  empty: string;
  fields: FieldSpec[];
  create?: ActionSpec;
  update?: ActionSpec;
  delete?: ActionSpec;
  actions?: ActionSpec[];
  create_flow?: string;
  variant_by?: string;
}

export type ResourceRecord = Record<string, unknown>;

/** The value a scalar field edits: a string for text and colour, a boolean
 *  for on/off, `'on' | 'off' | 'inherit'` for an inherit field. */
export type FieldValue = string | boolean;

/** A field's raw value: its own key, or its path inside the `edit` object. */
export function rawOf(f: FieldSpec, record: ResourceRecord): unknown {
  if (!f.merge_path?.length || !f.edit) return record[f.id];
  let v: unknown = record[f.edit];
  for (const k of f.merge_path) v = v && typeof v === 'object' ? (v as Record<string, unknown>)[k] : undefined;
  return v;
}

export function fieldValue(f: FieldSpec, record: ResourceRecord): FieldValue {
  const raw = rawOf(f, record);
  switch (f.type) {
    case 'bool':
      return typeof raw === 'boolean' ? raw : f.default;
    case 'inherit':
      return raw === null || raw === undefined ? 'inherit' : raw ? 'on' : 'off';
    default:
      return raw === null || raw === undefined ? '' : String(raw);
  }
}

/** What the update command takes for a field's new value. */
export function encodeField(f: FieldSpec, v: FieldValue): unknown {
  if (f.type === 'bool') return f.on_off ? (v ? 'on' : 'off') : v === true;
  return v;
}

/**
 * Apply's arguments for the changed fields: each under its `edit` name, or
 * — for a merged field — the record's whole `edit` object with every
 * changed path set, the rest kept as it is.
 */
export function updateArgs(
  record: ResourceRecord,
  changed: { f: FieldSpec; v: FieldValue }[],
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const { f, v } of changed) {
    if (!f.edit) continue;
    if (!f.merge_path?.length) {
      out[f.edit] = encodeField(f, v);
      continue;
    }
    const base = (out[f.edit] ?? JSON.parse(JSON.stringify(record[f.edit] ?? {}))) as Record<string, unknown>;
    let at = base;
    f.merge_path.forEach((k, i) => {
      if (i === f.merge_path!.length - 1) at[k] = encodeField(f, v);
      else {
        if (!at[k] || typeof at[k] !== 'object') at[k] = {};
        at = at[k] as Record<string, unknown>;
      }
    });
    out[f.edit] = base;
  }
  return out;
}

/** A choice's label for its value. */
export function choiceLabel(f: FieldSpec, v: string): string {
  return f.type === 'choice' ? (f.options.find(([o]) => o === v)?.[1] ?? v) : v;
}

/** Micro-USD as dollars: `$12.34`, `$0.00`. */
export function dollars(micros: unknown): string {
  const n = typeof micros === 'number' ? micros : 0;
  return `$${(n / 1_000_000).toFixed(2)}`;
}

/** Unix seconds as how long ago, from `now`. */
export function ago(secs: unknown, now: number): string {
  if (typeof secs !== 'number') return 'never';
  const d = Math.max(0, now - secs);
  if (d < 60) return 'just now';
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86_400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86_400)} d ago`;
}

/** The action applies to this record: no variants, or its variant is one. */
export function applies(r: ResourceType, a: ActionSpec, record: ResourceRecord): boolean {
  if (!a.variants?.length || !r.variant_by) return true;
  return a.variants.includes(String(record[r.variant_by] ?? ''));
}

/** Record fields as the strings a `when` compares. */
export function recordValues(r: ResourceType, record: ResourceRecord): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of r.fields) {
    if (f.type === 'items' || f.type === 'settings') continue;
    out[f.id] = String(fieldValue(f, record));
  }
  return out;
}

export function itemsOf(f: FieldSpec, record: ResourceRecord): unknown[] {
  const v = record[f.id];
  return Array.isArray(v) ? v : [];
}

export function itemLabel(label: ItemLabel, item: unknown): string {
  if (label.type === 'plain') return String(item);
  if (label.type === 'org_rule') return ruleChip(item as OrgRuleRow);
  if (label.type === 'device') return deviceChip(item as DeviceItem);
  if (label.type === 'admin_need') return needLine(item as AdminNeed).text;
  if (label.type === 'person_spend') return personSpendName(item as PersonSpend);
  if (label.type === 'member') {
    const m = item as { name?: string; display_name?: string; role?: string };
    return `${m.display_name || m.name || ''} · ${m.role ?? ''}`;
  }
  const v = (item as Record<string, unknown> | null)?.[label.field];
  return v === undefined || v === null ? '' : String(v);
}

/** One person's share of an org's spend (`service::org_spend::PersonSpend`);
 *  no `person_id` is nobody's. */
export interface PersonSpend {
  person_id?: number;
  name?: string;
  today_micros: number;
  week_micros: number;
  month_micros: number;
}

export function personSpendName(p: PersonSpend): string {
  return p.person_id === undefined || p.person_id === null ? 'Routines, missions and unclaimed' : (p.name ?? `person ${p.person_id}`);
}

/** One of an org's "Needs an admin" (`service::org_needs::AdminNeed`). */
export type AdminNeed =
  | { kind: 'budget'; period: 'daily' | 'monthly'; spent_micros: number; budget_micros: number; pace_day?: number }
  | { kind: 'untrusted_device'; device: string; paired_at: number }
  | { kind: 'unclaimed_sessions'; host: string; count: number };

const ORDINAL = (n: number) => {
  const t = n % 100;
  if (t >= 11 && t <= 13) return `${n}th`;
  return `${n}${['th', 'st', 'nd', 'rd'][n % 10] ?? 'th'}`;
};

/** What a need is, in a line, and why or when, in a second; `warn` for a
 *  budget reached or a device that may prompt, else `info`. */
export function needLine(
  n: AdminNeed,
  now = Math.floor(Date.now() / 1000),
): { text: string; detail: string; tone: 'warn' | 'info' } {
  switch (n.kind) {
    case 'budget': {
      const which = n.period === 'daily' ? 'Daily' : 'Monthly';
      const pct = n.budget_micros > 0 ? Math.floor((n.spent_micros * 100) / n.budget_micros) : 0;
      const reached = n.spent_micros >= n.budget_micros;
      return {
        text: reached ? `${which} budget reached` : `${which} budget at ${pct}%`,
        detail: reached
          ? `${dollars(n.spent_micros)} of ${dollars(n.budget_micros)} · Fleet warns, it never stops a session`
          : n.pace_day
            ? `at this pace it is reached on the ${ORDINAL(n.pace_day)}`
            : `${dollars(n.spent_micros)} of ${dollars(n.budget_micros)}`,
        tone: reached ? 'warn' : 'info',
      };
    }
    case 'untrusted_device':
      return {
        text: `${n.device} is not trusted yet`,
        detail: `paired ${ago(n.paired_at, now)} · what it types reaches agents marked until it is trusted in Settings → Devices`,
        tone: 'warn',
      };
    case 'unclaimed_sessions':
      return {
        text: `${n.count} unclaimed ${n.count === 1 ? 'session' : 'sessions'} on ${n.host}`,
        detail: 'nobody has claimed them; shown to the admins who may see the count',
        tone: 'info',
      };
  }
}

/** A tile's line under its value, or `''`. */
/** A `sync` field's value: a transfer in progress (11.12). */
export interface SyncProgress {
  done: number;
  total: number;
  both_ways: boolean;
  /** Unix seconds the transfer last moved. */
  since?: number;
}

/** 1280 → "1 280": the loaders' count reads in groups of three. */
export function grouped(n: number): string {
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
}

/** The Constellation's line: "412 of 1 280 messages · 18 s". */
export function syncLine(unit: string, s: SyncProgress, now: number): string {
  const head = `${grouped(s.done)} of ${grouped(s.total)} ${unit}`;
  if (s.since === undefined) return head;
  const secs = Math.max(0, now - s.since);
  return `${head} · ${secs < 60 ? `${secs} s` : `${Math.floor(secs / 60)} min`}`;
}

export function subLine(sub: Sub | undefined, value: unknown, record: ResourceRecord): string {
  if (!sub) return '';
  const n = record[sub.field];
  if (sub.type === 'count') return typeof n === 'number' && n > 0 ? `${n} ${sub.text}` : '';
  if (typeof n !== 'number' || n <= 0) return '';
  const spent = typeof value === 'number' ? value : 0;
  const budget = `$${n.toLocaleString('en-US')}`;
  return spent > 0 ? `${Math.floor((spent * 100) / (n * 1_000_000))}% of ${budget}` : `of ${budget} budget`;
}

/** A budget tile's Meter (the OrgOverview board): the share of the budget
 *  spent, warn from 80% (`NEAR_BUDGET_PCT`, as "Needs an admin" lists it)
 *  and crit once reached. `null` with no budget, or for a count's line. */
export function budgetMeter(
  sub: Sub | undefined,
  value: unknown,
  record: ResourceRecord,
): { value: number; level: 'ok' | 'warn' | 'crit' } | null {
  if (sub?.type !== 'budget') return null;
  const n = record[sub.field];
  if (typeof n !== 'number' || n <= 0) return null;
  const share = (typeof value === 'number' ? value : 0) / (n * 1_000_000);
  return { value: share, level: share >= 1 ? 'crit' : share >= 0.8 ? 'warn' : 'ok' };
}

/** A field's label with the record in it: `{title}` is the one placeholder
 *  (`FieldSpec::label`), so a consent names its org ("Allow Jev (decision
 *  model) for Acme's work"). */
export function labelOf(f: { label: string }, title: string): string {
  return f.label.replaceAll('{title}', title || 'this record');
}

/** The days a spend series went over a daily budget (whole USD), as the
 *  line under the chart says it: "2 Oct went over: $71.00". Empty with no
 *  budget. */
export function overBudgetDays(series: { day: string; cost_micros: number }[], budgetUsd: unknown): string[] {
  if (typeof budgetUsd !== 'number' || budgetUsd <= 0) return [];
  const limit = budgetUsd * 1_000_000;
  return series
    .filter((p) => p.cost_micros > limit)
    .map((p) => {
      const d = new Date(`${p.day}T00:00:00Z`);
      const day = Number.isNaN(d.getTime())
        ? p.day
        : `${d.getUTCDate()} ${d.toLocaleString('en-GB', { month: 'short', timeZone: 'UTC' })}`;
      return `${day} went over: ${dollars(p.cost_micros)}`;
    });
}

/** A paired device in an org's `devices` (`OrgDevice`). */
export interface DeviceItem {
  name: string;
  mode: string;
  trusted: boolean;
  last_seen_at?: number;
}

function deviceChip(d: DeviceItem): string {
  const tags = [d.mode === 'readonly' ? 'read-only' : null, d.trusted ? 'trusted' : null].filter(Boolean);
  return tags.length ? `${d.name} · ${tags.join(', ')}` : d.name;
}

/** A stable key for a sub-item: its `id`, else its `name`, else the item
 *  itself. */
export function itemKey(item: unknown): string {
  if (item && typeof item === 'object' && 'id' in item) return String((item as { id: unknown }).id);
  if (item && typeof item === 'object' && 'name' in item) return String((item as { name: unknown }).name);
  return String(item);
}

/** The value an option-source select picks for an item already in the
 *  list, so the select can leave it out. */
export function itemValue(item: unknown): string {
  return itemKey(item);
}

export function badgesOf(r: ResourceType, record: ResourceRecord): string[] {
  const out: string[] = [];
  for (const f of r.fields) {
    if (!f.badge) continue;
    const v = fieldValue(f, record);
    if (f.badge.when === 'true' && v === true) out.push(f.badge.text);
    else if (f.badge.when === 'false' && v === false) out.push(f.badge.text);
    else if (f.badge.when === 'set' && v !== 'inherit') out.push(`${f.badge.text} ${v}`);
    else if (f.badge.when === 'label' && v !== '') out.push(choiceLabel(f, String(v)));
  }
  return out;
}

export function titleOf(r: ResourceType, record: ResourceRecord): string {
  return String(record[r.title_field] ?? '');
}

export function idOf(r: ResourceType, record: ResourceRecord): string {
  return String(record[r.id_field] ?? '');
}

/**
 * The arguments `action` sends, from the record it runs on, the sub-item
 * (for a list's remove) and the form's values. An empty optional param is
 * sent as null; an options param holding a number-like id is sent as a
 * number, as the commands take it.
 */
export function buildArgs(
  action: ActionSpec,
  record: ResourceRecord | null,
  item: unknown,
  params: Record<string, string>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [arg, bind] of action.bind) {
    switch (bind.from) {
      case 'record':
        out[arg] = record?.[bind.name] ?? null;
        break;
      case 'item':
        out[arg] = item;
        break;
      case 'item_field':
        out[arg] = (item as Record<string, unknown> | null)?.[bind.name] ?? null;
        break;
      case 'param': {
        const spec = action.params.find((p) => p.name === bind.name);
        const v = (params[bind.name] ?? '').trim();
        if (v === '') out[arg] = null;
        else if (spec?.type === 'options' && /^\d+$/.test(v)) out[arg] = Number(v);
        else out[arg] = v;
        break;
      }
      case 'null':
        out[arg] = null;
        break;
      case 'true':
        out[arg] = true;
        break;
      case 'false':
        out[arg] = false;
        break;
    }
  }
  return out;
}

/** A param's value as the form holds it: what was typed or picked, else
 *  a choice's first option (preselected). */
export function paramValue(p: ParamSpec, params: Record<string, string>): string {
  return params[p.name] ?? (p.type === 'choice' ? (p.options[0]?.[0] ?? '') : '');
}

/** The form can be sent: every required param has a value. */
export function formReady(action: ActionSpec, params: Record<string, string>): boolean {
  return action.params.every((p) => !p.required || paramValue(p, params).trim() !== '');
}

/** What the form sends: its values, with each choice's preselected option. */
export function formValues(action: ActionSpec, params: Record<string, string>): Record<string, string> {
  return Object.fromEntries(action.params.map((p) => [p.name, paramValue(p, params)]));
}

export function runAction(action: ActionSpec, args: Record<string, unknown>): Promise<Result<unknown>> {
  return invokeCmd<unknown>(action.command, { args });
}

export function listRecords(r: ResourceType): Promise<Result<ResourceRecord[]>> {
  return invokeCmd<ResourceRecord[]>(r.list);
}

/**
 * The stores other views keep of a resource, re-read after a change here
 * so the sidebar's scopes and colours follow. Frontend glue, a closed map
 * like the custom components.
 */
export const RESOURCE_RELOADERS: Record<string, (() => Promise<unknown>)[]> = {
  org: [loadOrgs, loadTrackers, loadDevices],
  tracker: [loadTrackers, loadOrgs],
  catalog: [loadCatalogStatuses, loadOrgs],
  device: [loadDevices, loadOrgs, loadCatalogStatuses],
  person: [loadDevices],
};

export async function afterChange(r: ResourceType): Promise<void> {
  await Promise.all((RESOURCE_RELOADERS[r.id] ?? []).map((f) => f()));
}
