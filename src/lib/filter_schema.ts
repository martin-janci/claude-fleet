// The left list's Filters section (Orbit Fleet redesign step 3.7): one
// typed schema per list, so the Sessions list and the Work view draw the
// same section from the same shape. The section is one row while closed
// (search · Filters · Group); its panel holds every other control, under
// the same headings in both lists.
//
// The engines stay where they are: `rowMatches` (sidebar_index.ts) and
// `work_filters.ts` narrow the Sessions list, `WorkTreeFilters` (the hub's
// `work_tree`) narrows the Work view. This file only says which controls
// exist, where each sits and what it is called; `Record<Id, …>` makes a
// facet without a control a type error, and `filter_schema.test.ts` checks
// each control renders.
import type { SessionFacetId, WorkFacetId } from './filter_facets';
import type { SidebarGroupBy } from './sessions';
import type { WorkGroupBy } from './work_view';

/** The panel's headings, in order. */
export const FILTER_SECTIONS = ['Saved view', 'Quick', 'Scope', 'Time', 'Work', 'Include'] as const;
export type FilterSection = (typeof FILTER_SECTIONS)[number];

export interface FilterControl {
  label: string;
  /** Where it sits: the closed row, or a heading in the panel. */
  place: 'row' | FilterSection;
  /** The control's test id, so the schema can be checked against the DOM. */
  testid: string;
  /** When it is drawn, if not always. */
  when?: string;
}

/** Sessions: every facet `sessionFacets` can name, plus Archived (a switch
 *  with no chip of its own while off). */
export type SessionControlId = SessionFacetId | 'archived';

export const SESSION_FILTER_SCHEMA: Record<SessionControlId, FilterControl> = {
  search: { label: 'Search', place: 'row', testid: 'sidebar-search' },
  'needs-you': { label: 'Needs you', place: 'Quick', testid: 'needs-you-filter' },
  scope: { label: 'Organisation', place: 'Scope', testid: 'filter-scope', when: 'two or more organisations' },
  host: { label: 'Machine', place: 'Scope', testid: 'filter-host-all' },
  recency: { label: 'Last active', place: 'Time', testid: 'recency-all' },
  'wf-tracker': { label: 'Tracker', place: 'Work', testid: 'wf-tracker-all', when: 'two or more trackers' },
  'wf-status': { label: 'Status', place: 'Work', testid: 'wf-status-all', when: 'there is work' },
  'wf-mine': { label: 'Assigned to me', place: 'Work', testid: 'wf-mine', when: 'there is work' },
  'wf-session': { label: 'Session', place: 'Work', testid: 'wf-session-any', when: 'grouped by work' },
  bg: { label: 'Background agents', place: 'Include', testid: 'bg-toggle' },
  archived: { label: 'Archived work', place: 'Include', testid: 'wf-hide-archived', when: 'there is work' },
};

/** Work: every facet `workFacets` can name, plus Archived and the saved
 *  view. */
export type WorkControlId = WorkFacetId | 'archived' | 'view';

export const WORK_FILTER_SCHEMA: Record<WorkControlId, FilterControl> = {
  query: { label: 'Search', place: 'row', testid: 'work-search' },
  view: { label: 'Saved view', place: 'Saved view', testid: 'work-view-select' },
  mine: { label: 'Assigned to me', place: 'Quick', testid: 'work-filter-mine' },
  review: { label: 'To review', place: 'Quick', testid: 'work-filter-review' },
  org: { label: 'Organisation', place: 'Scope', testid: 'work-filter-org' },
  tracker: { label: 'Tracker', place: 'Work', testid: 'work-filter-tracker' },
  status: { label: 'Status', place: 'Work', testid: 'work-filter-status' },
  status_name: { label: 'Tracker column', place: 'Work', testid: 'work-filter-column', when: 'a tracker names its columns' },
  assignee: { label: 'Assignee', place: 'Work', testid: 'work-filter-assignee', when: 'a task has an assignee' },
  has: { label: 'Sessions', place: 'Work', testid: 'work-filter-has' },
  archived: { label: 'Archived tasks', place: 'Include', testid: 'work-filter-archived' },
};

/** The controls under one heading, in schema order. */
export function controlsIn<Id extends string>(schema: Record<Id, FilterControl>, place: FilterControl['place']): Id[] {
  return (Object.keys(schema) as Id[]).filter((id) => schema[id].place === place);
}

// ── Group ──

export interface GroupOption<T extends string> {
  id: T;
  label: string;
  title?: string;
}

/** The Sessions list's groupings (3.6), Project first. */
export const SESSION_GROUPS: readonly GroupOption<SidebarGroupBy>[] = [
  { id: 'project', label: 'Project' },
  { id: 'work', label: 'Work', title: 'A ticket key (ABC-123) in a tag, branch or worktree name' },
  { id: 'state', label: 'State' },
  { id: 'host', label: 'Host' },
  { id: 'agent', label: 'Agent' },
];

/** The Work view's groupings: its List layout (by status), or its Grouped
 *  layout with each org's sections by `filters.group_by` (redesign 6.2). */
export type WorkGroupChoice = 'list' | WorkGroupBy;
export const WORK_GROUPS: readonly GroupOption<WorkGroupChoice>[] = [
  { id: 'list', label: 'Status', title: 'By status: To do, Doing, Done' },
  { id: 'group', label: 'Group', title: 'Organisation → group (a person, a rule, a tracker container, a repo or a key)' },
  { id: 'org', label: 'Organisation', title: 'One section per organisation' },
  { id: 'person', label: 'Person', title: 'Organisation → the person it is assigned to' },
  { id: 'mission', label: 'Mission', title: 'Organisation → its mission' },
  { id: 'account', label: 'Account', title: 'Organisation → the account its sessions run on' },
  { id: 'repo', label: 'Repo', title: 'Organisation → its repo' },
];
