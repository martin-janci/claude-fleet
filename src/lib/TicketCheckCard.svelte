<script lang="ts">
  // The AI check on done (docs/work-graph.md#ai-check-on-done): what the
  // tester found for the task's finished work, Test now / Rerun, Accept
  // anyway, and the project's switch.
  import { errorText } from './error_copy';
  import { timeAgo } from './session_status';
  import StatusChip from './kit/StatusChip.svelte';
  import Badge from './Badge.svelte';
  import {
    CHECK_MODES,
    CHECK_MODE_LABEL,
    acceptCheck,
    canAccept,
    checkHeadline,
    runLabel,
    runTicketCheck,
    setTicketCheck,
    type CheckMode,
    type TicketCheckView,
  } from './ticket_check';

  let {
    check,
    itemId,
    onChanged,
  }: {
    check: TicketCheckView;
    itemId: number;
    onChanged?: () => void;
  } = $props();

  /** How often a running check is read again. */
  const POLL_MS = 10_000;

  const head = $derived(checkHeadline(check));
  const blockers = $derived(check.blockers ?? []);
  const testsRun = $derived(check.tests_run ?? []);
  const setup = $derived(check.setup ?? null);

  let running = $state(false);
  let runError = $state<string | null>(null);

  let accepting = $state(false);
  let acceptOpen = $state(false);
  let acceptNote = $state('');
  let acceptError = $state<string | null>(null);

  let setupOpen = $state(false);
  let mode = $state<CheckMode>('off');
  let command = $state('');
  let saving = $state(false);
  let setupError = $state<string | null>(null);

  function openSetup() {
    mode = setup?.mode ?? 'off';
    command = setup?.command ?? '';
    setupError = null;
    setupOpen = true;
  }

  async function run() {
    if (running) return;
    running = true;
    const r = await runTicketCheck(itemId);
    running = false;
    if (r.ok) {
      runError = null;
      onChanged?.();
    } else runError = errorText(r.error);
  }

  async function accept() {
    if (accepting) return;
    accepting = true;
    const r = await acceptCheck(itemId, acceptNote);
    accepting = false;
    if (r.ok) {
      acceptError = null;
      acceptOpen = false;
      acceptNote = '';
      onChanged?.();
    } else acceptError = errorText(r.error);
  }

  async function save() {
    if (!setup || saving) return;
    saving = true;
    const r = await setTicketCheck(setup.project_id, mode, command);
    saving = false;
    if (r.ok) {
      setupError = null;
      setupOpen = false;
      onChanged?.();
    } else setupError = errorText(r.error);
  }

  // A running check finishes on the hub: read the card again until it does.
  $effect(() => {
    if (check.state !== 'running') return;
    const timer = setInterval(() => onChanged?.(), POLL_MS);
    return () => clearInterval(timer);
  });
</script>

