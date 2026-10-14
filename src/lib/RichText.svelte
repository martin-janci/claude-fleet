<script lang="ts">
  // An assistant text block: Markdown, with the parts rich_blocks.ts
  // recognises (a task report, a fleet.ui/1 block, a work handover) drawn
  // as cards. A card
  // that acts only ever fills the session's composer; nothing is sent from
  // here. `sessionId` null (an earlier conversation, a read-only row) keeps
  // every card but turns its actions off.
  //
  // `result` is the task result of the work this session is on (design
  // 2026-10-13, D6). The report it was read from, matched on its done
  // marker, draws as the result panel; every other report stays a
  // ReportCard.
  import { fenced, splitRich } from './rich_blocks';
  import Markdown from './MarkdownView.svelte';
  import ReportCard from './rich/ReportCard.svelte';
  import UiBlockCard from './rich/UiBlockCard.svelte';
  import HandoverCard from './rich/HandoverCard.svelte';
  import TaskResultPanel from './TaskResultPanel.svelte';
  import type { TaskResultView } from './task_result';

  let {
    source,
    sessionId = null,
    result = null,
    resultTitle = '',
    onOpenTask,
    onResultChanged,
  }: {
    source: string;
    sessionId?: number | null;
    result?: TaskResultView | null;
    resultTitle?: string;
    onOpenTask?: (id: number) => void;
    onResultChanged?: () => void;
  } = $props();

  const segments = $derived(splitRich(source));
</script>

{#each segments as s, i (i)}
  {#if s.t === 'md'}
    <Markdown source={s.source} />
  {:else if s.t === 'report' && result && result.marker === s.marker}
    <TaskResultPanel {result} title={resultTitle} {sessionId} {onOpenTask} onChanged={onResultChanged} />
  {:else if s.t === 'report'}
    <ReportCard report={s.report} raw={s.raw} marker={s.marker} {sessionId} />
  {:else if s.t === 'handover'}
    <HandoverCard handover={s.handover} raw={s.raw} nonce={s.nonce} {sessionId} />
  {:else if s.t === 'ui'}
    <UiBlockCard block={s.block} raw={s.raw} {sessionId} />
  {:else}
    <Markdown source={fenced(s.lang, s.raw)} />
    <p class="invalid" data-testid="rich-invalid">
      Not shown as a card: {s.problems.join('; ')}.
    </p>
  {/if}
{/each}

<style>
  .invalid {
    margin: -0.3em 0 0.7em;
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
</style>
