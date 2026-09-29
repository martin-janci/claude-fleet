// Declarative pages L8: the embed pages that place catalog items inside the
// desktop's own screens. Each one fills a named slot (`model.rs` `Slot`); the
// screen that owns the slot renders `<EmbedSlot slot=…>` with the context the
// slot promises, and the spec decides what goes there. The specs are
// compiled into the backend like every page, but `list_pages` never carries
// them: this file is generated from the same specs (`REGEN_PAGE_DOCS=1 cargo
// test -p fleet-core page_docs_are_current`, checked current in CI), so a
// slot draws on the first frame.
import generated from './embeds.generated.json';
import type { Item, Page, Slot } from './pages';

/** Every slot the desktop renders. `embeds.test.ts` holds it equal to the
 *  slots the generated specs fill. */
export const RENDERED_SLOTS: readonly Slot[] = [
  'host_detail',
  'hosts_group_title',
  'hosts_group',
  'new_session_chip',
  'new_session_host',
  'status_footer',
];

export const embedPages = generated.pages as unknown as Page[];

const bySlot = new Map<Slot, Item[]>(
  embedPages.map((p) => [p.slot as Slot, (p.sections ?? []).flatMap((s) => s.items)]),
);

/** The items `slot` holds, in spec order; none when no page fills it. */
export function embedItems(slot: Slot): Item[] {
  return bySlot.get(slot) ?? [];
}
