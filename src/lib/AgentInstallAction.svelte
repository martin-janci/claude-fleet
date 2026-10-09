<!--
  Orbit Fleet 4.9: the "Install <version>" action on a fleet-agent row
  (the add-host wizard's check, Host detail's health checklist) and the
  job's live progress: the Hex field (the Loader kit's job for a health
  checklist and re-provision, step 4.13) beside the step it is on, then
  done or why it failed. The job starts only on the click; a job already
  running for the host (started elsewhere, or before a restart of this
  view) is picked up and followed.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Button from './kit/Button.svelte';
  import Loader from './Loader.svelte';
  import { push, pushError } from './toasts';
  import {
    INSTALL_POLL_MS,
    agentInstalls,
    installAgent,
    installStepText,
    runningInstall,
    type AgentInstall,
  } from './agent_install';

  let {
    alias,
    version = null,
    blocked = null,
    ondone,
    testid = 'agent-install',
  }: {
    alias: string;
    /** The version the button names (the hub's); absent: "Install fleet-agent". */
    version?: string | null;
    /** Why this client cannot start it, or null. */
    blocked?: string | null;
    /** The host is on its agent now. */
    ondone?: () => void;
    testid?: string;
  } = $props();

  let job = $state<AgentInstall | null>(null);
  let starting = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let gone = false;

  function stop() {
    if (timer) clearTimeout(timer);
    timer = null;
  }

  async function follow() {
    stop();
    const r = await agentInstalls(alias);
    if (gone || !r.ok) return;
    const latest = (r.value ?? []).find((j) => job === null || j.id === job.id) ?? null;
    if (!latest) return;
    const was = job?.state;
    job = latest;
    if (latest.state === 'running') {
      timer = setTimeout(() => void follow(), INSTALL_POLL_MS);
    } else if (was === 'running' && latest.state === 'done') {
      push({ kind: 'success', message: `fleet-agent ${latest.version} runs on ${alias}` });
      ondone?.();
    }
  }

  onMount(() => {
    // Pick up a job already running for this host.
    void agentInstalls(alias).then((r) => {
      if (gone || !r.ok) return;
      const running = runningInstall(r.value);
      if (running) {
        job = running;
        timer = setTimeout(() => void follow(), INSTALL_POLL_MS);
      }
    });
  });
  onDestroy(() => {
    gone = true;
    stop();
  });

  async function start() {
    if (blocked !== null || starting) return;
    starting = true;
    const r = await installAgent(alias, version);
    starting = false;
    if (!r.ok) {
      pushError(r.error, `Installing fleet-agent on ${alias} failed`);
      return;
    }
    job = r.value;
    timer = setTimeout(() => void follow(), INSTALL_POLL_MS);
  }

  const label = $derived(version ? `Install ${version}` : 'Install fleet-agent');
</script>

{#if job?.state === 'running'}
  <span class="live" data-testid="{testid}-live">
    <Loader name="hex-field" size={48} testid="{testid}-loader" />
    <span class="step" data-testid="{testid}-step" aria-live="polite">{installStepText(job)}</span>
  </span>
{:else}
  <Button
    size="sm"
    testid={testid}
    busy={starting}
    busyLabel="Starting…"
    disabled={blocked !== null}
    title={blocked ?? `Install fleet-agent on ${alias} and move it onto the agent`}
    onclick={() => void start()}>{label}</Button
  >
  {#if job?.state === 'failed'}
    <span class="failed" role="alert" data-testid="{testid}-failed">Failed · {job.detail ?? 'no detail'}</span>
  {/if}
{/if}

<style>
  .live {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .step {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .failed {
    font-size: var(--text-2xs);
    color: var(--status-failed);
  }
</style>
