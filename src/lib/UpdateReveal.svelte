<script lang="ts">
  // Redesign step 3.15, "After an update": the Wordmark reveal plays once with
  // "Orbit Fleet <version> · What's new". A card in the corner, not a modal:
  // the app is usable under it, and it stays until closed.
  import { openUrl } from '@tauri-apps/plugin-opener';
  import Loader from './Loader.svelte';
  import { releaseNotesUrl } from './startup';

  let { version, onclose }: { version: string; onclose: () => void } = $props();
</script>

<div class="update-reveal" data-testid="update-reveal" role="status">
  <Loader name="wordmark-reveal" size={184} delay={0} label="Orbit Fleet" testid="update-reveal-wordmark" />
  <p>
    Orbit Fleet {version} ·
    <a
      href={releaseNotesUrl(version)}
      data-testid="update-reveal-notes"
      onclick={(e) => {
        e.preventDefault();
        void openUrl(releaseNotesUrl(version));
      }}>What's new</a
    >
  </p>
  <button type="button" class="btn btn--quiet close" aria-label="Close" data-testid="update-reveal-close" onclick={onclose}
    >×</button
  >
</div>

<style>
  .update-reveal {
    position: fixed;
    right: var(--space-4);
    bottom: calc(var(--status-h) + var(--space-4));
    z-index: 800;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-6) var(--space-3);
    background: var(--bg-raise);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-pop);
  }
  p {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--fg-2);
  }
  .close {
    position: absolute;
    top: var(--space-1);
    right: var(--space-1);
  }
</style>
