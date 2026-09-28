// Proposed settings changes (declarative pages P5, layout L6, design §5):
// an agent proposes over the control API (`set_setting { propose: true }`),
// a person applies or rejects here. Nothing is written until then; an
// applied change is audited with its proposal (`setting_history`).
import { writable, get } from 'svelte/store';
import { invokeCmd, type Result } from '../result';
import { loadFleetSettings } from '../fleet_settings';
import { optionLabel, toDisplay, UNIT_WORDS, type Descriptor } from './pages';

export interface SettingProposal {
  id: number;
  at: number;
  key: string;
  /** The value it would store. */
  value: string;
  /** The value when it was proposed. */
  before: string;
  /** The value now: it may have moved since. */
  current: string;
  why?: string;
  source: 'agent' | 'person' | 'system';
  source_detail?: string;
  state: 'pending';
}

export interface Decided {
  applied: number[];
  rejected: number[];
  failed: { id: number; error: string }[];
}

export interface SettingAudit {
  id: number;
  at: number;
  key: string;
  /** Absent when the key was unset (its default applied). */
  before?: string | null;
  after: string;
  actor: 'person' | 'agent' | 'system';
  actor_detail?: string;
  proposal_id?: number;
}

/** Pending proposals, oldest first. */
export const settingProposals = writable<SettingProposal[]>([]);

export async function loadProposals(): Promise<Result<SettingProposal[]>> {
  const r = await invokeCmd<SettingProposal[]>('setting_proposals');
  if (r.ok && r.value) settingProposals.set(r.value);
  return r;
}

/** Apply `accept`, reject `reject`; then re-read the proposals and, when
 *  something was applied, the values. */
export async function decideProposals(accept: number[], reject: number[]): Promise<Result<Decided>> {
  const r = await invokeCmd<Decided>('decide_setting_proposals', { accept, reject });
  if (r.ok) {
    const done = new Set([...r.value.applied, ...r.value.rejected]);
    settingProposals.set(get(settingProposals).filter((p) => !done.has(p.id)));
    if (r.value.applied.length > 0) await loadFleetSettings();
    void loadProposals();
  }
  return r;
}

export function settingHistory(key: string, limit?: number): Promise<Result<SettingAudit[]>> {
  return invokeCmd<SettingAudit[]>('setting_history', { key, limit: limit ?? null });
}

/** A setting's value in words, as a person reads it: On / Off, an option's
 *  label, a number in its unit, "(empty)". */
export function valueInWords(d: Descriptor | undefined, value: string): string {
  if (value === '') return '(empty)';
  if (!d) return value;
  if (d.kind.type === 'bool') return value === 'true' ? 'On' : 'Off';
  if (d.kind.type === 'choice') return optionLabel(d, value);
  if (d.kind.type === 'choice_set') return value.split(',').map((v) => optionLabel(d, v)).join(', ');
  if (d.kind.type === 'secs' || d.kind.type === 'int') {
    const unit = UNIT_WORDS[d.unit];
    return `${toDisplay(d, value)}${unit ? ` ${unit}` : ''}`;
  }
  return value;
}

/** Who proposed or wrote it, in a few words. */
export function whoWords(kind: string, detail?: string | null): string {
  const base = kind === 'agent' ? 'an agent' : kind === 'person' ? 'a person' : 'fleet';
  return detail ? `${base} (${detail})` : base;
}
