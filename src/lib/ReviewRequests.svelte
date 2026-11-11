<script lang="ts">
  // The Inbox's review asks ("Ask for review…", docs/review-notes.md): each
  // open ask to this person — who asked, on which session, their note —
  // opening the session, and Done to take it off the list. The list is the
  // `reviewRequests` store, which `trackReviewRequests` (main.ts) keeps fresh.
  import { get } from 'svelte/store';
  import { errorSentence } from './error_copy';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { askSessionName, finishReviewRequest, levelWords, reviewRequests, type ReviewRequest } from './review_requests';
  import { selectSessionExplicitly } from './selection';
  import { shortAge } from './session_status';
  import { loadSessions, sessions } from './sessions';
  import { push } from './toasts';

  const blocked = $derived(hubActionBlocked('review_request', $hubStatus, $hubConnection));

  async function open(r: ReviewRequest): Promise<void> {
    let s = $sessions.find((x) => x.id === r.session_id);
    if (!s) {
      // The share may be newer than this device's list: read it once more.
      await loadSessions({ force: true });
      s = get(sessions).find((x) => x.id === r.session_id);
    }
    if (s) selectSessionExplicitly(s);
    else push({ kind: 'info', message: 'You cannot open that session any more: its owner took the share back. Mark the ask done to clear it.' });
  }
  async function done(r: ReviewRequest): Promise<void> {
    if (blocked !== null) return;
    const out = await finishReviewRequest(r.id);
    if (!out.ok) push({ kind: 'error', message: errorSentence(out.error) });
  }
</script>

{#if $reviewRequests.length > 0}
  <div class="asks" data-testid="review-requests" role="group" aria-label="Reviews asked of you">
    {#each $reviewRequests as r (r.id)}
      {@const s = $sessions.find((x) => x.id === r.session_id)}
      <div class="item" data-testid="review-request">
        <span class="dot" aria-label="Needs you"></span>
        <div class="text">
          <button type="button" class="name" data-testid="review-request-open" onclick={() => void open(r)}
            >{askSessionName(r, s)} <span class="chip">review</span></button
          >
          <span class="line">{r.from_name} asks you to review · you {levelWords(r.level)} · {shortAge(r.created_at)}</span>
          {#if r.note}<span class="note" data-testid="review-request-note">{r.note}</span>{/if}
        </div>
        <button
          type="button"
          class="done"
          data-testid="review-request-done"
          disabled={blocked !== null}
          title={blocked ?? 'Take it off your Inbox'}
          onclick={() => void done(r)}>Done</button
        >
      </div>
    {/each}
  </div>
{/if}

<style>
  .asks { display: flex; flex-direction: column; gap: 2px; padding: 4px 8px; }
  .item { display: flex; gap: 8px; align-items: flex-start; padding: 6px 4px; border-radius: var(--radius-md); }
  .item:hover { background: var(--bg-hover); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--status-waiting); margin-top: 5px; flex: none; }
  .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
  .name { border: 0; background: none; padding: 0; color: var(--fg); font: inherit; font-weight: 500; text-align: left; cursor: pointer; }
  .chip { font-size: var(--text-2xs); color: var(--fg-muted); border: 1px solid var(--border); border-radius: var(--radius-pill); padding: 0 6px; font-weight: 400; }
  .line { color: var(--fg-muted); font-size: var(--text-xs); overflow-wrap: anywhere; }
  .note { font-size: var(--text-xs); white-space: pre-wrap; overflow-wrap: anywhere; }
  .done { flex: none; font-size: var(--text-xs); padding: 0.1rem 0.5rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; }
  .done:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
