<script lang="ts">
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import ChangesetCard from './ChangesetCard.svelte';
  import IdentityRow from './IdentityRow.svelte';
  import RowName from './RowName.svelte';
  import type { AssetIdentity } from './assets';
  import { keepCard, SECTION_LABEL, type Inbox, type InboxRow, type InboxSection } from './assets_inbox';
  import { keep, type ParsedQuery } from './assets_query';
  import { scopeBadge, type ChangesetSummary, type ChangesetView } from './assets_workspace';
  import type { CardVerbs } from './card_actions';

  const NO_CARD_VERBS: CardVerbs = { apply: () => {}, dismiss: () => {}, undo: () => {}, synchost: () => {} };

  /** The Inbox (spec, Workspace shell; Rulings R14, R16): open cards first,
   *  each a `ChangesetCard` with its verbs; recently applied, undoable ones
   *  as a banner with Undo; then Needs you, Drifted, Behind the catalog, New on
   *  hosts; In sync folds to one line. Every row is one asset identity — or
   *  one card — never one host copy. An unmanaged identity is S1a's own row
   *  (`IdentityRow`), Import included. */
  let {
    inbox,
    order,
    selectedKey,
    query,
    onselect,
    onimport = () => {},
    readonly = false,
    views = {},
    busy = false,
    primaryId = null,
    canActOn = () => true,
    oncard = NO_CARD_VERBS,
  }: {
    inbox: Inbox;
    order: string[];
    selectedKey: string | null;
    query: ParsedQuery;
    onselect: (key: string) => void;
    /** Import an unmanaged identity (the panel's import dialog). */
    onimport?: (identity: AssetIdentity) => void;
    /** A hub client without the grant: the same rows, nothing to import. */
    readonly?: boolean;
    /** Every open card in full, by id (the `cardViews` store). */
    views?: Record<number, ChangesetView>;
    /** A card verb is running: the cards' verbs are disabled. */
    busy?: boolean;
    /** The card whose verb is the main column's one `.btn--primary`, if any. */
    primaryId?: number | null;
    /** Whether this window may act on a card (a grant on every catalog it names). */
    canActOn?: (c: ChangesetSummary) => boolean;
    /** What the cards' verbs do. */
    oncard?: CardVerbs;
  } = $props();

  const OPEN: InboxSection[] = ['needs', 'drifted', 'behind', 'fresh'];
  let insyncOpen = $state(false);

  /** A card has no host, scope, layer or state of its own: only free words
   *  (its sentence) and `catalog:` (the catalogs its apply commits to) hide
   *  it (T7 ruling; R2). */
  const keepRow = (r: InboxRow) => (r.card ? keepCard(query, r.card) : keep(query, r.query));

  const shown = $derived(
    Object.fromEntries(
      (Object.keys(inbox.sections) as InboxSection[]).map((s) => [s, inbox.sections[s].filter(keepRow)]),
    ) as Record<InboxSection, InboxRow[]>,
  );
  const filtered = $derived(query.tokens.length > 0 || query.text !== '');
  const quiet = $derived(shown.cards.length + OPEN.reduce((n, s) => n + shown[s].length, 0) === 0);
</script>

