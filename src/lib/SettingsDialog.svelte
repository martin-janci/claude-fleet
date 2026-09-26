<script lang="ts">
  import WorkSettings from './WorkSettings.svelte';
  import { onMount, tick } from 'svelte';
  import { hosts } from './hosts';
  import { mcpStatus } from './mcp';
  import { onboardingDismissed, onboardingWelcomed } from './onboarding';
  import { hintsEnabled, resetHints } from './hints';
  import { composerPresets, resetComposerPresets, addPreset, updatePreset, removePreset } from './composer_presets';
  import { copyOnSelect } from './prefs';
  import { collectDiagnostics, copyDiagnostics, openLogFolder } from './diagnostics';
  import { pushError } from './toasts';
  import Modal from './Modal.svelte';
  import McpSettings from './McpSettings.svelte';
  import { loadHostTokens } from './host_actions';
  import { hostsChordLabel, requestHostsView } from './app_views';
  import { detectMac } from './terminal_keys';
  import { copyText } from './clipboard';
  import './settings_dialog.css';
  import {
    fleetSettings,
    loadFleetSettings,
    setFleetSetting,
    settingBool,
    settingSecs,
    settingInt,
    parseHoursInput,
    parseBoundedIntInput,
    parseIntInput,
    TIDY_IDLE_UNLINKED_DAYS_MAX,
    TIDY_IDLE_UNLINKED_DAYS_MIN,
    parsePricesJsonInput,
    secsToHours,
    hoursToSecs,
    SETTING_KEYS,
    MAX_SECS,
    MOVE_MAX_TRANSCRIPT_MB_MAX,
    MOVE_MAX_BUNDLE_MB_MAX,
    MOVE_IGNORED_ENTRY_KB_MAX,
    MOVE_IGNORED_TOTAL_MB_MAX,
    MOVE_MAX_SESSION_STATE_MB_MAX,
    REPORTS_MAX_ROWS_MIN,
    REPORTS_MAX_ROWS_MAX,
    MOVE_WAIT_MAX_MINS_MAX,
    PROJECTS_LOCAL_ENV_KEY,
    settingPathMap,
    settingLayout,
    basePathError,
    projectPathPreview,
    projectsDefaultRoot,
    AUTO_TIDY_REASONS,
    parseAutoTidyReasons,
    toggleAutoTidyReason,
    type SettingKey,
    type ProjectsLayout,
  } from './fleet_settings';
  import { TIDY_REASON_LABELS, autoTidyPreview, refreshTidy, tidyReport, tidyReasonLabel, formatIdle, type TidyCandidate } from './tidy';
  import { refreshProjects } from './projects';
  import {
    hubStatus,
    hubPair,
    hubDisconnect,
    hubBlock,
    hubStrandedToken,
    ownsTheFleet,
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
  // Not the same thing: a configured hub this launch cannot use is not a hub
  // client, but it owns no fleet either, and the backend refuses the same
  // panels. See `ownsTheFleet`.
  const ownsFleet = $derived(ownsTheFleet($hubStatus));
  let hubUrlDraft = $state('');
  let hubCode = $state('');
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

  const hubPairable = $derived(
    !hubBusy && hubUrlDraft.trim() !== '' && hubCode.trim() !== '',
  );

  async function doPair() {
    hubBusy = true;
    hubError = null;
    const r = await hubPair(hubUrlDraft, hubCode, hubAllowPlaintext);
    hubBusy = false;
    if (r.ok) {
      hubRestartNeeded = r.value.restart_required;
      hubPlaintextRefused = false;
      // The code dies on first use; leaving it in the box invites a second
      // attempt that can only fail.
      hubCode = '';
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

  onMount(async () => {
    hubUrlDraft = $hubStatus.configured_url ?? '';
    if (!ownsTheFleet($hubStatus)) {
      // Neither of these applies to a hub client, and both are guarded on
      // the backend. Asking anyway would put two error toasts on the screen
      // every time Settings is opened.
      return;
    }
    const r = await mcpStatus();
    // Optional call: Svelte nulls a `bind:this` ref on teardown, so closing
    // Settings while mcpStatus() is in flight leaves it unset — and a throw
    // here would also skip resetProjectDrafts() below.
    mcpSettings?.applyStatus(r);
    const fs = await loadFleetSettings();
    if (!fs.ok) automationError = fs.error.message;
    resetProjectDrafts();
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

  // --- Automation: playbooks + GC (backend settings table) ---
  let automationError: string | null = $state(null);
  let automationBusy = $state(false);
  async function applySetting(key: SettingKey, value: string) {
    automationBusy = true;
    automationError = null;
    const r = await setFleetSetting(key, value);
    automationBusy = false;
    if (!r.ok) automationError = r.error.message;
  }
  /** Projects in the `work.trusted_branch_projects` id set. */
  function trustedProjectCount(raw: string | undefined): number {
    try {
      const v: unknown = JSON.parse(raw ?? '[]');
      return Array.isArray(v) ? v.length : 0;
    } catch {
      return 0;
    }
  }
  // Work graph M7.3: "Show what auto-tidy would do" — the current
  // candidates auto-tidy would act on with the ticked reasons.
  let dryRun = $state<TidyCandidate[] | null>(null);
  let dryRunBusy = $state(false);
  async function showDryRun() {
    dryRunBusy = true;
    await refreshTidy();
    dryRunBusy = false;
    dryRun = autoTidyPreview(
      $tidyReport.candidates,
      parseAutoTidyReasons($fleetSettings[SETTING_KEYS.workAutoTidyReasons]),
    );
  }
  function toggleSetting(key: SettingKey) {
    void applySetting(key, settingBool($fleetSettings, key) ? 'false' : 'true');
  }
  // Hours in the inputs, seconds on the wire. `null` while the field is
  // cleared; nothing is written until the value parses.
  function onHoursChange(key: SettingKey, e: Event) {
    const raw = (e.currentTarget as HTMLInputElement).value;
    const hours = Number.parseFloat(raw);
    if (!Number.isFinite(hours) || hours < 0) return;
    void applySetting(key, String(hoursToSecs(hours)));
  }
  function onSecsChange(key: SettingKey, e: Event) {
    const raw = (e.currentTarget as HTMLInputElement).value;
    const secs = Number.parseInt(raw, 10);
    if (!Number.isFinite(secs) || secs < 0) return;
    void applySetting(key, String(secs));
  }
  // --- Limits: task TTL + move transcript cap (backend settings table) ---
  // Values go to the backend as entered: it owns the range check, and its
  // E_INVALID message is what the row shows.
  let limitsError: string | null = $state(null);
  let limitsBusy = $state(false);
  async function applyLimit(key: SettingKey, value: string) {
    limitsBusy = true;
    limitsError = null;
    const r = await setFleetSetting(key, value);
    limitsBusy = false;
    if (!r.ok) limitsError = r.error.message;
  }
  // Nothing is dropped silently: an input that cannot be sent (empty,
  // negative hours, a value that rounds to 0 s = "never", a non-integer)
  // gets a message naming the field instead.
  function onLimitHoursChange(key: SettingKey, label: string, e: Event) {
    const r = parseHoursInput((e.currentTarget as HTMLInputElement).value);
    if ('error' in r) {
      limitsError = `${label}: ${r.error}`;
      return;
    }
    void applyLimit(key, String(r.secs));
  }
  function onLimitIntChange(key: SettingKey, label: string, e: Event) {
    const r = parseIntInput((e.currentTarget as HTMLInputElement).value);
    if ('error' in r) {
      limitsError = `${label}: ${r.error}`;
      return;
    }
    void applyLimit(key, r.value);
  }
  function onIdleUnlinkedDaysChange(e: Event) {
    const r = parseBoundedIntInput(
      (e.currentTarget as HTMLInputElement).value,
      TIDY_IDLE_UNLINKED_DAYS_MIN,
      TIDY_IDLE_UNLINKED_DAYS_MAX,
    );
    if ('error' in r) {
      limitsError = `Tidy: unlinked for: ${r.error}`;
      return;
    }
    void applyLimit(SETTING_KEYS.workTidyIdleUnlinkedDays, r.value);
  }
  function onUsagePricesChange(e: Event) {
    const r = parsePricesJsonInput((e.currentTarget as HTMLTextAreaElement).value);
    if ('error' in r) {
      limitsError = `Usage prices: ${r.error}`;
      return;
    }
    void applyLimit(SETTING_KEYS.usagePricesJson, r.value);
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

<!-- Escape + backdrop are handled by Modal (native <dialog>). -->
<Modal label="Settings" onclose={onClose} width="min(640px, 92vw)">
  <div class="dialog settings-dialog">
    <header>
      <h3>Settings</h3>
      <button class="close" onclick={onClose} aria-label="Close">×</button>
    </header>

    <section class="block hosts-line" data-testid="settings-hosts-line">
      <h4>Hosts</h4>
      <span class="hosts-summary" data-testid="settings-hosts-summary"
        >{$hosts.length} configured · {offlineCount} offline</span
      >
      <button class="hook-btn" onclick={openHosts} data-testid="settings-open-hosts"
        >Open Hosts <kbd>{hostsChord}</kbd></button
      >
    </section>

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
        {#if $hubStatus.warning}
          <p class="err" data-testid="hub-status-warning">⚠ {$hubStatus.warning}</p>
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
          ⚠ This app is set to use
          {#if $hubStatus.configured_url}<code>{$hubStatus.configured_url}</code>{:else}a hub{/if},
          but this launch cannot: {$hubStatus.unavailable}.
        </p>
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
            ⚠ A hub <strong>client token</strong> is still stored on this
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
          <span class="lbl">url</span>
          <input
            class="port base-input"
            type="text"
            spellcheck="false"
            placeholder="https://fleet.example.com"
            bind:value={hubUrlDraft}
            disabled={hubBusy}
            data-testid="hub-url"
            aria-label="Hub URL" />
        </div>
        <div class="mcp-field">
          <span class="lbl">code</span>
          <input
            class="port base-input"
            type="text"
            spellcheck="false"
            placeholder="ABCD1234"
            bind:value={hubCode}
            disabled={hubBusy}
            data-testid="hub-code"
            aria-label="Pairing code" />
          <button onclick={doPair} disabled={!hubPairable} data-testid="hub-pair">
            {hubBusy ? 'Pairing…' : 'Pair'}
          </button>
        </div>
      {/if}

      {#if hubError}
        <p class="err" role="alert" data-testid="hub-error">{hubError}</p>
      {/if}
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
      {#if hubRestartNeeded}
        <p class="hook-desc" data-testid="hub-restart">
          Saved. <strong>Restart claude-fleet to apply it</strong> — which
          fleet this app is a window onto is decided once, at startup, so that
          half the app can never be talking to a hub while the other half
          talks to the local database.
        </p>
      {/if}
    </section>

    {#if !ownsFleet}
      <section class="block" data-testid="projects-remote-section">
        <div class="section-header"><h4>Projects</h4></div>
        <p class="hook-desc" data-testid="projects-remote">
          {hubBlock('get_fleet_settings', $hubStatus)}
        </p>
      </section>
    {:else}
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

    <section class="block" data-testid="onboarding-section">
      <div class="section-header">
        <h4>Setup guide</h4>
      </div>
      <div class="hook-section">
        <p class="hook-desc">Re-show the "Get started" checklist in the sidebar.</p>
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
      <div class="hook-section">
        <p class="hook-desc">Show inline tips the first time a feature is used.</p>
        <label class="toggle">
          <input type="checkbox" bind:checked={$hintsEnabled} />
          Show feature hints
        </label>
        <button class="hook-btn" onclick={resetHints} data-testid="reset-hints">
          Reset hints
        </button>
      </div>
      <div class="hook-section">
        <p class="hook-desc">
          Copy a terminal drag-selection to the clipboard as soon as the mouse
          is released. Off: use Cmd+C / Ctrl+Shift+C or the context menu.
        </p>
        <label class="toggle">
          <input type="checkbox" bind:checked={$copyOnSelect} data-testid="copy-on-select" />
          Copy on select
        </label>
      </div>
    </section>

    <section class="block" data-testid="notifications-section">
      <div class="section-header">
        <h4>Notifications</h4>
      </div>
      <p class="mcp-blurb">
        When a session becomes stuck (auth menu, trust prompt, reconnect,
        out of memory, press Enter) fleet announces it for screen readers and,
        optionally, shows a toast and an OS notification.
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
          OS notification on stuck transitions
        </label>
        <span class="status status-{permission === 'granted' ? 'on' : permission === 'denied' ? 'off' : 'neutral'}" data-testid="notify-permission">
          {permission}
        </span>
      </div>
      {#if permission === 'unsupported'}
        <p class="hook-desc">This webview does not expose the Notification API; toasts and the live region still work.</p>
      {:else if permission === 'denied'}
        <p class="hook-desc">Notifications were denied at the OS level; allow them for claude-fleet in your system settings.</p>
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

    <section class="block" data-testid="composer-section">
      <h4>Conversation composer</h4>
      <div class="hook-section">
        <p class="hook-desc">
          Quick-action chips above the prompt box in the Conversation tab. A
          click fills the box; Shift+click sends at once. Chips with an empty
          label or text are not shown.
        </p>
        {#each $composerPresets as p, i (i)}
          <div class="preset-row">
            <input
              class="preset-label"
              data-testid="preset-label"
              placeholder="Label"
              value={p.label}
              oninput={(e) => updatePreset(i, { label: e.currentTarget.value })}
            />
            <textarea
              class="preset-text"
              data-testid="preset-text"
              rows="1"
              placeholder="Prompt or /command"
              value={p.text}
              oninput={(e) => updatePreset(i, { text: e.currentTarget.value })}
            ></textarea>
            <button class="hook-btn" data-testid="preset-remove" title="Remove" onclick={() => removePreset(i)}>×</button>
          </div>
        {/each}
        <div class="preset-actions">
          <button class="hook-btn" data-testid="preset-add" onclick={addPreset}>Add chip</button>
          <button class="hook-btn" data-testid="preset-reset" onclick={resetComposerPresets}>Reset to defaults</button>
        </div>
      </div>
    </section>

    <WorkSettings />

    {#if !ownsFleet}
      <section class="block" data-testid="automation-remote-section">
        <div class="section-header"><h4>Automation</h4></div>
        <p class="hook-desc" data-testid="automation-remote">
          {hubBlock('get_fleet_settings', $hubStatus)}
        </p>
      </section>
      <section class="block" data-testid="limits-remote-section">
        <div class="section-header"><h4>Limits</h4></div>
        <p class="hook-desc" data-testid="limits-remote">
          {hubBlock('get_fleet_settings', $hubStatus)}
        </p>
      </section>
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
    <section class="block" data-testid="automation-section">
      <div class="section-header">
        <h4>Automation</h4>
      </div>
      <p class="mcp-blurb">
        Stuck-session playbooks, the idle-session GC and workspace repair run
        from the background reconcile tick. Everything here is off by default; changes apply on the
        next tick.
      </p>
      <label class="toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.playbookPressEnter)}
          disabled={automationBusy}
          data-testid="playbook-press-enter"
          onchange={() => toggleSetting(SETTING_KEYS.playbookPressEnter)} />
        Press Enter for sessions stuck on a "Press Enter" prompt
      </label>
      <label class="toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.playbookOomRecreate)}
          disabled={automationBusy}
          data-testid="playbook-oom-recreate"
          onchange={() => toggleSetting(SETTING_KEYS.playbookOomRecreate)} />
        Recreate sessions that ran out of memory (at most once per hour)
      </label>
      <p class="hook-desc">Auth menus, trust prompts and reconnects are always notify-only.</p>

      <label class="toggle gc-toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.gcEnabled)}
          disabled={automationBusy}
          data-testid="gc-enabled"
          onchange={() => toggleSetting(SETTING_KEYS.gcEnabled)} />
        Garbage-collect idle sessions
      </label>
      <div class="mcp-field">
        <span class="lbl">bg</span>
        <input class="port" type="number" min="0" step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.gcBgIdleSecs))}
          disabled={automationBusy}
          data-testid="gc-bg-hours"
          onchange={(e) => onHoursChange(SETTING_KEYS.gcBgIdleSecs, e)} />
        <span class="hook-desc">hours idle before a background agent is stopped (0 = never)</span>
      </div>
      <div class="mcp-field">
        <span class="lbl">shell</span>
        <input class="port" type="number" min="0" step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.gcShellIdleSecs))}
          disabled={automationBusy}
          data-testid="gc-shell-hours"
          onchange={(e) => onHoursChange(SETTING_KEYS.gcShellIdleSecs, e)} />
        <span class="hook-desc">hours inactive before a shell session is killed (0 = never)</span>
      </div>
      <div class="mcp-field">
        <span class="lbl">work</span>
        <input class="port" type="number" min="0" step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.gcWorkIdleSecs))}
          disabled={automationBusy}
          data-testid="gc-work-hours"
          onchange={(e) => onHoursChange(SETTING_KEYS.gcWorkIdleSecs, e)} />
        <span class="hook-desc">hours idle before a work session is removed — dirty worktrees go through safe-remove (0 = never)</span>
      </div>
      <div class="mcp-field">
        <span class="lbl">sweep</span>
        <input class="port" type="number" min="0"
          value={settingSecs($fleetSettings, SETTING_KEYS.gcSweepIntervalSecs)}
          disabled={automationBusy}
          data-testid="gc-sweep-secs"
          onchange={(e) => onSecsChange(SETTING_KEYS.gcSweepIntervalSecs, e)} />
        <span class="hook-desc">seconds between GC sweeps</span>
      </div>

      <label class="toggle gc-toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.repairAutoOnTick)}
          disabled={automationBusy}
          data-testid="repair-auto-on-tick"
          onchange={() => toggleSetting(SETTING_KEYS.repairAutoOnTick)} />
        Re-create vanished worktree directories automatically
      </label>
      <p class="hook-desc">
        Re-adds deleted worktrees without anyone opening them. A worktree
        git still lists is dropped and re-added only when its parent folder
        is the same one seen while it was healthy (so an unmounted or
        remounted volume is never touched); otherwise use Repair workspace.
        Never touches the controller, review sessions or a session being
        safely removed.
      </p>
      <div class="mcp-field">
        <span class="lbl">repair</span>
        <input class="port" type="number" min="60"
          value={settingSecs($fleetSettings, SETTING_KEYS.repairTickIntervalSecs)}
          disabled={automationBusy}
          data-testid="repair-tick-secs"
          onchange={(e) => onSecsChange(SETTING_KEYS.repairTickIntervalSecs, e)} />
        <span class="hook-desc">seconds between workspace checks (60 or more; at most 5 repairs each)</span>
      </div>
      <div class="mcp-field">
        <span class="lbl">tick</span>
        <input class="port" type="number" min="0"
          value={settingSecs($fleetSettings, SETTING_KEYS.reconcileIntervalSecs)}
          disabled={automationBusy}
          data-testid="reconcile-secs"
          onchange={(e) => onSecsChange(SETTING_KEYS.reconcileIntervalSecs, e)} />
        <span class="hook-desc">seconds between reconcile passes (0 disables; restart to apply)</span>
      </div>
      {#if automationError}<p class="err">{automationError}</p>{/if}
    </section>

    <section class="block" data-testid="limits-section">
      <div class="section-header">
        <h4>Limits</h4>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-tasks-hours">tasks</label>
        <input class="port" id="limit-tasks-hours" type="number" min="0" max={secsToHours(MAX_SECS)} step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.tasksMaxAgeSecs))}
          disabled={limitsBusy}
          aria-describedby="limit-tasks-desc"
          data-testid="tasks-max-age-hours"
          onchange={(e) => onLimitHoursChange(SETTING_KEYS.tasksMaxAgeSecs, 'Task timeout', e)} />
        <span class="hook-desc" id="limit-tasks-desc">hours before an open task (counted from its start, else its creation) is failed by the liveness sweep (0 = never)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-lost-ttl-hours">lost sessions</label>
        <input class="port" id="limit-lost-ttl-hours" type="number" min="0" max={secsToHours(MAX_SECS)} step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.sessionsLostTtlSecs))}
          disabled={limitsBusy}
          aria-describedby="limit-lost-ttl-desc"
          data-testid="sessions-lost-ttl-hours"
          onchange={(e) => onLimitHoursChange(SETTING_KEYS.sessionsLostTtlSecs, 'Lost session TTL', e)} />
        <span class="hook-desc" id="limit-lost-ttl-desc">hours a resumable session lost to a host reboot or the tmux server exiting is kept before it is deleted, counted from when it was lost (0 = off: removed on the next pass like any vanished session)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="restore-batch-size">restore batch</label>
        <input class="port" id="restore-batch-size" type="number" min="1" max="16" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.restoreBatchSize)}
          disabled={limitsBusy}
          aria-describedby="restore-batch-size-desc"
          data-testid="restore-batch-size"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.restoreBatchSize, 'Concurrent restores', e)} />
        <span class="hook-desc" id="restore-batch-size-desc">Sessions resumed in parallel by Restore lost sessions</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="restore-stagger-ms">restore delay</label>
        <input class="port" id="restore-stagger-ms" type="number" min="0" max="60000" step="500"
          value={settingInt($fleetSettings, SETTING_KEYS.restoreStaggerMs)}
          disabled={limitsBusy}
          aria-describedby="restore-stagger-ms-desc"
          data-testid="restore-stagger-ms"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.restoreStaggerMs, 'Delay between restores (ms)', e)} />
        <span class="hook-desc" id="restore-stagger-ms-desc">Pause between starting each resumed session</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-move-mb">move</label>
        <input class="port" id="limit-move-mb" type="number" min="1" max={MOVE_MAX_TRANSCRIPT_MB_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveMaxTranscriptMb)}
          disabled={limitsBusy}
          aria-describedby="limit-move-desc"
          data-testid="move-max-transcript-mb"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveMaxTranscriptMb, 'Move transcript cap', e)} />
        <span class="hook-desc" id="limit-move-desc">largest transcript (MiB, 1–{MOVE_MAX_TRANSCRIPT_MB_MAX}) Move to host… copies; a bigger one is refused (E_MOVE_TOO_LARGE)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-move-bundle-mb">carry bundle</label>
        <input class="port" id="limit-move-bundle-mb" type="number" min="1" max={MOVE_MAX_BUNDLE_MB_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveMaxBundleMb)}
          disabled={limitsBusy}
          aria-describedby="limit-move-bundle-desc"
          data-testid="move-max-bundle-mb"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveMaxBundleMb, 'Move max bundle', e)} />
        <span class="hook-desc" id="limit-move-bundle-desc">largest git bundle (MiB, 1–{MOVE_MAX_BUNDLE_MB_MAX}) Move to host… relays; a bigger one is refused (E_MOVE_TOO_LARGE)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-move-ignored-entry-kb">carry entry</label>
        <input class="port" id="limit-move-ignored-entry-kb" type="number" min="1" max={MOVE_IGNORED_ENTRY_KB_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveIgnoredEntryKb)}
          disabled={limitsBusy}
          aria-describedby="limit-move-ignored-entry-desc"
          data-testid="move-ignored-entry-kb"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveIgnoredEntryKb, 'Move ignored entry', e)} />
        <span class="hook-desc" id="limit-move-ignored-entry-desc">largest single git-ignored entry (KiB, 1–{MOVE_IGNORED_ENTRY_KB_MAX}) Move to host… carries — a file, or a whole ignored directory measured together; bigger ones are left behind</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-move-ignored-total-mb">carry ignored</label>
        <input class="port" id="limit-move-ignored-total-mb" type="number" min="1" max={MOVE_IGNORED_TOTAL_MB_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveIgnoredTotalMb)}
          disabled={limitsBusy}
          aria-describedby="limit-move-ignored-total-desc"
          data-testid="move-ignored-total-mb"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveIgnoredTotalMb, 'Move ignored total', e)} />
        <span class="hook-desc" id="limit-move-ignored-total-desc">total git-ignored payload (MiB, 1–{MOVE_IGNORED_TOTAL_MB_MAX}) Move to host… carries</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-move-session-state-mb">carry session</label>
        <input class="port" id="limit-move-session-state-mb" type="number" min="1" max={MOVE_MAX_SESSION_STATE_MB_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveMaxSessionStateMb)}
          disabled={limitsBusy}
          aria-describedby="limit-move-session-state-desc"
          data-testid="move-session-state-mb"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveMaxSessionStateMb, 'Move session state', e)} />
        <span class="hook-desc" id="limit-move-session-state-desc">largest per-session Claude directory (MiB, 1–{MOVE_MAX_SESSION_STATE_MB_MAX}: subagent transcripts, tool results) Move to host… carries; above it the biggest files stay behind</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="limit-move-wait-max-mins">transfer wait</label>
        <input class="port" id="limit-move-wait-max-mins" type="number" min="1" max={MOVE_WAIT_MAX_MINS_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveWaitMaxMins)}
          disabled={limitsBusy}
          aria-describedby="limit-move-wait-max-mins-desc"
          data-testid="move-wait-max-mins"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveWaitMaxMins, 'Move wait timeout', e)} />
        <span class="hook-desc" id="limit-move-wait-max-mins-desc">minutes (1–{MOVE_WAIT_MAX_MINS_MAX}) "Transfer when it finishes" waits for the session to go idle before giving up</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="usage-enabled">usage</label>
        <input id="usage-enabled" type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.usageEnabled)}
          disabled={limitsBusy}
          aria-describedby="usage-enabled-desc"
          data-testid="usage-enabled"
          onchange={(e) => void applyLimit(SETTING_KEYS.usageEnabled, String((e.currentTarget as HTMLInputElement).checked))} />
        <span class="hook-desc" id="usage-enabled-desc">sum each session's token usage from its Claude transcript and show an estimated cost</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="usage-interval-secs">usage every</label>
        <input class="port" id="usage-interval-secs" type="number" min="0" max={MAX_SECS} step="1"
          value={settingSecs($fleetSettings, SETTING_KEYS.usageIntervalSecs)}
          disabled={limitsBusy}
          aria-describedby="usage-interval-desc"
          data-testid="usage-interval-secs"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.usageIntervalSecs, 'Usage interval', e)} />
        <span class="hook-desc" id="usage-interval-desc">seconds between usage passes (one batched read per host; 0 = off)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="usage-prices-json">prices</label>
        <textarea id="usage-prices-json" rows="3" spellcheck="false"
          value={$fleetSettings[SETTING_KEYS.usagePricesJson] ?? '{}'}
          disabled={limitsBusy}
          aria-describedby="usage-prices-desc"
          data-testid="usage-prices-json"
          onchange={onUsagePricesChange}></textarea>
        <span class="hook-desc" id="usage-prices-desc">per-model price overrides for the estimated cost, USD per million tokens, e.g. {'{"opus-4-1":{"input":15,"output":75,"cache_write":30,"cache_read":1.5}}'} ({'{}'} = built-in prices only)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="reports-max-rows">error reports</label>
        <input class="port" id="reports-max-rows" type="number" min={REPORTS_MAX_ROWS_MIN} max={REPORTS_MAX_ROWS_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.reportsMaxRows)}
          disabled={limitsBusy}
          aria-describedby="reports-max-rows-desc"
          data-testid="reports-max-rows"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.reportsMaxRows, 'Error reports row cap', e)} />
        <span class="hook-desc" id="reports-max-rows-desc">newest error/warn reports kept ({REPORTS_MAX_ROWS_MIN}–{REPORTS_MAX_ROWS_MAX}); pruned on every insert</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="reports-max-age-hours">error reports age</label>
        <input class="port" id="reports-max-age-hours" type="number" min="0" max={secsToHours(MAX_SECS)} step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.reportsMaxAgeSecs))}
          disabled={limitsBusy}
          aria-describedby="reports-max-age-desc"
          data-testid="reports-max-age-hours"
          onchange={(e) => onLimitHoursChange(SETTING_KEYS.reportsMaxAgeSecs, 'Error reports max age', e)} />
        <span class="hook-desc" id="reports-max-age-desc">hours an error/warn report is kept before the age sweep deletes it (0 = never)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="work-journal-days">work memory</label>
        <input class="port" id="work-journal-days" type="number" min="0" max="3650" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.workJournalDays)}
          disabled={limitsBusy}
          aria-describedby="work-journal-days-desc"
          data-testid="work-journal-days"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.workJournalDays, 'Work memory', e)} />
        <span class="hook-desc" id="work-journal-days-desc">days the resume journal of unlinked conversations is kept (0 = forever; linked work is always kept)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="work-recent-days">recent work</label>
        <input class="port" id="work-recent-days" type="number" min="1" max="365" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.workRecentDays)}
          disabled={limitsBusy}
          aria-describedby="work-recent-days-desc"
          data-testid="work-recent-days"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.workRecentDays, 'Recent work', e)} />
        <span class="hook-desc" id="work-recent-days-desc">days ended work with no live session still gets a sidebar group (by work)</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="work-sync-interval">tracker sync</label>
        <input class="port" id="work-sync-interval" type="number" min="0" max="86400" step="60"
          value={settingInt($fleetSettings, SETTING_KEYS.workSyncIntervalSecs)}
          disabled={limitsBusy}
          aria-describedby="work-sync-interval-desc"
          data-testid="work-sync-interval"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.workSyncIntervalSecs, 'Tracker sync', e)} />
        <span class="hook-desc" id="work-sync-interval-desc">seconds between tracker (Jira) sync passes (0 = off; read at launch)</span>
      </div>
      <label class="toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.workEvidenceSnippets)}
          disabled={automationBusy}
          data-testid="work-evidence-snippets"
          onchange={() => toggleSetting(SETTING_KEYS.workEvidenceSnippets)} />
        Keep a short, redacted prompt snippet as evidence for a detected ticket
      </label>
      <label class="toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.workSessionStartContext)}
          disabled={automationBusy}
          data-testid="work-session-start-context"
          onchange={() => toggleSetting(SETTING_KEYS.workSessionStartContext)} />
        Give Claude the linked ticket at session start (makes the start hook wait up to 2 s when the hub is down; applies when hooks are reinstalled)
      </label>
      <label class="toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.workClassifyNudge)}
          disabled={automationBusy}
          data-testid="work-classify-nudge"
          onchange={() => toggleSetting(SETTING_KEYS.workClassifyNudge)} />
        After three prompts with no ticket, ask Claude once which of your few open tickets it is on (only ever a suggestion)
      </label>
      <h5 class="sub" data-testid="work-lifecycle">Lifecycle</h5>
      <div class="mcp-field">
        <label class="lbl" for="work-tidy-done-days">tidy: done for</label>
        <input class="port" id="work-tidy-done-days" type="number" min="1" max="365" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.workTidyDoneDays)}
          disabled={limitsBusy}
          aria-describedby="work-tidy-done-days-desc"
          data-testid="work-tidy-done-days"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.workTidyDoneDays, 'Tidy: done for', e)} />
        <span class="hook-desc" id="work-tidy-done-days-desc">days a linked ticket must be done before Tidy up suggests its session</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="work-tidy-idle-hours">tidy: idle for</label>
        <input class="port" id="work-tidy-idle-hours" type="number" min="1" max="720" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.workTidyIdleHours)}
          disabled={limitsBusy}
          aria-describedby="work-tidy-idle-hours-desc"
          data-testid="work-tidy-idle-hours"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.workTidyIdleHours, 'Tidy: idle for', e)} />
        <span class="hook-desc" id="work-tidy-idle-hours-desc">hours a session must be idle before any reason suggests it</span>
      </div>
      <div class="mcp-field">
        <label class="lbl" for="work-tidy-idle-unlinked-days">tidy: unlinked for</label>
        <input class="port" id="work-tidy-idle-unlinked-days" type="number"
          min={TIDY_IDLE_UNLINKED_DAYS_MIN} max={TIDY_IDLE_UNLINKED_DAYS_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.workTidyIdleUnlinkedDays)}
          disabled={limitsBusy}
          aria-describedby="work-tidy-idle-unlinked-days-desc"
          data-testid="work-tidy-idle-unlinked-days"
          onchange={onIdleUnlinkedDaysChange} />
        <span class="hook-desc" id="work-tidy-idle-unlinked-days-desc">days a session with no work linked must sit idle and unprompted before Tidy up suggests it (1–90; only ever suggested, never auto-tidied)</span>
      </div>
      <label class="toggle">
        <input
          type="checkbox"
          checked={settingBool($fleetSettings, SETTING_KEYS.workAutoTidy)}
          disabled={automationBusy}
          data-testid="work-auto-tidy"
          onchange={() => toggleSetting(SETTING_KEYS.workAutoTidy)} />
        Auto-tidy: let the sweep act on the reasons below by itself
      </label>
      <p class="hook-desc warn" data-testid="work-auto-tidy-warning">
        Off, Tidy up only suggests. On, the sweep safe-kills finished sessions without asking:
        Claude is asked to commit and push first, and a session that is working, waiting on you,
        linked to in-progress work or used in the last hour is never touched.
        An organisation can turn it on or off for its own sessions (Organisations).
      </p>
      <div class="mcp-field" data-testid="work-auto-tidy-reasons">
        <span class="lbl">auto-tidy reasons</span>
        {#each AUTO_TIDY_REASONS as reason (reason)}
          <label class="toggle inline">
            <input
              type="checkbox"
              checked={parseAutoTidyReasons($fleetSettings[SETTING_KEYS.workAutoTidyReasons]).has(reason)}
              disabled={automationBusy}
              data-testid={`work-auto-tidy-reason-${reason}`}
              onchange={() =>
                void applySetting(
                  SETTING_KEYS.workAutoTidyReasons,
                  toggleAutoTidyReason($fleetSettings[SETTING_KEYS.workAutoTidyReasons], reason),
                )} />
            {TIDY_REASON_LABELS[reason]}
          </label>
        {/each}
      </div>
      <div class="mcp-field">
        <button class="btn" type="button" data-testid="work-auto-tidy-dry-run" disabled={dryRunBusy}
          onclick={() => void showDryRun()}>Show what auto-tidy would do</button>
      </div>
      {#if dryRun !== null}
        <div class="hook-desc" data-testid="work-auto-tidy-preview">
          {#if dryRun.length === 0}
            Nothing right now.
          {:else}
            Auto-tidy would {settingBool($fleetSettings, SETTING_KEYS.workAutoTidy) ? '' : '(once turned on) '}safe-kill or archive:
            <ul>
              {#each dryRun as c (c.session_id)}
                <li data-testid="work-auto-tidy-preview-row">
                  {c.label || c.tmux_name} on {c.host_alias}{c.key ? ` · ${c.key}` : ''} — {tidyReasonLabel(c.reason)}, idle {formatIdle(c.idle_secs)}
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
      <div class="mcp-field">
        <span class="lbl">trusted branch keys</span>
        <span class="hook-desc" data-testid="work-trusted-projects">
          {trustedProjectCount($fleetSettings[SETTING_KEYS.workTrustedBranchProjects])} project(s) link a sole branch key automatically (set per project from a work chip)
        </span>
        <button
          class="btn"
          type="button"
          disabled={automationBusy || trustedProjectCount($fleetSettings[SETTING_KEYS.workTrustedBranchProjects]) === 0}
          data-testid="work-trusted-clear"
          onclick={() => void applySetting(SETTING_KEYS.workTrustedBranchProjects, '[]')}>Trust none</button>
      </div>
      {#if limitsError}<p class="err" role="alert" data-testid="limits-error">{limitsError}</p>{/if}
    </section>

    <!-- Provisioning mints host tokens: refresh the shared token cache the
         Hosts view reads (host_actions.ts). Module-level, so it is safe even
         when a slow multi-host provision outlives this dialog. -->
    <McpSettings bind:this={mcpSettings} onProvisioned={loadHostTokens} />
    {/if}

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
          {diagBusy ? 'Collecting…' : 'Copy diagnostics'}
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
</Modal>

<style>
  .dialog {
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  header { display: flex; align-items: center; justify-content: space-between; }
  header h3 { margin: 0; font-size: 1rem; }
  .close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.2rem;
    cursor: pointer;
    padding: 0 0.4rem;
  }
  .close:hover { color: var(--fg); }

  .log-path code { word-break: break-all; }

  .hosts-line {
    display: flex;
    align-items: baseline;
    gap: 0.8rem;
  }
  .hosts-line h4 {
    margin: 0;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .hosts-summary { flex: 1; font-size: 0.8rem; color: var(--fg-muted); font-variant-numeric: tabular-nums; }
  .hosts-line kbd {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
  }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }

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
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  /* Qualified with .hook-desc: the preview span carries both classes, and the
     later `.hook-desc { margin: 0 }` used to win the equal-specificity tie and
     cancel the indent that lines the preview up under the input. */
  .hook-desc.project-preview {
    display: block;
    margin-left: 6.4rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    word-break: break-all;
  }
  /* Same tie, for the invalid-path message: .hook-desc's muted colour used to
     beat the .err red the class:err toggle asks for. */
  .project-preview.err { color: #e64a4a; }
  .layout-select {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
  }

  .hook-desc {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary, #888);
  }

  .gc-toggle { margin-top: 0.6rem; }
</style>
