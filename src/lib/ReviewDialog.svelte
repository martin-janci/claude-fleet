<script lang="ts">
  // Start a review run (step 5.10's one dialog pattern): who reviews (a skill
  // on the session's host, run by Claude Code or Codex, M15 G7.12) and what
  // it reads (the scope).
  // Both become lines shown above the prompt; the prompt stays editable, and
  // what is sent is exactly what the dialog shows.
  import { onMount } from 'svelte';
  import { spawnReview, DEFAULT_REVIEW_PROMPT, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import { loadInventory } from './assets';
  import DialogSheet from './DialogSheet.svelte';
  import {
    REVIEW_SCOPES,
    REVIEWER_AGENTS,
    reviewPreamble,
    reviewPrompt,
    reviewerSkills,
    type ReviewerAgent,
    type ReviewScope,
  } from './review_scope';
  import type { AssetInventoryRow } from './assets';

  let { source, onClose }: { source: SessionRow; onClose: () => void } = $props();

  let prompt = $state(DEFAULT_REVIEW_PROMPT);
  let skill = $state<string | null>(null);
  let agent = $state<ReviewerAgent>('claude');
  let inventory = $state<AssetInventoryRow[]>([]);
  const skills = $derived(reviewerSkills(inventory, source.host_alias, agent));
  let scope = $state<ReviewScope>('branch');
  let spawning = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;

  // Both halves (multi-user M1, F2a). `spawn_review` routes, so it needs the
  // live connection up — and it is `own` in `share.ts::SESSION_TIER`: it starts
  // a session in the owner's worktree with a terminal of its own. The opener in
  // `SessionDetails` already composes both, but this dialog does not re-ask on
  // confirm, so a reason that arrived while it was open (a revoke, a narrow)
  // would not have reached the Start button.
  const spawnBlocked = $derived(
    hubActionBlocked('spawn_review', $hubStatus, $hubConnection) ??
      $sessionBlocked(source, 'spawn_review'),
  );
  const canStart = $derived(prompt.trim().length > 0 && !spawning && spawnBlocked === null);
  const preamble = $derived(reviewPreamble(skill, scope));

  onMount(async () => {
    // Best effort: without an inventory the reviewer is plain Claude Code.
    const r = await loadInventory();
    if (r.ok && Array.isArray(r.value)) {
      inventory = r.value;
      skill = skills.find((n) => /review/i.test(n)) ?? null;
    }
  });

  function pickAgent(a: ReviewerAgent) {
    agent = a;
    // The skill list is the agent's harness's; keep the pick only if it is there.
    if (skill && !skills.includes(skill)) skill = skills.find((n) => /review/i.test(n)) ?? null;
  }

  async function start() {
    if (!canStart) return;
    spawning = true;
    error = null;
    controller = new AbortController();
    try {
      const r = await spawnReview(source.id, reviewPrompt(skill, scope, prompt), controller.signal, agent);
      if (r.ok) {
        selectSessionExplicitly(r.value);
        onClose();
      } else if (r.error.code !== 'E_CANCELLED') {
        error = r.error.message;
      }
    } finally {
      spawning = false;
      controller = null;
    }
  }
</script>

<DialogSheet
  title="Start a review run"
  lead="A second session reads the diff and comments; it never pushes."
  verb="Start review"
  busyVerb="Starting…"
  busy={spawning}
  canConfirm={canStart}
  onconfirm={() => void start()}
  onclose={onClose}
  {error}
  errorTestid="review-error"
  confirmTitle={spawnBlocked}
  confirmTestid="review-start"
  width="560px"
>
  <p class="src"><span class="tag tag--mono">{source.host_alias}</span> <span class="tag tag--mono">{source.tmux_name}</span></p>

  <label class="field">
    <span class="field-label">Reviewer</span>
    <span class="row">
      <select
        data-testid="review-skill"
        value={skill ?? ''}
        onchange={(e) => (skill = (e.currentTarget as HTMLSelectElement).value || null)}
      >
        <option value="">No skill</option>
        {#each skills as s (s)}<option value={s}>{s} skill</option>{/each}
      </select>
      <span class="field-note">run by</span>
      <select
        data-testid="review-agent"
        aria-label="Reviewer agent"
        value={agent}
        onchange={(e) => pickAgent((e.currentTarget as HTMLSelectElement).value as ReviewerAgent)}
      >
        {#each REVIEWER_AGENTS as a (a.value)}<option value={a.value}>{a.label}</option>{/each}
      </select>
    </span>
  </label>

  <label class="field">
    <span class="field-label">Scope</span>
    <select
      data-testid="review-scope"
      value={scope}
      onchange={(e) => (scope = (e.currentTarget as HTMLSelectElement).value as ReviewScope)}
    >
      {#each REVIEW_SCOPES as s (s.value)}<option value={s.value}>{s.label}</option>{/each}
    </select>
  </label>

  <div class="field">
    <span class="field-label" id="review-prompt-h">Prompt</span>
    <pre class="preamble" data-testid="review-preamble">{preamble}</pre>
    <textarea bind:value={prompt} rows="9" aria-labelledby="review-prompt-h" data-testid="review-textarea"></textarea>
  </div>

  {#if spawnBlocked}
    <p class="blocked" data-testid="review-blocked" role="status">{spawnBlocked}</p>
  {/if}
</DialogSheet>

<style>
  .src { margin: 0; display: flex; gap: var(--space-1); }
  .row { display: flex; align-items: center; gap: var(--space-2); }
  .preamble {
    margin: 0;
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--fg-muted);
    padding: var(--space-1) var(--space-2);
    border-left: 2px solid var(--border);
  }
  textarea { width: 100%; box-sizing: border-box; min-height: 8rem; }
  .blocked { color: var(--danger); font-size: var(--text-sm); margin: 0; }
</style>
