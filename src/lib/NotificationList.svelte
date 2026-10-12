<script lang="ts">
  // The notification centre's list (notifications.ts): every toast this
  // window showed, newest first. A toast's button is offered here only while
  // the toast is still up; after that the entry is something to read.
  import { markAllNoticesRead, clearNotices, notices, removeNotice, unreadNotices, type Notice } from './notifications';
  import { runToastAction, toasts } from './toasts';
  import { openSettingsAt } from './app_views';
  import Icon from './kit/Icon.svelte';

  // `onleave`: the sheet holding the list closes before ⚙ opens Settings,
  // so the two dialogs do not stack.
  let { onleave }: { onleave?: () => void } = $props();

  /** ⚙: Settings › Notifications, where what reaches you (and where) is set. */
  function openNotificationSettings() {
    onleave?.();
    openSettingsAt('notifications');
  }

  const KIND_MARK = { info: 'ℹ', success: '✓', warning: '!', error: '✕' } as const;

  function age(at: number, now = Date.now()): string {
    const s = Math.max(0, Math.floor((now - at) / 1000));
    if (s < 60) return 'just now';
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return `${Math.floor(s / 86400)} d ago`;
  }

  const live = (n: Notice) => $toasts.find((t) => t.id === n.toastId && t.action);
</script>

<div class="bar">
  <button type="button" data-testid="notices-mark-read" disabled={$unreadNotices === 0} onclick={markAllNoticesRead}>Mark all read</button>
  <button type="button" data-testid="notices-clear" disabled={$notices.length === 0} onclick={clearNotices}>Clear all</button>
  <button
    type="button"
    class="gear"
    data-testid="notices-settings"
    aria-label="Notification settings"
    title="Notification settings"
    onclick={openNotificationSettings}><Icon name="settings" size={14} /></button
  >
</div>
{#if $notices.length === 0}
  <p class="hint" data-testid="notices-empty">Nothing yet. What fleet tells you in the corner stays here for this window.</p>
{:else}
  <ul class="list">
    {#each $notices as n (n.id)}
      {@const t = live(n)}
      <li class="row {n.kind}" class:unread={!n.read} data-testid="notice-row" data-kind={n.kind}>
        <span class="mark" aria-hidden="true">{KIND_MARK[n.kind]}</span>
        <div class="main">
          <span class="message">{n.message}{#if n.count > 1}<span class="count"> ×{n.count}</span>{/if}</span>
          {#if n.sub}<span class="sub" data-testid="notice-sub">{n.sub}</span>{/if}
          <span class="meta">{#if n.code}<code>{n.code}</code> · {/if}{age(n.at)}</span>
        </div>
        <div class="actions">
          {#if t?.action}
            <button type="button" data-testid="notice-action" onclick={() => runToastAction(t.id)}>{t.action.label}</button>
            {#if t.secondary}
              <button type="button" data-testid="notice-secondary" onclick={() => runToastAction(t.id, 'secondary')}>{t.secondary.label}</button>
            {/if}
          {/if}
          <button type="button" aria-label="Remove" title="Remove" onclick={() => removeNotice(n.id)}>✕</button>
        </div>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .bar {
    display: flex;
    justify-content: flex-end;
    gap: 0.35rem;
    margin-bottom: 0.3rem;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 60vh;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    padding: 0.5rem 0;
    border-bottom: 1px solid var(--border);
  }
  .mark {
    width: 1rem;
    flex: none;
    text-align: center;
    color: var(--fg-muted);
  }
  .success .mark { color: var(--usage-ok); }
  .warning .mark { color: var(--usage-warn); }
  .error .mark { color: var(--usage-crit); }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .message { word-break: break-word; }
  .sub { font-size: var(--text-2xs); color: var(--fg-muted); }
  .gear { display: inline-flex; align-items: center; padding: 0.2rem 0.4rem; }
  .unread .message { font-weight: 600; }
  .count,
  .meta,
  .hint {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .actions {
    display: flex;
    gap: 0.35rem;
    flex: none;
  }
  button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.2rem 0.55rem;
  }
  button:hover:not(:disabled) { color: var(--fg); }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
