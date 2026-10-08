<script lang="ts">
  // The add-host wizard (Orbit Fleet redesign step 4.9, board Wizard). Five
  // steps on a left rail; the "Check the host" step runs its live checks one
  // at a time so each row lands as its answer comes back. Every step change
  // is saved on the backend, so closing the wizard — or the app — keeps what
  // was entered, and the next "+ Add host" offers to continue it. Nothing is
  // installed: the checks only read. The Layout: Classic picker
  // (AddHostPicker) stays as it was.
  import { onMount } from 'svelte';
  import { discoverHosts, addHost, type SshHost } from './hosts';
  import { probeSshAliasAbortable } from './accounts';
  import {
    CHECK_GLYPH,
    CHECK_KEYS,
    CLAUDE_INSTALL_HINT,
    LAST_STEP,
    RESUME_NOTE,
    STEPS,
    agentLines,
    canAdvance,
    checkRows,
    discardHostSetup,
    listHostSetups,
    nextLabel,
    resumeLine,
    runHostSetupCheck,
    saveHostSetup,
    stepHeading,
    type CheckKey,
    type HostSetup,
    type SetupCheck,
    type WizardAnswers,
  } from './add_host_wizard';
  import Modal from './Modal.svelte';

  let {
    onClose,
    onNewSession,
  }: {
    onClose: () => void;
    /** "New session here" once the host is added. */
    onNewSession?: (alias: string) => void;
  } = $props();

  let step = $state(1);
  let sshAlias = $state('');
  let alias = $state('');
  /** Whether the person edited the fleet name; until then it follows the SSH alias. */
  let aliasEdited = $state(false);
  let answers = $state<WizardAnswers>({});
  let checks = $state<SetupCheck[]>([]);
  let running = $state<CheckKey | null>(null);
  let drafts = $state<HostSetup[]>([]);
  let discovered = $state<SshHost[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let accountRead = $state<'idle' | 'reading' | 'done'>('idle');
  let adding = $state(false);
  let added = $state(false);
  /** Bumped by every new check pass, so a stale pass stops writing rows. */
  let pass = 0;

  const rows = $derived(checkRows(checks, running));
  const agents = $derived(agentLines(checks));
  const nextOpen = $derived(canAdvance(step, sshAlias, alias, checks, running !== null));

  onMount(async () => {
    const [d, h] = await Promise.all([listHostSetups(), discoverHosts()]);
    loading = false;
    if (d.ok) drafts = d.value ?? [];
    if (h.ok) discovered = h.value ?? [];
    else error = h.error.message;
  });

  function pickAlias(a: string) {
    sshAlias = a;
    if (!aliasEdited) alias = a;
  }

  async function save() {
    if (!sshAlias.trim()) return;
    const r = await saveHostSetup({ ssh_alias: sshAlias.trim(), alias: alias.trim(), step, answers });
    if (!r.ok) error = r.error.message;
  }

  function resume(d: HostSetup) {
    sshAlias = d.ssh_alias;
    alias = d.alias;
    aliasEdited = d.alias !== d.ssh_alias;
    answers = d.answers ?? {};
    checks = d.checks;
    step = Math.min(Math.max(d.step, 1), LAST_STEP);
    if (step === 2 && !checks.length) void runChecks();
    if (step === 4 && answers.account === undefined) void readAccount();
  }

  async function discard(d: HostSetup) {
    const r = await discardHostSetup(d.ssh_alias);
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    drafts = drafts.filter((x) => x.ssh_alias !== d.ssh_alias);
  }

  /** One pass: every check in order, each row landing as it answers. */
  async function runChecks() {
    const mine = ++pass;
    checks = [];
    error = null;
    for (const key of CHECK_KEYS) {
      if (mine !== pass) return;
      running = key;
      const r = await runHostSetupCheck(sshAlias.trim(), key);
      if (mine !== pass) return;
      const answer: SetupCheck = r.ok
        ? r.value
        : { key, state: 'fail', label: key, detail: r.error.message };
      checks = [...checks.filter((c) => c.key !== key), answer];
      // No SSH, no point asking the rest: each would time out the same way.
      if (key === 'ssh' && answer.state !== 'ok') break;
    }
    if (mine === pass) running = null;
  }

  async function readAccount() {
    accountRead = 'reading';
    const r = await probeSshAliasAbortable(sshAlias.trim());
    if (r.ok) {
      const a = r.value.account;
      answers = { ...answers, account: a ? (a.email ?? a.display_name ?? a.uuid) : null };
      await save();
    } else {
      error = r.error.message;
    }
    accountRead = 'done';
  }

  async function next() {
    if (!nextOpen) return;
    error = null;
    if (step === LAST_STEP) return finish();
    step += 1;
    await save();
    if (step === 2 && !checks.length) void runChecks();
    if (step === 4 && answers.account === undefined) void readAccount();
  }

  async function back() {
    if (step <= 1) return;
    pass++;
    running = null;
    step -= 1;
    await save();
  }

  async function finish() {
    adding = true;
    error = null;
    const r = await addHost(alias.trim(), sshAlias.trim());
    adding = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    await discardHostSetup(sshAlias.trim());
    added = true;
  }

  function close() {
    pass++;
    onClose();
  }
</script>

<Modal label="Add a host" onclose={close} width="760px">
  <div class="wizard" data-testid="add-host-wizard">
    <nav class="rail" aria-label="Steps">
      <h3>Add a host</h3>
      <ol>
        {#each STEPS as name, i (name)}
          {@const n = i + 1}
          <li
            class:done={n < step || added}
            class:current={n === step && !added}
            aria-current={n === step ? 'step' : undefined}
            data-testid="wizard-step"
          >
            <span class="num" aria-hidden="true">{n < step || added ? '✓' : n}</span>
            {name}
          </li>
        {/each}
      </ol>
      <p class="note">{RESUME_NOTE}</p>
    </nav>

    <section class="body">
      <p class="count">Step {step} of {LAST_STEP}</p>
      <h2>{stepHeading(step, alias || sshAlias || 'the host')}</h2>

      {#if step === 1}
        {#if drafts.length}
          <div class="drafts" data-testid="wizard-drafts">
            <p class="sub">Continue where you left off</p>
            {#each drafts as d (d.ssh_alias)}
              <div class="draft">
                <button type="button" class="link" data-testid="wizard-resume" onclick={() => resume(d)}
                  >{resumeLine(d)}</button
                >
                <button type="button" class="quiet" data-testid="wizard-discard" onclick={() => discard(d)}>Discard</button>
              </div>
            {/each}
          </div>
        {/if}
        <p class="sub">Pick a host from ~/.ssh/config, or type its SSH alias.</p>
        {#if loading}
          <p class="muted">Scanning ~/.ssh/config…</p>
        {:else if discovered.length}
          <ul class="hosts" role="listbox" aria-label="SSH hosts">
            {#each discovered as h (h.alias)}
              <li>
                <button
                  type="button"
                  role="option"
                  aria-selected={sshAlias === h.alias}
                  class="host"
                  class:picked={sshAlias === h.alias}
                  data-testid="wizard-host"
                  onclick={() => pickAlias(h.alias)}
                >
                  <span class="mono">{h.alias}</span>
                  {#if h.hostname}<span class="muted">{h.user ? `${h.user}@` : ''}{h.hostname}{h.port ? `:${h.port}` : ''}</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        <label class="field">
          SSH alias
          <input
            data-testid="wizard-ssh-alias"
            value={sshAlias}
            oninput={(e) => pickAlias(e.currentTarget.value)}
            placeholder="mercury"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <label class="field">
          Name in fleet
          <input
            data-testid="wizard-alias"
            value={alias}
            oninput={(e) => {
              alias = e.currentTarget.value;
              aliasEdited = true;
            }}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      {:else if step === 2}
        <p class="sub">Fleet connects over SSH and checks what sessions need. Nothing is installed without asking.</p>
        <ul class="checks" data-testid="wizard-checks">
          {#each rows as c (c.key)}
            <li class="check {c.state}" data-testid="wizard-check" data-key={c.key} data-state={c.state}>
              <span class="glyph" aria-hidden="true">{CHECK_GLYPH[c.state]}</span>
              <span class="label">{c.label}</span>
              <span class="detail">{c.detail || (c.state === 'pending' ? 'next' : '')}</span>
            </li>
          {/each}
        </ul>
        <button type="button" class="quiet" data-testid="wizard-recheck" disabled={running !== null} onclick={runChecks}
          >Check again</button
        >
      {:else if step === 3}
        <p class="sub">The agents fleet can start on {alias}. Fleet runs Claude Code today; the others are shown as they arrive.</p>
        <ul class="checks" data-testid="wizard-agents">
          {#each agents as a (a.bin)}
            <li class="check {a.found ? 'ok' : a.required ? 'fail' : 'na'}" data-testid="wizard-agent" data-bin={a.bin}>
              <span class="glyph" aria-hidden="true">{a.found ? '✓' : a.required ? '✗' : '–'}</span>
              <span class="label">{a.name}</span>
              <span class="detail">{a.found ? 'on PATH' : a.required ? 'not found' : 'not installed'}</span>
            </li>
          {/each}
        </ul>
        {#if !agents[0].found}
          <p class="hint" data-testid="wizard-claude-hint">
            Install Claude Code on {alias} to start sessions there: <code>{CLAUDE_INSTALL_HINT}</code>. You can add the host now and
            install it later.
          </p>
        {/if}
      {:else if step === 4}
        <p class="sub">Sessions on {alias} use the Claude account signed in there.</p>
        {#if accountRead === 'reading'}
          <p class="muted" data-testid="wizard-account-reading">Reading the account…</p>
        {:else if answers.account}
          <p data-testid="wizard-account">Signed in as <strong>{answers.account}</strong>.</p>
        {:else if answers.account === null}
          <p data-testid="wizard-account-none">
            No Claude account is signed in on {alias}. After adding it, run <code>claude</code> there and sign in with
            <code>/login</code>; fleet picks the account up on its next probe.
          </p>
        {/if}
      {:else}
        {#if added}
          <p data-testid="wizard-added"><strong>{alias}</strong> is in fleet.</p>
        {:else}
          <dl class="summary" data-testid="wizard-summary">
            <dt>Name</dt><dd class="mono">{alias}</dd>
            <dt>SSH alias</dt><dd class="mono">{sshAlias}</dd>
            <dt>Checks</dt><dd>{checks.filter((c) => c.state === 'ok').length} of {CHECK_KEYS.length} ok</dd>
            <dt>Account</dt><dd>{answers.account ?? 'none yet'}</dd>
          </dl>
        {/if}
      {/if}

      {#if error}<p class="err" role="alert">{error}</p>{/if}

      <div class="actions">
        {#if added}
          {#if onNewSession}
            <button
              type="button"
              data-testid="wizard-new-session"
              onclick={() => {
                onNewSession?.(alias.trim());
                onClose();
              }}>New session here</button
            >
          {/if}
          <button type="button" class="primary" data-testid="wizard-close" onclick={close}>Close</button>
        {:else}
          <button type="button" data-testid="wizard-cancel" onclick={close}>Cancel</button>
          {#if step > 1}
            <button type="button" data-testid="wizard-back" onclick={back}>‹ Back</button>
          {/if}
          <button type="button" class="primary" data-testid="wizard-next" disabled={!nextOpen || adding} onclick={next}
            >{adding ? 'Adding…' : nextLabel(step, alias.trim() || 'host')}</button
          >
        {/if}
      </div>
    </section>
  </div>
</Modal>

<style>
  .wizard { display: grid; grid-template-columns: 11rem 1fr; gap: 1.25rem; min-height: 22rem; }
  .rail { border-right: 1px solid var(--border); padding-right: 1rem; display: flex; flex-direction: column; gap: 0.6rem; }
  .rail h3 { margin: 0; font-size: 0.95rem; }
  .rail ol { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.35rem; }
  .rail li { display: flex; align-items: center; gap: 0.5rem; font-size: 0.85rem; color: var(--fg-muted); }
  .rail li.current { color: var(--fg); font-weight: 600; }
  .rail li.done { color: var(--fg); }
  .num {
    width: 1.3rem; height: 1.3rem; border-radius: 50%; border: 1px solid var(--border);
    display: inline-flex; align-items: center; justify-content: center; font-size: 0.7rem;
  }
  .current .num { border-color: var(--accent); }
  .done .num { border-color: var(--ok, var(--accent)); color: var(--ok, var(--accent)); }
  .note { margin-top: auto; font-size: 0.75rem; color: var(--fg-muted); }
  .body { display: flex; flex-direction: column; gap: 0.6rem; min-width: 0; }
  .count { margin: 0; font-size: 0.75rem; color: var(--fg-muted); }
  h2 { margin: 0; font-size: 1.1rem; }
  .sub { margin: 0; font-size: 0.85rem; color: var(--fg-muted); }
  .muted { color: var(--fg-muted); font-size: 0.8rem; }
  .mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .hosts { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.2rem; max-height: 10rem; overflow: auto; }
  .host {
    width: 100%; text-align: left; display: flex; gap: 0.6rem; align-items: baseline;
    background: transparent; border: 1px solid var(--border); border-radius: 5px;
    padding: 0.35rem 0.6rem; color: var(--fg); cursor: pointer;
  }
  .host.picked, .host:hover { border-color: var(--accent); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; font-size: 0.75rem; color: var(--fg-muted); }
  .field input { font: inherit; font-size: 0.85rem; padding: 0.3rem 0.5rem; border: 1px solid var(--border); border-radius: 4px; background: var(--bg); color: var(--fg); }
  .drafts { display: flex; flex-direction: column; gap: 0.25rem; padding-bottom: 0.4rem; border-bottom: 1px solid var(--border); }
  .draft { display: flex; justify-content: space-between; gap: 0.5rem; align-items: center; }
  .checks { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .check { display: grid; grid-template-columns: 1.2rem 1fr auto; gap: 0.5rem; align-items: baseline; font-size: 0.85rem; }
  .check .detail { color: var(--fg-muted); font-size: 0.75rem; }
  .check.ok .glyph { color: var(--ok, var(--accent)); }
  .check.warn .glyph, .check.warn .detail { color: var(--usage-warn); }
  .check.fail .glyph, .check.fail .detail { color: var(--usage-crit); }
  .check.pending, .check.na { color: var(--fg-muted); }
  .hint { font-size: 0.8rem; margin: 0; }
  .summary { display: grid; grid-template-columns: max-content 1fr; gap: 0.3rem 1rem; margin: 0; font-size: 0.85rem; }
  .summary dt { color: var(--fg-muted); }
  .summary dd { margin: 0; }
  .err { color: var(--danger); font-size: 0.8rem; margin: 0; }
  .actions { margin-top: auto; display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button, .quiet, .link {
    font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border);
    background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer;
  }
  .link { border: none; padding: 0; text-align: left; color: var(--accent); }
  .quiet { align-self: flex-start; font-size: 0.75rem; padding: 0.2rem 0.6rem; }
  .actions button.primary { border-color: var(--accent); }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
