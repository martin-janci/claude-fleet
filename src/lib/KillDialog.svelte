<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
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
  const cleanable = $derived(targets.filter((t) => cleanUpBlocked(t) === null));
  const n = $derived(targets.length);
  const many = $derived(n !== 1);
  const nameOf = (t: SessionRow) => t.friendly_name || t.tmux_name;

  const title = $derived.by(() => {
    const what = many ? `${n} sessions` : n === 1 ? `“${nameOf(targets[0])}”` : 'sessions';
    return mode === 'cleanup' ? `Clean up ${what}?` : `Force kill ${what}?`;
  });
  /** The one plain sentence (Dialogs board): what is lost, in words. */
  const lead = $derived.by(() => {
    if (n === 0) return 'None of the selected sessions can be killed from here.';
    if (checking) return 'Checking each worktree for work not yet saved.';
    if (dirty.length === 1) {
      const c = checks[dirty[0].id];
      return `${c?.state === 'dirty' ? workLine(c) : 'Unsaved work'} in ${nameOf(dirty[0])} will be lost. This can't be undone.`;
    }
    if (dirty.length > 1) return `Unsaved work in ${dirty.length} sessions will be lost. This can't be undone.`;
    return `This ends ${many ? `${n} sessions` : 'the session'} and any agent running in it. This can't be undone.`;
  });
  const MARK: Record<WorkCheck['state'], string> = { clean: '✓', dirty: '!', unknown: '?', checking: '…' };
  /** Name the host only when the targets are on more than one. */
  const hostsDiffer = $derived(new Set(targets.map((t) => t.host_alias)).size > 1);

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

<!-- Dialogs board, Force kill: every target on its own line, clean (✓) or
     not (!), then Clean up beside the red verb. -->
<DialogSheet
  {title}
  {lead}
  verb={many ? `Force kill ${n}` : 'Force kill'}
  danger
  onconfirm={onkill}
  onclose={oncancel}
  canConfirm={n > 0}
  {busy}
  width="460px"
  testid="kill-dialog"
  confirmTestid={confirmTestId}
  cancelTestid="confirm-cancel"
>
  {#if n > 0}
    <ul class="targets" data-testid="kill-targets">
      {#each targets as t (t.id)}
        {@const c = checks[t.id] ?? { state: 'checking' }}
        <li
          class="target"
          data-state={c.state}
          data-testid={c.state === 'dirty' ? 'kill-dirty-row' : c.state === 'clean' ? 'kill-clean-row' : 'kill-target-row'}
        >
          <span class="mark" aria-hidden="true">{MARK[c.state]}</span>
          <span class="what">
            <span class="who">{nameOf(t)}{#if hostsDiffer}<span class="host"> · {t.host_alias}</span>{/if}</span>
            {#if c.state === 'dirty'}
              <span class="line warn">{workLine(c)}</span>
              {#if c.files.length > 0}
                <ul class="files">
                  {#each c.files.slice(0, FILES_SHOWN) as f (f.path)}<li><code>{f.status}</code> {f.path}</li>{/each}
                  {#if c.files.length > FILES_SHOWN}<li class="muted">+{c.files.length - FILES_SHOWN} more</li>{/if}
                </ul>
              {/if}
            {:else if c.state === 'clean'}
              <span class="line muted" data-testid="kill-clean">clean · pushed</span>
            {:else if c.state === 'unknown'}
              <span class="line muted" data-testid="kill-unknown">Couldn't check: {c.why}</span>
            {:else}
              <span class="line muted">checking…</span>
            {/if}
          </span>
        </li>
      {/each}
    </ul>
    {#if checking}
      <p class="field-note" data-testid="kill-checking">Checking for work not yet saved…</p>
    {/if}
    <p class="field-note">Clean up keeps the work: Claude commits and pushes, then removes the worktree.</p>
  {/if}
  {#if notes}<p class="field-note">{@render notes()}</p>{/if}
  <button
    type="button"
    class="btn cleanup {mode === 'cleanup' || dirty.length > 0 ? 'btn--primary' : 'btn--quiet is-bounded'}"
    onclick={() => void doCleanUp()}
    disabled={busy || checking || cleanable.length === 0}
    title={cleanable.length === 0 && n > 0 ? (cleanUpBlocked(targets[0]) ?? '') : ''}
    data-testid="kill-cleanup"
    >Clean up (commits and pushes first){many && cleanable.length > 0 ? ` · ${cleanable.length}` : ''}</button
  >
</DialogSheet>

<style>
  .targets { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: var(--space-2); }
  .target { display: flex; gap: var(--space-2); align-items: flex-start; font-size: var(--text-sm); }
  .mark { width: 1rem; flex: none; text-align: center; font-weight: 600; color: var(--fg-muted); }
  .target[data-state='clean'] .mark { color: var(--status-done); }
  .target[data-state='dirty'] .mark { color: var(--status-failed); }
  .what { display: flex; flex-direction: column; min-width: 0; gap: 1px; }
  .who { font-weight: 500; overflow-wrap: anywhere; }
  .host { color: var(--fg-muted); font-weight: 400; }
  .line { font-size: var(--text-xs); }
  .warn { color: var(--status-waiting); }
  .muted { color: var(--fg-muted); }
  .files { margin: var(--space-1) 0 0; padding: 0; list-style: none; font-size: var(--text-xs); }
  .files :global(code) {
    font-family: var(--font-mono);
    background: var(--bg-pane);
    padding: 0 0.3rem;
    border-radius: var(--radius-xs);
  }
  .cleanup { width: 100%; }
</style>
