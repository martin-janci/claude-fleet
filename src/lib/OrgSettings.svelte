<script lang="ts">
  // Settings → Work → Organisations (work graph M5.4).
  //
  // Optional by design: with no org, scopes come from the GitHub owners and
  // nothing is fenced. An org is worth naming to merge or split owners, to
  // attach a tracker, or to turn on the boundary for its hosts: a host's
  // per-host token (the Claude running there) then reads only its org's
  // work. On a desktop paired with a hub, orgs belong to the hub (its
  // `work_admin` is master-only): the list is read-only here, with the CLI
  // line that changes it.
  import { onMount } from 'svelte';
  import {
    orgs,
    loadOrgs,
    loadOrgSuggestions,
    addOrg,
    updateOrg,
    removeOrg,
    addOrgRule,
    removeOrgRule,
    assignHostOrg,
    assignTrackerOrg,
    createFromSuggestion,
    ruleChip,
    type OrgDetail,
    type OrgSuggestion,
    orgAutoTidy,
    type OrgAutoTidy,
  } from './orgs';
  import { hosts } from './hosts';
  import { trackers, loadTrackers } from './trackers';
  import { hubStatus, hubBlock, ownsTheFleet } from './hub';
  import { pushError } from './toasts';
  import type { Result } from './result';

  const owns = $derived(ownsTheFleet($hubStatus));
  let suggestions = $state<OrgSuggestion[]>([]);
  let newName = $state('');
  let newColor = $state('#3b82f6');
  let busy = $state(false);
  let ruleDraft = $state<Record<number, { kind: 'owner' | 'path' | 'host'; value: string }>>({});

  async function refresh() {
    await loadOrgs();
    const s = await loadOrgSuggestions();
    suggestions = s.ok && Array.isArray(s.value) ? s.value : [];
  }

  onMount(() => {
    void refresh();
  });

  async function run(p: Promise<Result<unknown>>, what: string) {
    busy = true;
    const r = await p;
    busy = false;
    if (!r.ok) pushError(r.error, what);
    await refresh();
    void loadTrackers();
  }

  function draftOf(o: OrgDetail) {
    return ruleDraft[o.id] ?? { kind: 'owner' as const, value: '' };
  }

  async function addRule(o: OrgDetail) {
    const d = draftOf(o);
    const v = d.value.trim();
    if (!v) return;
    const rule =
      d.kind === 'owner'
        ? v.includes('/')
          ? { org_id: o.id, owner: v.split('/')[0], repo: v.split('/')[1] || null }
          : { org_id: o.id, owner: v }
        : d.kind === 'path'
          ? { org_id: o.id, path_prefix: v }
          : { org_id: o.id, host_alias: v };
    await run(addOrgRule(rule), 'Add rule failed');
    ruleDraft = { ...ruleDraft, [o.id]: { ...d, value: '' } };
  }
</script>

