<script lang="ts">
  import { errorText } from './error_copy';
  // Settings → Hub & sync → Fleets (multi-hub design): every hub this window
  // shows. The primary is the pairing above (or this machine's own fleet) and
  // is listed read-only; each extra fleet is its own pairing, made here with a
  // code minted on THAT hub, and removed here. Adding and removing take effect
  // at once — an extra fleet is never this app's to own, so there is no
  // second brain to guard against by waiting for a restart (design D3).
  import { onMount } from 'svelte';
  import {
    fleets,
    loadFleets,
    addFleet,
    removeFleet,
    retryFleet,
    connectionWord,
    fleetTrouble,
    nextFleetColor,
    startFleets,
  } from './fleets';
  import Button from './kit/Button.svelte';
  import LoadError from './states/LoadError.svelte';
  import type { IpcError } from './result';
  import FleetProfiles from './FleetProfiles.svelte';
  import { multiFleet } from './fleets';

  let url = $state('');
  let code = $state('');
  let label = $state('');
  // The colour the backend would pick, so the swatch never shows black while
  // the fleet gets orange (review w01-14). Follows the list until touched.
  let colorTouched = $state(false);
  let picked = $state('');
  const color = $derived(colorTouched ? picked : nextFleetColor($fleets));
  let plaintext = $state(false);
  let plaintextAsk = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let removeAsk = $state<string | null>(null);

  // Review w17-13: a failed list read says so with Retry; otherwise the
  // section is an empty list and the add form, with not even the primary.
  let loadError = $state<IpcError | null>(null);
  let retrying = $state(false);
  async function reloadFleets() {
    const r = await loadFleets();
    loadError = r.ok ? null : r.error;
  }

  onMount(() => {
    void startFleets();
    void reloadFleets();
  });

  async function add() {
    busy = true;
    error = null;
    const r = await addFleet({
      url: url.trim(),
      code: code.trim(),
      label: label.trim() || undefined,
      color: colorTouched ? picked : undefined,
      allow_plaintext: plaintext,
    });
    busy = false;
    if (r.ok) {
      url = code = label = '';
      colorTouched = false;
      plaintext = plaintextAsk = false;
      return;
    }
    if (r.error.code === 'E_HUB_PLAINTEXT') plaintextAsk = true;
    error = errorText(r.error);
  }

  async function remove(id: string) {
    busy = true;
    const r = await removeFleet(id);
    busy = false;
    removeAsk = null;
    error = r.ok ? null : errorText(r.error);
  }
</script>

<section class="block" data-testid="fleets-section">
  <div class="section-header">
    <h4>Fleets</h4>
  </div>
  <p class="hook-desc">
    This window can show more than one hub at once — say a personal fleet and
    a company one. Each row and every action stays with the hub that owns it,
    and each hub has its own pairing, its own token and its own trust: a
    device trusted on one hub is not trusted on another.
  </p>
  {#if loadError}
    <LoadError
      title="Couldn't load the fleets"
      error={loadError}
      {retrying}
      onretry={async () => {
        retrying = true;
        await reloadFleets();
        retrying = false;
      }}
      testid="fleets-load-error" />
  {/if}
  <ul class="fleet-list" data-testid="fleets-list">
    {#each $fleets as f (f.id)}
      {@const cw = connectionWord(f)}
      {@const trouble = fleetTrouble(f)}
      <li style="--fleet: {f.color}" data-testid="fleets-row-{f.id}">
        <span class="swatch" aria-hidden="true"></span>
        <strong>{f.label}</strong>
        {#if f.url}<code>{f.url}</code>{/if}
        <span class="muted">{f.primary ? 'primary' : 'extra'} · {cw.word}</span>
        <span class="grow"></span>
        {#if !f.primary}
          {#if cw.tone !== 'ok'}
            <Button size="sm" variant="quiet" onclick={() => void retryFleet(f.id)}>Retry</Button>
          {/if}
          {#if removeAsk === f.id}
            <Button size="sm" variant="quiet" onclick={() => (removeAsk = null)}>Cancel</Button>
            <Button size="sm" variant="danger" {busy} busyLabel="Removing…" testid="fleets-remove-confirm-{f.id}" onclick={() => void remove(f.id)}>Remove</Button>
          {:else}
            <Button size="sm" variant="quiet" testid="fleets-remove-{f.id}" onclick={() => (removeAsk = f.id)}>Remove…</Button>
          {/if}
        {/if}
      </li>
      {#if f.unavailable}<li class="err">{f.unavailable}</li>{/if}
      {#if f.duplicate_of}
        <li class="err" data-testid="fleets-duplicate-{f.id}">Same hub as {f.duplicate_of}; remove this one.</li>
      {/if}
      {#if trouble}<li class="err" data-testid="fleets-trouble-{f.id}">{trouble}</li>{/if}
      {#if removeAsk === f.id}
        <li class="hook-desc">
          Remove forgets this hub's URL and token <em>on this machine</em>. It
          <strong>does not revoke</strong> anything: the client stays in that
          hub's list until its operator revokes it (<code>fleet-hub client revoke</code>).
        </li>
      {/if}
    {/each}
  </ul>

  <h5>Add a hub</h5>
  <p class="hook-desc">
    On the other hub, run <code>fleet-hub pair --name &lt;this machine&gt;</code>
    and paste the code it prints. The code works once.
  </p>
  <form
    class="add"
    data-testid="fleets-add-form"
    onsubmit={(e) => {
      e.preventDefault();
      void add();
    }}
  >
    <input bind:value={url} placeholder="https://fleet.example.com" aria-label="Hub address" data-testid="fleets-add-url" />
    <input bind:value={code} placeholder="pairing code" aria-label="Pairing code" data-testid="fleets-add-code" />
    <input bind:value={label} placeholder="label (optional)" aria-label="Label" />
    <input
      value={color}
      oninput={(e) => {
        picked = e.currentTarget.value;
        colorTouched = true;
      }}
      type="color"
      aria-label="Colour"
      data-testid="fleets-add-color"
    />
    {#if plaintextAsk}
      <label class="plain">
        <input type="checkbox" bind:checked={plaintext} />
        Send this hub's token over plain http anyway — only if the hop is already private (a tunnel, a VPN).
      </label>
    {/if}
    <Button variant="primary" type="submit" {busy} busyLabel="Pairing…" testid="fleets-add-submit" disabled={!url.trim() || !code.trim()}>Add hub</Button>
  </form>
  {#if error}<p class="err" data-testid="fleets-error">{error}</p>{/if}
</section>
{#if $multiFleet}
  <!-- Phase 3b: who this device is on each fleet. -->
  <FleetProfiles />
{/if}

<style>
  .fleet-list {
    list-style: none;
    padding: 0;
    margin: var(--space-2) 0;
  }
  .fleet-list li {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) 0;
    font-size: var(--text-sm);
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: var(--radius-xs);
    background: var(--fleet);
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .grow {
    flex: 1;
  }
  .add {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
  }
  .add input:not([type='color']):not([type='checkbox']) {
    flex: 1;
    min-width: 10em;
  }
  .plain {
    flex-basis: 100%;
    font-size: var(--text-xs);
  }
  .err {
    color: var(--status-failed);
    font-size: var(--text-xs);
  }
</style>
