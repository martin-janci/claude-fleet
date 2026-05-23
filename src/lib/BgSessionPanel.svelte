<script lang="ts">
  import { onDestroy } from 'svelte';
  import { peekSession, type SessionRow } from './sessions';

  let { session }: { session: SessionRow } = $props();

  const POLL_MS = 10_000;

  let logs = $state<string>('');
  let logError = $state<string | null>(null);
  let loading = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  async function fetchLogs() {
    const id = session.claude_session_id;
    if (!id) return;
    loading = true;
    const r = await peekSession(session.host_alias, id);
    loading = false;
    if (r.ok) {
      logs = r.value;
      logError = null;
    } else {
      // Keep the last good transcript; surface the error inline.
      // `r.error` is an IpcError (see src/lib/result.ts) with a `.message: string`.
      logError = r.error.message;
    }
  }

  // Re-arm whenever the selected bg session changes. Clears any prior interval,
  // resets state, fetches immediately, then polls. No fetch when there is no
  // claude_session_id (synthetic row with nothing to read).
  $effect(() => {
    const id = session.claude_session_id;
    if (timer) clearInterval(timer);
    logs = '';
    logError = null;
    if (!id) return;
    void fetchLogs();
    timer = setInterval(() => void fetchLogs(), POLL_MS);
    return () => { if (timer) clearInterval(timer); };
  });

  onDestroy(() => { if (timer) clearInterval(timer); });
</script>

<div class="bg-panel" data-testid="bg-panel">
  <header class="bg-head">
    <span class="bg-name">🤖 {session.tmux_name}</span>
    {#if session.claude_status}
      <span class="bg-status" data-testid="bg-status">{session.claude_status}</span>
    {/if}
    <button
      class="refresh"
      data-testid="bg-refresh"
      disabled={!session.claude_session_id || loading}
      onclick={() => void fetchLogs()}
    >↻ Refresh</button>
  </header>

  {#if session.current_activity}
    <p class="bg-activity">{session.current_activity}</p>
  {/if}
  {#if session.pr_url}
    <p class="bg-pr"><a href={session.pr_url} target="_blank" rel="noreferrer">{session.pr_url}</a></p>
  {/if}

  {#if !session.claude_session_id}
    <p class="bg-empty">Background agent — no logs available yet.</p>
  {:else}
    {#if logError}
      <p class="err" data-testid="bg-log-error">log error: {logError}</p>
    {/if}
    <pre class="bg-logs" data-testid="bg-logs">{logs}</pre>
  {/if}
</div>

<style>
  .bg-panel { display: flex; flex-direction: column; height: 100%; padding: 0.5rem; gap: 0.4rem; overflow: hidden; }
  .bg-head { display: flex; align-items: center; gap: 0.5rem; }
  .bg-name { font-weight: 600; }
  .bg-status { font-size: 0.85em; opacity: 0.85; }
  .refresh { margin-left: auto; }
  .bg-activity { margin: 0; font-size: 0.9em; opacity: 0.9; }
  .bg-pr { margin: 0; font-size: 0.85em; }
  .bg-empty { opacity: 0.7; font-style: italic; }
  .bg-logs { flex: 1; overflow: auto; white-space: pre-wrap; font-family: var(--mono, monospace); font-size: 0.85em; margin: 0; }
  .err { color: var(--err, #c00); font-size: 0.85em; margin: 0; }
</style>
