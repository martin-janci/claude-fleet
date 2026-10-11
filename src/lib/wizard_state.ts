// Wizards that resume on another device (gap plan G7.2): a wizard saves its
// step and answers on the hub (`wizard_state`), and the person's other
// device offers "Resume" where they left it. Standalone, the rows are this
// app's own. Answers never hold a secret: the backend refuses one.
import { invokeCmd, type Result } from './result';
import { ago } from './assets_workspace';

export type WizardKind = 'add_host' | 'add_project' | 'add_account' | 'new_session' | 'link_peer' | 'form';

export interface WizardState {
  kind: WizardKind | string;
  key: string;
  person_id?: number | null;
  /** What the resume line names ("acme/api"). */
  label?: string | null;
  step: number;
  answers: Record<string, unknown>;
  checks?: unknown[];
  /** The device that saved it last; absent = the hub's own desktop. */
  device?: string | null;
  created_at: number;
  updated_at: number;
}

type Args = { action: string; kind?: string; key?: string; step?: number; answers?: Record<string, unknown>; label?: string };

function call<T>(args: Args): Promise<Result<T>> {
  return invokeCmd<T>('wizard_state', { args });
}

export const listWizards = (kind?: WizardKind) => call<WizardState[]>({ action: 'list', ...(kind ? { kind } : {}) });
export const getWizard = (kind: WizardKind, key?: string) => call<WizardState | null>({ action: 'get', kind, ...(key ? { key } : {}) });
export const saveWizard = (kind: WizardKind, step: number, answers: Record<string, unknown>, opts: { key?: string; label?: string } = {}) =>
  // `null` when a hub before contract 17 kept nothing.
  call<WizardState | null>({
    action: 'save',
    kind,
    step,
    answers,
    ...(opts.key ? { key: opts.key } : {}),
    ...(opts.label ? { label: opts.label } : {}),
  });
export const clearWizard = (kind: WizardKind, key?: string) => call<{ removed: boolean }>({ action: 'clear', kind, ...(key ? { key } : {}) });

/** "on Ada's Pixel 5 min ago" (or "here 5 min ago" for this app's own). */
export function startedWhere(w: Pick<WizardState, 'device' | 'updated_at'>, nowSecs: number): string {
  const where = w.device ? `on ${w.device}` : 'here';
  return `${where} ${ago(w.updated_at, nowSecs)}`;
}
