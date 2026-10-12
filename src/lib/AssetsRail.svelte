<script lang="ts">
  import Icon from './kit/Icon.svelte';
  import { RAIL_VIEWS, type WorkspaceView } from './assets_workspace';

  /** The rail (spec: Inbox default, Library; Layers and Hosts join with
   *  their views in M6, R15). Secrets opens the existing panel. */
  let {
    view,
    counts,
    readOnly,
    busy = false,
    onview,
    onsecrets,
  }: {
    view: WorkspaceView;
    counts: Record<WorkspaceView, number>;
    readOnly: boolean;
    /** Something is running: Secrets waits, as the old toolbar button did. */
    busy?: boolean;
    onview: (v: WorkspaceView) => void;
    onsecrets: () => void;
  } = $props();
</script>

<nav class="rail" aria-label="Assets views">
  {#each RAIL_VIEWS as r (r.id)}
    <button
      type="button"
      class:on={view === r.id}
      aria-current={view === r.id ? 'page' : undefined}
      onclick={() => onview(r.id)}
      aria-label={counts[r.id] ? `${r.label}, ${counts[r.id]}` : r.label}
      title={r.label}
      data-testid={`assets-rail-${r.id}`}
    >
      <Icon name={r.id} size={15} />
      <span class="lbl">{r.label}</span>
      {#if counts[r.id]}<span class="ct">{counts[r.id]}</span>{/if}
    </button>
  {/each}
  {#if !readOnly}
    <div class="sep" role="separator"></div>
    <button type="button" onclick={onsecrets} disabled={busy} aria-label="Secrets" title="Secrets" data-testid="assets-secrets">
      <Icon name="key" size={15} /><span class="lbl">Secrets</span>
    </button>
  {/if}
</nav>

<style>
  .rail { display: flex; flex-direction: column; gap: 2px; padding: 10px 8px; border-right: 1px solid var(--border); background: var(--bg-pane); }
  .rail button {
    display: flex; align-items: center; gap: 9px; height: 28px; padding: 0 8px; border: 0; border-radius: var(--radius-md);
    background: none; color: var(--control-fg-quiet); font: inherit; font-size: var(--text-xs); cursor: pointer; text-align: left;
  }
  .rail button:hover:not(:disabled) { background: var(--control-bg-hover); }
  .rail button:disabled { opacity: 0.5; cursor: default; }
  .rail button.on { background: var(--accent-soft); color: var(--fg); font-weight: 600; }
  .rail button:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .ct { margin-left: auto; font-size: var(--text-2xs); font-variant-numeric: tabular-nums; color: var(--fg-muted); font-weight: 400; }
  .on .ct { color: var(--accent); }
  .sep { height: 1px; margin: 8px 4px; background: var(--border); }
  /* Narrow: an icon strip. The buttons keep their names (aria-label, title). */
  @media (max-width: 1100px) {
    .rail { padding: 10px 6px; }
    .rail button { justify-content: center; gap: 4px; padding: 0 4px; }
    .lbl { display: none; }
    .ct { margin-left: 0; }
  }
</style>
