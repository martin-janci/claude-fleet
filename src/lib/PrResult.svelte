<script lang="ts">
  // The Result card: what stands between this session's PR and a merge,
  // from the reading the row already carries (`pr_evidence`,
  // `pr_checked_at`). The rule is `evidence.ts` (Rust `service::evidence`'s
  // mirror); this only draws it. Nothing here gates or merges anything.
  import type { SessionRow } from './sessions';
  import { assessRow, describeReason, hasReading, shortSha, verdictLabel, verdictState } from './evidence';
  import StatusChip from './kit/StatusChip.svelte';
  import { timeAgo } from './session_status';

  let { session, nowSec }: { session: SessionRow; nowSec: number } = $props();

  const assessed = $derived(assessRow(session, nowSec));
  const result = $derived(hasReading(assessed) ? assessed : null);
  const ev = $derived(session.pr_evidence ?? null);
  const failing = $derived(result?.reasons.includes('checks_failing') ? (ev?.checks.failing ?? []) : []);
</script>

{#if result}
  <div class="result" data-testid="pr-result" data-verdict={result.verdict}>
    <div class="head">
      <StatusChip state={verdictState(result.verdict)} label={verdictLabel(result.verdict)} testid="pr-result-verdict" />
      {#if result.commit}<code class="sha" title={result.commit}>{shortSha(result.commit)}</code>{/if}
      {#if result.checked_at != null}
        <span class="muted" data-testid="pr-result-checked">checked {timeAgo(result.checked_at, nowSec * 1000)}</span>
      {/if}
    </div>
    <ul class="reasons">
      {#each result.reasons as r (r)}
        <li data-testid="pr-result-reason" data-reason={r}>{describeReason(r, ev, result.checked_at, nowSec * 1000)}</li>
      {/each}
    </ul>
    {#if failing.length > 0}
      <ul class="checks" data-testid="pr-result-failing">
        {#each failing as f, i (i)}
          <li>
            {#if f.url}<a href={f.url} target="_blank" rel="noreferrer">{f.name}</a>{:else}{f.name}{/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<style>
  .result {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }
  .sha {
    font-size: var(--text-2xs);
  }
  .muted {
    opacity: 0.65;
    font-size: var(--text-2xs);
  }
  .reasons,
  .checks {
    margin: 0;
    padding-left: 16px;
    font-size: var(--text-xs);
  }
  .checks {
    opacity: 0.8;
  }
</style>
