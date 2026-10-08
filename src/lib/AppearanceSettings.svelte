<script lang="ts">
  // Settings → Appearance (redesign step 0.3): the layout switch and the
  // theme picker. Hand-written, not a generated page, because both are
  // per-device prefs in localStorage rather than fleet settings. Copy
  // follows the Settings board of the Orbit Fleet canvas. This is the one
  // theme picker: step 1.4 removed the sidebar's "theme: …" line.
  import SegmentedControl from './SegmentedControl.svelte';
  import { uiLayout, type UiLayout } from './prefs';
  import { applyTheme, theme, type Theme } from './theme';

  const layouts = [
    { id: 'classic', label: 'Classic' },
    { id: 'new', label: 'New' },
  ] as const satisfies readonly { id: UiLayout; label: string }[];

  const themes = [
    { id: 'auto', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ] as const satisfies readonly { id: Theme; label: string }[];
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
        inspector. Classic keeps 0.5.3's layout while the new one reaches parity.
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
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .pref :global(.seg) { flex: 0 0 auto; min-width: 180px; }
</style>
