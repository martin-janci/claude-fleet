<script lang="ts">
  // The Result card: what stands between this session's PR and a merge,
  // from the reading the row already carries (`pr_evidence`,
  // `pr_checked_at`). The rule is `evidence.ts` (Rust `service::evidence`'s
  // mirror); this only draws it. Nothing here gates or merges anything.
  import type { SessionRow } from './sessions';
  import { assessRow, describeReason, hasReading, shortSha, verdictColor, verdictLabel } from './evidence';
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
      <span
        class="verdict"
        data-testid="pr-result-verdict"
        style="color: {verdictColor(result.verdict)}; border-color: color-mix(in srgb, {verdictColor(result.verdict)} 33%, transparent);"
      >{verdictLabel(result.verdict)}</span>
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
  .verdict {
    font-size: 11px;
    border: 1px solid;
    border-radius: 3px;
    padding: 0 5px;
  }
  .sha {
    font-size: 11px;
  }
  .muted {
    opacity: 0.65;
    font-size: 11px;
  }
  .reasons,
  .checks {
    margin: 0;
    padding-left: 16px;
    font-size: 12px;
  }
  .checks {
    opacity: 0.8;
  }
</style>
