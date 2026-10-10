<script lang="ts">
  import HubProjectsPick from './HubProjectsPick.svelte';
  import Icon from './kit/Icon.svelte';
  import Loader from './Loader.svelte';
  import WorkSettings from './WorkSettings.svelte';
  import ShortcutSettings from './ShortcutSettings.svelte';
  import { DEFAULT_LEAF, leafById, leafForPage, resolveSection, type PanelId } from './settings_tree';
  import { onDestroy, onMount, tick } from 'svelte';
  import PageView from './pages/PageView.svelte';
  import SettingsNav from './pages/SettingsNav.svelte';
  import {
    descriptors,
    loadDescriptors,
    loadPages,
    pagesBundle,
    settingValues,
  } from './pages/pages';
  import { subscribeToRowEvents } from './events';
  import { loadProposals, settingProposals, settingsWritable } from './pages/review';
  import { allPages, guideProposals, loadGuides } from './pages/guides';
  import { hosts } from './hosts';
  import { mcpStatus } from './mcp';
  import { healthCheck } from './ipc';
  import { appVersion, loadAppVersion } from './app_version';
  import { onboardingDismissed, onboardingWelcomed } from './onboarding';
  import { hintsEnabled, resetHints } from './hints';
  import { toolkitTab } from './toolkit_skills';
  import { goTo } from './destination';
  import { copyOnSelect } from './prefs';
  import { startTour } from './tour';
  import { collectDiagnostics, copyDiagnostics, openLogFolder } from './diagnostics';
  import { pushError } from './toasts';
  import WizardDialog from './forms/WizardDialog.svelte';
  import { WIZARDS } from './forms/wizards';
  import type { Values } from './forms/forms';
  import McpSettings from './McpSettings.svelte';
  import AppearanceSettings from './AppearanceSettings.svelte';
  import { loadHostTokens } from './host_actions';
  import { hostsChordLabel, requestHostsView, settingsKey, settingsOpen, settingsSection } from './app_views';
  import { detectMac } from './terminal_keys';
  import { copyText } from './clipboard';
  import './settings_dialog.css';
  import {
    fleetSettings,
    loadFleetSettings,
    setFleetSetting,
    SETTING_KEYS,
    PROJECTS_LOCAL_ENV_KEY,
    settingPathMap,
    settingLayout,
    basePathError,
    projectPathPreview,
    projectsDefaultRoot,
    type ProjectsLayout,
  } from './fleet_settings';
  import { refreshProjects } from './projects';
  import {
    hubStatus,
    hubPair,
    hubDisconnect,
    hubBlock,
    resourceBlock,
    hubStrandedToken,
    ownsTheFleet,
    plainUnavailableReason,
  } from './hub';
  import {
    attentionIdleMinutes,
    notificationPermission,
    notifyStuckOs,
    notifyStuckToast,
    requestNotificationPermission,
    type NotificationPermissionState,
  } from './notify';

  let { onClose }: { onClose: () => void } = $props();

  // The chip editor lives in Toolkit › Prompts & snippets (UX audit
  // 2026-10-09, Martin's call); Settings keeps a link to it.
  function openPrompts() {
    toolkitTab.set('prompts');
    goTo('assets');
    onClose();
  }

  // Hosts live in the Hosts view; Settings keeps fleet-wide configuration and
  // a one-line summary that opens the view.
  let mcpSettings = $state<ReturnType<typeof McpSettings>>();
  const hostsChord = hostsChordLabel(
    detectMac(typeof navigator === 'undefined' ? undefined : navigator),
  );
  const offlineCount = $derived($hosts.filter((h) => !h.reachable).length);

  async function openHosts() {
    onClose();
    // After the dialog has unmounted and restored focus, so the Hosts view
    // remembers the right element to hand focus back to.
    await tick();
    requestHostsView();
  }

  // --- Hub: which fleet this window is onto ---
  // Pointed at a hub, four of the sections below are about a fleet this app
  // does not own: their commands are guarded on the backend and answer
  // E_LOCAL_ONLY. They are replaced by the reason rather than left to fail at
  // the click — and, just as importantly, their `onMount` fetches are not made
  // at all, or opening Settings would raise two error toasts every time.
  const isRemote = $derived($hubStatus.remote);
  // The hub's own version, for the Hub section's one plain sentence about
  // which is which. Read here rather than handed down: the footer's line
  // (`App.svelte` + `app_version.ts`) is the always-on-screen answer, and
  // this is the screen somebody opens when that line raised the question.
  let hubVersion = $state<string | null>(null);
  // Told apart from "not read yet", because the two are opposite news and
  // the read is one round trip away: an unreachable hub says so, an
  // in-flight one says nothing.
  let hubVersionFailed = $state(false);
  // Not the same thing: a configured hub this launch cannot use is not a hub
  // client, but it owns no fleet either, and the backend refuses the same
  // panels. See `ownsTheFleet`.
  const ownsFleet = $derived(ownsTheFleet($hubStatus));
  /** The URL the link wizard opens with: the configured hub's, to pair again. */
  let hubUrlDraft = $state('');
  /** The hub link wizard (step 10.12: one fleet.form/1 spec, `link_hub`). */
  let hubLinking = $state(false);
  let hubAllowPlaintext = $state(false);
  /** Set only after the backend has refused plaintext *in words*: the opt-in
   *  is not a checkbox anyone can tick past without having read why. */
  let hubPlaintextRefused = $state(false);
  let hubBusy = $state(false);
  let hubError: string | null = $state(null);
  let hubRestartNeeded = $state(false);
  /** A client token left on this machine by a pairing that crashed before it
   *  wrote its URL. No launch reads it, and nothing else offers to clear it —
   *  see `hubStrandedToken`. Asked once, on open, and only while standalone. */
  let hubStranded = $state(false);

  function openHubLink() {
    hubError = null;
    hubPlaintextRefused = false;
    hubAllowPlaintext = false;
    hubLinking = true;
  }

  async function doPair(values: Values) {
    hubBusy = true;
    hubError = null;
    const r = await hubPair(String(values.url ?? ''), String(values.code ?? ''), hubAllowPlaintext);
    hubBusy = false;
    if (r.ok) {
      hubRestartNeeded = r.value.restart_required;
      hubPlaintextRefused = false;
      hubUrlDraft = r.value.configured_url ?? String(values.url ?? '');
      // The code dies on first use: the wizard closes with it rather than
      // inviting a second attempt that can only fail.
      hubLinking = false;
    } else {
      hubError = r.error.message;
      if (r.error.code === 'E_HUB_PLAINTEXT') hubPlaintextRefused = true;
    }
  }

  async function doDisconnect() {
    hubBusy = true;
    hubError = null;
    const r = await hubDisconnect();
    hubBusy = false;
    if (r.ok) {
      hubRestartNeeded = r.value.restart_required;
      hubUrlDraft = '';
      // Disconnect clears the token too, so whatever was stranded is gone.
      hubStranded = false;
    } else {
      hubError = r.error.message;
    }
  }


  // --- The Settings tree (redesign step 7.1, `settings_tree.ts`): a leaf
  // is a hand-written panel below, a generated page (declarative pages P3;
  // `crates/fleet-core/pages/`) or one section of one, picked in the nav. ---
  let view = $state(DEFAULT_LEAF);
  let focusKey = $state<string | null>(null);
  let settingsLoadError: string | null = $state(null);
  // Declarative pages P6: on a paired desktop the pages are the hub's
  // settings, read (and, when the hub's operator trusts this device,
  // written) through it. `off` until they load; a refusal keeps the reason.
  let hubPages = $state<'off' | 'loading' | 'ok'>('off');
  let hubPagesError = $state<string | null>(null);
  /** The generated pages have the fleet's settings to show here. */
  const pagesHere = $derived(ownsFleet || hubPages === 'ok');
  const leaf = $derived(leafById(view, $allPages) ?? leafById(DEFAULT_LEAF, $allPages));
  const panel = $derived<PanelId | undefined>(leaf?.panel);
  // The panel edits the same settings as its page where this app owns the
  // fleet; elsewhere the page (the hub's settings) stands in for it.
  const currentPage = $derived(
    leaf?.page && !(leaf.pageOnlyRemote && ownsFleet) ? $allPages.find((p) => p.id === leaf.page) : undefined,
  );

  /** Open a leaf by id, or the leaf showing a page (a search hit, a link
   *  between pages): the one holding `key`'s section when there is one. */
  function select(next: string, key?: string) {
    view = leafById(next, $allPages) ? next : (leafForPage(next, $allPages, key)?.id ?? `page:${next}`);
    focusKey = key ?? null;
  }

  // Another window, or an agent over the control API, changed a setting:
  // re-read the values so an open page never shows a stale one.
  let unlistenSettings: (() => void) | null = null;
  let destroyed = false;
  onDestroy(() => {
    destroyed = true;
    unlistenSettings?.();
  });

  // Opened at a section (a "Reconnect Jira (acme)" Attention item, work
  // graph M12.4): a leaf id, a page id or one of the old General panels'
  // names opens its leaf, then the request is forgotten.
  onMount(() => {
    const section = $settingsSection;
    if (!section) return;
    const key = $settingsKey;
    settingsSection.set(null);
    settingsKey.set(null);
    // With a setting named, a page id opens the leaf that holds it.
    select(key ? section : (resolveSection(section, $allPages) ?? section), key ?? undefined);
  });

  async function subscribeSettings() {
    // A write, a proposal or a review: re-read the values and what waits.
    const off = await subscribeToRowEvents({
      onSettingsChanged: () => {
        void loadFleetSettings();
        void loadProposals();
      },
    });
    if (destroyed) off();
    else unlistenSettings = off;
  }

  /** A paired desktop: the hub's settings, proposals and whether this
   *  device may change them. A hub that refuses (older, or this device is
   *  bound to one org) leaves the pages on its reason. */
  async function loadHubPages() {
    hubPages = 'loading';
    const [fs, ds] = await Promise.all([loadFleetSettings(), loadDescriptors(), loadProposals()]);
    const failed = !ds.ok ? ds : !fs.ok ? fs : null;
    if (failed && !failed.ok) {
      hubPages = 'off';
      hubPagesError = failed.error.message;
      return;
    }
    hubPages = 'ok';
    resetProjectDrafts();
    await subscribeSettings();
  }

  onMount(async () => {
    hubUrlDraft = $hubStatus.configured_url ?? '';
    // The page specs are compiled in: the same list whether or not a hub
    // owns the fleet, so the nav shows it either way.
    void loadPages();
    // Guides an agent proposed and a person approved: this app's, or the
    // hub's on a paired desktop.
    void loadGuides();
    if (!ownsTheFleet($hubStatus)) {
      // The control API and the stranded-token check do not apply to a hub
      // client, and both are guarded on the backend. The settings do: a
      // connected hub serves them (P6).
      if ($hubStatus.remote) {
        void loadAppVersion();
        // Not awaited and never surfaced as an error: a hub that cannot be
        // reached has the banner and the footer already, and this line just
        // stays off.
        void healthCheck().then((r) => {
          if (r.ok && r.value) hubVersion = r.value.version;
          else hubVersionFailed = true;
        });
        await loadHubPages();
      }
      return;
    }
    const r = await mcpStatus();
    // Optional call: Svelte nulls a `bind:this` ref on teardown, so closing
    // Settings while mcpStatus() is in flight leaves it unset — and a throw
    // here would also skip resetProjectDrafts() below.
    mcpSettings?.applyStatus(r);
    const [fs, ds] = await Promise.all([loadFleetSettings(), loadDescriptors(), loadProposals()]);
    if (!fs.ok) settingsLoadError = fs.error.message;
    else if (!ds.ok) settingsLoadError = ds.error.message;
    resetProjectDrafts();
    await subscribeSettings();
    // Last, and only standalone: on macOS this reads the keychain, which is
    // the one call here that can block on a locked one. Nothing else on this
    // screen waits for it, and with a hub configured there is nothing to ask
    // — that token belongs to that hub and Disconnect is on screen already.
    if (!$hubStatus.configured_url) {
      const st = await hubStrandedToken();
      // A token store that will not open is not evidence of a leftover, and
      // this app is working normally otherwise; an error toast on every
      // Settings open would be noise about a state that almost certainly does
      // not exist. The backend's error carries the reason for a log.
      hubStranded = st.ok && st.value === true;
    }
  });

  // --- Projects: per-host projects root + layout (backend settings) ---
  // Drafts are edited locally and written together by "Save & rescan".
  let baseDrafts = $state<Record<string, string>>({});
  let layoutDraft = $state<ProjectsLayout>('github');
  let projectsBusy = $state(false);
  // Controls stay disabled until the drafts are seeded from the backend, so
  // typing during the initial load cannot be wiped by the seeding.
  let projectsLoaded = $state(false);
  let projectsError: string | null = $state(null);
  let projectsMsg: string | null = $state(null);
  const savedBases = $derived(settingPathMap($fleetSettings, SETTING_KEYS.projectsBasePath));
  const localEnv = $derived(($fleetSettings[PROJECTS_LOCAL_ENV_KEY] ?? '').trim());
  const savedLayout = $derived(settingLayout($fleetSettings));
  const projectsInvalid = $derived(
    Object.values(baseDrafts).some((p) => basePathError(p) !== null),
  );

  function resetProjectDrafts() {
    baseDrafts = { ...savedBases };
    layoutDraft = savedLayout;
    projectsLoaded = true;
  }

  // Root a host uses when its field is blank: on this machine the env var
  // if set, otherwise (and on every remote host) the default for the layout
  // currently selected, so the preview tracks an unsaved layout change.
  function fallbackRoot(alias: string): string {
    if (alias === 'local' && localEnv) return localEnv;
    return projectsDefaultRoot(layoutDraft);
  }

  function previewRoot(alias: string): string {
    return (baseDrafts[alias] ?? '').trim() || fallbackRoot(alias);
  }

  function onBaseInput(alias: string, e: Event) {
    baseDrafts = { ...baseDrafts, [alias]: (e.currentTarget as HTMLInputElement).value };
  }

  async function saveProjects() {
    projectsBusy = true;
    projectsError = null;
    projectsMsg = null;
    const map: Record<string, string> = {};
    for (const [alias, p] of Object.entries(baseDrafts)) {
      const t = p.trim();
      if (t) map[alias] = t;
    }
    const wantLayout = layoutDraft;
    let r = await setFleetSetting(SETTING_KEYS.projectsBasePath, JSON.stringify(map));
    if (r.ok && wantLayout !== savedLayout) {
      r = await setFleetSetting(SETTING_KEYS.projectsLayout, wantLayout);
    }
    if (r.ok) {
      const pr = await refreshProjects();
      if (pr.ok) projectsMsg = `Saved. Rescanned ${pr.value?.length ?? 0} local project(s).`;
      else projectsError = pr.error.message;
      resetProjectDrafts();
    } else {
      projectsError = r.error.message;
    }
    projectsBusy = false;
  }

  // --- Notifications (stuck transitions) ---
  let permission = $state<NotificationPermissionState>(notificationPermission());
  async function enableOsNotifications() {
    permission = await requestNotificationPermission();
    if (permission === 'granted') notifyStuckOs.set(true);
  }

  function onIdleMinutesChange(e: Event) {
    const v = Number.parseInt((e.currentTarget as HTMLInputElement).value, 10);
    if (Number.isFinite(v) && v >= 0) attentionIdleMinutes.set(v);
  }

  // --- Diagnostics ---
  let diagBusy = $state(false);
  // Shown once known (after a copy, or when opening the folder failed) so the
  // user can always find the logs by hand.
  let logDir: string | null = $state(null);

  async function onCopyDiagnostics() {
    diagBusy = true;
    const b = await copyDiagnostics();
    if (b) logDir = b.log_dir;
    diagBusy = false;
  }

  async function onOpenLogFolder() {
    const r = await openLogFolder();
    if (r.ok) {
      logDir = r.value;
    } else {
      pushError(r.error, 'Open log folder failed');
      // Fall back to showing the path (with a copy button) so the logs can
      // still be found by hand. Collect only; nothing is copied here.
      if (!logDir) {
        const b = await collectDiagnostics();
        if (b.ok) logDir = b.value.log_dir;
      }
    }
  }

