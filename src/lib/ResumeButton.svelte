<script lang="ts">
  // Resume ▾ (roadmap M2.5): the main half continues the last conversation
  // when the hub says that is possible, and otherwise opens the resume
  // dialog; ▾ always opens it (every mode, its reason, the brief preview).
  // The tooltip names where it would land from the link's own snapshot.
  import ResumeDialog from './ResumeDialog.svelte';
  import { resumeWork, workResumePlan, type WorkLink } from './work';
  import { selectSessionExplicitly } from './selection';
  import { pushError } from './toasts';

  let {
    workKey,
    link = null,
  }: {
    workKey: string;
    /** The ended link to resume from (default: the key's newest). */
    link?: WorkLink | null;
  } = $props();

  let open = $state(false);
  let busy = $state(false);

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
    disabled={busy}
    title={where ? `Continue the last conversation on ${where}` : 'Continue the last conversation'}
    data-testid="resume-quick"
    onclick={quick}>{busy ? '…' : 'Resume'}</button
  ><button
    type="button"
    class="more"
    title="More ways to resume"
    aria-label="More ways to resume {workKey}"
    data-testid="resume-more"
    onclick={more}>▾</button
  >
</span>

{#if open}
  <ResumeDialog {workKey} linkId={link?.id ?? null} onclose={() => (open = false)} />
{/if}

<style>
  .resume {
    display: inline-flex;
    flex: none;
  }
  button {
    font-size: 0.75em;
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
