// Guides (declarative pages, layout L9 `guide`): step-by-step walks through
// a task. Some are compiled in; the rest an agent proposed over the control
// API (`guide { propose }`, the fleet-guides catalog skill) and a person
// approved. Approved guides join the page list under "Guides"; on a paired
// desktop they are the hub's (`list_guides` routes there).
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type Result } from '../result';
import { pagesBundle, type Page } from './pages';

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

export interface GuidesView {
  guides: Page[];
  proposals: GuideProposal[];
  can_write: boolean;
  /** Approved rows that are NOT being served, and why. Absent from an older hub. */
  withheld?: WithheldGuide[];
}

/** An approved guide the backend is withholding, with a reason a person can act on.
 *  It still holds a MAX_APPROVED slot, so it has to be shown somewhere: before
 *  this it vanished from every surface with nothing saying why. */
export interface WithheldGuide {
  id: number;
  page_id: string;
  why: string;
}

/** The approved guides, as pages. */
export const liveGuides = writable<Page[]>([]);
export const guideProposals = writable<GuideProposal[]>([]);
/** Approved but not served — shown so a person can see why and remove it. */
export const withheldGuides = writable<WithheldGuide[]>([]);
/** This app may approve, reject and remove guides: always standalone; on a
 *  paired desktop, when the hub's operator trusts it. */
export const guidesWritable = writable<boolean>(true);

/** Why the last read of the guides failed, or null when it did not.
 *
 *  `loadGuides`'s `Result` was discarded, so a failed read left `liveGuides`
 *  and `guideProposals` at `[]` and the page stated "No guide is waiting" as
 *  FACT — the one thing a failed call cannot establish. `guidesWritable` also
 *  stayed at its `true` initialiser, which suppressed the read-only note built
 *  for exactly that case. A revision-6 hub (no `guide` tool) and a
 *  readonly-paired desktop both reach it. */
export const guidesError = writable<string | null>(null);

/** Every page a person navigates: the compiled ones and the live guides. */
export const allPages = derived([pagesBundle, liveGuides], ([$b, $g]) => {
  const compiled = new Set($b.pages.map((p) => p.id));
  return [...$b.pages, ...$g.filter((g) => !compiled.has(g.id))];
});

function apply(v: GuidesView) {
  liveGuides.set(v.guides ?? []);
  guideProposals.set(v.proposals ?? []);
  withheldGuides.set(v.withheld ?? []);
  guidesWritable.set(v.can_write === true);
}

export async function loadGuides(): Promise<Result<GuidesView>> {
  const r = await invokeCmd<GuidesView>('list_guides');
  if (r.ok && r.value) {
    apply(r.value);
    guidesError.set(null);
  } else if (!r.ok) {
    // What `loadHubPages` and the sibling `loadProposals` both already do: say
    // the read failed, and do not claim the fleet has no guides. `can_write`
    // goes false too — this device established nothing about what it may do.
    guidesError.set(r.error.message);
    guidesWritable.set(false);
  }
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

