<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import Modal from './Modal.svelte';
  import { pushError } from './toasts';
  import type { SessionRow } from './sessions';
  import { FILES_SHOWN, checkWork, cleanUp, cleanUpBlocked, workLine, type WorkCheck } from './kill_check';
  import { sessionBlocked } from './share';

  // Kill or Clean up one session or a selection (redesign step 1.7). It
  // reads every worktree first and lists what a Kill would leave behind;
  // Clean up commits and pushes through the agent, then removes. Kill itself
  // stays the caller's (it owns the side effects: forgetting the layout,
  // dropping the selection), so this dialog only decides and reports.
  let {
    targets,
    mode = 'kill',
    notes,
    onkill,
    oncleaned,
    oncancel,
    confirmTestId = 'confirm-kill',
  }: {
    targets: SessionRow[];
    /** Which press opened it: Kill or Clean up. Both buttons are offered. */
    mode?: 'kill' | 'cleanup';
    /** The caller's lines about selected rows left out (not yours to kill). */
    notes?: Snippet;
    onkill: () => void;
    /** Clean up was pressed: `removed` are gone already, `asked` finish
     *  through their agent (Safe remove) and stay listed until then. */
    oncleaned: (removed: SessionRow[], asked: SessionRow[]) => void;
    oncancel: () => void;
    confirmTestId?: string;
  } = $props();

  let checks = $state<Record<number, WorkCheck>>({});
  let busy = $state(false);

  onMount(() => {
    for (const t of targets) {
      const refused = $sessionBlocked(t, 'inspect_safe_kill');
      if (refused !== null) {
        checks[t.id] = { state: 'unknown', why: refused };
        continue;
      }
      checks[t.id] = { state: 'checking' };
      void checkWork(t).then((c) => (checks[t.id] = c));
    }
  });

  const checking = $derived(targets.some((t) => (checks[t.id]?.state ?? 'checking') === 'checking'));
  const dirty = $derived(targets.filter((t) => checks[t.id]?.state === 'dirty'));
  const unknown = $derived(targets.filter((t) => checks[t.id]?.state === 'unknown'));
  const cleanable = $derived(targets.filter((t) => cleanUpBlocked(t) === null));
  const n = $derived(targets.length);
  const many = $derived(n !== 1);
  const nameOf = (t: SessionRow) => t.friendly_name || t.tmux_name;

  const title = $derived(
    mode === 'cleanup'
      ? `Clean up ${many ? `${n} sessions` : 'session'}?`
      : `Kill ${many ? `${n} sessions` : 'session'}?`,
  );

  async function doCleanUp() {
    if (busy || cleanable.length === 0) return;
    busy = true;
    const removed: SessionRow[] = [];
    const asked: SessionRow[] = [];
    await Promise.all(
      cleanable.map(async (t) => {
        if ($sessionBlocked(t, 'safe_kill_session') !== null) return;
        const r = await cleanUp(t, checks[t.id] ?? { state: 'unknown', why: '' });
        if (r.ok) (r.value === 'removed' ? removed : asked).push(t);
        else pushError(r.error, `Clean up ${nameOf(t)} failed`);
      }),
    );
    busy = false;
    oncleaned(removed, asked);
  }
</script>

<Modal {title} onclose={oncancel} width="460px" testid="kill-dialog">
  <div class="body">
    {#if n === 0}
      <!-- the caller's notes say why -->
    {:else if !many}
      <p>
        Kill ends <code>{targets[0].tmux_name}</code> on <code>{targets[0].host_alias}</code> and loses any running agent
        state. Clean up first commits and pushes the work, then removes the worktree.
      </p>
    {:else}
      <p>
        Kill ends {n} sessions and loses any running agent state. Clean up first commits and pushes each one's work, then
        removes its worktree.
      </p>
    {/if}

    {#if checking}
      <p class="muted" data-testid="kill-checking">Checking for work not yet saved…</p>
    {:else if dirty.length > 0}
      <ul class="dirty" data-testid="kill-dirty">
        {#each dirty as t (t.id)}
          {@const c = checks[t.id]}
          {#if c?.state === 'dirty'}
            <li data-testid="kill-dirty-row">
              <span class="who">{nameOf(t)} on {t.host_alias}</span>: {workLine(c)}
              {#if c.files.length > 0}
                <ul class="files">
                  {#each c.files.slice(0, FILES_SHOWN) as f (f.path)}<li><code>{f.status}</code> {f.path}</li>{/each}
                  {#if c.files.length > FILES_SHOWN}<li class="muted">+{c.files.length - FILES_SHOWN} more</li>{/if}
                </ul>
              {/if}
            </li>
          {/if}
        {/each}
      </ul>
    {:else if unknown.length === 0 && n > 0}
      <p class="muted" data-testid="kill-clean">Nothing uncommitted or unpushed.</p>
    {/if}
    {#if !checking && unknown.length > 0}
      <p class="muted" data-testid="kill-unknown">
        Couldn't check {unknown.length === 1 ? nameOf(unknown[0]) : `${unknown.length} sessions`}{#if unknown.length === 1 && checks[unknown[0].id]?.state === 'unknown'}: {(checks[unknown[0].id] as { why: string }).why}{/if}.
      </p>
    {/if}
    {#if notes}<p class="muted">{@render notes()}</p>{/if}
  </div>
  <div class="actions">
    <button onclick={oncancel} disabled={busy} data-autofocus data-testid="confirm-cancel">Cancel</button>
    <button
      class:primary={mode === 'cleanup' || dirty.length > 0}
      onclick={() => void doCleanUp()}
      disabled={busy || checking || cleanable.length === 0}
      title={cleanable.length === 0 && n > 0 ? (cleanUpBlocked(targets[0]) ?? '') : ''}
      data-testid="kill-cleanup">Clean up{many && cleanable.length > 0 ? ` (${cleanable.length})` : ''}</button
    >
    <button class="danger" onclick={onkill} disabled={busy || n === 0} data-testid={confirmTestId}
      >{many ? 'Kill all' : 'Kill'}</button
    >
  </div>
</Modal>

<style>
  .body { font-size: 0.85rem; line-height: 1.4; }
  .body p { margin: 0 0 var(--space-2); }
  .body :global(code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg-pane);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
  }
  .muted { color: var(--fg-muted); }
  .dirty { margin: 0 0 var(--space-2); padding-left: var(--space-4); }
  .dirty .who { font-weight: 600; }
  .files { margin: var(--space-1) 0 0; padding-left: var(--space-3); list-style: none; font-size: var(--text-xs); }
  .actions { display: flex; justify-content: flex-end; gap: var(--space-2); margin-top: var(--space-3); }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
  .actions button.danger { color: var(--danger); border-color: var(--danger); }
  .actions button.danger:hover:not(:disabled) { background: color-mix(in srgb, var(--danger) 12%, transparent); }
</style>
