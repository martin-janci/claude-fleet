<script lang="ts">
  import Icon from './kit/Icon.svelte';
  import { tick } from 'svelte';
  import type { RepoStatus } from './assets';

  /** One catalog in the footer (spec, Footer; Rulings R24): HEAD, how far
   *  ahead of its upstream, uncommitted files. The personal catalog, when this
   *  window may write it, gets pull, commit and push in a popover (SB4:
   *  pushing is a person's step); any other shows its status, load problem
   *  included, and points to `catalog.auto_push`. */
  let {
    name,
    head,
    state: load = 'loaded',
    problem = null,
    path = null,
    remote = null,
    repo = null,
    writable = false,
    busy = false,
    onpull,
    oncommit,
    onpush,
  }: {
    name: string;
    head: string | null;
    state?: 'loaded' | 'problem' | 'not_loaded';
    problem?: string | null;
    /** Where the catalog lives: its checkout and, when set, its remote. */
    path?: string | null;
    remote?: string | null;
    repo?: RepoStatus | null;
    writable?: boolean;
    busy?: boolean;
    onpull?: () => void;
    oncommit?: () => void;
    onpush?: () => void;
  } = $props();

  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  let chip: HTMLButtonElement | undefined = $state();
  let dialog: HTMLElement | undefined = $state();
  const personal = $derived(name === 'personal');
  const title = $derived([path, remote, problem].filter(Boolean).join('\n'));
  const short = $derived((repo?.head ?? head ?? '').slice(0, 7) || '—');
  const ahead = $derived(repo?.ahead ?? 0);
  const dirty = $derived(repo?.dirty ?? 0);
  const text = $derived(
    `${name} @${short}${ahead ? ` ↑${ahead}` : ''}${dirty ? ` ±${dirty}` : ''}${load === 'problem' ? ' ⚠' : ''}`,
  );

  $effect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (root && !root.contains(e.target as Node)) open = false;
    };
    document.addEventListener('mousedown', away);
    return () => document.removeEventListener('mousedown', away);
  });

  // The focus moves into the popover when it opens (its first control, else
  // the dialog) and back to the chip when it closes by Esc or an action.
  async function toggle() {
    open = !open;
    if (!open) return;
    await tick();
    (dialog?.querySelector<HTMLElement>('button:not(:disabled)') ?? dialog)?.focus();
  }

  function close(refocus: boolean) {
    open = false;
    if (refocus) chip?.focus();
  }

  function act(fn?: () => void) {
    close(true);
    fn?.();
  }
</script>

<span class="cchip" bind:this={root} onfocusout={(e) => {
  const next = e.relatedTarget as Node | null;
  if (open && next && root && !root.contains(next)) open = false;
}}>
  <button
    type="button"
    class="btn btn--quiet is-bounded chip"
    aria-haspopup="dialog"
    aria-expanded={open}
    {title}
    bind:this={chip}
    onclick={toggle}
    data-testid={`catalog-chip-${name}`}
  ><span class="mono" data-testid={personal ? 'assets-head' : undefined}>{text}</span></button>
  {#if open}
    <div
      class="pop"
      role="dialog"
      tabindex="-1"
      aria-label={`Catalog ${name}`}
      bind:this={dialog}
      onkeydown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          close(true);
        }
      }}
    >
      {#if path || remote}
        <p class="where" data-testid={`catalog-where-${name}`}>
          {#if path}<span class="path">{path}</span>{/if}
          {#if remote}<span class="remote">{remote}</span>{/if}
        </p>
      {/if}
      {#if repo}
        <p class="status" data-testid={personal ? 'assets-repo-status' : `catalog-status-${name}`}>
          {repo.head.slice(0, 7)} · {repo.dirty} dirty
          {#if repo.has_upstream}· ↑{repo.ahead ?? 0} ↓{repo.behind ?? 0}{:else}· no upstream{/if}
        </p>
      {/if}
      {#if load === 'problem'}
        <p class="status"><Icon name="warning" size={12} /> Could not load: {problem ?? 'unknown error'}</p>
      {:else if load === 'not_loaded'}
        <p class="status muted">not loaded</p>
      {:else if !repo}
        <p class="status muted">@{short}</p>
      {/if}
      {#if writable && personal}
        <div class="acts">
          <button class="btn btn--quiet is-bounded" disabled={busy} onclick={() => act(onpull)} data-testid="assets-pull">Pull</button>
          {#if dirty > 0}
            <button class="btn btn--quiet is-bounded" disabled={busy} onclick={() => act(oncommit)} data-testid="assets-commit-pending">Commit pending</button>
          {/if}
          <button
            class="btn btn--quiet is-bounded"
            disabled={busy || !repo?.has_upstream}
            title={repo?.has_upstream ? '' : 'no upstream configured'}
            onclick={() => act(onpush)}
            data-testid="assets-push"
          >Push{ahead ? ` ↑${ahead}` : ''}</button>
        </div>
      {:else if !personal}
        <p class="muted">Cards commit here; with <code>catalog.auto_push</code> on, fleet pushes after each apply.</p>
      {/if}
    </div>
  {/if}
</span>

<style>
  .cchip { position: relative; display: inline-flex; }
  .chip { height: 18px; padding: 0 6px; }
  .mono { font-family: var(--mono); font-size: var(--text-2xs); color: var(--fg); }
  .pop {
    position: absolute; bottom: calc(100% + 6px); left: 0; z-index: 10; display: grid; gap: 8px; min-width: 240px;
    padding: 10px 12px; border: 1px solid var(--control-border); border-radius: var(--radius-md); background: var(--bg); font-size: var(--text-xs);
  }
  .status { margin: 0; font-family: var(--mono); font-size: var(--text-2xs); }
  .where { display: grid; gap: 2px; margin: 0; font-family: var(--mono); font-size: var(--text-2xs); color: var(--fg-muted); overflow-wrap: anywhere; user-select: text; }
  .acts { display: flex; gap: 6px; flex-wrap: wrap; }
  .muted { margin: 0; color: var(--fg-muted); }
</style>
