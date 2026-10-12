// Guides (declarative pages, layout L9 `guide`): step-by-step walks through
// a task. Some are compiled in; the rest an agent proposed over the control
// API (`guide { propose }`, the fleet-guides catalog skill) and a person
// approved. Approved guides join the page list under "Guides"; on a paired
// desktop they are the hub's (`list_guides` routes there).
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type Result } from '../result';
import { pagesBundle, type Page } from './pages';
import { whoWords } from './review';

export interface GuideProposal {
  id: number;
  at: number;
  page_id: string;
  title: string;
  why?: string;
  source: 'agent' | 'person';
  source_detail?: string;
  /** It replaces a guide that is live now. */
  replaces: boolean;
  /** The spec, for a preview. */
  page: Page;
}

/** Where a live guide came from (G7.15, Guide board's provenance line).
 *  Mirrors `service::guides::GuideApproval`. */
export interface GuideApproval {
  page_id: string;
  source: 'agent' | 'person' | string;
  source_detail?: string;
  approved_at?: number;
  /** As the decision recorded it: `person`, `person (pixel)`. */
  approved_by?: string;
}

export interface GuidesView {
  guides: Page[];
  proposals: GuideProposal[];
  can_write: boolean;
  /** Absent from a hub that predates it. */
  approvals?: GuideApproval[];
}

/** The approved guides, as pages. */
export const liveGuides = writable<Page[]>([]);
export const guideProposals = writable<GuideProposal[]>([]);
/** The live guides' provenance, by page id. */
export const guideApprovals = writable<Map<string, GuideApproval>>(new Map());
/** This app may approve, reject and remove guides: always standalone; on a
 *  paired desktop, when the hub's operator trusts it. */
export const guidesWritable = writable<boolean>(true);

/** Every page a person navigates: the compiled ones and the live guides. */
export const allPages = derived([pagesBundle, liveGuides], ([$b, $g]) => {
  const compiled = new Set($b.pages.map((p) => p.id));
  return [...$b.pages, ...$g.filter((g) => !compiled.has(g.id))];
});

function apply(v: GuidesView) {
  liveGuides.set(v.guides ?? []);
  guideProposals.set(v.proposals ?? []);
  guideApprovals.set(new Map((v.approvals ?? []).map((a) => [a.page_id, a])));
  guidesWritable.set(v.can_write === true);
}

export async function loadGuides(): Promise<Result<GuidesView>> {
  const r = await invokeCmd<GuidesView>('list_guides');
  if (r.ok && r.value) apply(r.value);
  return r;
}

export async function decideGuide(id: number, approve: boolean): Promise<Result<GuidesView>> {
  const r = await invokeCmd<GuidesView>('decide_guide', { id, approve });
  if (r.ok && r.value) apply({ ...r.value, can_write: get(guidesWritable) });
  return r;
}

export async function removeGuide(pageId: string): Promise<Result<GuidesView>> {
  const r = await invokeCmd<GuidesView>('remove_guide', { pageId });
  if (r.ok && r.value) apply({ ...r.value, can_write: get(guidesWritable) });
  return r;
}

/** Who proposed a guide, in words. */
export function guideAuthor(p: GuideProposal): string {
  const who = p.source === 'agent' ? 'an agent' : 'a person';
  return p.source_detail ? `${who} (${p.source_detail})` : who;
}


/** An actor as the store records it (`agent`, `person (pixel)`), in words:
 *  "an agent", "a person (pixel)". */
function recordedWho(recorded: string): string {
  const m = /^(\w+)(?: \((.*)\))?$/.exec(recorded.trim());
  return m ? whoWords(m[1], m[2]) : recorded;
}

/** The day, as the Guide board writes it: "6 Oct". */
function dayWords(sec: number, timeZone?: string): string {
  return new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', timeZone }).format(new Date(sec * 1000));
}

/** A live guide's provenance line (Guide board): "Guide · proposed by an
 *  agent (host web-1), approved by a person on 6 Oct". A guide compiled into
 *  the app has no approval: "Guide · comes with Fleet". */
export function guideProvenance(a: GuideApproval | undefined, timeZone?: string): string {
  if (!a) return 'Guide · comes with Fleet';
  const proposed = `proposed by ${whoWords(a.source, a.source_detail)}`;
  const approved = a.approved_by
    ? `approved by ${recordedWho(a.approved_by)}${a.approved_at ? ` on ${dayWords(a.approved_at, timeZone)}` : ''}`
    : '';
  return `Guide · ${[proposed, approved].filter(Boolean).join(', ')}`;
}