{#snippet row(r: InboxRow)}
  {#if r.identity}
    <IdentityRow
      identity={r.identity}
      {order}
      {readonly}
      {onimport}
      states={r.dots}
      why={r.why}
      testid={`inbox-row-${r.key}`}
      select={{ key: r.key, testid: `inbox-row-${r.key}`, selected: selectedKey === r.key, onselect: () => onselect(r.key) }}
    />
  {:else}
    <button
      type="button"
      class="row"
      class:selected={selectedKey === r.key}
      aria-current={selectedKey === r.key ? 'true' : undefined}
      data-row-key={r.key}
      data-testid={`inbox-row-${r.key}`}
      onclick={() => onselect(r.key)}
    >
      <RowName kind={r.kind} name={r.name} why={r.why}>
        {#if r.asset}
          {@const b = scopeBadge(r.asset)}
          <Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} />
        {:else}
          <Badge tone="warn" label="orphan" />
        {/if}
      </RowName>
      <HostStrip {order} states={r.dots} />
    </button>
  {/if}
{/snippet}

{#snippet cardRow(r: InboxRow)}
  {@const c = r.card}
  {#if c}
    <ChangesetCard
      card={c}
      view={views[c.id] ?? null}
      selected={selectedKey === r.key}
      readOnly={readonly || !canActOn(c)}
      {busy}
      primary={primaryId === c.id}
      onselect={() => onselect(r.key)}
      onapply={(p) => oncard.apply(c.id, p)}
      ondismiss={() => oncard.dismiss(c.id)}
      onundo={() => oncard.undo(c.id)}
      onsynchost={oncard.synchost}
    />
  {/if}
{/snippet}

<div class="inbox" data-testid="assets-inbox">
  {#if shown.cards.length}
    <h3 class="grp" data-testid="inbox-section-cards">{SECTION_LABEL.cards} <span class="n">{shown.cards.length}</span></h3>
    {#each shown.cards as r (r.key)}{@render cardRow(r)}{/each}
  {/if}
  {#if shown.applied.length}
    <h3 class="grp" data-testid="inbox-section-applied">{SECTION_LABEL.applied} <span class="n">{shown.applied.length}</span></h3>
    {#each shown.applied as r (r.key)}{@render cardRow(r)}{/each}
  {/if}
  {#each OPEN as s (s)}
    {#if shown[s].length}
      <h3 class="grp" data-testid={`inbox-section-${s}`}>{SECTION_LABEL[s]} <span class="n">{shown[s].length}</span></h3>
      {#each shown[s] as r (r.key)}{@render row(r)}{/each}
    {/if}
  {/each}
  {#if quiet}
    <p class="quiet" data-testid="inbox-quiet">{filtered ? 'No matches.' : 'Nothing needs you.'}</p>
  {/if}
  {#if shown.insync.length}
    <button
      type="button"
      class="grp fold"
      aria-expanded={insyncOpen}
      onclick={() => (insyncOpen = !insyncOpen)}
      data-testid="inbox-insync-toggle"
    ><span class="tri" aria-hidden="true">{insyncOpen ? '▾' : '▸'}</span>{SECTION_LABEL.insync} <span class="n">{shown.insync.length}</span></button>
    {#if insyncOpen}{#each shown.insync as r (r.key)}{@render row(r)}{/each}{/if}
  {/if}
  {#if inbox.hidden}
    <p class="note">{inbox.hidden} fleet internal{inbox.hidden === 1 ? '' : 's'} hidden — fleet's own hooks, MCP entry and skills, and harness internals.</p>
  {/if}
</div>

<style>
  .inbox { display: flex; flex-direction: column; font-size: 13px; }
  .grp {
    display: flex; align-items: center; gap: 8px; margin: 0; padding: 12px 14px 6px;
    font-size: 11px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--fg-muted);
  }
  .grp .n { font-variant-numeric: tabular-nums; letter-spacing: 0; }
  .fold { border: 0; background: none; cursor: pointer; text-align: left; font: inherit; font-size: 11px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--fg-muted); }
  .fold:focus-visible, .row:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .tri { display: inline-block; width: 8px; }
  .row {
    display: grid; grid-template-columns: 18px minmax(0, 1fr) auto; gap: 10px; align-items: center;
    width: 100%; min-height: 34px; padding: 0 14px; border: 0; border-bottom: 1px solid var(--border);
    background: none; color: var(--fg); font: inherit; text-align: left; cursor: pointer;
  }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
  .quiet, .note { margin: 0; padding: 10px 14px; color: var(--fg-muted); font-size: 12px; }
</style>
