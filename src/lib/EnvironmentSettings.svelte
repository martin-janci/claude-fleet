<script lang="ts">
  // Settings › Environment (session env and secrets, person scope), on any
  // fleet this window shows. The page is the declarative `settings.env`
  // resource page; this wrapper decides, per fleet, what this device may
  // change there (`env_access.ts`) — a person's own variables on any paired
  // device, the shared ones on a trusted one — and, with more than one fleet,
  // which fleet's variables it shows. An extra fleet's list and actions are
  // routed by `fleet` (multi-hub design D6), like its sessions.
  import { untrack } from 'svelte';
  import PageView from './pages/PageView.svelte';
  import type { Descriptor, Page, PageAction, SourceSpec } from './pages/pages';
  import type { ResourceType } from './pages/resources';
  import FleetPicker from './FleetPicker.svelte';
  import { fleetPlace, fleets, PRIMARY } from './fleets';
  import { hubStatus } from './hub';
  import {
    NO_ACCESS,
    OWN_FLEET,
    envReadonlyReason,
    envRecordWritable,
    envResourceFor,
    gitIdentityWrites,
    loadEnvAccess,
    setPersonVars,
    type EnvAccess,
  } from './env_access';
  import { push } from './toasts';

  let {
    page,
    pages,
    descs,
    values,
    sources,
    resources,
    actions,
    section = null,
    ownsFleet,
    onnavigate,
  }: {
    page: Page;
    pages: Page[];
    descs: Map<string, Descriptor>;
    values: Record<string, string>;
    sources: SourceSpec[];
    resources: ResourceType[];
    actions: PageAction[];
    section?: string | null;
    /** This app owns the primary fleet (standalone). */
    ownsFleet: boolean;
    onnavigate: (pageId: string) => void;
  } = $props();

  /** The fleet whose variables are shown: the primary, or an extra's slot. */
  let fleet = $state<string>(PRIMARY);
  let access = $state<EnvAccess>(NO_ACCESS);
  let accessLoaded = $state(false);
  /** Bumped after the Git & tokens form wrote: the list re-reads. */
  let tick = $state(0);
  /** Bumped by Try again after an access read that did not get through. */
  let retry = $state(0);

  // Where the picked fleet is, and the name this device has there.
  const place = $derived(fleetPlace(fleet, $fleets, $hubStatus));
  const current = $derived(place.current);
  const own = $derived(fleet === PRIMARY && ownsFleet);

  $effect(() => {
    const f = fleet;
    void retry;
    if (own) {
      access = OWN_FLEET;
      accessLoaded = true;
      return;
    }
    accessLoaded = false;
    access = NO_ACCESS;
    let stale = false;
    void loadEnvAccess(f).then((a) => {
      if (stale) return;
      access = a;
      accessLoaded = true;
    });
    return () => {
      stale = true;
    };
  });

  const where = $derived(place.where);
  const clientName = $derived(place.clientName);
  const shownResources = $derived(
    resources.map((r) => (r.id === page.resource ? envResourceFor(r, access) : r)),
  );
  const readonly = $derived(!access.person && !access.shared);
  const reason = $derived(accessLoaded && !own ? envReadonlyReason(access, where, clientName) : null);
  const routedFleet = $derived(fleet === PRIMARY ? null : fleet);

  // --- Git & tokens: the person-scope variables most people want, in one form.
  let gitOpen = $state(false);
  let gitBusy = $state(false);
  let gitError = $state<string | null>(null);
  let git = $state({ name: '', email: '', ghToken: '', jiraEmail: '', jiraToken: '' });
  const gitWrites = $derived(gitIdentityWrites(git));
  // What was typed is meant for the fleet it was typed under: a switch
  // empties the form, so a token cannot be Set on the newly picked hub.
  $effect(() => {
    void fleet;
    untrack(() => {
      git = { name: '', email: '', ghToken: '', jiraEmail: '', jiraToken: '' };
      gitError = null;
      gitOpen = false;
    });
  });

  async function saveGit(e: SubmitEvent) {
    e.preventDefault();
    if (gitWrites.length === 0) return;
    gitBusy = true;
    gitError = null;
    const r = await setPersonVars(routedFleet, gitWrites);
    gitBusy = false;
    tick += 1;
    if (r.error) {
      gitError = r.set > 0 ? `${r.set} set, then: ${r.error}` : r.error;
      return;
    }
    push({ kind: 'success', message: `${r.set} variable${r.set === 1 ? '' : 's'} set for your next session start` });
    git = { name: '', email: '', ghToken: '', jiraEmail: '', jiraToken: '' };
    gitOpen = false;
  }
</script>

<div class="env-settings" data-testid="env-settings">
  <FleetPicker bind:value={fleet} testid="env-fleet" />

  {#key `${fleet}:${accessLoaded}:${tick}`}
    <PageView
      {page}
      {pages}
      {descs}
      {values}
      {sources}
      resources={shownResources}
      {actions}
      {readonly}
      {reason}
      remote={!own}
      {section}
      fleet={routedFleet}
      canChange={(r) => envRecordWritable(access, r)}
      {onnavigate} />
  {/key}

  {#if accessLoaded && access.error}
    <button type="button" class="btn btn--quiet retry" data-testid="env-access-retry" onclick={() => (retry += 1)}>Try again</button>
  {/if}

  {#if accessLoaded && access.person}
    <section class="git" data-testid="env-git">
      {#if !gitOpen}
        <button type="button" class="btn" data-testid="env-git-open" onclick={() => (gitOpen = true)}>Git & tokens…</button>
      {:else}
        <form onsubmit={saveGit} data-testid="env-git-form">
          <p class="hint">
            Your own variables, in every session you start{fleet === PRIMARY ? '' : ` on ${current?.label ?? 'that fleet'}`}. A blank field
            changes nothing; a token is kept secret.
          </p>
          <label>Name <input data-testid="env-git-name" bind:value={git.name} placeholder="Ada Lovelace" autocomplete="off" /></label>
          <label>Email <input data-testid="env-git-email" bind:value={git.email} placeholder="ada@example.com" autocomplete="off" /></label>
          <label
            >GitHub token <input data-testid="env-git-gh" type="password" bind:value={git.ghToken} placeholder="GH_TOKEN" autocomplete="off" /></label>
          <label>Jira email <input data-testid="env-git-jira-email" bind:value={git.jiraEmail} placeholder="optional" autocomplete="off" /></label>
          <label
            >Jira token <input
              data-testid="env-git-jira-token"
              type="password"
              bind:value={git.jiraToken}
              placeholder="optional · JIRA_API_TOKEN"
              autocomplete="off" /></label>
          {#if gitError}<p class="err" role="alert" data-testid="env-git-error">{gitError}</p>{/if}
          <div class="row">
            <button class="btn" type="submit" data-testid="env-git-save" disabled={gitBusy || gitWrites.length === 0}>
              Set {gitWrites.length || ''} variable{gitWrites.length === 1 ? '' : 's'}
            </button>
            <button type="button" class="btn btn--quiet" onclick={() => (gitOpen = false)}>Cancel</button>
          </div>
        </form>
      {/if}
    </section>
  {/if}
</div>

<style>
  .env-settings {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .retry {
    align-self: flex-start;
  }
  .git form {
    display: grid;
    gap: 0.4rem;
    max-width: 28rem;
  }
  .git label {
    display: grid;
    gap: 0.15rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .hint {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin: 0;
  }
  .row {
    display: flex;
    gap: 0.5rem;
  }
  .err {
    font-size: var(--text-2xs);
    color: var(--usage-crit);
    margin: 0;
  }
</style>
