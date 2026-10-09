<script lang="ts">
  // This desktop's own update (update-channel design S7, §11): "0.5.5 is
  // available — restart to update", or the required form a hub that refuses
  // this build needs. A strip above the layout like the hub banners, never
  // an overlay. The decision and the install are the backend's
  // (src/lib/updates.ts); this only shows them.
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { bannerFor, desktopUpdate, installUpdate, updateError, updateProgress } from './updates';

  const DISMISS_KEY = 'claude-fleet.update.dismissed';
  let dismissed = $state<string | null>(readDismissed());
  function readDismissed(): string | null {
    try {
      return sessionStorage.getItem(DISMISS_KEY);
    } catch {
      return null;
    }
  }
  function dismiss(version: string | null) {
    dismissed = version;
    try {
      if (version) sessionStorage.setItem(DISMISS_KEY, version);
    } catch {
      // Not remembered across reloads; the banner still goes away now.
    }
  }

  const u = $derived($desktopUpdate);
  const banner = $derived(bannerFor(u, dismissed));
  const progress = $derived($updateProgress);
  const percent = $derived(
    progress && progress.total ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : null,
  );

  async function act() {
    if (!banner || !u) return;
    if (banner.action === 'restart') await installUpdate();
    else if (banner.action === 'download' && u.download_url) await openUrl(u.download_url);
  }
</script>

{#if banner}
  <div class="update-banner" data-tone={banner.tone} role="status" data-testid="update-banner">
    <span class="text">{banner.text}</span>
    {#if progress}
      <span class="progress" data-testid="update-progress">
        {percent !== null ? `Downloading… ${percent}%` : 'Downloading…'}
      </span>
    {:else if banner.action}
      <button type="button" class="act" data-testid="update-action" onclick={act}>
        {banner.action === 'restart' ? 'Restart to update' : 'Download'}
      </button>
    {/if}
    {#if u?.notes_url}
      <button type="button" class="link" onclick={() => u?.notes_url && openUrl(u.notes_url)}>What's new</button>
    {/if}
    {#if $updateError}<span class="error" data-testid="update-error">{$updateError}</span>{/if}
    {#if banner.dismissible && !progress}
      <button type="button" class="close" aria-label="Not now" data-testid="update-dismiss" onclick={() => dismiss(u?.version ?? null)}>×</button>
    {/if}
  </div>
{/if}

<style>
  .update-banner {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.4rem 0.9rem;
    font-size: 0.85rem;
    background: var(--surface-2, #eef3ff);
    color: var(--text, inherit);
    border-bottom: 1px solid var(--border, rgba(0, 0, 0, 0.1));
  }
  .update-banner[data-tone='warn'] {
    background: var(--warn-bg, #fff4e5);
  }
  .text {
    flex: 1;
  }
  .act {
    font-weight: 600;
  }
  .link,
  .close {
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
  }
  .link {
    text-decoration: underline;
  }
  .error {
    color: var(--danger, #b42318);
  }
</style>
