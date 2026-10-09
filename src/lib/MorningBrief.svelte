<!--
  Redesign step 9.11: Today's morning brief, an LLM draft of today's digest.
  It shows the brief drafted last with its time; it never drafts on open.
  Draft (the first time) and Refresh ask the hub for a new one, which runs
  `claude -p` on a host of the brief's org and books the run as `brief`.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import DraftField from './DraftField.svelte';
  import { HUB_HAS_NO_DRAFTS, draftedAt, todayBrief, type Draft } from './drafts';

  let draft = $state<Draft | null>(null);
  let text = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** False on a hub older than 9.11: the block stays away. */
  let served = $state(true);

  function show(d: Draft | null | undefined) {
    draft = d ?? null;
    text = d?.text ?? '';
  }

  async function load(refresh: boolean) {
    if (refresh) busy = true;
    error = null;
    const r = await todayBrief(refresh);
    busy = false;
    if (r.ok) {
      show(r.value.draft);
    } else if (!refresh && HUB_HAS_NO_DRAFTS.includes(r.error.code)) {
      served = false;
    } else {
      error = r.error.message;
    }
  }

  onMount(() => void load(false));
</script>

{#if served}
  <section class="brief" data-testid="morning-brief" aria-label="Morning brief">
    <h3>
      Morning brief
      {#if draft}
        <span class="when" data-testid="morning-brief-at">{draftedAt(draft.at)}</span>
      {/if}
    </h3>
    {#if draft || busy}
      <DraftField
        bind:value={text}
        label="Brief"
        model={draft?.model}
        host={draft?.host_alias}
        from={draft ? `from ${draft.from}` : null}
        {busy}
        rows={6}
        onregenerate={() => void load(true)}
        onclear={() => show(null)}
        testid="morning-brief-draft"
      />
    {:else}
      <button class="btn btn--quiet" type="button" data-testid="morning-brief-draft-btn" onclick={() => void load(true)}
        >Draft the brief</button
      >
    {/if}
    {#if error}
      <p class="error" role="alert" data-testid="morning-brief-error">{error}</p>
    {/if}
  </section>
{/if}

<style>
  .brief {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 12px;
  }
  h3 {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin: 0;
  }
  .when {
    font-size: 11px;
    font-weight: 400;
    color: var(--fg-muted);
  }
  .error {
    color: var(--danger);
    font-size: 12px;
    margin: 0;
  }
</style>
