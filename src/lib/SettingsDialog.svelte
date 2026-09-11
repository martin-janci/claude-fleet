<script lang="ts">
  import { onMount } from 'svelte';
  import { hosts } from './hosts';
  import { mcpStatus } from './mcp';
  import { onboardingDismissed, onboardingWelcomed } from './onboarding';
  import { hintsEnabled, resetHints } from './hints';
  import { copyOnSelect } from './prefs';
  import { collectDiagnostics, copyDiagnostics, openLogFolder } from './diagnostics';
  import { pushError } from './toasts';
  import AddHostPicker from './AddHostPicker.svelte';
  import Modal from './Modal.svelte';
  import HostsTable from './HostsTable.svelte';
  import McpSettings from './McpSettings.svelte';
  import {
    fleetSettings,
    loadFleetSettings,
    setFleetSetting,
    settingBool,
    settingSecs,
    settingInt,
    parseHoursInput,
    parseIntInput,
    parsePricesJsonInput,
    secsToHours,
    hoursToSecs,
    SETTING_KEYS,
    MAX_SECS,
    MOVE_MAX_TRANSCRIPT_MB_MAX,
    PROJECTS_LOCAL_ENV_KEY,
    settingPathMap,
    settingLayout,
    basePathError,
    projectPathPreview,
    projectsDefaultRoot,
    type SettingKey,
    type ProjectsLayout,
  } from './fleet_settings';
  import { refreshProjects } from './projects';
  import {
    attentionIdleMinutes,
    notificationPermission,
    notifyStuckOs,
    notifyStuckToast,
    requestNotificationPermission,
    type NotificationPermissionState,
  } from './notify';

  let { onClose }: { onClose: () => void } = $props();

  let showAddPicker = $state(false);
  // The Hosts table owns the per-host tokens and the Control API section its
  // status; both load from this onMount, in the same order as before.
  let hostsTable = $state<ReturnType<typeof HostsTable>>();
  let mcpSettings = $state<ReturnType<typeof McpSettings>>();

  onMount(async () => {
    const r = await mcpStatus();
    mcpSettings?.applyStatus(r);
    // Independent fetches, in parallel: a failed token fetch must not hide
    // the automation section's state and vice versa.
    const [fs] = await Promise.all([loadFleetSettings(), hostsTable?.loadHostTokens()]);
    if (!fs.ok) automationError = fs.error.message;
    resetProjectDrafts();
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

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* clipboard unavailable — no-op */
    }
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

<!-- Escape + backdrop are handled by Modal (native <dialog>). When the
     AddHostPicker is stacked on top, Escape reaches only that topmost dialog. -->
<Modal label="Settings" onclose={onClose} width="600px">
  <div class="dialog">
    <header>
      <h3>Settings</h3>
      <button class="close" onclick={onClose} aria-label="Close">×</button>
    </header>

    <HostsTable bind:this={hostsTable} onAddHost={() => (showAddPicker = true)} />

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
      {#if limitsError}<p class="err" role="alert" data-testid="limits-error">{limitsError}</p>{/if}
    </section>

    <McpSettings
      bind:this={mcpSettings}
      onProvisioned={() => hostsTable?.loadHostTokens() ?? Promise.resolve()} />

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

{#if showAddPicker}
  <AddHostPicker onClose={() => (showAddPicker = false)} />
{/if}

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

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  .section-header h4 {
    margin: 0;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .status {
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
  }
  .status-on { background: rgba(60,180,90,0.18); color: rgb(80,200,110); }
  .status-off { background: rgba(180,100,100,0.18); color: rgb(220,130,130); }

  .hook-actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
  .log-path code { word-break: break-all; }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }

  .mcp-blurb {
    font-size: 0.78rem;
    color: var(--fg-muted);
    margin: 0 0 0.6rem;
    line-height: 1.4;
  }
  .mcp-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.5rem;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .mcp-field {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.4rem;
    font-size: 0.82rem;
  }
  .mcp-field .lbl {
    width: 3.2rem;
    color: var(--fg-muted);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .mcp-field .port {
    width: 6rem;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
  }
  .mcp-field .port.invalid {
    border-color: #e64a4a;
  }
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
  .project-preview {
    display: block;
    margin-left: 6.4rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    word-break: break-all;
  }
  .layout-select {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
  }
  .mcp-field button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .mcp-field button:hover:not(:disabled) { border-color: var(--accent); }
  .mcp-field button:disabled { opacity: 0.5; cursor: default; }

  .hook-section {
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .hook-desc {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary, #888);
  }
  .hook-btn {
    align-self: flex-start;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .hook-btn:hover:not(:disabled) { border-color: var(--accent); }
  .hook-btn:disabled { opacity: 0.5; cursor: default; }

  .status-neutral {
    background: rgba(127, 127, 127, 0.15);
    color: var(--fg-muted);
  }
  .gc-toggle { margin-top: 0.6rem; }
</style>
