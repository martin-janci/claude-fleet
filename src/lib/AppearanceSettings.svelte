<script lang="ts">
  // Settings → Appearance (redesign step 0.3): the layout switch, the
  // theme picker, (3.6) row density and (0.6) Motion. Hand-written, not a
  // generated page, because all four are per-device prefs in localStorage
  // rather than fleet settings. Copy
  // follows the Settings board of the Orbit Fleet canvas. This is the one
  // theme picker: step 1.4 removed the sidebar's "theme: …" line.
  import SegmentedControl from './SegmentedControl.svelte';
  import { uiDensity, uiLayout, type UiDensity, type UiLayout } from './prefs';
  import { applyTheme, theme, type Theme } from './theme';
  import { motionPref, type MotionPref } from './motion';

  const layouts = [
    { id: 'new', label: 'New' },
    { id: 'classic', label: 'Classic' },
  ] as const satisfies readonly { id: UiLayout; label: string }[];

  const densities = [
    { id: 'compact', label: 'Compact' },
    { id: 'comfortable', label: 'Comfortable' },
  ] as const satisfies readonly { id: UiDensity; label: string }[];

  const themes = [
    { id: 'auto', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ] as const satisfies readonly { id: Theme; label: string }[];

  const motions = [
    { id: 'system', label: 'System' },
    { id: 'full', label: 'Full' },
    { id: 'reduced', label: 'Reduced' },
    { id: 'off', label: 'Off' },
  ] as const satisfies readonly { id: MotionPref; label: string }[];
</script>

<section class="block" data-testid="appearance-section">
  <div class="section-header">
    <h4>Appearance</h4>
  </div>
  <div class="pref">
    <div class="pref-text">
      <span class="lbl">Layout</span>
      <p class="hook-desc">
        New uses the rail, the left list with filters and grouping, and the
        inspector. Classic keeps 0.5.3's layout for this release only.
      </p>
    </div>
    <SegmentedControl
      options={layouts}
      value={$uiLayout}
      label="Layout"
      testidPrefix="appearance-layout-"
      onchange={(id) => uiLayout.set(id)} />
  </div>
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
</section>

<style>
  .pref {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 6px 0;
  }
  .pref-text { flex: 1; min-width: 0; }
  .lbl { font-size: 0.85rem; font-weight: 600; }
  .hook-desc {
    margin: 2px 0 0;
    font-size: 11px;
    color: var(--fg-muted);
  }
  .pref :global(.seg-group) { flex: 0 0 auto; min-width: 180px; }
</style>
