// Progress blocks that update in place. An agent reports a long job by
// writing a `progress` block with the same `id` again as the job moves on;
// each block is its own card in the transcript, so the cards of one id find
// each other here. In one conversation the first card (in document order)
// shows the newest state and the later ones draw as one line pointing up.
//
// The scope is the conversation panel a card sits in: the same id in two
// panels on screen is two jobs. Per window, never saved.
import { untrack } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';
import type { UiBlock } from '../rich_blocks';

export type ProgressBlock = Extract<UiBlock, { kind: 'progress' }>;

interface Entry {
  el: HTMLElement;
  block: ProgressBlock;
}

/** Every progress card on screen. A reactive set of plain entries: a deep
 *  proxy would wrap the DOM node and the block. */
const entries = new SvelteSet<Entry>();

const scopeOf = (el: HTMLElement): Element | null => el.closest('.conversation-panel');

/** Start tracking a card's block; the returned function stops it. Called
 *  from an effect, so it reads nothing reactively: the effect must not
 *  depend on the other cards. */
export function track(el: HTMLElement, block: ProgressBlock): () => void {
  const entry: Entry = { el, block };
  untrack(() => entries.add(entry));
  return () => untrack(() => entries.delete(entry));
}

/** The blocks of `el`'s id in its conversation, in document order, and
 *  whether `el` is the first of them (the card that shows the newest). */
export function peers(el: HTMLElement, id: string): { blocks: ProgressBlock[]; home: boolean } {
  const scope = scopeOf(el);
  const all = [...entries]
    .filter((e) => e.block.id === id && scopeOf(e.el) === scope)
    .sort((a, b) => (a.el === b.el ? 0 : a.el.compareDocumentPosition(b.el) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1));
  return { blocks: all.map((e) => e.block), home: all.length === 0 || all[0].el === el };
}