<div class="orgs" data-testid="org-section">
  <h5>Organisations</h5>
  <p class="blurb">
    Optional. Name an org to merge or split GitHub owners, to attach a tracker, or to make it a
    boundary: a host in an org lets its Claude read only that org's (and unassigned) work.
  </p>

  {#if owns && suggestions.length > 0}
    <ul class="suggestions" data-testid="org-suggestions">
      {#each suggestions as sg (sg.name + (sg.tracker_id ?? ''))}
        <li>
          <button
            class="btn"
            data-testid="org-suggestion"
            disabled={busy}
            title={sg.reason}
            onclick={() => void run(createFromSuggestion(sg), 'Create org failed')}
            >Create org {sg.name}{sg.owner ? ` from ${sg.owner}/*` : ''}{sg.tracker_id != null
              ? ' with its tracker'
              : ''}</button
          >
          <span class="hint">{sg.reason}</span>
        </li>
      {/each}
    </ul>
  {/if}

  {#each $orgs as o (o.id)}
    <div class="org" data-testid="org-row">
      <div class="org-head">
        <span class="swatch" style:background={o.color ?? 'transparent'}></span>
        <strong>{o.name}</strong>
        {#if o.isolate_sessions}<span class="badge" data-testid="org-isolated">isolates sessions</span>{/if}
        {#if o.auto_tidy != null}<span class="badge" data-testid="org-auto-tidy-badge">auto-tidy {o.auto_tidy ? 'on' : 'off'}</span>{/if}
        {#if owns}
          <input
            type="color"
            aria-label="Colour of {o.name}"
            value={o.color ?? '#888888'}
            onchange={(e) =>
              void run(updateOrg(o.id, { color: (e.currentTarget as HTMLInputElement).value }), 'Recolour failed')}
          />
          <button class="btn" data-testid="org-remove" disabled={busy} onclick={() => void run(removeOrg(o.id), 'Remove org failed')}
            >Remove</button
          >
        {/if}
      </div>
      <div class="chips" data-testid="org-rules">
        {#each o.rules as r (r.id)}
          <span class="chip" data-testid="org-rule">{ruleChip(r)}{#if owns}<button
                class="x"
                aria-label="Remove rule {ruleChip(r)}"
                onclick={() => void run(removeOrgRule(r.id), 'Remove rule failed')}>×</button
              >{/if}</span
          >
        {/each}
        {#each o.hosts as h (h)}
          <span class="chip host" data-testid="org-host">host {h}{#if owns}<button
                class="x"
                aria-label="Take {h} out of {o.name}"
                onclick={() => void run(assignHostOrg(h, null), 'Unassign failed')}>×</button
              >{/if}</span
          >
        {/each}
        {#each o.trackers as t (t.id)}
          <span class="chip tracker" data-testid="org-tracker">{t.name}{#if owns}<button
                class="x"
                aria-label="Take {t.name} out of {o.name}"
                onclick={() => void run(assignTrackerOrg(t.id, null), 'Unassign failed')}>×</button
              >{/if}</span
          >
        {/each}
      </div>
      {#if owns}
        {@const d = draftOf(o)}
        <div class="row">
          <select
            aria-label="Rule kind"
            value={d.kind}
            onchange={(e) =>
              (ruleDraft = {
                ...ruleDraft,
                [o.id]: { ...d, kind: (e.currentTarget as HTMLSelectElement).value as 'owner' | 'path' | 'host' },
              })}
          >
            <option value="owner">owner[/repo]</option>
            <option value="path">path</option>
            <option value="host">host</option>
          </select>
          <input
            data-testid="org-rule-input"
            placeholder={d.kind === 'owner' ? 'acme or acme/api' : d.kind === 'path' ? '/home/me/work/acme' : 'hetzner-a'}
            value={d.value}
            oninput={(e) => (ruleDraft = { ...ruleDraft, [o.id]: { ...d, value: (e.currentTarget as HTMLInputElement).value } })}
          />
          <button class="btn" data-testid="org-rule-add" disabled={busy || !d.value.trim()} onclick={() => void addRule(o)}
            >Add rule</button
          >
        </div>
        <div class="row">
          <select
            aria-label="Put a host in {o.name}"
            data-testid="org-assign-host"
            onchange={(e) => {
              const v = (e.currentTarget as HTMLSelectElement).value;
              if (v) void run(assignHostOrg(v, o.id), 'Assign host failed');
            }}
          >
            <option value="">+ host…</option>
            {#each $hosts.filter((h) => h.org_id !== o.id) as h (h.alias)}
              <option value={h.alias}>{h.alias}</option>
            {/each}
          </select>
          <select
            aria-label="Put a tracker in {o.name}"
            data-testid="org-assign-tracker"
            onchange={(e) => {
              const v = Number((e.currentTarget as HTMLSelectElement).value);
              if (v) void run(assignTrackerOrg(v, o.id), 'Assign tracker failed');
            }}
          >
            <option value="">+ tracker…</option>
            {#each $trackers.filter((t) => t.org_id !== o.id) as t (t.id)}
              <option value={t.id}>{t.name}</option>
            {/each}
          </select>
          <label class="isolate" title="Also hide this org's sessions from other orgs' hosts (and theirs from its hosts): list, peer status, messages. It can break a controller that dispatches across companies.">
            <input
              type="checkbox"
              data-testid="org-isolate"
              checked={o.isolate_sessions ?? false}
              onchange={(e) =>
                void run(
                  updateOrg(o.id, { isolate_sessions: (e.currentTarget as HTMLInputElement).checked }),
                  'Update failed',
                )}
            />
            isolate sessions
          </label>
          <label class="isolate" title="Auto-tidy for this org's sessions: on or off regardless of the fleet-wide setting, or inherit it (Settings → Work → Lifecycle). Safe kill only, never a session in use.">
            auto-tidy
            <select
              data-testid="org-auto-tidy"
              value={orgAutoTidy(o)}
              onchange={(e) =>
                void run(
                  updateOrg(o.id, {
                    auto_tidy: (e.currentTarget as HTMLSelectElement).value as OrgAutoTidy,
                  }),
                  'Update failed',
                )}
            >
              <option value="inherit">inherit</option>
              <option value="on">on</option>
              <option value="off">off</option>
            </select>
          </label>
        </div>
        {#if o.isolate_sessions}
          <p class="hint warn">
            Hosts outside {o.name} can no longer list or message its sessions, and its hosts cannot
            see other orgs' sessions. Work data is fenced either way.
          </p>
        {/if}
      {/if}
    </div>
  {/each}

  {#if !owns}
    <p class="hint" data-testid="org-remote">
      {hubBlock('add_org', $hubStatus)} On the hub: <code>fleet-hub org add &lt;name&gt;</code>,
      <code>fleet-hub org rule add &lt;id&gt; --owner &lt;owner&gt;</code>,
      <code>fleet-hub org assign-host &lt;host&gt; &lt;id&gt;</code>.
    </p>
  {:else}
    <form
      class="row"
      data-testid="org-add-form"
      onsubmit={(e) => {
        e.preventDefault();
        const n = newName.trim();
        if (!n) return;
        void run(addOrg(n, newColor, false), 'Add org failed').then(() => (newName = ''));
      }}
    >
      <input data-testid="org-add-name" placeholder="Company A" bind:value={newName} />
      <input type="color" aria-label="Colour" bind:value={newColor} />
      <button class="btn" type="submit" data-testid="org-add" disabled={busy || !newName.trim()}>Add org</button>
    </form>
  {/if}
</div>

<style>
  .orgs {
    margin-top: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  h5 {
    margin: 0;
    font-size: 0.8rem;
  }
  .blurb,
  .hint {
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .hint.warn {
    color: var(--warn, #f59e0b);
  }
  .suggestions {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .org {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.35rem 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.8rem;
  }
  .org-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .swatch {
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 2px;
    border: 1px solid var(--border);
  }
  .badge {
    font-size: 0.68rem;
    padding: 0 0.35rem;
    border-radius: 4px;
    border: 1px solid var(--border);
    color: var(--warn, #f59e0b);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }
  .chip {
    font-size: 0.72rem;
    padding: 0 0.4rem;
    border-radius: 999px;
    border: 1px solid var(--border);
  }
  .x {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0 0 0 0.2rem;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    align-items: center;
  }
  .row input:not([type='color']):not([type='checkbox']) {
    font: inherit;
    font-size: 0.78rem;
    padding: 0.2rem 0.35rem;
  }
  .isolate {
    font-size: 0.75rem;
    display: flex;
    align-items: center;
    gap: 0.2rem;
  }
  .btn {
    font-size: 0.75rem;
  }
</style>
