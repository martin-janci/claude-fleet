// Assets M6 (R12, R13): one place that runs a card verb — busy while it
// runs, the reload after, and the toast: what happened, with Undo when the
// card can be undone. A failed card (a host sync that failed) comes back as
// a view, not an error, and is worded from its error. An apply that held
// some hosts' copies back says so as a warning, naming them, with Show.
import { applyChangeset, dismissChangeset, rejectItems, undoChangeset, type ChangesetView, type ItemView } from './assets_workspace';
import { olderHubWords } from './assets_cards';
import { push, pushError } from './toasts';
import type { Result } from './result';

/** What a card's verbs do, as the Inbox and the Inspector call them. */
export interface CardVerbs {
  apply: (id: number, positions?: number[] | null) => void;
  dismiss: (id: number) => void;
  undo: (id: number) => void;
  synchost: (host: string) => void;
}

type Verb = 'apply' | 'undo' | 'dismiss' | 'reject';
const DONE: Record<Verb, string> = { apply: 'Applied', undo: 'Undone', dismiss: 'Dismissed', reject: 'Ignored' };
const DO: Record<Verb, string> = { apply: 'Apply', undo: 'Undo', dismiss: 'Dismiss', reject: 'Ignore' };

/** The hosts whose copies a card's apply held back, in the card's order. */
export function heldHostsOf(v: ChangesetView): string[] {
  const host = (i: ItemView) => (i.kind === 'host' ? i.name : (i.params.host ?? i.name));
  return [...new Set(v.items.filter((i) => (i.outcome?.held?.length ?? 0) > 0).map(host))];
}

export async function runCardVerb(
  verb: Verb,
  id: number,
  opts: {
    positions?: number[] | null;
    setBusy: (b: string) => void;
    onchanged: () => Promise<void> | void;
    /** Select a card (the held-hosts warning's Show). */
    select?: (id: number) => void;
  },
): Promise<ChangesetView | null> {
  opts.setBusy('card');
  let r: Result<ChangesetView>;
  try {
    r =
      verb === 'apply' ? await applyChangeset(id, opts.positions)
      : verb === 'undo' ? await undoChangeset(id)
      : verb === 'dismiss' ? await dismissChangeset(id)
      : await rejectItems(id, opts.positions ?? []);
  } finally {
    opts.setBusy('');
  }
  await opts.onchanged();
  if (!r.ok) {
    const older = olderHubWords(r.error, DO[verb].toLowerCase() + ' this');
    if (older) push({ kind: 'error', message: older });
    else pushError(r.error, `${DO[verb]} card ${id}`);
    return null;
  }
  const v = r.value;
  const held = verb === 'apply' && v.state === 'applied' ? heldHostsOf(v) : [];
  if (verb === 'apply' && v.state === 'failed') {
    push({ kind: 'error', message: `Not applied: ${v.error ?? 'see the card'}` });
  } else if (held.length) {
    const select = opts.select;
    push({
      kind: 'warning',
      message: `${v.kind === 'rollout' ? 'Rolled out' : 'Applied'}; held on ${held.join(', ')} — sync those yourself`,
      action: select ? { label: 'Show', run: () => select(v.id) } : undefined,
    });
  } else {
    push({
      kind: 'info',
      message: `${DONE[verb]}: ${v.summary}`,
      action: verb === 'apply' && v.undoable ? { label: 'Undo', run: () => void runCardVerb('undo', v.id, opts) } : undefined,
    });
  }
  return v;
}
