<script lang="ts">
  // Add host on a desktop paired with a hub: the Add host form (the same
  // fleet.form/1 spec the chat opens, in its paired wording) over the HUB's
  // ~/.ssh/config, run by the hub's `add_host`, which probes the alias over
  // its own SSH and keeps the host only if it answers. The guided wizard
  // (AddHostWizard) checks and drafts on this machine, which would test the
  // wrong route; the phone adds hosts this way too. The hub asks for its
  // owner's trusted full device and says so (`fleet-hub client trust
  // <name>`) to any other.
  //
  // A host the hub cannot reach is added for fleet-agent (`transport:
  // "agent"`, no probe), and the sheet after says what to run: the token on
  // the hub, the install on the host (review w14-2). An SSH host gets New
  // session here and the Install fleet-agent action (review w14-10).
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import Modal from './Modal.svelte';
  import WizardDialog from './forms/WizardDialog.svelte';
  import AgentInstallAction from './AgentInstallAction.svelte';
  import {
    VIA_AGENT,
    addHostTarget,
    agentSetupCommands,
    hubAddHostWizard,
    runAddHost,
  } from './forms/add_host_wizard';
  import { discoverHosts } from './hosts';
  import { hubStatus } from './hub';
  import { copyText } from './clipboard';
  import { push } from './toasts';
  import type { FieldProblem, Values } from './forms/forms';
  import type { Wizard } from './forms/wizards';

  let {
    onClose,
    onNewSession,
  }: {
    onClose: () => void;
    /** "New session here" on the sheet after an SSH host is added. */
    onNewSession?: (alias: string) => void;
  } = $props();

  let wizard = $state<Wizard>(hubAddHostWizard([]));
  let busy = $state(false);
  let error = $state<string | null>(null);
  let problems = $state<FieldProblem[]>([]);
  /** The host just added: the sheet shows instead of the form. */
  let added = $state<{ alias: string; agent: boolean; summary: string } | null>(null);

  onMount(async () => {
    const r = await discoverHosts();
    // No list is not a dead end: "Another alias" still takes a typed one.
    if (r.ok) wizard = hubAddHostWizard(r.value ?? []);
  });

  async function run(v: Values) {
    busy = true;
    error = null;
    problems = [];
    const out = await runAddHost(v);
    busy = false;
    if (out.ok) {
      added = {
        alias: addHostTarget(v).alias,
        agent: v.via === VIA_AGENT,
        summary: out.summary ?? '',
      };
      return;
    }
    error = out.error ?? null;
    problems = out.problems ?? [];
  }

  const commands = $derived(added ? agentSetupCommands(added.alias, get(hubStatus).url) : null);

  async function copy(text: string) {
    if (await copyText(text)) push({ kind: 'success', message: 'Command copied.' });
  }
</script>

{#if added}
  <Modal label="Host added" onclose={onClose} width="480px" testid="hub-add-host-done">
    <div class="sheet">
      <h3>{added.alias} is in the fleet</h3>
      <p class="lead" data-testid="hub-add-host-summary">{added.summary}.</p>
      {#if added.agent && commands}
        <ol class="steps" data-testid="hub-add-host-agent-steps">
          <li>
            On the hub, print {added.alias}'s token (treat it like a password):
            <span class="cmd"
              ><code data-testid="hub-add-host-token-cmd">{commands.token}</code>
              <button type="button" class="small" onclick={() => void copy(commands.token)}>Copy</button></span
            >
            <span class="muted">Under Docker: <code>docker compose run --rm fleet-hub agent-token {added.alias}</code></span>
          </li>
          <li>
            On {added.alias}, with fleet-agent from the release (docs/hub.md, <em>Set it up</em>), install it and paste
            the token, then Ctrl-D:
            <span class="cmd"
              ><code data-testid="hub-add-host-install-cmd">{commands.install}</code>
              <button type="button" class="small" onclick={() => void copy(commands.install)}>Copy</button></span
            >
          </li>
        </ol>
      {:else}
        <p class="install">
          <AgentInstallAction alias={added.alias} testid="hub-add-host-agent-install" />
          <span class="muted">Optional: the hub keeps reaching it over SSH either way.</span>
        </p>
      {/if}
      <div class="row">
        <span class="grow"></span>
        {#if !added.agent && onNewSession}
          <button
            type="button"
            class="btn"
            data-testid="hub-add-host-new-session"
            onclick={() => {
              onNewSession?.(added!.alias);
              onClose();
            }}>New session here</button
          >
        {/if}
        <button type="button" class="btn primary" data-testid="hub-add-host-close" onclick={onClose}>Done</button>
      </div>
    </div>
  </Modal>
{:else}
  <WizardDialog {wizard} {busy} {error} {problems} errorTestid="hub-add-host-error" run={(v) => void run(v)} onclose={onClose} />
{/if}

<style>
  .sheet {
    padding: 1rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  h3 {
    margin: 0;
    font-size: var(--text-md);
  }
  .lead,
  .muted {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .steps {
    margin: 0;
    padding-left: 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    font-size: var(--text-xs);
  }
  .steps li {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .cmd {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  code {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-sunk);
    word-break: break-all;
  }
  .install {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .grow {
    flex: 1;
  }
</style>
