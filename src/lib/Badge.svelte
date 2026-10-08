<script lang="ts">
  import type { BadgeTone } from './assets_visual';

  /** One small label for a scope, a catalog, a count, an op or a state
   *  (spec, Visuals: one Badge replaces `.chip`, `.op-badge`, `.count-chip`).
   *  Information, never a control — a clickable one is a `.btn` holding a
   *  Badge. `tone` colours it; `label` (and `glyph`, `dashed`) carry the
   *  meaning, so it is never colour alone. */
  let {
    label,
    tone = 'neutral',
    glyph,
    dashed = false,
    mono = false,
    title,
    testid,
  }: {
    label: string;
    tone?: BadgeTone;
    glyph?: string;
    dashed?: boolean;
    mono?: boolean;
    title?: string;
    testid?: string;
  } = $props();
</script>

<span class="badge {tone}" class:dashed class:mono {title} data-testid={testid}
  >{#if glyph}<span class="glyph" aria-hidden="true">{glyph}</span>{/if}{label}</span
>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    box-sizing: border-box;
    height: 18px;
    padding: 0 6px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--control-fg-quiet);
    font-size: var(--control-font-sm);
    line-height: 1;
    white-space: nowrap;
  }
  .dashed { border-style: dashed; }
  .mono { font-family: var(--mono); }
  .ok { color: var(--usage-ok); border-color: color-mix(in srgb, var(--usage-ok) 45%, var(--control-border)); }
  .warn { color: var(--usage-warn); border-color: color-mix(in srgb, var(--usage-warn) 45%, var(--control-border)); }
  .crit { color: var(--usage-crit); border-color: color-mix(in srgb, var(--usage-crit) 45%, var(--control-border)); }
  .accent { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 45%, var(--control-border)); }
  .muted { color: var(--fg-muted); background: var(--bg-pane); }
  .glyph { font-size: 11px; }
</style>
