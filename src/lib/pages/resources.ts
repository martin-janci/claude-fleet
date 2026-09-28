// Resources on generated pages (declarative pages P4): the types
// `crates/fleet-core/src/pages/resources.rs` serialises, and the pure
// helpers a `master_detail` page uses — read a field, encode an edit, build
// an action's arguments, say a sub-item, list the badges. Actions run the
// existing desktop commands they name, so every hub verdict applies as is.
import { invokeCmd, type Result } from '../result';
import { loadOrgs, ruleChip, type OrgRuleRow } from '../orgs';
import { loadTrackers } from '../trackers';

export type Bind =
  | { from: 'record'; name: string }
  | { from: 'item' }
  | { from: 'item_field'; name: string }
  | { from: 'param'; name: string }
  | { from: 'null' };

export type OptionSource = 'hosts' | 'trackers';

export type ParamSpec = { name: string; label: string; required: boolean } & (
  | { type: 'text'; max: number; placeholder: string }
  | { type: 'color' }
  | { type: 'options'; source: OptionSource }
);

export interface ActionSpec {
  id: string;
  label: string;
  command: string;
  envelope: 'args';
  bind: [string, Bind][];
  params: ParamSpec[];
  confirm?: string;
}

export type ItemLabel = { type: 'plain' } | { type: 'field'; field: string } | { type: 'org_rule' };

export type Badge =
  | { when: 'true'; text: string }
  | { when: 'false'; text: string }
  | { when: 'set'; text: string };

export type FieldKind =
  | { type: 'text'; max: number }
  | { type: 'color' }
  | { type: 'bool'; on_off: boolean; default: boolean }
  | { type: 'inherit' }
  | { type: 'items'; item_label: ItemLabel; remove?: ActionSpec; add: ActionSpec[] };

export type FieldSpec = {
  id: string;
  label: string;
  help: string;
  edit?: string;
  badge?: Badge;
  confirm?: string;
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
}

export type ResourceRecord = Record<string, unknown>;

/** The value a scalar field edits: a string for text and colour, a boolean
 *  for on/off, `'on' | 'off' | 'inherit'` for an inherit field. */
export type FieldValue = string | boolean;

export function fieldValue(f: FieldSpec, record: ResourceRecord): FieldValue {
  const raw = record[f.id];
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

/** Record fields as the strings a `when` compares. */
export function recordValues(r: ResourceType, record: ResourceRecord): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of r.fields) {
    if (f.type === 'items') continue;
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
  const v = (item as Record<string, unknown> | null)?.[label.field];
  return v === undefined || v === null ? '' : String(v);
}

/** A stable key for a sub-item: its `id`, else the item itself. */
export function itemKey(item: unknown): string {
  if (item && typeof item === 'object' && 'id' in item) return String((item as { id: unknown }).id);
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
    }
  }
  return out;
}

/** The form can be sent: every required param has a value. */
export function formReady(action: ActionSpec, params: Record<string, string>): boolean {
  return action.params.every((p) => !p.required || (params[p.name] ?? '').trim() !== '');
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
  org: [loadOrgs, loadTrackers],
};

export async function afterChange(r: ResourceType): Promise<void> {
  await Promise.all((RESOURCE_RELOADERS[r.id] ?? []).map((f) => f()));
}
