<script lang="ts">
  // Settings → Appearance (redesign step 0.3): the theme picker, (3.6) row
  // density and (0.6) Motion; step 13.1 removed the Classic/New layout
  // switch; gap plan G4.6 adds text size, the agent tab's name and badge
  // counts. Hand-written, not a
  // generated page, because all of them are per-device prefs in localStorage
  // rather than fleet settings. Copy
  // follows the Settings board of the Orbit Fleet canvas. This is the one
  // theme picker: step 1.4 removed the sidebar's "theme: …" line.
  import SegmentedControl from './SegmentedControl.svelte';
  import {
    agentTabName,
    badgeCounts,
    uiDensity,
    type AgentTabName,
    type BadgeCounts,
    type UiDensity,
  } from './prefs';
  import { TEXT_SIZES, textSize, textSizeLabel, type TextSize } from './text_size';
  import { applyTheme, theme, type Theme } from './theme';
  import { motionPref, type MotionPref } from './motion';

  const densities = [
    { id: 'compact', label: 'Compact' },
    { id: 'comfortable', label: 'Comfortable' },
  ] as const satisfies readonly { id: UiDensity; label: string }[];

  const themes = [
    { id: 'auto', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ] as const satisfies readonly { id: Theme; label: string }[];

  const tabNames = [
    { id: 'agent', label: 'Per agent' },
    { id: 'terminal', label: 'Terminal' },
  ] as const satisfies readonly { id: AgentTabName; label: string }[];

  const badges = [
    { id: 'needs_you', label: 'Needs you only' },
    { id: 'off', label: 'Off' },
  ] as const satisfies readonly { id: BadgeCounts; label: string }[];

  const motions = [
    { id: 'system', label: 'System' },
    { id: 'full', label: 'Full' },
    { id: 'reduced', label: 'Reduced' },
    { id: 'off', label: 'Off' },
  ] as const satisfies readonly { id: MotionPref; label: string }[];
</script>

<section class="block" data-testid="appearance-section">
  <div class="pref">
    <div class="pref-text">
      <span class="lbl">Theme</span>
      <p class="hook-desc">Follows the system unless you pick one.</p>
    </div>
    <SegmentedControl
      options={themes}
      value={$theme}
      label="Theme"
      testidPrefix="appearance-theme-"
      onchange={(id) => applyTheme(id)} />
  </div>
  <div class="pref">
    <div class="pref-text">
      <span class="lbl">Density</span>
      <p class="hook-desc">Compact shows one meta line per row and hides chips until hover.</p>
    </div>
    <SegmentedControl
      options={densities}
      value={$uiDensity}
      label="Density"
      testidPrefix="appearance-density-"
      onchange={(id) => uiDensity.set(id)} />
  </div>
  <div class="pref">
    <div class="pref-text">
      <span class="lbl">Motion</span>
      <p class="hook-desc">
        Full explains changes in 80–280 ms. Reduced keeps only fades. Off stops
        UI motion. System follows your reduce-motion setting.
      </p>
    </div>
    <SegmentedControl
      options={motions}
      value={$motionPref}
      label="Motion"
      testidPrefix="appearance-motion-"
      onchange={(id) => motionPref.set(id)} />
  </div>
  <div class="pref">
    <div class="pref-text">
      <label class="lbl" for="appearance-text-size">Text size</label>
      <p class="hook-desc">Scales the whole app from the 13 px base.</p>
    </div>
    <select
      id="appearance-text-size"
      data-testid="appearance-text-size"
      value={$textSize}
      onchange={(e) => textSize.set(Number((e.currentTarget as HTMLSelectElement).value) as TextSize)}>
      {#each TEXT_SIZES as pct (pct)}<option value={pct}>{textSizeLabel(pct)}</option>{/each}
    </select>
  </div>
  <div class="pref">
    <div class="pref-text">
      <span class="lbl">Agent tab name</span>
      <p class="hook-desc">
        Per agent shows the agent's own name and icon: Claude Code, Codex, Agy. Terminal names every one Terminal.
      </p>
    </div>
    <SegmentedControl
      options={tabNames}
      value={$agentTabName}
      label="Agent tab name"
      testidPrefix="appearance-agent-tab-"
      onchange={(id) => agentTabName.set(id)} />
  </div>
  <div class="pref">
    <div class="pref-text">
      <label class="lbl" for="appearance-badge-counts">Badge counts</label>
      <p class="hook-desc">
        Only Waiting, Failed and Blocked raise the Inbox badge. Ghost and lost rows never do. Off shows no number.
      </p>
    </div>
    <select
      id="appearance-badge-counts"
      data-testid="appearance-badge-counts"
      value={$badgeCounts}
      onchange={(e) => badgeCounts.set((e.currentTarget as HTMLSelectElement).value as BadgeCounts)}>
      {#each badges as b (b.id)}<option value={b.id}>{b.label}</option>{/each}
    </select>
  </div>
</section>

<style>
  .pref {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border);
  }
  .pref-text { flex: 1; min-width: 0; }
  .lbl { font-size: var(--text-sm); font-weight: 500; }
  .hook-desc {
    margin: 2px 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .pref :global(.seg-group) { flex: 0 0 auto; min-width: 180px; }
  .pref select { flex: 0 0 auto; min-width: 180px; }
</style>
