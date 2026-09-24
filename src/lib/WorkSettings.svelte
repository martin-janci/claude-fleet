<script lang="ts">
  // Settings → Work (work graph M3, M6): the trackers fleet reads tickets from.
  //
  // Connect asks for as little as possible: paste any ticket or issue URL and
  // the provider and site are inferred from it (atlassian.net → Jira Cloud,
  // github.com → GitHub, app.asana.com → Asana, linear.app → Linear; Jira
  // Data Center is picked by hand). Then only what that provider needs:
  // GitHub nothing but a host whose `gh` is logged in (fleet stores no GitHub
  // token), Asana a personal access token, Linear an API key, Jira Cloud an
  // email and API token, Data Center a token and optionally its CA. A token
  // goes to the backend once and is never shown again — a tracker row
  // carries only a `…abcd` hint. On a desktop paired with a hub, trackers
  // belong to the hub (its `work_admin` is master-only): the list is shown,
  // read-only, with the CLI lines that configure one.
  import { onMount } from 'svelte';
  import {
    trackers,
    loadTrackers,
    addTracker,
    setTrackerCredential,
    testTracker,
    removeTracker,
    updateTracker,
    inferProvider,
    providerInfo,
    sectionMapRows,
    PROVIDERS,
    trackerStateBadge,
    syncedAgo,
    type ProviderId,
    type TrackerRow,
  } from './trackers';
  import { hosts } from './hosts';
  import { hubStatus, hubBlock, ownsTheFleet } from './hub';
  import { pushError, push } from './toasts';
  import OrgSettings from './OrgSettings.svelte';

  let {
    now = () => Math.floor(Date.now() / 1000),
    initialUrl = '',
  }: {
    /** Unix seconds; injectable for tests. */
    now?: () => number;
    /** Pre-fill the Connect form (an unbound chip's "Connect"). */
    initialUrl?: string;
  } = $props();

  const owns = $derived(ownsTheFleet($hubStatus));
  let connecting = $state(false);
  // svelte-ignore state_referenced_locally
  let url = $state(initialUrl);
  let email = $state('');
  let token = $state('');
  /** A provider picked by hand (Data Center, or overriding the guess). */
  let picked = $state<ProviderId | ''>('');
  let ghHost = $state('');
  let extraCa = $state('');
  let allowPrivate = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let testing = $state<number | null>(null);

  const inferred = $derived(inferProvider(url));
  const provider = $derived<ProviderId | null>(picked || inferred?.provider || null);
  const info = $derived(provider ? PROVIDERS[provider] : null);
  /** The site to add: the inferred one, or (Data Center) the URL as typed. */
  const site = $derived(
    inferred && (!picked || picked === inferred.provider) ? inferred.site : url.trim(),
  );
  const canConnect = $derived.by(() => {
    if (!provider || !info || !site) return false;
    if (info.needs === 'host_with_gh') return !!ghHost;
    if (info.needs === 'email_token') return email.trim().length > 3 && token.trim().length > 0;
    return token.trim().length > 0;
  });

  onMount(() => {
    void loadTrackers();
    if (initialUrl) connecting = true;
  });

  async function connect() {
    if (!canConnect || !provider || !info || busy) return;
    busy = true;
    error = null;
    const existing = $trackers.find((t) => t.provider === provider && t.site_url === site);
    let row: TrackerRow | null = existing ?? null;
    if (!row) {
      const a = await addTracker(url.trim(), {
        provider,
        transport: info.needs === 'host_with_gh' ? `via_cli:${ghHost}` : undefined,
        settings:
          provider === 'jira_dc' && (extraCa.trim() || allowPrivate)
            ? { extra_ca: extraCa.trim() || null, allow_private_network: allowPrivate }
            : undefined,
      });
      if (!a.ok) {
        busy = false;
        error = a.error.message;
        return;
      }
      row = a.value;
    }
    if (info.needs !== 'host_with_gh') {
      const c = await setTrackerCredential(
        row.id,
        info.needs === 'email_token' ? email.trim() : null,
        token.trim(),
      );
      // The token leaves this component's state whatever happened next.
      token = '';
      if (!c.ok) {
        busy = false;
        error = c.error.message;
        return;
      }
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
    picked = '';
    extraCa = '';
    allowPrivate = false;
    push({
      kind: 'success',
      message: `Connected ${row.site_url} — your work appears in ⌘K within one sync.`,
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

  // --- Asana: which sections mean what (asked inline, not as a setup step).
  let sectionEdits = $state<Record<number, Record<string, string>>>({});
  function sectionValue(t: TrackerRow, section: string, fallback: string): string {
    return sectionEdits[t.id]?.[section] ?? fallback;
  }
  function setSection(t: TrackerRow, section: string, category: string) {
    sectionEdits = { ...sectionEdits, [t.id]: { ...(sectionEdits[t.id] ?? {}), [section]: category } };
  }
  async function confirmSections(t: TrackerRow) {
    const map: Record<string, string> = {};
    for (const r of sectionMapRows(t)) map[r.section] = sectionValue(t, r.section, r.category);
    const u = await updateTracker(t.id, {
      settings: { ...(t.settings ?? {}), section_map: map, section_map_confirmed: true },
    });
    if (!u.ok) {
      pushError(u.error, 'Saving the section map failed');
      return;
    }
    const { [t.id]: _, ...rest } = sectionEdits;
    sectionEdits = rest;
    await loadTrackers();
    push({ kind: 'success', message: `${t.name}: statuses follow your section map from the next sync.` });
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
        {@const prov = providerInfo(t.provider)}
        <li class="tracker" data-testid="tracker-row">
          {#if prov}<span class="prov" title={prov.label} data-testid="tracker-provider"
              >{prov.icon}</span
            >{/if}
          <span class="name">{t.name}</span>
          <span class="site">{t.site_url}</span>
          <span class="badge {badge.tone}" data-testid="tracker-state" title={t.last_error ?? ''}
            >{badge.label}</span
          >
          <span class="synced">{syncedAgo(t, now())}</span>
          {#if t.has_credential}<span class="cred" title="credential">{t.username ?? ''} {t.credential_hint ?? ''}</span>{/if}
          {#if t.transport?.startsWith('via_cli:')}<span class="cred" title="read through gh on that host, with its own login"
              >gh on {t.transport.slice('via_cli:'.length)}</span
            >{:else if t.transport?.startsWith('via_host:')}<span class="cred" title="requests leave from that host"
              >via {t.transport.slice('via_host:'.length)}</span
            >{/if}
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
            {#if t.provider === 'jira'}Atlassian API tokens expire within a year — create a new one
              and connect again.{:else}The tracker refused the credential — create a new one and
              connect again.{/if}
          </li>
        {:else if t.state === 'captcha'}
          <li class="hint">Log in to {t.site_url} in a browser once, then Test.</li>
        {:else if t.state === 'unreachable' && t.last_error}
          <li class="hint" data-testid="tracker-unreachable">{t.last_error}</li>
        {/if}
        {#if t.provider === 'asana' && owns && sectionMapRows(t).length > 0 && !t.settings?.section_map_confirmed}
          <li class="sections" data-testid="asana-sections">
            <span class="hint">Which Asana sections mean <em>in progress</em>? A completed task is
              always done.</span>
            {#each sectionMapRows(t) as r (r.section)}
              <label class="section-row">
                <span>{r.section}</span>
                <select
                  data-testid="asana-section-{r.section}"
                  value={sectionValue(t, r.section, r.category)}
                  onchange={(e) => setSection(t, r.section, (e.currentTarget as HTMLSelectElement).value)}
                >
                  <option value="todo">to do</option>
                  <option value="in_progress">in progress</option>
                  <option value="done">done</option>
                </select>
              </label>
            {/each}
            <button class="btn" data-testid="asana-sections-confirm" onclick={() => void confirmSections(t)}
              >Confirm</button
            >
          </li>
        {/if}
      {/each}
    </ul>
  {/if}

  {#if !owns}
    <p class="hint" data-testid="work-remote">
      {hubBlock('add_tracker', $hubStatus)} On the hub:
      <code>fleet-hub tracker add &lt;ticket-url&gt;</code> (GitHub:
      <code>--via-cli &lt;host with gh&gt;</code>), then
      <code>fleet-hub tracker set-credential &lt;id&gt; [--email &lt;you&gt;] &lt; token.txt</code>.
    </p>
  {:else if !connecting}
    <button class="btn" data-testid="connect-jira" onclick={() => (connecting = true)}
      >Connect a tracker</button
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
      <label for="jira-url">Any ticket or issue URL (or the site)</label>
      <input
        id="jira-url"
        data-testid="connect-url"
        bind:value={url}
        placeholder="https://acme.atlassian.net/browse/ABC-123"
        autocomplete="off"
        spellcheck="false"
      />
      <label for="tracker-provider">Tracker</label>
      <select id="tracker-provider" data-testid="connect-provider" bind:value={picked}>
        <option value="">{inferred ? `${PROVIDERS[inferred.provider].label} (from the URL)` : 'from the URL'}</option>
        {#each Object.entries(PROVIDERS) as [id, p] (id)}
          <option value={id}>{p.label}</option>
        {/each}
      </select>
      {#if url.trim() && !provider}
        <span class="err" data-testid="connect-url-error"
          >Not a URL fleet recognises (Jira Cloud, GitHub, Asana, Linear) — or pick Jira Data Center</span
        >
      {:else if provider && site}
        <span class="ok" data-testid="connect-site"
          >{site}{inferred?.key && (!picked || picked === inferred.provider) ? ` · ${inferred.key}` : ''}</span
        >
      {/if}
      {#if info?.needs === 'host_with_gh'}
        <label for="gh-host">A host where <code>gh</code> is logged in</label>
        <select id="gh-host" data-testid="connect-gh-host" bind:value={ghHost}>
          <option value="">choose…</option>
          {#each $hosts as h (h.alias)}
            <option value={h.alias}>{h.alias}{h.reachable ? '' : ' (unreachable)'}</option>
          {/each}
        </select>
        <span class="hint"
          >Fleet runs <code>gh</code> on that host with its own login and stores no GitHub token.</span
        >
      {:else if info}
        {#if info.needs === 'email_token'}
          <label for="jira-email">Atlassian account email</label>
          <input id="jira-email" data-testid="connect-email" bind:value={email} autocomplete="username" />
        {/if}
        <label for="jira-token">{info.secretLabel}</label>
        <input
          id="jira-token"
          data-testid="connect-token"
          type="password"
          bind:value={token}
          autocomplete="off"
        />
        <span class="hint">{info.secretHelp} It is stored on this machine and never shown again.</span>
        {#if provider === 'jira_dc'}
          <label for="dc-ca">Internal CA (PEM, optional)</label>
          <textarea id="dc-ca" data-testid="connect-ca" bind:value={extraCa} rows="3" spellcheck="false"
          ></textarea>
          <label class="check"
            ><input type="checkbox" data-testid="connect-private" bind:checked={allowPrivate} /> The
            site resolves to this machine or a link-local address (off: refused)</label
          >
        {/if}
      {/if}
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
  .prov {
    font-size: 0.62rem;
    font-weight: 600;
    padding: 0 0.3rem;
    border-radius: 3px;
    border: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .sections {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    padding-left: 1rem;
    font-size: 0.75rem;
  }
  .section-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .connect select,
  .connect textarea {
    font: inherit;
    font-size: 0.8rem;
  }
  .check {
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
</style>
