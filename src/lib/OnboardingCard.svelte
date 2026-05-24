<script lang="ts">
  import { hosts } from './hosts';
  import { projects, refreshProjects } from './projects';
  import { sessions } from './sessions';
  import { mcpStatus, mcpConfigure, provisionHosts } from './mcp';
  import {
    deriveSteps,
    allRequiredComplete,
    checkLocalPrereqs,
    tunnelStatus,
    onboardingDismissed,
    type LocalPrereqs,
    type TunnelStatusRow,
    type StepId,
  } from './onboarding';

  // Parent supplies actions that open existing dialogs.
  let { onaddhost, onnewsession }: { onaddhost: () => void; onnewsession: () => void } =
    $props();

  // Async snapshots not derivable from stores.
  let prereqs = $state<LocalPrereqs | null>(null);
  let tunnels = $state<TunnelStatusRow[]>([]);
  let mcpEnabled = $state(false);

  // In-flight UI per step.
  let busy = $state<StepId | null>(null);
  let errorText = $state<string | null>(null);

  // Refresh backend snapshots on mount and whenever hosts change.
  async function refreshSnapshots() {
    const [p, t, m] = await Promise.all([checkLocalPrereqs(), tunnelStatus(), mcpStatus()]);
    if (p.ok) prereqs = p.value;
    if (t.ok) tunnels = t.value;
    if (m.ok) mcpEnabled = m.value.enabled;
  }
  $effect(() => {
    // Re-read snapshots when the host list size changes (host added/provisioned).
    void $hosts.length;
    refreshSnapshots();
  });

  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));
  const workSessions = $derived($sessions.filter((s) => s.kind !== 'bg'));

  const steps = $derived(
    deriveSteps({
      prereqs,
      visibleHostCount: visibleHosts.length,
      provisionedHost: visibleHosts.some((h) => h.provisioned),
      firstHostAlias: visibleHosts[0]?.alias ?? null,
      tunnels,
      projectCount: $projects.length,
      mcpEnabled,
      workSessionCount: workSessions.length,
    }),
  );

  const doneCount = $derived(steps.filter((s) => !s.optional && s.status === 'done').length);
  const requiredCount = $derived(steps.filter((s) => !s.optional).length);
  const complete = $derived(allRequiredComplete(steps));

  async function runStep(id: StepId) {
    errorText = null;
    if (id === 'add-host') {
      onaddhost();
      return;
    }
    if (id === 'session') {
      onnewsession();
      return;
    }
    busy = id;
    try {
      if (id === 'prereqs') {
        await refreshSnapshots();
      } else if (id === 'provision') {
        const r = await provisionHosts();
        if (!r.ok) errorText = r.error.message;
        else {
          const failed = r.value.find((h) => h.status === 'failed');
          if (failed) errorText = `${failed.host}: ${failed.detail ?? 'provision failed'}`;
        }
        await refreshSnapshots();
      } else if (id === 'projects') {
        const r = await refreshProjects();
        if (!r.ok) errorText = r.error.message;
      } else if (id === 'mcp') {
        const r = await mcpConfigure({ enabled: true });
        if (!r.ok) errorText = r.error.message;
        else mcpEnabled = r.value.enabled;
        await refreshSnapshots();
      }
    } finally {
      busy = null;
    }
  }

  function dismiss() {
    onboardingDismissed.set(true);
  }
</script>

<div class="card" data-testid="onboarding-card">
  <div class="top">
    <b>Get started</b>
    <button class="x" onclick={dismiss} aria-label="Dismiss setup guide" title="Dismiss">✕</button>
  </div>

  {#if complete}
    <p class="done-msg">You're all set 🎉</p>
    <button class="dismiss-all" onclick={dismiss}>Dismiss</button>
  {:else}
    <div class="prog">{doneCount} of {requiredCount} done</div>
    <div class="pbar"><i aria-hidden="true" style="width:{requiredCount > 0 ? (doneCount / requiredCount) * 100 : 0}%"></i></div>

    {#each steps as step (step.id)}
      <button
        class="step"
        class:muted={step.status === 'pending'}
        onclick={() => runStep(step.id)}
        disabled={busy !== null}
      >
        <span class="ic {step.status}" aria-hidden="true">{step.status === 'done' ? '✓' : busy === step.id ? '◐' : ''}</span>
        <span class="body">
          <span class="label">
            {step.label}
            {#if step.optional}<span class="opt">optional</span>{/if}
          </span>
          {#if busy === step.id}
            <span class="sub">Working…</span>
          {:else if step.sublabel}
            <span class="sub">{step.sublabel}</span>
          {/if}
          {#if step.badge}
            <span class="badge {step.badge.tone}">{step.badge.text}</span>
          {/if}
        </span>
      </button>
    {/each}

    {#if errorText}
      <p class="err">{errorText}</p>
    {/if}
  {/if}
</div>

<style>
  .card {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 11px 12px;
    margin: 0 0 10px;
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 4px;
  }
  .top b {
    font-size: 0.72rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .x {
    background: none;
    border: none;
    color: var(--fg-muted, #777);
    cursor: pointer;
    font-size: 0.85rem;
  }
  .prog {
    font-size: 0.7rem;
    color: var(--fg-muted, #777);
    margin-bottom: 7px;
  }
  .pbar {
    height: 4px;
    background: var(--border);
    border-radius: 3px;
    overflow: hidden;
    margin-bottom: 9px;
  }
  .pbar > i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .step {
    display: flex;
    gap: 9px;
    align-items: flex-start;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-top: 1px solid var(--border);
    padding: 6px 0;
    cursor: pointer;
    font-size: 0.82rem;
    color: var(--fg);
  }
  .step:first-of-type {
    border-top: none;
  }
  .step:disabled {
    cursor: default;
    opacity: 0.8;
  }
  .step.muted .body {
    color: var(--fg-muted, #777);
  }
  .ic {
    width: 17px;
    height: 17px;
    flex: 0 0 17px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.7rem;
    margin-top: 1px;
  }
  .ic.done {
    background: var(--accent);
    color: #fff;
  }
  .ic.active {
    border: 1.5px solid var(--accent);
    color: var(--accent);
  }
  .ic.pending {
    border: 1.5px solid var(--border);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .opt {
    font-size: 0.6rem;
    color: var(--fg-muted, #777);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 0 5px;
    margin-left: 6px;
  }
  .sub {
    font-size: 0.68rem;
    color: var(--fg-muted, #777);
  }
  .badge {
    font-size: 0.62rem;
    padding: 1px 6px;
    border-radius: 10px;
    margin-top: 3px;
    align-self: flex-start;
  }
  .badge.up {
    background: #e7f6ec;
    color: #1a7f37;
  }
  .badge.warn {
    background: #fdf1e3;
    color: #b06a00;
  }
  .err {
    font-size: 0.7rem;
    color: #c0392b;
    margin: 6px 0 0;
  }
  .done-msg {
    font-size: 0.9rem;
    margin: 4px 0 8px;
  }
  .dismiss-all {
    font-size: 0.8rem;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 5px 10px;
    cursor: pointer;
  }
</style>
