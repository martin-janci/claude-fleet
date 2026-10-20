<script lang="ts">
  // Settings → Hub & sync → Fleets → Profiles (multi-hub Phase 3b): who this
  // device is on each hub it shows. One human is often a different person on
  // each — a work identity on the company hub, a private one on the personal
  // hub — and this panel puts the two side by side: the label this desktop
  // gives each profile (kept here only, keyed by fleet), the person that
  // hub knows this device as, its sign-in, and which of the person's own
  // variables are set there (names only; a secret's value never comes here).
  import { onMount } from 'svelte';
  import { fleets, connectionWord } from './fleets';
  import {
    envChecklist,
    fleetProfiles,
    issuerShort,
    loadProfiles,
    profileLabelOf,
    profileLabels,
    profilePerson,
    setProfileLabel,
  } from './fleet_profiles';
  import Button from './kit/Button.svelte';

  let editing = $state<string | null>(null);
  let draft = $state('');

  onMount(() => {
    void loadProfiles();
  });

  function edit(id: string, current: string) {
    editing = id;
    draft = current;
  }
  function save(id: string) {
    setProfileLabel(id, draft);
    editing = null;
  }
</script>

<section class="block" data-testid="fleet-profiles">
  <div class="section-header">
    <h4>Profiles</h4>
    <Button size="sm" variant="quiet" testid="fleet-profiles-refresh" onclick={() => void loadProfiles()}>Refresh</Button>
  </div>
  <p class="hook-desc">
    Who this device is on each hub. The same human can be a different person on each — say Work on the company
    hub and Private on your own — with their own GitHub token, e-mail and sign-in. A label is this app's alone and
    is never sent to any hub.
  </p>
  <ul class="profiles">
    {#each $fleets.filter((f) => !f.duplicate_of) as f (f.id)}
      {@const label = profileLabelOf(f, $profileLabels)}
      {@const st = $fleetProfiles.get(f.id)}
      {@const p = st?.profile ?? null}
      {@const who = profilePerson(p)}
      <li style="--fleet: {f.color}" data-testid="fleet-profile-{f.id}">
        <div class="head">
          <span class="swatch" aria-hidden="true"></span>
          {#if editing === f.id}
            <form
              class="label-form"
              onsubmit={(e) => {
                e.preventDefault();
                save(f.id);
              }}
            >
              <input
                bind:value={draft}
                aria-label="Profile label for {f.label}"
                placeholder={f.label}
                maxlength="32"
                data-testid="fleet-profile-label-input-{f.id}"
              />
              <Button size="sm" variant="primary" type="submit" testid="fleet-profile-label-save-{f.id}">Save</Button>
              <Button size="sm" variant="quiet" onclick={() => (editing = null)}>Cancel</Button>
            </form>
          {:else}
            <strong data-testid="fleet-profile-label-{f.id}">{label}</strong>
            {#if label !== f.label}<span class="muted">on {f.label}</span>{/if}
            <Button size="sm" variant="quiet" testid="fleet-profile-rename-{f.id}" onclick={() => edit(f.id, $profileLabels[f.id] ?? '')}
              >Rename…</Button
            >
          {/if}
          <span class="muted">{connectionWord(f).word}</span>
        </div>
        {#if f.unavailable}
          <p class="muted">{f.unavailable}</p>
        {:else if st?.error}
          <p class="err" data-testid="fleet-profile-error-{f.id}">Could not read the profile: {st.error}</p>
        {:else if !st}
          <p class="muted">Reading…</p>
        {:else}
          <dl>
            <dt>Person</dt>
            <dd data-testid="fleet-profile-person-{f.id}">
              {#if who}{who}{#if p?.is_owner} <span class="muted">· owns this hub</span>{/if}{:else}<span class="muted"
                  >This device speaks for nobody on this hub.</span
                >{/if}
            </dd>
            {#if p?.email}
              <dt>E-mail</dt>
              <dd>{p.email}</dd>
            {/if}
            <dt>Sign-in</dt>
            <dd data-testid="fleet-profile-sso-{f.id}">
              {#if p?.sso && p.sso.length > 0}
                {p.sso.map((s) => issuerShort(s.issuer)).join(', ')}
              {:else if st.source === 'fallback'}
                <span class="muted">This hub is too old to say.</span>
              {:else}
                <span class="muted">No single sign-on linked.</span>
              {/if}
            </dd>
            {#if p?.git}
              <dt>Git</dt>
              <dd data-testid="fleet-profile-git-{f.id}">{[p.git.name, p.git.email ? `<${p.git.email}>` : ''].filter(Boolean).join(' ')}</dd>
            {/if}
            {#if p?.person_id != null}
              <dt>Variables</dt>
              <dd class="vars" data-testid="fleet-profile-env-{f.id}">
                {#each envChecklist(p) as v (v.name)}
                  <span class="var" class:set={v.set} title={v.set ? (v.secret ? 'set (secret)' : 'set') : 'not set'}
                    >{v.name} {v.set ? '✓' : '—'}</span
                  >
                {/each}
              </dd>
            {/if}
          </dl>
        {/if}
      </li>
    {/each}
  </ul>
  <p class="hook-desc">Set a profile's variables in Settings → Environment, with that fleet picked.</p>
</section>

<style>
  .profiles {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .profiles li {
    border-left: 3px solid var(--fleet);
    padding: var(--space-1) var(--space-2);
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: var(--radius-xs);
    background: var(--fleet);
  }
  .label-form {
    display: flex;
    gap: var(--space-1);
    align-items: center;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px var(--space-2);
    margin: var(--space-1) 0 0;
    font-size: var(--text-sm);
  }
  dt {
    color: var(--fg-muted);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .vars {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .var {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .var.set {
    color: var(--fg);
  }
  .muted {
    color: var(--fg-muted);
  }
  .err {
    color: var(--status-failed);
  }
</style>
