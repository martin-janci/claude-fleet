<script lang="ts">
  import Badge from './Badge.svelte';
  import { PERSONAL, keyOf, type CatalogStatus, type LayerListing } from './assets_workspace';
  import { acceptanceOf, roleIn } from './assets_layers';
  import type { HostRow } from './hosts';

  /** Hosts (spec, Workspace shell; R18): per host its org, its role per
   *  catalog, and the catalogs it accepts. An accepted catalog is a toggle:
   *  admit or unadmit (the hub refuses `personal` and a host that has an
   *  org, so those are locked, and the lock says why). A hub client may toggle
   *  only a catalog it holds a grant on (`canAdmit`); the others say so. */
  let { hosts, statuses, layers, orgName, selectedKey, readOnly, busy, canAdmit = () => true, onselect, ontoggle }: {
    hosts: HostRow[];
    statuses: CatalogStatus[] | null;
    layers: Record<string, LayerListing> | null;
    orgName: (id: number | null) => string | null;
    selectedKey: string | null;
    readOnly: boolean;
    busy: boolean;
    /** Whether this window may admit to / unadmit from the catalog (R20). */
    canAdmit?: (catalog: string) => boolean;
    onselect: (key: string) => void;
    ontoggle: (host: string, catalog: string, on: boolean) => void;
  } = $props();

  const cats = $derived(
    [...(statuses ?? [])].sort((a, b) => (a.name === PERSONAL ? -1 : b.name === PERSONAL ? 1 : a.name.localeCompare(b.name))),
  );
  const roleOf = (alias: string, catalog: string) => {
    const l = layers?.[catalog];
    return l ? roleIn(l, alias) : null;
  };
  function onRowKey(e: KeyboardEvent, key: string) {
    // A toggle inside the row keeps its own Enter and Space.
    if (e.target !== e.currentTarget || (e.key !== 'Enter' && e.key !== ' ')) return;
    e.preventDefault();
    onselect(key);
  }
</script>

<div class="hosts" data-testid="hosts-view">
  <div class="head">
    <span class="sentence">{hosts.length} host{hosts.length === 1 ? '' : 's'}</span>
    <span class="hint">An org-less host receives an org catalog only when you admit it.</span>
  </div>
  {#if hosts.length === 0}
    <p class="quiet">No hosts yet.</p>
  {/if}
  {#each hosts as h (h.alias)}
    {@const key = keyOf({ type: 'host', alias: h.alias })}
    {@const org = orgName(h.org_id ?? null)}
    <div
      class="row"
      class:selected={selectedKey === key}
      role="button"
      tabindex="0"
      aria-current={selectedKey === key ? 'true' : undefined}
      data-row-key={key}
      data-testid={`host-row-${h.alias}`}
      onclick={() => onselect(key)}
      onkeydown={(e) => onRowKey(e, key)}
    >
      <div class="who">
        <b>{h.alias}</b>
        <span data-testid={`host-org-${h.alias}`}>
          {#if org}<Badge tone="neutral" label={org} />{:else}<Badge tone="muted" label="no org" />{/if}
        </span>
      </div>
      <div class="cats">
        {#each cats as c (c.name)}
          {@const acc = acceptanceOf(h, c)}
          {@const role = roleOf(h.alias, c.name)}
          <div class="cat">
            {#if role}<span class="role" data-testid={`host-role-${h.alias}-${c.name}`}>role {role}</span>{/if}
            <button
              type="button"
              class="btn btn--chip"
              aria-pressed={acc.state !== 'none'}
              disabled={acc.locked || readOnly || busy || !canAdmit(c.name)}
              title={!acc.locked && !readOnly && !canAdmit(c.name) ? `Needs a grant on ${c.name}: ask the operator` : acc.why}
              data-testid={`host-accept-${h.alias}-${c.name}`}
              onclick={(e) => {
                e.stopPropagation();
                ontoggle(h.alias, c.name, acc.state === 'none');
              }}
              ><span aria-hidden="true">{acc.state === 'none' ? '○' : '●'}</span> {c.name}{acc.state === 'shared' ? ' · shared only' : ''}{readOnly ? ` · ${acc.state === 'none' ? 'not admitted' : acc.why}` : ''}</button
            >
          </div>
        {/each}
      </div>
    </div>
  {/each}
</div>

<style>
  .hosts { display: grid; align-content: start; }
  .head { display: flex; align-items: baseline; gap: 10px; padding: 8px 12px; }
  .sentence { font-weight: 600; }
  .hint { color: var(--fg-muted); font-size: var(--text-xs); }
  .row { display: grid; gap: 6px; padding: 8px 12px; border-bottom: 1px solid var(--border); cursor: pointer; }
  .row.selected { background: var(--accent-soft); }
  .row:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: -2px; }
  .who { display: flex; align-items: center; gap: 8px; }
  .cats { display: flex; flex-wrap: wrap; gap: 6px 14px; }
  .cat { display: inline-flex; align-items: center; gap: 6px; }
  .role { font-size: var(--text-xs); color: var(--fg-muted); }
  .quiet { padding: 12px; color: var(--fg-muted); }
</style>
