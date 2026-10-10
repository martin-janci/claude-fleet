<script lang="ts">
  // An org's Sharing tab (M15 G4.7, board Sharing): every live share on its
  // sessions, filtered by level and owner. An admin takes one back or
  // narrows it to watch; nothing here widens a share. A session the caller
  // may not see reads "a private session".
  import { shareLine, type ActionSpec, type OrgShare } from './resources';

  let {
    shares,
    revoke,
    narrow,
    readonly = false,
    busy = false,
    onaction,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    shares: OrgShare[];
    revoke?: ActionSpec;
    narrow?: ActionSpec;
    readonly?: boolean;
    busy?: boolean;
    onaction: (action: ActionSpec, share: OrgShare) => void;
    now?: () => number;
  } = $props();

  const LEVELS = ['watch', 'answer', 'drive'];
  let level = $state('');
  let owner = $state('');
  const owners = $derived([...new Set(shares.map((x) => shareLine(x).owner))].sort());
  const shown = $derived(shares.filter((x) => (!level || x.level === level) && (!owner || shareLine(x).owner === owner)));

  function since(at: number): string {
    const days = Math.floor((now() - at) / 86_400);
    return days <= 0 ? 'today' : `${days} d`;
  }
</script>

<div class="shares" data-testid="org-shares">
  <div class="filters">
    <select aria-label="Level" data-testid="shares-level" bind:value={level}>
      <option value="">Every level</option>
      {#each LEVELS as l (l)}<option value={l}>{l}</option>{/each}
    </select>
    <select aria-label="Owner" data-testid="shares-owner" bind:value={owner}>
      <option value="">Every owner</option>
      {#each owners as o (o)}<option value={o}>{o}</option>{/each}
    </select>
  </div>
  <table>
    <thead>
      <tr>
        <th scope="col">Session · owner</th>
        <th scope="col">Shared with</th>
        <th scope="col">Level</th>
        <th scope="col">Since</th>
        {#if !readonly}<th scope="col"><span class="sr">Actions</span></th>{/if}
      </tr>
    </thead>
    <tbody>
      {#each shown as x (x.id)}
        {@const line = shareLine(x)}
        <tr data-testid="item-shares">
          <td><span class:private={!x.session}>{line.session}</span> <span class="dim">· {line.owner}</span></td>
          <td>{x.shared_with}</td>
          <td data-testid="share-level">{x.level}</td>
          <td class="dim">{since(x.since)}</td>
          {#if !readonly}
            <td class="acts">
              {#if narrow && x.level !== 'watch'}
                <button
                  type="button"
                  class="btn btn--quiet"
                  disabled={busy}
                  aria-label={`${narrow.label}: ${line.session}, ${x.shared_with}`}
                  data-testid="share-narrow"
                  onclick={() => onaction(narrow, x)}>{narrow.label}</button>
              {/if}
              {#if revoke}
                <button
                  type="button"
                  class="btn btn--quiet"
                  disabled={busy}
                  aria-label={`${revoke.label}: ${line.session}, ${x.shared_with}`}
                  data-testid="share-revoke"
                  onclick={() => onaction(revoke, x)}>{revoke.label}</button>
              {/if}
            </td>
          {/if}
        </tr>
      {:else}
        <tr><td class="none" colspan="5">{shares.length ? 'No share matches the filters.' : 'Nothing on this org is shared.'}</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .filters {
    display: flex;
    gap: 0.4rem;
    margin-bottom: 0.4rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  td {
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  .dim,
  .private,
  .none {
    color: var(--fg-muted);
  }
  .private {
    font-style: italic;
  }
  .acts {
    text-align: right;
    white-space: nowrap;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