<section class="card {head.tone}" data-testid="ticket-check" data-state={check.state} aria-label="AI check">
  <header>
    <StatusChip state={head.state} label={head.label} testid="ticket-check-headline" />
    <span class="title">AI check</span>
    <Badge label="AI" tone="muted" title="A headless Claude ran the tests and read the change against the ticket" />
    {#if check.at}<span class="when">{timeAgo(check.at)}</span>{/if}
  </header>

  {#if check.state === 'ready'}
    <p class="summary muted">The work is finished and has not been tested yet.</p>
  {:else if check.state === 'running'}
    <p class="summary muted">The tester is running the tests and reading the change.</p>
  {/if}
  {#if check.summary}<p class="summary" data-testid="ticket-check-summary">{check.summary}</p>{/if}
  {#if check.error}<p class="err" data-testid="ticket-check-error">{check.error}</p>{/if}
  {#if check.note}<p class="summary muted" data-testid="ticket-check-note">Accepted: {check.note}</p>{/if}

  {#if blockers.length > 0}
    <div class="group crit">
      <h6>Must change</h6>
      <ul data-testid="ticket-check-blockers">
        {#each blockers as b, i (i)}<li>{b}</li>{/each}
      </ul>
    </div>
  {/if}
  {#if testsRun.length > 0}
    <details>
      <summary>Ran {testsRun.length} {testsRun.length === 1 ? 'command' : 'commands'}</summary>
      <ul class="plain" data-testid="ticket-check-tests">
        {#each testsRun as t, i (i)}<li class="mono">{t}</li>{/each}
      </ul>
    </details>
  {/if}

  <div class="actions">
    {#if check.can_run}
      <button type="button" class="ghost" disabled={running} onclick={() => void run()} data-testid="ticket-check-run">
        {running ? 'Starting…' : runLabel(check)}
      </button>
    {:else if check.why_not && check.state !== 'running'}
      <span class="muted small" data-testid="ticket-check-why-not">{check.why_not}</span>
    {/if}
    {#if canAccept(check) && !acceptOpen}
      <button type="button" class="ghost" onclick={() => (acceptOpen = true)} data-testid="ticket-check-accept">Accept anyway</button>
    {/if}
    {#if setup && !setupOpen}
      <button
        type="button"
        class="ghost push"
        title="Whether {setup.project} is tested when its work is finished"
        onclick={openSetup}
        data-testid="ticket-check-setup"
      >
        {setup.project}: {CHECK_MODE_LABEL[setup.mode]}
      </button>
    {/if}
  </div>
  {#if runError}<p class="err">{runError}</p>{/if}

  {#if acceptOpen}
    <div class="row" data-testid="ticket-check-accept-form">
      <input class="grow" placeholder="Why it is fine (optional)" bind:value={acceptNote} maxlength="500" aria-label="Why it is fine" />
      <button type="button" class="ghost" disabled={accepting} onclick={() => void accept()}>{accepting ? 'Accepting…' : 'Accept'}</button>
      <button type="button" class="ghost" onclick={() => (acceptOpen = false)}>Cancel</button>
    </div>
    {#if acceptError}<p class="err">{acceptError}</p>{/if}
  {/if}

  {#if setupOpen && setup}
    <fieldset class="setup" data-testid="ticket-check-setup-form">
      <legend>Test {setup.project}'s finished work</legend>
      <div class="modes" role="radiogroup" aria-label="When to test">
        {#each CHECK_MODES as m (m)}
          <label><input type="radio" name="check-mode-{setup.project_id}" value={m} bind:group={mode} /> {CHECK_MODE_LABEL[m]}</label>
        {/each}
      </div>
      <label class="row">
        <span class="muted small">Test command</span>
        <input class="grow mono" placeholder="e.g. cargo test" bind:value={command} maxlength="200" disabled={mode === 'off'} />
      </label>
      <p class="muted small">
        Without one, the tester runs the ticket's <code>test:</code> lines. Each check is a headless Claude run and counts toward usage.
      </p>
      <div class="row">
        <button type="button" class="ghost" disabled={saving} onclick={() => void save()} data-testid="ticket-check-save">{saving ? 'Saving…' : 'Save'}</button>
        <button type="button" class="ghost" onclick={() => (setupOpen = false)}>Cancel</button>
      </div>
      {#if setupError}<p class="err">{setupError}</p>{/if}
    </fieldset>
  {/if}
</section>

<style>
  .card {
    --tone: var(--fg-muted);
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--tone);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--tone) 6%, var(--bg-pane));
  }
  .success { --tone: var(--usage-ok); }
  .warning { --tone: var(--usage-warn); }
  .danger { --tone: var(--usage-crit); }
  header,
  .actions,
  .modes {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    align-items: center;
  }
  .title { font-weight: 600; font-size: 0.92em; }
  .summary { margin: 0; font-size: 0.92em; }
  .when,
  .push { margin-left: auto; }
  .when {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  h6 {
    margin: 0 0 0.2rem;
    font-size: var(--text-2xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .group.crit h6 { color: var(--usage-crit); }
  ul {
    margin: 0;
    padding-left: 1.2em;
    font-size: 0.88em;
  }
  ul.plain {
    list-style: none;
    padding-left: 0;
  }
  li { margin: 0.1em 0; }
  .row {
    display: flex;
    gap: 0.45rem;
    align-items: center;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .mono { font-family: var(--mono); font-size: 0.8em; word-break: break-word; }
  .muted { color: var(--fg-muted); }
  .small { font-size: var(--text-2xs); }
  details > summary {
    cursor: pointer;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  details > ul { margin-top: 0.2rem; }
  input:not([type='radio']) {
    background: var(--bg);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-size: 0.88em;
    padding: 0.1rem 0.4rem;
  }
  .ghost {
    background: none;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--control-fg-quiet);
    font-size: var(--text-2xs);
    padding: 0 0.4rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .ghost:hover:not(:disabled) { color: var(--fg); }
  .ghost:disabled { opacity: 0.55; cursor: default; }
  .setup {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin: 0;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  legend { font-size: var(--text-2xs); color: var(--fg-muted); padding: 0 0.2rem; }
  .modes label { font-size: 0.88em; }
  .setup p { margin: 0; }
  .err {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--usage-crit);
  }
</style>
