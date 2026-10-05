<script lang="ts">
  import { oddHosts, type AssetIdentity } from './assets';
  import type { DotState } from './assets_visual';
  import HostStrip from './HostStrip.svelte';
  import Badge from './Badge.svelte';

  /** The one row of an asset found on hosts but not in the catalog (S1a),
   *  shared by the Library's list and the Inbox (spec: "Rows reuse S1a's
   *  identity rows"): name, kind, why it needs a person, one dot per host,
   *  and Import. With `select` the row is a selectable list row — one
   *  button carrying `data-row-key`, Import beside it (a button never holds
   *  a button); without it, the plain S1a row. */
  let {
    identity,
    order,
    testid,
    readonly = false,
    onimport,
    states,
    why = '',
    select,
  }: {
    identity: AssetIdentity;
    order: string[];
    testid: string;
    /** An overview (a hub-client desktop): nothing to import. */
    readonly?: boolean;
    onimport: (identity: AssetIdentity) => void;
    /** Dots by state (the Inbox's, with stale scans); else present/differs
     *  from the identity itself. */
    states?: Record<string, DotState>;
    /** A line saying why the row is listed, for an identity with no reason
     *  badge of its own. */
    why?: string;
    select?: { key: string; testid: string; selected: boolean; onselect: () => void };
  } = $props();

  const KIND_LETTER: Record<string, string> = { skill: 'S', agent: 'A', hook: 'H', mcp_server: 'M', plugin_ref: 'P' };
  const internal = $derived(identity.class === 'fleet_internal' || identity.class === 'harness_internal');
  const present = $derived([...new Set(identity.hosts.map((h) => h.host_alias))]);
  const odd = $derived(oddHosts(identity));
</script>

{#snippet reason()}
  {#if identity.class === 'needs_person'}
    <Badge tone="warn" label={identity.reason ?? 'needs a person'} title={identity.reason ?? ''} />
  {:else if why}
    <span class="why">{why}</span>
  {/if}
{/snippet}

{#snippet importer()}
  {#if !readonly && !internal}
    <button type="button" class="link" onclick={() => onimport(identity)} title="Import this asset">Import</button>
  {/if}
{/snippet}

{#if select}
  <div class="line">
    <button
      type="button"
      class="row pick"
      class:selected={select.selected}
      aria-current={select.selected ? 'true' : undefined}
      data-row-key={select.key}
      data-testid={select.testid}
      onclick={select.onselect}
    >
      <span class="kico" title={identity.kind} aria-hidden="true">{KIND_LETTER[identity.kind] ?? '?'}</span>
      <span class="nm"><b>{identity.name}</b>{@render reason()}</span>
      <HostStrip {order} {present} {odd} {states} />
    </button>
    {@render importer()}
  </div>
{:else}
  <div class="row unmanaged" data-testid={testid}>
    <span class="name">{identity.name}</span>
    <span class="meta">{identity.kind}</span>
    {@render reason()}
    <HostStrip {order} {present} {odd} {states} />
    {@render importer()}
  </div>
{/if}

<style>
  .row { display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; padding: 5px 10px; background: none; border: 0; color: var(--fg); }
  .row.unmanaged { cursor: default; }
  .row.unmanaged:hover { background: var(--bg-pane); }
  .name { flex: 1; font-family: ui-monospace, monospace; }
  .meta { color: var(--fg-muted); font-size: 11px; }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; font-size: 12px; }
  .why { color: var(--fg-muted); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .line { display: flex; align-items: center; border-bottom: 1px solid var(--border); }
  .line .link { flex: none; padding: 0 14px; height: 34px; }
  .row.pick {
    display: grid; grid-template-columns: 18px minmax(0, 1fr) auto; gap: 10px; min-height: 34px; padding: 0 14px;
    flex: 1; min-width: 0; font: inherit; cursor: pointer;
  }
  .row.pick:hover { background: var(--bg-pane); }
  .row.pick.selected { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
  .row.pick:focus-visible, .link:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .nm { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .nm b { font-weight: 560; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .kico {
    display: grid; place-items: center; width: 16px; height: 16px; border-radius: var(--radius-sm);
    background: var(--control-bg-active); color: var(--control-fg-quiet); font-family: var(--mono); font-size: 9.5px; font-weight: 700;
  }
</style>
