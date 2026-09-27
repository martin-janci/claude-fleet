<script lang="ts">
  // The sidebar's `Sessions | Work` switch (work graph M14.2), ⌘⇧W /
  // Ctrl+Shift+W. Sessions is the tree as it always was; Work is the Work
  // view. The choice is kept across restarts.
  import { sidebarMode } from './work_tree';
  import { workChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';

  const chord = workChordLabel(detectMac(typeof navigator === 'undefined' ? undefined : navigator));
</script>

<div class="switch" role="radiogroup" aria-label="Sidebar view" data-testid="sidebar-switch">
  <button
    type="button"
    role="radio"
    aria-checked={$sidebarMode === 'sessions'}
    class:active={$sidebarMode === 'sessions'}
    title="Sessions by host and project ({chord})"
    data-testid="sidebar-switch-sessions"
    onclick={() => sidebarMode.set('sessions')}>Sessions</button
  >
  <button
    type="button"
    role="radio"
    aria-checked={$sidebarMode === 'work'}
    class:active={$sidebarMode === 'work'}
    title="Work by organisation, group and task ({chord})"
    data-testid="sidebar-switch-work"
    onclick={() => sidebarMode.set('work')}>Work</button
  >
</div>

<style>
  .switch {
    display: flex;
    gap: 1px;
    margin: 0.4rem 0.6rem 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 1px;
    flex: none;
  }
  button {
    flex: 1 1 0;
    background: transparent;
    border: none;
    border-radius: var(--radius-pill);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.74rem;
    padding: 0.15rem 0.6rem;
  }
  button:hover { color: var(--fg); }
  button.active { background: var(--accent-soft); color: var(--fg); }
</style>
