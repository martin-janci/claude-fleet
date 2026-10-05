<script lang="ts">
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import IdentityRow from './IdentityRow.svelte';
  import RowName from './RowName.svelte';
  import type { AssetIdentity } from './assets';
  import { SECTION_LABEL, type Inbox, type InboxRow, type InboxSection } from './assets_inbox';
  import { keep, type ParsedQuery } from './assets_query';
  import { scopeBadge } from './assets_workspace';

  /** The Inbox (spec, Workspace shell; Rulings R14, R16): open cards first,
   *  read-only until M6; then Needs you, Drifted, Behind the catalog, New on
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
  } = $props();

  const OPEN: InboxSection[] = ['needs', 'drifted', 'behind', 'fresh'];
  let insyncOpen = $state(false);

  /** A card has no host, scope, layer or state of its own, and the summary
   *  does not yet name its catalogs (rollout and drift cards are built per
   *  org catalog too): no token can say it does not match, so only free
   *  words, against its sentence, hide a card (T7 ruling; catalogs: M6). */
  const keepCard = (r: InboxRow) => !query.text || query.text.split(' ').every((w) => r.name.toLowerCase().includes(w));
  const keepRow = (r: InboxRow) => (r.card ? keepCard(r) : keep(query, r.query));

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
    <button
      type="button"
      class="row card"
      class:selected={selectedKey === r.key}
      aria-current={selectedKey === r.key ? 'true' : undefined}
      data-row-key={r.key}
      data-testid={`inbox-row-${r.key}`}
      onclick={() => onselect(r.key)}
    >
      <Badge tone={c.state === 'failed' ? 'crit' : 'accent'} glyph={c.state === 'failed' ? '✗' : undefined} label={c.state === 'failed' ? `${c.kind} · failed` : c.kind} />
      <RowName name={c.summary} strong why={c.error ?? ''} whyTitle={c.error ?? undefined} />
      <span class="groups">
        {#each Object.entries(c.groups ?? {}).slice(0, 3) as [g, n] (g)}<Badge label={`${g} ${n}`} />{/each}
      </span>
    </button>
  {/if}
{/snippet}

<div class="inbox" data-testid="assets-inbox">
  {#if shown.cards.length}
    <h3 class="grp" data-testid="inbox-section-cards">{SECTION_LABEL.cards} <span class="n">{shown.cards.length}</span></h3>
    {#each shown.cards as r (r.key)}{@render cardRow(r)}{/each}
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
  .row.card { grid-template-columns: auto minmax(0, 1fr) auto; }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
  .groups { display: flex; gap: 4px; }
  .quiet, .note { margin: 0; padding: 10px 14px; color: var(--fg-muted); font-size: 12px; }
</style>
