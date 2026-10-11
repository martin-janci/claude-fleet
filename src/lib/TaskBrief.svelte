<script lang="ts">
  // The task page's Brief block (G7.6, the Task detail and Work boards):
  // what goes with the first prompt of a session, and, with Writing help's
  // "Draft agent briefs" on, "Draft brief from the ticket". The draft runs
  // as the start popover's does (redesign 6.10: on the planned host, from
  // the ticket and the task's earlier work), shows Drafted with Regenerate,
  // Clear and Undo (`DraftField`), and is held for this task in this window
  // (`taskBriefDrafts`): every start of it from here sends it until it is
  // cleared. Nothing is written to the task or the tracker. Ticket text and
  // the draft render as text.
  import DraftField from './DraftField.svelte';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';
  import { untrack } from 'svelte';
  import {
    baseStartArgs,
    draftBrief,
    draftSource,
    holdTaskBrief,
    taskBriefDrafts,
    type BriefDraft,
  } from './start_preview';
  import { readErrorText, type WorkTask } from './work_view';

  let {
    task,
    notes = null,
  }: {
    task: Pick<WorkTask, 'task_id' | 'item_id' | 'key' | 'project_id'>;
    /** A native task's own brief (its notes). */
    notes?: string | null;
  } = $props();

  const enabled = $derived(settingBool($fleetSettings, SETTING_KEYS.workDraftBriefs));
  const canDraft = $derived(enabled && (task.item_id != null || !!task.key));
  const held = $derived($taskBriefDrafts.get(task.task_id) ?? null);

  // The field's text and what drafted it; the held brief follows every
  // edit, Undo included. `meta` stays after a Clear, so its Undo does too.
  let text = $state('');
  let meta = $state<BriefDraft | null>(null);
  let drafting = $state(false);
  let error = $state<string | null>(null);
  let seq = 0;
  let shownFor: string | null = null;
  $effect.pre(() => {
    // Another task opened: show its own held draft.
    if (shownFor === task.task_id) return;
    shownFor = task.task_id;
    seq++;
    drafting = false;
    error = null;
    const h = untrack(() => $taskBriefDrafts.get(task.task_id));
    text = h?.brief ?? '';
    meta = h?.draft ?? null;
  });
  $effect(() => {
    const id = task.task_id;
    const t = text;
    const m = meta;
    if (drafting || !m) return;
    const now = untrack(() => $taskBriefDrafts.get(id));
    if (t.trim() ? now?.brief === t : !now) return;
    holdTaskBrief(id, t.trim() ? { brief: t, draft: m } : null);
  });

  async function draft() {
    const mine = ++seq;
    drafting = true;
    error = null;
    const r = await draftBrief(baseStartArgs(task));
    if (mine !== seq) return;
    drafting = false;
    if (!r.ok) {
      error = readErrorText(r.error);
      return;
    }
    meta = r.value.draft;
    text = r.value.brief;
  }

  function cleared() {
    seq++;
    drafting = false;
  }
</script>

{#if notes || canDraft || meta}
  <section data-testid="task-brief">
    <h3>Brief <span class="hint">sent with the first prompt of every session</span></h3>
    {#if meta || drafting}
      <DraftField
        label="Brief"
        bind:value={text}
        model={meta?.model}
        host={meta?.host_alias}
        from={meta ? draftSource(meta) : null}
        busy={drafting}
        rows={6}
        onregenerate={canDraft ? () => void draft() : undefined}
        onclear={cleared}
        testid="task-brief-draft"
      />
      {#if held}
        <p class="muted" data-testid="task-brief-held">Goes with the sessions you start for this task from here.</p>
      {/if}
    {:else}
      {#if notes}<p class="text" data-testid="task-notes">{notes}</p>{/if}
      {#if canDraft}
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="task-brief-draft-ask"
          title="Write the brief from the ticket and earlier sessions on it, with a model call on the host a start would use"
          onclick={() => void draft()}>Draft brief from the ticket</button
        >
      {/if}
    {/if}
    {#if error}<p class="err" role="alert" data-testid="task-brief-error">{error}</p>{/if}
  </section>
{/if}

<style>
  h3 {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0 0 6px;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .hint {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
  }
  .text {
    margin: 0 0 6px;
    white-space: pre-wrap;
    max-width: 72ch;
  }
  .muted {
    margin: 4px 0 0;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .err {
    margin: 4px 0 0;
    color: var(--usage-crit);
    font-size: var(--text-2xs);
  }
</style>
