<script lang="ts">
  // One autofix finding (src/lib/autofix.ts): what is wrong, in plain words,
  // and its one-click fix. Shared by the Needs-you "Problems & fixes" rows,
  // the `fix` chat card and Control's own finding in the Agent panel.
  //
  // Fix always asks first, listing what the fix changes; only the desktop or
  // a trusted device may press it (`canApplyFixes`, from the hub). Later
  // hides the finding for a day. Details opens the explanation, the evidence
  // and the changes. A finding fleet cannot fix may carry a login on its
  // host, run here as the `error` card's login next step is.
  import ConfirmDialog from './ConfirmDialog.svelte';
  import AddAccountDialog from './AddAccountDialog.svelte';
  import { push } from './toasts';
  import { applyFinding, autoWords, canApplyFixes, dismissFinding, stateWords, subjectWords, type Finding } from './autofix';

  let { finding, compact = false }: { finding: Finding; compact?: boolean } = $props();

  let open = $state(false);
  let confirming = $state(false);
  let busy = $state(false);
  let failure = $state<string | null>(null);
  let loginOn = $state<string | null>(null);

  const fix = $derived(finding.fix ?? null);
  const status = $derived(stateWords(finding));
  const autoLine = $derived(autoWords(finding, Date.now() / 1000));
  const running = $derived(
    finding.state === 'verifying' && finding.last_attempt_at != null && Date.now() / 1000 - finding.last_attempt_at < 60,
  );
  const fixLabel = $derived(finding.state === 'apply_failed' ? 'Try again…' : 'Fix…');

  async function apply() {
    confirming = false;
    if (busy) return;
    busy = true;
    failure = null;
    const r = await applyFinding(finding.id);
    busy = false;
    if (!r.ok) {
      failure = r.error.message;
      return;
    }
    if (r.value.state === 'cleared') push({ kind: 'success', message: `Fixed: ${finding.title ?? finding.kind}` });
    else if (r.value.state === 'apply_failed') failure = r.value.last_error ?? 'The fix did not work.';
  }

  async function later() {
    if (busy) return;
    busy = true;
    failure = null;
    const r = await dismissFinding(finding.id);
    busy = false;
    if (!r.ok) failure = r.error.message;
  }
</script>

<section
  class="fix-card"
  class:compact
  class:blocking={finding.severity === 'blocking'}
  data-testid="fix-card"
  data-finding={finding.id}
  data-kind={finding.kind}
  aria-label={finding.title ?? 'Problem'}
>
  <header>
    <span class="dot" aria-hidden="true"></span>
    <strong class="title" data-testid="fix-card-title">{finding.title ?? finding.kind}</strong>
    <span class="subject">{subjectWords(finding.subject)}</span>
  </header>
  {#if !compact || open}
    {#if finding.explanation}<p class="why" data-testid="fix-card-explanation">{finding.explanation}</p>{/if}
  {/if}
  {#if open}
    <div class="details" data-testid="fix-card-details">
      {#if finding.evidence.length}
        <p class="label">What fleet saw</p>
        <ul>{#each finding.evidence as e, i (i)}<li>{e}</li>{/each}</ul>
      {/if}
      {#if fix}
        <p class="label">What the fix does{fix.reversible ? '' : ' (cannot be undone)'}</p>
        <ul>{#each fix.changes as c, i (i)}<li>{c}</li>{/each}</ul>
      {/if}
    </div>
  {/if}
  {#if status}<p class="status" class:warn={finding.state === 'apply_failed'} data-testid="fix-card-status">{status}</p>{/if}
  {#if failure}<p class="status warn" role="alert" data-testid="fix-card-failure">{failure}</p>{/if}
  {#if autoLine}<p class="meta" data-testid="fix-card-auto">{autoLine}</p>{/if}
  <div class="actions">
    {#if fix}
      {#if $canApplyFixes}
        <button
          type="button"
          class="btn btn--primary"
          disabled={busy || running}
          data-testid="fix-card-fix"
          title={fix.summary}
          onclick={() => (confirming = true)}>{busy ? 'Fixing…' : fixLabel}</button
        >
      {:else}
        <span class="meta" data-testid="fix-card-readonly">The desktop or a trusted device can apply this fix.</span>
      {/if}
    {:else if finding.action?.action === 'login'}
      <button type="button" class="btn btn--primary" data-testid="fix-card-login" onclick={() => (loginOn = finding.action!.host)}
        >Log in on {finding.action.host}</button
      >
    {/if}
    {#if $canApplyFixes && finding.state !== 'dismissed'}
      <button type="button" class="btn" disabled={busy} data-testid="fix-card-later" onclick={() => void later()}>Later</button>
    {/if}
    <button type="button" class="link" aria-expanded={open} data-testid="fix-card-details-btn" onclick={() => (open = !open)}
      >{open ? 'Hide details' : 'Details'}</button
    >
  </div>
</section>

{#if confirming && fix}
  <ConfirmDialog
    title={fix.summary}
    confirmLabel="Fix"
    danger={!fix.reversible}
    confirmTestId="fix-card-confirm"
    onconfirm={() => void apply()}
    oncancel={() => (confirming = false)}
  >
    <p>This changes:</p>
    <ul data-testid="fix-card-changes">{#each fix.changes as c, i (i)}<li>{c}</li>{/each}</ul>
    {#if !fix.reversible}<p>It cannot be undone.</p>{/if}
  </ConfirmDialog>
{/if}

{#if loginOn}
  <AddAccountDialog host={loginOn} onclose={() => (loginOn = null)} />
{/if}

<style>
  .fix-card {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.55rem 0.7rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--status-waiting);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .fix-card.blocking {
    border-left-color: var(--usage-crit);
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 0.45rem;
    min-width: 0;
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--status-waiting);
    transform: translateY(-1px);
  }
  .blocking .dot {
    background: var(--usage-crit);
  }
  .title {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--fg);
    min-width: 0;
  }
  .subject {
    margin-left: auto;
    flex: none;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .why,
  .status,
  .meta,
  .details {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-2);
  }
  .details ul {
    margin: 0.1rem 0 0.3rem;
    padding-left: 1.1rem;
  }
  .label {
    margin: 0.2rem 0 0;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--fg-muted);
  }
  .meta {
    color: var(--fg-muted);
  }
  .status.warn {
    color: var(--usage-warn);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.45rem;
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
    text-decoration: underline;
  }
</style>
