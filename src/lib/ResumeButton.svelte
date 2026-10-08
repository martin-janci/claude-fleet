<script lang="ts">
  // Resume ▾ (roadmap M2.5): the main half continues the last conversation
  // when the hub says that is possible, and otherwise opens the resume
  // dialog; ▾ always opens it (every mode, its reason, the brief preview).
  // The tooltip names where it would land from the link's own snapshot.
  // The New layout says "Continue", the word every task start uses (6.6).
  import ResumeDialog from './ResumeDialog.svelte';
  import { linkSessionId, resumeWork, workResumePlan, type WorkLink } from './work';
  import { selectSessionExplicitly } from './selection';
  import { sessions } from './sessions';
  import { sessionIdBlocked } from './share';
  import { pushError } from './toasts';
  import { uiLayout } from './prefs';

  let {
    workKey,
    link = null,
    sessionId = null,
  }: {
    workKey: string;
    /** The ended link to resume from (default: the key's newest). */
    link?: WorkLink | null;
    /** The SOURCE session, where the caller knows it (TidyReview does). Falls
     *  back to resolving `link`'s snapshot. */
    sessionId?: number | null;
  } = $props();

  let open = $state(false);
  let busy = $state(false);

  /**
   * Multi-user M1 (F2c): Resume is `resume_work`, `share.ts`'s `own` tier —
   * it re-opens somebody's Claude conversation. The question is about the
   * session the transcript belongs to, so the row is resolved from the link's
   * snapshot (`linkSessionId`) and `$sessionIdBlocked` fails closed when it
   * cannot be: the sidebar's past-work groups come from the hub's org-scoped
   * `recent_ended` read, so a past link there is not necessarily this person's.
   */
  const sourceId = $derived(sessionId ?? linkSessionId(link, $sessions));
  const shareBlocked = $derived($sessionIdBlocked(sourceId, 'resume_work'));

  const where = $derived(
    link
      ? [link.snap_host, link.snap_branch ? `branch ${link.snap_branch}` : null]
          .filter(Boolean)
          .join(' · ')
      : '',
  );

  async function quick(e: MouseEvent) {
    e.stopPropagation();
    if (busy) return;
    // Re-asked at the write: the row sits in a list that outlives a revoke.
    if (shareBlocked !== null) return;
    busy = true;
    const plan = await workResumePlan(workKey, { linkId: link?.id ?? null });
    const last = plan.ok ? plan.value.modes.find((m) => m.mode === 'last') : undefined;
    if (!plan.ok || !last?.ok || (plan.value.live ?? []).length > 0) {
      busy = false;
      // Say why in the dialog rather than guessing another mode here.
      open = true;
      return;
    }
    const r = await resumeWork({ key: workKey, mode: 'last', linkId: plan.value.link_id ?? null });
    busy = false;
    if (!r.ok) pushError(r.error, `Resume ${workKey} failed`);
    else selectSessionExplicitly(r.value);
  }

  function more(e: MouseEvent) {
    e.stopPropagation();
    open = true;
  }
</script>

<span class="resume" data-testid="resume-button">
  <button
    type="button"
    class="main"
    disabled={busy || shareBlocked !== null}
    title={shareBlocked ?? (where ? `Continue the last conversation on ${where}` : 'Continue the last conversation')}
    data-testid="resume-quick"
    onclick={quick}>{busy ? '…' : $uiLayout === 'new' ? 'Continue' : 'Resume'}</button
  ><button
    type="button"
    class="more"
    title="More ways to resume"
    aria-label="More ways to resume {workKey}"
    aria-haspopup="dialog"
    aria-expanded={open}
    data-testid="resume-more"
    onclick={more}>▾</button
  >
</span>

{#if open}
  <ResumeDialog {workKey} linkId={link?.id ?? null} sessionId={sourceId} onclose={() => (open = false)} />
{/if}

<style>
  .resume {
    display: inline-flex;
    flex: none;
  }
  button {
    font-size: 11px;
    padding: 0 0.35rem;
    line-height: 1.5;
    border: 1px solid var(--border, #444);
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .main {
    border-radius: 3px 0 0 3px;
  }
  .more {
    border-left: none;
    border-radius: 0 3px 3px 0;
  }
  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
