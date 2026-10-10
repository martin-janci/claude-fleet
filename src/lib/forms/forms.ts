// Chat forms: an agent's `ask` opens a fleet.form/1 form in its session's
// chat; a person answers or declines here. The backend checks everything
// again (crates/fleet-core/src/pages/forms.rs); this file only carries it.
import { invokeCmd, type Result } from '../result';

export type FieldType = 'text' | 'textarea' | 'number' | 'bool' | 'select' | 'multiselect' | 'secret';

export interface FieldCondition {
  field?: string;
  eq?: unknown;
  in?: unknown[];
  truthy?: boolean;
  all?: FieldCondition[];
  any?: FieldCondition[];
  not?: FieldCondition;
}

export interface FormField {
  name: string;
  type: FieldType;
  label: string;
  help?: string;
  required?: boolean;
  value?: unknown;
  when?: FieldCondition;
  placeholder?: string;
  max_len?: number;
  min?: number;
  max?: number;
  integer?: boolean;
  options?: FormOption[];
  /** select: offer "Another…", a free entry beside the options. */
  other?: boolean;
  /** Shown, not answerable, with this reason under it. */
  disabled_reason?: string;
  /** The default `value` was drafted by an AI. */
  drafted?: { by: string; from: string };
  /** secret: where the value goes, under the field. */
  secret_note?: string;
}

/** Who proposes an option (forms.rs `ProposedBy`). */
export type OptionProposer = 'rule' | 'jev' | 'llm';

/** `{value, label, detail?, proposed?}` (forms.rs `OptionSpec`). */
export interface OptionSpec {
  value: string;
  label: string;
  detail?: string;
  proposed?: { by: OptionProposer; reason: string };
}

/** An option: the original `[value, label]` pair, or an object. */
export type FormOption = [string, string] | OptionSpec;

/** One option read the same whichever shape it was written in. */
export function readOption(o: FormOption): OptionSpec {
  return Array.isArray(o) ? { value: o[0], label: o[1] } : o;
}

/** A field's options, each read as an object. */
export function optionsOfField(f: { options?: FormOption[] }): OptionSpec[] {
  return (f.options ?? []).map(readOption);
}

export interface FormStep {
  title: string;
  /** The word on the step chip; the title when absent. */
  name?: string;
  /** `review`: a summary of the earlier steps, with no fields. */
  kind?: 'fields' | 'review';
  intro?: string;
  when?: FieldCondition;
  fields: FormField[];
}

export interface FormSpec {
  spec: 'fleet.form/1';
  title: string;
  intro?: string;
  submit?: string;
  /** Offer "Save and finish later". */
  save_later?: boolean;
  steps: FormStep[];
  /** What the answers need from a host, warned about before sending (G7.4). */
  checks?: HostCheck[];
}

/** One thing the answers need from a host (forms.rs `HostCheck`). */
export interface HostCheck {
  /** What needs it, in words: "Postgres". */
  label: string;
  needs: 'disk_free_gb' | 'mem_free_gb';
  /** The least it needs, in GB. */
  at_least: number;
  /** The select field whose answer is the host; absent: the session's. */
  host_field?: string;
  when?: FieldCondition;
}

export type FormState = 'pending' | 'answered' | 'declined' | 'cancelled' | 'expired';

export interface FormView {
  form_id: string;
  session_id: number;
  host_alias: string;
  title: string;
  spec: FormSpec;
  why?: string | null;
  state: FormState;
  answers?: Record<string, unknown> | null;
  secrets?: Record<string, string> | null;
  note?: string | null;
  answered_by?: string | null;
  created_at: number;
  decided_at?: number | null;
  proposal?: FormProposal | null;
}

/** Jev's likely option for a pending form's first choice (J5, step 10.9):
 *  the field and the option's value. Absent when there is none. */
export interface FormProposal {
  field: string;
  value: string;
  source: 'jev' | 'rule' | 'llm';
  confidence_pct?: number | null;
  run_id?: number | null;
}

export interface FieldProblem {
  field: string;
  problem: string;
}

export type Values = Record<string, unknown>;

export function listForms(sessionId?: number, state?: FormState): Promise<Result<FormView[]>> {
  return invokeCmd<FormView[]>('list_forms', { sessionId: sessionId ?? null, state: state ?? null });
}

export function getForm(formId: string): Promise<Result<FormView>> {
  return invokeCmd<FormView>('get_form', { formId });
}

export function answerForm(formId: string, values: Values): Promise<Result<FormView>> {
  return invokeCmd<FormView>('answer_form', { formId, values });
}

export function declineForm(formId: string, note?: string): Promise<Result<FormView>> {
  return invokeCmd<FormView>('decline_form', { formId, note: note?.trim() ? note.trim() : null });
}
