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
  options?: [string, string][];
}

export interface FormStep {
  title: string;
  intro?: string;
  when?: FieldCondition;
  fields: FormField[];
}

export interface FormSpec {
  spec: 'fleet.form/1';
  title: string;
  intro?: string;
  submit?: string;
  steps: FormStep[];
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