</script>

<!-- Settings is a rail destination (UX audit 2026-10-09, S1): a page in
     the list and right columns, its nav where the session list sits. The rail
     or Esc (App.svelte) leaves it; no modal, no close button. -->
<!-- `settings-dialog` scopes settings_dialog.css, shared with the panels. -->
<section class="settings-page settings-dialog" aria-label="Settings" data-testid="settings-page">
    <div class="settings-body">
    <div class="settings-side">
    <h2 class="settings-title">Settings</h2>
    <SettingsNav
      pages={$allPages}
      descs={$descriptors}
      values={$settingValues}
      selected={leaf?.id ?? view}
      counts={pagesHere ? { 'settings.review': $settingProposals.length, guides: $guideProposals.length } : {}}
      canWrite={pagesHere && $settingsWritable}
      onselect={select} />
    </div>
    <div class="settings-scroll">
    <div class="settings-content">
    <!-- UX audit S2: the open leaf's name heads the page (Settings board). -->
    <h2 class="page-title" data-testid="settings-page-title">{leaf?.label ?? 'Settings'}</h2>
    <!-- The hand-written panels stay mounted and are hidden when another
         leaf is open, so a draft (a hub URL, a projects root) survives a
         look at another section. -->
    <div class="panel" hidden={panel !== 'appearance'} data-testid="settings-panel-appearance">
    <AppearanceSettings />
    </div>

    <div class="panel" hidden={panel !== 'hosts'} data-testid="settings-panel-hosts">
    <section class="block hosts-line" data-testid="settings-hosts-line">
      <h4>Hosts</h4>
      <span class="hosts-summary" data-testid="settings-hosts-summary"
        >{$hosts.length} configured · {offlineCount} offline</span
      >
      <button class="hook-btn" onclick={openHosts} data-testid="settings-open-hosts"
        >Open Hosts <kbd>{hostsChord}</kbd></button
      >
    </section>
    <p class="hook-desc">
      Accounts and hosts have their own view, beside Sessions and Work. Settings keeps the
      preferences; the hosts themselves are there.
    </p>
    </div>

    <div class="panel" hidden={panel !== 'hub'} data-testid="settings-panel-hub">
    <section class="block" data-testid="hub-section">
      <div class="section-header">
        <h4>Hub</h4>
      </div>
      {#if isRemote}
        <p class="mcp-blurb" data-testid="hub-connected">
          This window is a <strong>client</strong> of
          <code>{$hubStatus.url}</code>, paired as
          <code>{$hubStatus.client_name ?? 'desktop'}</code>. The fleet lives
          there: its database, its reconcile tick, its SSH connections. This app
          runs none of them.
        </p>
        <!-- Two programs, two release trains, and until now one `v…` in the
             footer that could have been either. Both are named here, in the
             section about the pairing itself. -->
        <p class="hook-desc" data-testid="hub-versions">
          Versions: this app is
          <code>{$appVersion ?? 'unknown'}</code>, the hub is
          <code>{hubVersion ?? (hubVersionFailed ? 'not answering' : 'reading…')}</code>.
          {#if $appVersion && hubVersion && $appVersion !== hubVersion}
            They differ, which is allowed — the two are released separately,
            and what decides whether they can talk is the wire contract, not
            matching versions. A contract this app cannot accept shows up as
            its own banner, not as this line.
          {/if}
        </p>
        <HubProjectsPick />
        {#if $hubStatus.warning}
          <p class="err" data-testid="hub-status-warning"><Icon name="warning" size={12} /> {$hubStatus.warning}</p>
        {/if}
        <!-- The trap: with `mcp.confirm_destructive` on, the hub refuses a
             kill, a worktree delete, a move or a task cancel until someone
             approves it. This desktop's confirmation dialog answers its OWN
             queue, which in remote mode is always empty — so the click is
             refused and there is nowhere to go unless we say where. -->
        <p class="hook-desc" data-testid="hub-confirm-note">
          If the hub has <code>mcp.confirm_destructive</code> on, killing a
          session, deleting a worktree, moving a session or cancelling a task
          comes back refused with <code>E_CONFIRM_REQUIRED</code>: this app's
          confirmation dialog answers its own queue, which is empty here.
          <strong>Approve it on the hub — this window will follow</strong>, as
          the change arrives over the hub's live event stream.
        </p>
        <!-- Correct behaviour, and a real difference from standalone that
             nobody would guess. `mcp::tools::support::apply_marker` refuses
             `raw=true` to any non-master caller, and a paired client is never
             the master. -->
        <p class="hook-desc" data-testid="hub-untrusted-note">
          A prompt sent from here reaches the agent marked
          <strong>untrusted</strong>, exactly as one typed on a phone does: a
          paired client is never the hub's master, and the hub marks every
          non-master prompt. Standalone, the desktop is the master and does
          not.
        </p>
        <div class="mcp-field">
          <button
            class="hook-btn"
            onclick={doDisconnect}
            disabled={hubBusy}
            data-testid="hub-disconnect">Disconnect</button>
        </div>
        <p class="hook-desc" data-testid="hub-disconnect-note">
          Disconnect forgets the URL and the client token <em>on this
          machine</em>. It <strong>does not revoke</strong> anything: the
          client stays in the hub's list and its token stays valid until an
          operator revokes it there (<code>fleet-hub client revoke</code>). A
          paired client is refused <code>revoke_client</code> by design, so
          this app could not do it even if it tried.
        </p>
      {:else if $hubStatus.unavailable}
        <!-- A hub is configured and this launch could not use it. Saying
             "This app runs its own fleet" here, as this section used to, was
             the opposite of the truth: it runs NO fleet until this is fixed. -->
        <p class="err" data-testid="hub-unavailable-reason">
          <Icon name="warning" size={12} /> This app is set to use
          {#if $hubStatus.configured_url}<code>{$hubStatus.configured_url}</code>{:else}a hub{/if},
          but this launch cannot: {plainUnavailableReason($hubStatus.unavailable)}.
        </p>
        <details class="hook-desc" data-testid="hub-unavailable-detail">
          <summary>Details</summary>
          <code>{$hubStatus.unavailable}</code>
        </details>
        <p class="hook-desc">
          Until that is fixed it manages no fleet at all — no reconcile tick, no
          control API, every fleet action refused — rather than quietly
          managing the hub's hosts behind the hub's back. Pair again below, or
          Disconnect to go back to running this app's own fleet. Either takes
          effect at the next launch.
        </p>
        <div class="mcp-field">
          <button
            class="hook-btn"
            onclick={doDisconnect}
            disabled={hubBusy}
            data-testid="hub-disconnect">Disconnect</button>
        </div>
        <p class="hook-desc" data-testid="hub-disconnect-note">
          Disconnect forgets the URL and any client token <em>on this
          machine</em>. It <strong>does not revoke</strong> anything on the hub:
          an operator does that with <code>fleet-hub client revoke</code>.
        </p>
      {:else}
        <p class="mcp-blurb" data-testid="hub-empty">
          This app runs its own fleet: its own database, its own reconcile
          tick, its own control API. Point it at a <code>fleet-hub</code> and
          it becomes a window onto that fleet instead — the same sessions a
          phone sees, live.
        </p>
        <p class="hook-desc">
          On the hub, run <code>fleet-hub pair --name &lt;this machine&gt;</code>
          and paste the code it prints. The code can be used once and expires
          in minutes.
        </p>
        {#if hubStranded}
          <!-- A pairing that crashed before it wrote its URL (the old write
               order) left a fleet-wide client token on this machine. No launch
               reads it — a blank URL is standalone — so this is the only place
               it is ever mentioned, and the only place it can be cleared. -->
          <p class="err" data-testid="hub-stranded-token">
            <Icon name="warning" size={12} /> A hub <strong>client token</strong> is still stored on this
            machine, left behind by a pairing that did not finish. Nothing uses
            it: no hub is configured, and this app runs its own fleet. It is a
            credential for someone else's fleet sitting in this machine's
            secure storage, so clear it unless you are about to pair again.
          </p>
          <div class="mcp-field">
            <button
              class="hook-btn"
              onclick={doDisconnect}
              disabled={hubBusy}
              data-testid="hub-disconnect">Clear leftover token</button>
          </div>
          <p class="hook-desc">
            This <strong>does not revoke</strong> anything. If that token was
            ever issued, its client row is still on the hub and the token is
            still valid there until an operator removes it
            (<code>fleet-hub client revoke</code>).
          </p>
        {/if}
      {/if}

      {#if !isRemote || hubRestartNeeded}
        <div class="mcp-field">
          <button onclick={openHubLink} disabled={hubBusy} data-testid="hub-link">
            {$hubStatus.configured_url ? 'Pair again…' : 'Link to a hub…'}
          </button>
        </div>
      {/if}

      {#if hubError && !hubLinking}
        <p class="err" role="alert" data-testid="hub-error">{hubError}</p>
      {/if}
      {#if hubLinking}
        <WizardDialog
          wizard={WIZARDS.link_hub}
          initial={{ url: hubUrlDraft }}
          busy={hubBusy}
          error={hubError}
          errorTestid="hub-error"
          run={(v) => void doPair(v)}
          onclose={() => (hubLinking = false)}>
          {#snippet extra()}
            {#if hubPlaintextRefused}
              <!-- Only after the refusal, and only with the reason above it: the
                   opt-in is a decision someone makes having read what it costs,
                   not a box that was already there to be ticked past. -->
              <label class="toggle">
                <input
                  type="checkbox"
                  bind:checked={hubAllowPlaintext}
                  data-testid="hub-allow-plaintext" />
                Send the client token in the clear anyway — this hop is already
                private (a tunnel, a VPN, a container network)
              </label>
            {/if}
          {/snippet}
        </WizardDialog>
      {/if}
      {#if hubRestartNeeded}
        <p class="hook-desc" data-testid="hub-restart">
          Saved. <strong>Restart Orbit Fleet to apply it</strong> — which
          fleet this app is a window onto is decided once, at startup, so that
          half the app can never be talking to a hub while the other half
          talks to the local database.
        </p>
      {/if}
    </section>
    </div>

    <div class="panel" hidden={panel !== 'projects'} data-testid="settings-panel-projects">
    <!-- A paired desktop: the leaf shows the hub's Projects page instead. -->
    {#if ownsFleet}
    <section class="block" data-testid="projects-section">
      <div class="section-header">
        <h4>Projects</h4>
      </div>
      <p class="mcp-blurb">
        Where each host keeps its git repositories. Leave a host blank for the
        default (<code>~/projects/github.com</code>, or
        <code>$CLAUDE_FLEET_PROJECTS_BASE</code> on this machine). Paths must be
        absolute or start with <code>~/</code>.
      </p>
      <div class="mcp-field">
        <span class="lbl">Layout</span>
        <select
          class="layout-select"
          bind:value={layoutDraft}
          disabled={projectsBusy || !projectsLoaded}
          data-testid="projects-layout"
          aria-label="Projects layout">
          <option value="github">github: &lt;base&gt;/&lt;owner&gt;/&lt;repo&gt;</option>
          <option value="flat">flat: &lt;base&gt;/&lt;repo&gt;</option>
        </select>
      </div>
      {#each $hosts as h (h.alias)}
        {@const draft = baseDrafts[h.alias] ?? ''}
        {@const pathErr = basePathError(draft)}
        <div class="project-base-row">
          <div class="mcp-field">
            <span class="lbl project-alias" title={h.alias}>{h.alias}</span>
            <input
              class="port base-input"
              class:invalid={pathErr !== null}
              type="text"
              spellcheck="false"
              value={draft}
              placeholder={fallbackRoot(h.alias)}
              disabled={projectsBusy || !projectsLoaded}
              data-testid="projects-base-{h.alias}"
              aria-label="Projects base path for {h.alias}"
              oninput={(e) => onBaseInput(h.alias, e)} />
          </div>
          <span
            class="hook-desc project-preview"
            class:err={pathErr !== null}
            data-testid="projects-preview-{h.alias}">
            {pathErr ?? projectPathPreview(previewRoot(h.alias), layoutDraft)}
          </span>
        </div>
      {/each}
      <div class="mcp-field">
        <button
          onclick={saveProjects}
          disabled={projectsBusy || projectsInvalid || !projectsLoaded}
          data-testid="projects-save">Save &amp; rescan</button>
        {#if projectsMsg}<span class="hook-desc" data-testid="projects-msg">{projectsMsg}</span>{/if}
      </div>
      {#if projectsError}<p class="err">{projectsError}</p>{/if}
    </section>
    {/if}
    </div>

    <div class="panel" hidden={panel !== 'appearance'} data-testid="settings-panel-onboarding">
    <section class="block" data-testid="onboarding-section">
      <div class="section-header">
        <h4>Setup guide</h4>
      </div>
      <!-- UX audit S2: one row per item, its name and help on the left and
           its control on the right (Settings board). -->
      <div class="pref-row">
        <div class="pref-text">
          <span class="pref-lbl">Get started checklist</span>
          <p class="hook-desc">Re-show the "Get started" checklist.</p>
        </div>
        <button
          class="hook-btn"
          onclick={() => {
            onboardingWelcomed.set(true);
            onboardingDismissed.set(false);
          }}
        >
          Replay setup guide
        </button>
      </div>
      <div class="pref-row">
        <div class="pref-text">
          <span class="pref-lbl">Tour</span>
          <p class="hook-desc">Walk through the main parts of the window again, six short steps.</p>
        </div>
        <button
          class="hook-btn"
          data-testid="settings-take-tour"
          onclick={() => {
            settingsOpen.set(false);
            startTour();
          }}
        >
          Take the tour
        </button>
      </div>
      <div class="pref-row">
        <div class="pref-text">
          <span class="pref-lbl">Feature hints</span>
          <p class="hook-desc">Show inline tips the first time a feature is used.</p>
        </div>
        <div class="pref-ctl">
          <label class="toggle">
            <input type="checkbox" bind:checked={$hintsEnabled} />
            Show feature hints
          </label>
          <button class="hook-btn" onclick={resetHints} data-testid="reset-hints">
            Reset hints
          </button>
        </div>
      </div>
      <div class="pref-row">
        <div class="pref-text">
          <span class="pref-lbl">Copy on select</span>
          <p class="hook-desc">
            Copy a terminal drag-selection to the clipboard as soon as the mouse
            is released. Off: use Cmd+C / Ctrl+Shift+C or the context menu.
          </p>
        </div>
        <label class="toggle">
          <input type="checkbox" bind:checked={$copyOnSelect} data-testid="copy-on-select" />
          Copy on select
        </label>
      </div>
    </section>
    </div>

    <div class="panel" hidden={panel !== 'notifications'} data-testid="settings-panel-notifications">
    <section class="block" data-testid="notifications-section">
      <div class="section-header">
        <h4>Stuck sessions</h4>
      </div>
      <p class="mcp-blurb">
        When a session becomes stuck (auth menu, trust prompt, reconnect,
        out of memory, press Enter) fleet announces it for screen readers and,
        optionally, shows a toast and an OS notification. With OS notifications
        on, the hub's Notifications page decides which other states reach this
        desktop (and which make a sound) while the window is in the background.
      </p>
      <label class="toggle">
        <input type="checkbox" bind:checked={$notifyStuckToast} data-testid="notify-toast" />
        In-app toast on stuck transitions
      </label>
      <div class="mcp-row">
        <label class="toggle">
          <input
            type="checkbox"
            checked={$notifyStuckOs}
            disabled={permission === 'unsupported'}
            data-testid="notify-os"
            onchange={() => {
              if ($notifyStuckOs) notifyStuckOs.set(false);
              else void enableOsNotifications();
            }} />
          OS notifications
        </label>
        <span class="status status-{permission === 'granted' ? 'on' : permission === 'denied' ? 'off' : 'neutral'}" data-testid="notify-permission">
          {permission}
        </span>
      </div>
      {#if permission === 'unsupported'}
        <p class="hook-desc">This webview does not expose the Notification API; toasts and the live region still work.</p>
      {:else if permission === 'denied'}
        <p class="hook-desc">Notifications were denied at the OS level; allow them for Orbit Fleet (listed as claude-fleet) in your system settings.</p>
      {/if}
      <div class="mcp-field">
        <span class="lbl">Idle</span>
        <input
          class="port"
          type="number"
          min="0"
          value={$attentionIdleMinutes}
          onchange={onIdleMinutesChange}
          data-testid="attention-idle-minutes" />
        <span class="hook-desc">minutes before an idle work session counts as "needs attention" (0 = never)</span>
      </div>
    </section>
    </div>

    {#if panel === 'shortcuts'}
      <ShortcutSettings />
    {/if}

    <div class="panel" hidden={panel !== 'composer'} data-testid="settings-panel-composer">
    <section class="block" data-testid="composer-section">
      <h4>Conversation composer</h4>
      <div class="hook-section">
        <p class="hook-desc">
          The quick-action chips above the prompt box are edited in Toolkit ›
          Prompts & snippets.
        </p>
        <button class="hook-btn" data-testid="composer-open-toolkit" onclick={openPrompts}
          >Open Prompts & snippets ↗</button
        >
      </div>
    </section>
    </div>

    <div class="panel" hidden={panel !== 'work'} data-testid="settings-panel-work">
    <WorkSettings onopen={(id) => select(id)} />
    </div>

    <div class="panel" hidden={panel !== 'mcp'} data-testid="settings-panel-mcp">
    {#if !ownsFleet}
      <section class="block" data-testid="mcp-remote-section">
        <div class="section-header"><h4>Control API (MCP)</h4></div>
        <p class="hook-desc" data-testid="mcp-remote">
          {hubBlock('mcp_status', $hubStatus)}
        </p>
        <p class="hook-desc" data-testid="provision-remote">
          {hubBlock('provision_hosts', $hubStatus)}
        </p>
      </section>
    {:else}
    <!-- Provisioning mints host tokens: refresh the shared token cache the
         Hosts view reads (host_actions.ts). Module-level, so it is safe even
         when a slow multi-host provision outlives this dialog. -->
    <McpSettings bind:this={mcpSettings} onProvisioned={loadHostTokens} active={panel === 'mcp'} />
    {/if}
    </div>

    <div class="panel" hidden={panel !== 'diagnostics'} data-testid="settings-panel-diagnostics">
    <!-- Redesign step 3.13: About gets the Wordmark reveal. It sits with
         Diagnostics, the section that already names the version, until the
         Settings tree (7.1) gives About a leaf of its own. -->
    <section class="block about" data-testid="about-section">
      <Loader name="wordmark-reveal" size={184} delay={0} label="Orbit Fleet" testid="about-wordmark" />
      <p class="hook-desc" data-testid="about-version">Orbit Fleet {$appVersion ?? ''}</p>
    </section>
    <section class="block" data-testid="diagnostics-section">
      <div class="section-header">
        <h4>Diagnostics</h4>
      </div>
      <p class="hook-desc">
        Copy a plain-text report for a bug report: app version, schema, hosts,
        tunnels, control-API state, session counts and the last 200 log lines.
        Tokens are never included; hostnames and paths are.
      </p>
      <div class="hook-actions">
        <button
          class="hook-btn"
          onclick={onCopyDiagnostics}
          disabled={diagBusy}
          data-testid="copy-diagnostics"
        >
          {#if diagBusy}<Loader name="comet" size={12} class="btn-loader" />{/if}{diagBusy ? 'Collecting…' : 'Copy diagnostics'}
        </button>
        <button class="hook-btn" onclick={onOpenLogFolder} data-testid="open-log-folder">
          Open log folder
        </button>
      </div>
      {#if logDir}
        <p class="hook-desc log-path" data-testid="log-dir">
          Logs: <code>{logDir}</code>
          <button onclick={() => copyText(logDir ?? '')} aria-label="Copy log folder path">Copy path</button>
        </p>
      {/if}
    </section>
    </div>

    {#if currentPage}
      {#if !ownsFleet && currentPage.layout === 'master_detail'}
        <!-- A resource's list routes to the hub: shown, read-only, with
             where to change it instead. -->
        {@const res = $pagesBundle.resources.find((r) => r.id === currentPage.resource)}
        {#key view}
          <PageView
            page={currentPage}
            pages={$allPages}
            descs={$descriptors}
            values={$settingValues}
            sources={$pagesBundle.sources}
            resources={$pagesBundle.resources}
            actions={$pagesBundle.actions}
            readonly
            reason={res ? resourceBlock(res, $hubStatus) : null}
            section={leaf?.section}
            onnavigate={(id) => select(id)} />
        {/key}
      {:else if !pagesHere}
        <section class="block" data-testid="pages-remote">
          <div class="section-header"><h4>{leaf?.section ?? currentPage.title}</h4></div>
          {#if hubPages === 'loading'}
            <p class="hook-desc">Reading the hub’s settings…</p>
          {:else}
            <p class="hook-desc">{hubBlock('fleet_settings', $hubStatus)}</p>
            {#if hubPagesError}<p class="hook-desc" data-testid="pages-remote-error">The hub answered: {hubPagesError}</p>{/if}
          {/if}
        </section>
      {:else}
        {#if settingsLoadError}<p class="err" role="alert" data-testid="settings-load-error">{settingsLoadError}</p>{/if}
        {#if !ownsFleet}
          <!-- P6: one line, not a refusal per section. -->
          <p class="hook-desc hub-scope" data-testid="hub-scope-note">
            The fleet’s settings are the hub’s: read from and written to {$hubStatus.url ?? 'the hub'}{#if $hubStatus.client_name} (paired as {$hubStatus.client_name}){/if}.
            {#if !$settingsWritable}
              <span data-testid="hub-scope-readonly">This device reads them. To change them here, the hub’s operator trusts it:
                <code>fleet-hub client trust {$hubStatus.client_name ?? '<name>'}</code>.</span>
            {/if}
          </p>
        {/if}
        {#key view}
          <PageView
            readonly={!ownsFleet && !$settingsWritable}
            remote={!ownsFleet}
            page={currentPage}
            pages={$allPages}
            descs={$descriptors}
            values={$settingValues}
            sources={$pagesBundle.sources}
            resources={$pagesBundle.resources}
            actions={$pagesBundle.actions}
            {focusKey}
            section={leaf?.section}
            proposals={$settingProposals}
            onnavigate={(id) => select(id)}
            onopen={(id, key) => select(id, key)} />
        {/key}
      {/if}
    {/if}
    </div>
    </div>
    </div>
</section>

<style>
  .settings-page {
    height: 100%;
    min-height: 0;
    background: var(--bg);
    color: var(--fg);
  }
  .settings-body {
    display: grid;
    grid-template-columns: var(--settings-nav-w) minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }
  /* The nav takes the list column's place: its own scroll, a border on the
     right, the page title above it (Settings board). */
  .settings-side {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-height: 0;
    overflow: auto;
    padding: var(--space-3) var(--space-2) var(--space-3) var(--space-3);
    border-right: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .settings-title { margin: 0 0 var(--space-1) var(--space-1); font-size: var(--text-lg); font-weight: 600; }
  .settings-scroll { min-width: 0; min-height: 0; overflow: auto; }
  .pref-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border);
  }
  .pref-text { flex: 1; min-width: 0; }
  .pref-lbl { font-size: var(--text-sm); font-weight: 500; }
  .pref-ctl { display: flex; align-items: center; gap: var(--space-2); flex: 0 0 auto; }
  .pref-row .hook-btn { align-self: center; flex: 0 0 auto; }
  .page-title {
    margin: 0 0 var(--space-1);
    font-size: var(--text-xl);
    font-weight: 600;
  }
  .settings-content {
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
    min-width: 0;
    max-width: var(--prose-max);
    padding: var(--space-4) var(--space-6);
  }
  .panel {
    display: contents;
  }
  .panel[hidden] {
    display: none;
  }
  @media (max-width: 640px) {
    .settings-body {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(0, 1fr);
    }
    .settings-side { border-right: 0; border-bottom: 1px solid var(--border); max-height: 40vh; }
  }

  .log-path code { word-break: break-all; }

  .hosts-line {
    display: flex;
    align-items: baseline;
    gap: 0.8rem;
  }
  .hosts-line h4 {
    margin: 0;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .hosts-summary { flex: 1; font-size: var(--text-2xs); color: var(--fg-muted); font-variant-numeric: tabular-nums; }
  .hosts-line kbd {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }

  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }

  .project-base-row { margin-bottom: 0.3rem; }
  .project-base-row .mcp-field { margin-bottom: 0.1rem; }
  .mcp-field .project-alias {
    width: 6rem;
    text-transform: none;
    letter-spacing: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mcp-field .base-input {
    flex: 1;
    min-width: 0;
    width: auto;
    font-family: var(--font-mono);
  }
  /* Qualified with .hook-desc: the preview span carries both classes, and the
     later `.hook-desc { margin: 0 }` used to win the equal-specificity tie and
     cancel the indent that lines the preview up under the input. */
  .hook-desc.project-preview {
    display: block;
    margin-left: 6.4rem;
    font-family: var(--font-mono);
    word-break: break-all;
  }
  /* Same tie, for the invalid-path message: .hook-desc's muted colour used to
     beat the .err red the class:err toggle asks for. */
  .project-preview.err { color: var(--danger); }
  .layout-select {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: var(--radius-sm);
    padding: 0.2rem 0.4rem;
  }

  .hook-desc {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }

  .hub-scope {
    margin: 0 0 0.6rem;
  }
</style>
