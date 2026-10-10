// The active-filter summary: one removable chip per filter that narrows
// the list, for the Sessions list and the Work view alike. Pure — strings
// and ids only — so both views (and fleet-mobile's *My work*, which mirrors
// `WorkTreeFilters`) phrase an active filter the same way, and the empty
// states can name what hides the rows.
//
// A facet's `id` is what its owner clears; the owner maps it to a store
// write (`clearSessionFacet` / `clearWorkFacet` below build the patches).

import type { Recency } from './session_status';
import {
  DEFAULT_WORK_FILTERS,
  HAS_SESSION_LABELS,
  isStatusNameFilter,
  STATUS_FILTER_LABELS as SESSION_STATUS_LABELS,
  type StatusCategoryFilter,
  type WorkFilters,
} from './work_filters';
import { STATUS_NAME_PREFIX } from './sidebar_index';
import {
  HAS_FILTER_LABELS,
  normalizeFilters,
  STATUS_FILTER_LABELS as WORK_STATUS_LABELS,
  WORK_STAGE_LABELS,
  type WorkTreeFilters,
} from './work_view';

export interface Facet {
  id: string;
  /** The chip's text ("Host: gpu-box"). */
  label: string;
}

// ── Sessions list ──

export type SessionFacetId =
  | 'scope'
  | 'host'
  | 'agent'
  | 'recency'
  | 'search'
  | 'needs-you'
  | 'bg'
  | 'wf-tracker'
  | 'wf-status'
  | 'wf-mine'
  | 'wf-session';

export interface SessionFacetInput {
  /** The effective scope id (`all` = none). */
  scope: string;
  scopeLabel?: string;
  /** The effective host filter (`all` = none). */
  host: string;
  /** The agent filter's label (`undefined` = any agent). */
  agent?: string;
  recency: Recency;
  search: string;
  needsYou: boolean;
  showBgAgents: boolean;
  /** The effective work filters (`effectiveWorkFilters`). */
  work: WorkFilters;
  trackerName?: (id: number) => string | undefined;
}

