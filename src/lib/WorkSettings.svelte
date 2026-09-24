<script lang="ts">
  // Settings → Work (work graph M3): the trackers fleet reads tickets from.
  //
  // "Connect Jira" asks for as little as possible: paste any ticket URL (the
  // site is inferred from it), the account email and an API token. The
  // token goes to the backend once and is never shown again — a tracker row
  // carries only a `…abcd` hint. On a desktop paired with a hub, trackers
  // belong to the hub (its `work_admin` is master-only): the list is shown,
  // read-only, with the one CLI line that configures one.
  import { onMount } from 'svelte';
  import {
    trackers,
    loadTrackers,
    addTracker,
    setTrackerCredential,
    testTracker,
    removeTracker,
    parseJiraTicketUrl,
    trackerStateBadge,
    syncedAgo,
    type TrackerRow,
  } from './trackers';
  import { hubStatus, hubBlock, ownsTheFleet } from './hub';
  import { pushError, push } from './toasts';
  import OrgSettings from './OrgSettings.svelte';

  let {
    now = () => Math.floor(Date.now() / 1000),
    initialUrl = '',
  }: {
    /** Unix seconds; injectable for tests. */
    now?: () => number;
    /** Pre-fill the Connect form (an unbound chip's "Connect Jira"). */
    initialUrl?: string;
  } = $props();

  const owns = $derived(ownsTheFleet($hubStatus));
  let connecting = $state(false);
  // svelte-ignore state_referenced_locally
  let url = $state(initialUrl);
  let email = $state('');
  let token = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let testing = $state<number | null>(null);

  const parsed = $derived(parseJiraTicketUrl(url) ?? siteOnly(url));
  function siteOnly(raw: string): { site: string; key: string } | null {
    const m = raw.trim().match(/^https:\/\/([a-z0-9-]+)\.atlassian\.net\/?$/i);
    return m ? { site: `https://${m[1].toLowerCase()}.atlassian.net`, key: '' } : null;
  }
  const canConnect = $derived(!!parsed && email.trim().length > 3 && token.trim().length > 0);

  onMount(() => {
    void loadTrackers();
    if (initialUrl) connecting = true;
  });

  async function connect() {
    if (!canConnect || !parsed || busy) return;
    busy = true;
    error = null;
    const existing = $trackers.find((t) => t.site_url === parsed.site);
    let row: TrackerRow | null = existing ?? null;
    if (!row) {
      const a = await addTracker(url.trim());
      if (!a.ok) {
        busy = false;
        error = a.error.message;
        return;
      }
      row = a.value;
    }
    const c = await setTrackerCredential(row.id, email.trim(), token.trim());
    // The token leaves this component's state whatever happened next.
    token = '';
    if (!c.ok) {
      busy = false;
      error = c.error.message;
      return;
    }
    const t = await testTracker(row.id);
    busy = false;
    await loadTrackers();
    if (!t.ok) {
      error = t.error.message;
      return;
    }
    if (!t.value.ok) {
      error = t.value.error ?? 'the test failed';
      return;
    }
    connecting = false;
    url = '';
    email = '';
    push({
      kind: 'success',
      message: `Connected ${row.site_url} — My work appears in ⌘K within one sync.`,
    });
  }

  async function test(t: TrackerRow) {
    testing = t.id;
    const r = await testTracker(t.id);
    testing = null;
    await loadTrackers();
    if (!r.ok) pushError(r.error, 'Test failed');
    else if (!r.value.ok) push({ kind: 'error', message: r.value.error ?? 'the test failed' });
  }

  async function remove(t: TrackerRow) {
    const r = await removeTracker(t.id);
    if (!r.ok) pushError(r.error, 'Remove failed');
    await loadTrackers();
  }
</script>

