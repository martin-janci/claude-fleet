// Org administration phase C: an org at or over its budget, on the desktop.
//
// `fleet_health.org_budgets` (`health_check` here; the hub's `fleet_health`
// when paired) lists the orgs whose estimated spend reached a daily or
// monthly budget. Each raises ONE Attention item, "Acme over its daily
// budget", which opens Settings → Organisations. Fleet only warns: no
// session is stopped. Absent for a caller that does not see every session.
import { writable } from 'svelte/store';

/** `service::org_spend::OrgBudgetAlert`. */
export interface OrgBudgetAlert {
  org_id: number;
  org: string;
  period: 'daily' | 'monthly';
  spent_micros: number;
  budget_micros: number;
}

/** The latest list the desktop has read. */
export const orgBudgets = writable<OrgBudgetAlert[]>([]);

/** The Settings page an item opens. */
export const ORGS_PAGE = 'settings.orgs';

export interface OrgBudgetItem {
  key: string;
  label: string;
  /** Tooltip: spent against the budget. */
  detail: string;
  page: typeof ORGS_PAGE;
}

const usd = (micros: number) => `$${(micros / 1_000_000).toFixed(2)}`;

/** One item per org and period over budget. */
export function orgBudgetItems(alerts: OrgBudgetAlert[] | null | undefined): OrgBudgetItem[] {
  return (alerts ?? []).map((a) => ({
    key: `${a.org_id}:${a.period}`,
    label: `${a.org} over its ${a.period} budget`,
    detail: `${usd(a.spent_micros)} spent ${a.period === 'daily' ? 'today' : 'this month'} of a ${usd(a.budget_micros)} budget · fleet only warns · Open Settings → Organisations`,
    page: ORGS_PAGE,
  }));
}
