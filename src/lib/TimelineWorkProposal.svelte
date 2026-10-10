<!--
  Redesign step 6.8: the J1 suggested link at the head of the Details
  timeline, as on the SessionDetails board ("Linked to TASK-219 · you
  confirmed · Proposed by Jev"). Two shapes:

  * a suggestion the decision model made (rule R12) and nobody has decided:
    the chip with ✦, "Proposed by Jev · from the first prompt · 82%", and
    Link / Not this — a person's click, nothing links by itself;
  * the session's primary link that started as one: who proposed it, kept
    after the person confirmed it, as the AI patterns board's "When AI
    changed something" line with its Undo (G7.15), which puts the link back
    to a suggestion.

  The gating is the backend's: a J1 answer becomes a suggestion only in
  assist mode (`service/decide/work_link.rs`); shadow records it and writes
  no link, so nothing shows here.
-->
<script lang="ts">
  import ProposedBy from './ProposedBy.svelte';
  import AiChangeLine from './AiChangeLine.svelte';
  import WorkChip from './WorkChip.svelte';
  import Button from './kit/Button.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import { pushError } from './toasts';
  import {
    confirmSessionWork,
    rejectWorkLink,
    sessionWorkLinks,
    linkProposal,
    workWhy,
    JEV_RULE,
    type WorkLink,
  } from './work';
  import { reconsiderWorkLink, workSessionTasks } from './work_view';
  import type { ProposalLike } from './ai_proposal';
  import type { SessionRow } from './sessions';

  let { session }: { session: SessionRow } = $props();

  const suggestion = $derived(
    session.work_suggested?.rule === JEV_RULE ? session.work_suggested : null,
  );
  const linked = $derived(
    !suggestion && session.work?.rule === JEV_RULE && (session.work.state ?? 'confirmed') === 'confirmed'
      ? session.work
      : null,
  );
  const shown = $derived(suggestion ?? linked);

  // The evidence (and the confidence it carries) lives on the link, not on
  // the row: read it once per link.
  let links = $state<WorkLink[]>([]);
  let readFor = -1;
  $effect(() => {
    const id = shown?.link_id ?? -1;
    if (id < 0 || id === readFor) return;
    readFor = id;
    void sessionWorkLinks(session.id).then((r) => {
      if (readFor === id) links = r.ok && Array.isArray(r.value) ? r.value : [];
    });
  });

  const proposal = $derived.by<ProposalLike | null>(() => {
    if (!shown) return null;
    const l = links.find((x) => x.id === shown.link_id);
    return l ? linkProposal(l) : null;
  });
  const key = $derived(shown ? (shown.key || shown.title) : '');

  const blocked = $derived(
    hubActionBlocked('link_session_work', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'link_session_work'),
  );
  let busy = $state(false);

  async function decide(confirm: boolean): Promise<void> {
    const s = suggestion;
    if (!s || busy || blocked !== null) return;
    busy = true;
    const r = confirm ? await confirmSessionWork(session.id, s.link_id) : await rejectWorkLink(session.id, s.link_id);
    busy = false;
    if (!r.ok) pushError(r.error, confirm ? 'Link failed' : 'Not this failed');
  }

  /** Undo the confirmed link: back to a suggestion, a compare-and-set on
   *  the version the link has now (a link someone moved on meanwhile is
   *  refused, not overwritten). */
  async function undo(): Promise<void> {
    const l = linked;
    if (!l || busy || blocked !== null) return;
    busy = true;
    const t = await workSessionTasks(session.id);
    const v = t.ok ? (t.value?.links ?? []).find((x) => x.link_id === l.link_id)?.link_version : undefined;
    const r = await reconsiderWorkLink(session.id, l.link_id, typeof v === 'number' ? v : undefined);
    busy = false;
    if (!r.ok) pushError(r.error, 'Undo failed');
  }
</script>

{#if shown && key}
  <div class="j1" data-testid="timeline-work-proposal" data-state={suggestion ? 'suggested' : 'confirmed'}>
    <div class="line">
      {#if suggestion}
        <span>Suggested link</span>
        <WorkChip
          workKey={{ key, source: 'link', from: shown.title, why: workWhy({ ...shown, state: 'suggested' }) }}
          suggested
          proposed
          testid="timeline-work-chip"
        />
      {:else}
        <AiChangeLine
          what="Linked to {key}"
          source={proposal?.source ?? 'jev'}
          onundo={() => void undo()}
          undoing={busy}
          undoBlocked={blocked}
          testid="timeline-ai-change"
        />
      {/if}
    </div>
    {#if suggestion}<ProposedBy {proposal} field="work_link" testid="timeline-proposed-by" />{/if}
    {#if suggestion}
      <div class="acts">
        <Button
          size="sm"
          testid="timeline-work-link"
          disabled={busy || blocked !== null}
          title={blocked ?? 'Link this session to it'}
          onclick={() => void decide(true)}>Link</Button
        >
        <Button
          size="sm"
          variant="quiet"
          testid="timeline-work-reject"
          disabled={busy || blocked !== null}
          title={blocked ?? 'Not this: never proposed again'}
          onclick={() => void decide(false)}>Not this</Button
        >
      </div>
    {/if}
  </div>
{/if}

<style>
  .j1 {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    margin-bottom: 0.4rem;
    border: 1px dashed var(--border);
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .acts {
    display: flex;
    gap: 4px;
  }
</style>