<section class="block" data-testid="work-section">
  <div class="section-header"><h4>Work</h4></div>
  <p class="blurb">
    Trackers add a ticket's title and status to the sessions working on it, and list your tickets
    in ⌘K. Nothing needs one: keys in branch names group sessions without any tracker.
  </p>

  {#if $trackers.length > 0}
    <ul class="trackers" data-testid="tracker-list">
      {#each $trackers as t (t.id)}
        {@const badge = trackerStateBadge(t.state)}
        <li class="tracker" data-testid="tracker-row">
          <span class="name">{t.name}</span>
          <span class="site">{t.site_url}</span>
          <span class="badge {badge.tone}" data-testid="tracker-state" title={t.last_error ?? ''}
            >{badge.label}</span
          >
          <span class="synced">{syncedAgo(t, now())}</span>
          {#if t.has_credential}<span class="cred" title="credential">{t.username ?? ''} {t.credential_hint ?? ''}</span>{/if}
          {#if owns}
            <button
              class="btn"
              data-testid="tracker-test"
              disabled={testing === t.id}
              onclick={() => void test(t)}>{testing === t.id ? 'Testing…' : 'Test'}</button
            >
            <button class="btn" data-testid="tracker-remove" onclick={() => void remove(t)}
              >Remove</button
            >
          {/if}
        </li>
        {#if t.state === 'auth_failed'}
          <li class="hint" data-testid="tracker-expired">
            Atlassian API tokens expire within a year — create a new one and connect again.
          </li>
        {:else if t.state === 'captcha'}
          <li class="hint">Log in to {t.site_url} in a browser once, then Test.</li>
        {/if}
      {/each}
    </ul>
  {/if}

  {#if !owns}
    <p class="hint" data-testid="work-remote">
      {hubBlock('add_tracker', $hubStatus)} On the hub:
      <code>fleet-hub tracker add &lt;ticket-url&gt;</code>, then
      <code>fleet-hub tracker set-credential &lt;id&gt; --email &lt;you&gt; &lt; token.txt</code>.
    </p>
  {:else if !connecting}
    <button class="btn" data-testid="connect-jira" onclick={() => (connecting = true)}
      >Connect Jira</button
    >
  {:else}
    <form
      class="connect"
      data-testid="connect-form"
      onsubmit={(e) => {
        e.preventDefault();
        void connect();
      }}
    >
      <label for="jira-url">Any ticket URL (or the site)</label>
      <input
        id="jira-url"
        data-testid="connect-url"
        bind:value={url}
        placeholder="https://acme.atlassian.net/browse/ABC-123"
        autocomplete="off"
        spellcheck="false"
      />
      {#if url.trim() && !parsed}
        <span class="err" data-testid="connect-url-error"
          >Not a Jira Cloud URL (https://&lt;name&gt;.atlassian.net/…)</span
        >
      {:else if parsed}
        <span class="ok" data-testid="connect-site">{parsed.site}{parsed.key ? ` · ${parsed.key}` : ''}</span>
      {/if}
      <label for="jira-email">Atlassian account email</label>
      <input id="jira-email" data-testid="connect-email" bind:value={email} autocomplete="username" />
      <label for="jira-token">API token</label>
      <input
        id="jira-token"
        data-testid="connect-token"
        type="password"
        bind:value={token}
        autocomplete="off"
      />
      <span class="hint"
        >Create one at id.atlassian.com → Security → API tokens. It is stored on this machine and
        never shown again.</span
      >
      {#if error}<p class="err" role="alert" data-testid="connect-error">{error}</p>{/if}
      <div class="row">
        <button class="btn primary" type="submit" data-testid="connect-submit" disabled={!canConnect || busy}
          >{busy ? 'Connecting…' : 'Connect'}</button
        >
        <button class="btn" type="button" onclick={() => (connecting = false)}>Cancel</button>
      </div>
    </form>
  {/if}

  <OrgSettings />
</section>

<style>
  .blurb,
  .hint {
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .trackers {
    list-style: none;
    padding: 0;
    margin: 0.3rem 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .tracker {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
  }
  .site,
  .synced,
  .cred {
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  .badge {
    font-size: 0.68rem;
    padding: 0 0.35rem;
    border-radius: 4px;
    border: 1px solid var(--border);
  }
  .badge.ok {
    color: var(--ok, #22c55e);
  }
  .badge.warn {
    color: var(--warn, #f59e0b);
  }
  .badge.error {
    color: var(--err, #ef4444);
  }
  .connect {
    display: grid;
    grid-template-columns: 1fr;
    gap: 0.25rem;
    max-width: 28rem;
  }
  .connect input {
    font: inherit;
    font-size: 0.8rem;
    padding: 0.25rem 0.4rem;
  }
  .err {
    color: var(--err, #ef4444);
    font-size: 0.72rem;
  }
  .ok {
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  .row {
    display: flex;
    gap: 0.4rem;
  }
  .btn {
    font-size: 0.75rem;
  }
</style>
