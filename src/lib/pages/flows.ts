// Flows (declarative pages P4b, layout L3): the backend decides every step
// (`crates/fleet-core/src/pages/flows.rs`); this only carries the step and
// the values. A secret typed into a step is sent once and kept nowhere.
import { invokeCmd, type Result } from '../result';

export type StepField = {
  name: string;
  label: string;
  help?: string;
  value: string;
  required: boolean;
} & (
  | { type: 'text'; placeholder: string }
  | { type: 'secret' }
  | { type: 'textarea' }
  | { type: 'bool' }
  | { type: 'select'; options: [string, string][] }
);

export interface FlowStep {
  flow_id: string;
  flow: string;
  step: string;
  title: string;
  intro?: string;
  fields: StepField[];
  submit: string;
  error?: string;
  back: boolean;
}

export type FlowOutcome =
  | ({ state: 'step' } & FlowStep)
  | { state: 'done'; message: string; record_id?: number };

export function flowStart(flow: string, prefill: Record<string, string> = {}): Promise<Result<FlowStep>> {
  return invokeCmd<FlowStep>('flow_start', { flow, prefill });
}

export function flowSubmit(flowId: string, values: Record<string, string>): Promise<Result<FlowOutcome>> {
  return invokeCmd<FlowOutcome>('flow_submit', { flowId, values });
}

export function flowBack(flowId: string): Promise<Result<FlowStep>> {
  return invokeCmd<FlowStep>('flow_back', { flowId });
}

export function flowCancel(flowId: string): Promise<Result<void>> {
  return invokeCmd<void>('flow_cancel', { flowId });
}
