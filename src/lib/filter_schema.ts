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

/** The Sessions panel's headings, in order. */
export const FILTER_SECTIONS = ['Saved view', 'Quick', 'Scope', 'Time', 'Work', 'Include'] as const;
export type FilterSection = (typeof FILTER_SECTIONS)[number];

/** The Work panel's headings, in order (board "Work · tasks with filters
 *  open"; Sessions sits under the live-session switch, More holds what
 *  the board leaves out). */
export const WORK_FILTER_SECTIONS = ['Organisation', 'Status', 'Tracker', 'Assignee', 'Sessions', 'More'] as const;
export type WorkFilterSection = (typeof WORK_FILTER_SECTIONS)[number];

export interface FilterControl<S extends string = FilterSection> {
  label: string;
  /** Where it sits: the closed row, or a heading in the panel. */
  place: 'row' | S;
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
  agent: { label: 'Agent', place: 'Scope', testid: 'filter-agent-any' },
  recency: { label: 'Last active', place: 'Time', testid: 'recency-all' },
  'wf-tracker': { label: 'Tracker', place: 'Work', testid: 'wf-tracker-all', when: 'two or more trackers' },
  'wf-status': { label: 'Status', place: 'Work', testid: 'wf-status-all', when: 'there is work' },
  'wf-mine': { label: 'Assigned to me', place: 'Work', testid: 'wf-mine', when: 'there is work' },
  'wf-session': { label: 'Session', place: 'Work', testid: 'wf-session-any', when: 'grouped by work' },
  bg: { label: 'Background agents', place: 'Include', testid: 'bg-toggle' },
  archived: { label: 'Archived work', place: 'Include', testid: 'wf-hide-archived', when: 'there is work' },
};

/** Work: every facet `workFacets` can name, plus Archived and the saved
 *  view. A saved view's single `org` or `status` is cleared from the strip
 *  and edited with the chips that replaced it. */
export type WorkControlId = WorkFacetId | 'archived' | 'view';

export const WORK_FILTER_SCHEMA: Record<WorkControlId, FilterControl<WorkFilterSection>> = {
  query: { label: 'Search', place: 'row', testid: 'work-search' },
  org: { label: 'Organisation', place: 'Organisation', testid: 'work-filter-org' },
  orgs: { label: 'Organisation', place: 'Organisation', testid: 'work-filter-org' },
  status: { label: 'Status', place: 'Status', testid: 'work-filter-status' },
  stages: { label: 'Status', place: 'Status', testid: 'work-filter-status' },
  tracker: { label: 'Tracker', place: 'Tracker', testid: 'work-filter-tracker' },
  mine: { label: 'Me', place: 'Assignee', testid: 'work-filter-assignee' },
  assignee: { label: 'Assignee', place: 'Assignee', testid: 'work-filter-assignee' },
  has: { label: 'Only tasks with a live session', place: 'Sessions', testid: 'work-filter-live' },
  view: { label: 'Saved view', place: 'More', testid: 'work-view-select' },
  review: { label: 'To review', place: 'More', testid: 'work-filter-review' },
  status_name: { label: 'Tracker column', place: 'More', testid: 'work-filter-column', when: 'a tracker names its columns' },
  archived: { label: 'Archived tasks', place: 'More', testid: 'work-filter-archived' },
};

/** The controls under one heading, in schema order. */
export function controlsIn<Id extends string, S extends string>(schema: Record<Id, FilterControl<S>>, place: 'row' | S): Id[] {
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
  { id: 'org', label: 'Organisation', title: 'Its org, else its project’s owner' },
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
  { id: 'sprint', label: 'Sprint', title: 'Organisation → its current sprint' },
  { id: 'release', label: 'Release', title: 'Organisation → its release (the one still planned first)' },
  { id: 'epic', label: 'Epic', title: 'Organisation → the epic it is, or is filed under' },
];