export function sessionFacets(i: SessionFacetInput): (Facet & { id: SessionFacetId })[] {
  const out: (Facet & { id: SessionFacetId })[] = [];
  if (i.needsYou) out.push({ id: 'needs-you', label: 'Needs you' });
  if (i.scope !== 'all') out.push({ id: 'scope', label: `Org: ${i.scopeLabel ?? i.scope}` });
  if (i.host !== 'all') out.push({ id: 'host', label: `Host: ${i.host}` });
  if (i.agent) out.push({ id: 'agent', label: `Agent: ${i.agent}` });
  if (i.recency !== 'all') out.push({ id: 'recency', label: `Last ${i.recency}` });
  const q = i.search.trim();
  if (q) out.push({ id: 'search', label: `Search: “${q}”` });
  const w = i.work;
  if (w.tracker !== 'all') out.push({ id: 'wf-tracker', label: `Tracker: ${i.trackerName?.(w.tracker) ?? `#${w.tracker}`}` });
  if (w.status !== 'all') {
    out.push({
      id: 'wf-status',
      label: isStatusNameFilter(w.status)
        ? `Column: ${w.status.slice(STATUS_NAME_PREFIX.length)}`
        : `Status: ${SESSION_STATUS_LABELS[w.status as StatusCategoryFilter]}`,
    });
  }
  if (w.assignee === 'mine') out.push({ id: 'wf-mine', label: 'Assigned to me' });
  if (w.hasSession !== 'any') out.push({ id: 'wf-session', label: `Session: ${HAS_SESSION_LABELS[w.hasSession]}` });
  if (!i.showBgAgents) out.push({ id: 'bg', label: 'Background agents hidden' });
  return out;
}

/** The work-filters patch that clears one work facet (`null` for a facet
 *  that is not a work filter). */
export function clearWorkFilterPatch(id: SessionFacetId): Partial<WorkFilters> | null {
  switch (id) {
    case 'wf-tracker':
      return { tracker: DEFAULT_WORK_FILTERS.tracker };
    case 'wf-status':
      return { status: DEFAULT_WORK_FILTERS.status };
    case 'wf-mine':
      return { assignee: DEFAULT_WORK_FILTERS.assignee };
    case 'wf-session':
      return { hasSession: DEFAULT_WORK_FILTERS.hasSession };
    default:
      return null;
  }
}

// ── Work view ──

export type WorkFacetId =
  | 'org'
  | 'orgs'
  | 'tracker'
  | 'status'
  | 'stages'
  | 'status_name'
  | 'mine'
  | 'assignee'
  | 'has'
  | 'review'
  | 'iteration'
  | 'epic'
  | 'item_type'
  | 'query';

export interface WorkFacetNames {
  orgName?: (id: number) => string | undefined;
  trackerName?: (id: number) => string | undefined;
  /** An epic's title by its key or task id ("TK-10 Login"). */
  epicTitle?: (ref: string) => string | undefined;
}

export function workFacets(f: WorkTreeFilters, names: WorkFacetNames = {}): (Facet & { id: WorkFacetId })[] {
  const n = normalizeFilters(f);
  const out: (Facet & { id: WorkFacetId })[] = [];
  if (n.org !== undefined) {
    out.push({ id: 'org', label: `Org: ${n.org === 'none' ? 'Unassigned' : (names.orgName?.(n.org) ?? `#${n.org}`)}` });
  }
  if (n.orgs) {
    const each = n.orgs.map((o) => (o === 'none' ? 'Unassigned' : (names.orgName?.(o) ?? `#${o}`)));
    out.push({ id: 'orgs', label: `Org: ${each.join(' or ')}` });
  }
  if (n.tracker !== undefined) {
    const t =
      n.tracker === 'local' ? 'Local work' : n.tracker === 'ref' ? 'Bare keys' : (names.trackerName?.(n.tracker) ?? `#${n.tracker}`);
    out.push({ id: 'tracker', label: `Tracker: ${t}` });
  }
  if (n.status !== undefined) out.push({ id: 'status', label: `Status: ${WORK_STATUS_LABELS[n.status]}` });
  if (n.stages) out.push({ id: 'stages', label: `Status: ${n.stages.map((st) => WORK_STAGE_LABELS[st]).join(' or ')}` });
  if (n.status_name) out.push({ id: 'status_name', label: `Column: ${n.status_name}` });
  if (n.mine) out.push({ id: 'mine', label: 'Assigned to me' });
  if (n.assignee) out.push({ id: 'assignee', label: `Assignee: ${n.assignee}` });
  if (n.has !== undefined) out.push({ id: 'has', label: `Sessions: ${HAS_FILTER_LABELS[n.has]}` });
  if (n.review) out.push({ id: 'review', label: 'To review' });
  if (n.iteration) {
    const it = n.iteration;
    out.push({ id: 'iteration', label: it === 'current' ? 'Current sprint' : it === 'none' ? 'No sprint' : `Sprint: ${it}` });
  }
  if (n.epic) {
    const title = names.epicTitle?.(n.epic);
    out.push({ id: 'epic', label: `Epic: ${title ? `${n.epic} ${title}` : n.epic}` });
  }
  if (n.item_type) out.push({ id: 'item_type', label: `Type: ${n.item_type}` });
  if (n.query) out.push({ id: 'query', label: `Search: “${n.query}”` });
  return out;
}

/** The filters without one facet. */
export function withoutWorkFacet(f: WorkTreeFilters, id: WorkFacetId): WorkTreeFilters {
  const key: keyof WorkTreeFilters = id;
  const { [key]: _drop, ...rest } = normalizeFilters(f);
  return rest;
}

/** "Host: gpu-box, Last 1d" — for an empty state. */
export function facetSentence(facets: readonly Facet[]): string {
  return facets.map((f) => f.label).join(', ');
}
