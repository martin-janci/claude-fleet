// Start rules (redesign 8.11, "From AI to rule"): a task key pattern
// (`PD-*`) that names the repository, and optionally the host, a start of a
// matching task lands in. A rule decides before the key's history and
// before Jev, so it costs no call. After five identical starts fleet offers
// one on the start popover; the rules are listed and edited in Automation.
// The backend is `service::start_rules`, behind the `start_rules` command
// (standalone) or the hub's `start_rules` tool (paired).
import { invokeCmd, type Result } from './result';

/** `counting` rows are fleet's own tally and never reach the UI. */
export type StartRuleState = 'offered' | 'active' | 'dismissed';

export interface StartRule {
  id: number;
  org_id?: number | null;
  owner_person_id?: number | null;
  pattern: string;
  project_id: number;
  /** `null` = the project's last host. */
  host_alias?: string | null;
  state: StartRuleState | string;
  confirmations?: number;
  /** Starts the rule decided. */
  hits?: number;
  last_hit_at?: number | null;
  created_at: number;
  updated_at: number;
}

/** One rule as `list` and every change answer it. */
export interface StartRuleView extends StartRule {
  /** `owner/repo` of its project. */
  project?: string | null;
  /** Whether this person may change it: the buttons, not a fence. */
  may_change?: boolean;
}

export interface StartRuleInput {
  pattern: string;
  project_id: number;
  host_alias?: string | null;
  /** Only on a new rule: the org whose tasks it decides for. */
  org_id?: number | null;
}

function call<T>(args: { action: string; rule_id?: number; rule?: StartRuleInput }): Promise<Result<T>> {
  return invokeCmd<T>('start_rules', { args });
}

export const listStartRules = () => call<StartRuleView[]>({ action: 'list' });
export const saveStartRule = (rule: StartRuleInput, ruleId?: number) =>
  call<StartRuleView>({ action: 'save', rule: { ...rule, host_alias: rule.host_alias || null }, ...(ruleId != null ? { rule_id: ruleId } : {}) });
export const acceptStartRule = (ruleId: number) => call<StartRuleView>({ action: 'accept', rule_id: ruleId });
export const dismissStartRule = (ruleId: number) => call<StartRuleView>({ action: 'dismiss', rule_id: ruleId });
export const deleteStartRule = (ruleId: number) => call<{ removed: boolean }>({ action: 'delete', rule_id: ruleId });

/** The repository a rule names, as the line reads: its `owner/repo`, else
 *  the matching project from `projects`, else `project N`. */
export function ruleProject(rule: StartRule & { project?: string | null }, projects: { id: number; owner: string; repo: string }[] = []): string {
  if (rule.project) return rule.project;
  const p = projects.find((x) => x.id === rule.project_id);
  return p ? `${p.owner}/${p.repo}` : `project ${rule.project_id}`;
}

/** "PD-* → acme/pos", with " on mac" when the rule names a host. */
export function ruleLine(rule: StartRule & { project?: string | null }, projects: { id: number; owner: string; repo: string }[] = []): string {
  const host = rule.host_alias ? ` on ${rule.host_alias}` : '';
  return `${rule.pattern} → ${ruleProject(rule, projects)}${host}`;
}

/** Mirrors `service::start_rules::check_pattern`: `null` when `pattern` is
 *  one, else why not. The backend checks again. */
export function patternProblem(pattern: string): string | null {
  const p = pattern.trim();
  if (!p) return 'A rule needs a pattern, like PD-*.';
  if ([...p].length > 64) return 'A pattern is at most 64 characters.';
  if (!/^[A-Za-z0-9\-_.:/#*]+$/.test(p)) return 'A pattern holds letters, digits, - _ . : / # and *.';
  if (!/[^*]/.test(p)) return 'A pattern needs a character besides *: it would match every task.';
  return null;
}
