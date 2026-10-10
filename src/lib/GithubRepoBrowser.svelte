<script lang="ts">
  // The Add-project dialog's "From GitHub" source (redesign 6.11, the
  // AddProject board): the repositories `gh` can see on one host, for one
  // owner, filtered client-side. Rows are ticked, not picked: the dialog
  // adds every ticked repository. A repository already in the fleet says so
  // and cannot be ticked.
  import { untrack } from 'svelte';
  import { listGithubRepos, type GithubRepo } from './projects';
  import { timeAgo } from './session_status';

  let {
    host,
    owner = '',
    selected,
    ontoggle,
    inFleet,
    autofocus = false,
    disabled = false,
  }: {
    host: string;
    /** A GitHub user or organisation; empty lists the host login's own. */
    owner?: string;
    /** Ticked `owner/repo`s. */
    selected: readonly string[];
    ontoggle: (nameWithOwner: string) => void;
    /** Whether the fleet already has this repository. */
    inFleet: (nameWithOwner: string) => boolean;
    /** Move focus to the filter box once the list lands (the source was
     *  opened with a click / Enter, not arrowed past on the way). */
    autofocus?: boolean;
    disabled?: boolean;
  } = $props();

  type ListState =
    | { status: 'loading' }
    | { status: 'ready'; repos: GithubRepo[] }
    | { status: 'error'; message: string };
  let list = $state<ListState>({ status: 'loading' });
  let filter = $state('');
  let activeKey = $state<string | null>(null);
  /** Bumped by Retry to re-run the listing for the same host and owner. */
  let attempt = $state(0);
  let filterEl: HTMLInputElement | undefined = $state();
  let wantFocus = untrack(() => autofocus);

  // A slow reply for a previous host or owner must not land after a newer
  // request (same guard as NewSessionDialog's `scanSeq`).
  let seq = 0;
  $effect(() => {
    const h = host;
    const o = owner.trim();
    void attempt;
    const mine = ++seq;
    list = { status: 'loading' };
    void listGithubRepos(h, o || undefined).then((r) => {
      if (mine !== seq) return;
      // gh's own stderr (not installed, "run gh auth login"…) is shown
      // verbatim — never an empty list pretending there are no repos.
      if (!r.ok) {
        list = { status: 'error', message: r.error.message };
        return;
      }
      // A hub older than 6.11 ignores `owner` and answers the login's own
      // repositories: keep only the owner's, so the list never says
      // something it was not asked.
      const lower = o.toLowerCase();
      const repos = (r.value ?? []).filter((x) => !lower || x.name_with_owner.toLowerCase().startsWith(`${lower}/`));
      list = { status: 'ready', repos };
    });
    return () => {
      seq++;
    };
  });

  const rows = $derived.by((): GithubRepo[] => {
    if (list.status !== 'ready') return [];
    const q = filter.trim().toLowerCase();
    return list.repos.filter(
      (r) => !q || r.name_with_owner.toLowerCase().includes(q) || (r.description ?? '').toLowerCase().includes(q),
    );
  });

  /** "TypeScript · updated 2h ago · private": what the board's rows say. */
  function metaOf(r: GithubRepo): string {
    const updated = r.updated_at ? Date.parse(r.updated_at) : NaN;
    return [
      r.language,
      Number.isFinite(updated) ? `updated ${timeAgo(updated / 1000)}` : null,
      r.is_private ? 'private' : null,
    ]
      .filter(Boolean)
      .join(' · ');
  }

  // Once per request for it (not on every later host switch, which would
  // yank focus off the chip the user just pressed).
  $effect(() => {
    if (filterEl && wantFocus) {
      wantFocus = false;
      filterEl.focus();
    }
  });

  function retry() {
    // The Retry button disappears with the error; land the user in the list.
    wantFocus = true;
    attempt++;
  }

  // Keep the highlight on a visible row as the filter narrows the list.
  $effect(() => {
    if (!rows.some((r) => r.name_with_owner === activeKey)) activeKey = rows[0]?.name_with_owner ?? null;
  });

  function toggle(name: string) {
    if (disabled || inFleet(name)) return;
    ontoggle(name);
  }

  function onFilterKeydown(e: KeyboardEvent) {
    const i = rows.findIndex((r) => r.name_with_owner === activeKey);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (next) activeKey = next.name_with_owner;
    } else if (e.key === 'Enter') {
      // Enter ticks the highlighted repository rather than submitting the
      // dialog; the footer's verb adds what is ticked.
      e.preventDefault();
      e.stopPropagation();
      if (activeKey) toggle(activeKey);
    }
  }
</script>

{#if list.status === 'loading'}
  <p class="status" data-testid="gh-loading">Listing repositories on {host}…</p>
{:else if list.status === 'error'}
  <p class="err" role="alert" data-testid="gh-error">{list.message}</p>
  <button type="button" class="retry" data-testid="gh-retry" onclick={retry}>Retry</button>
{:else}
  <input
    id="gh-filter"
    type="text"
    data-testid="gh-filter"
    bind:this={filterEl}
    bind:value={filter}
    onkeydown={onFilterKeydown}
    placeholder="Filter repositories"
    aria-label="Filter repositories"
    aria-controls="gh-list"
  />
  {#if rows.length === 0}
    <p class="status" data-testid="gh-empty">
      {list.repos.length === 0
        ? owner.trim()
          ? `gh lists no repositories for ${owner.trim()} on ${host}.`
          : `gh lists no repositories on ${host}.`
        : 'Nothing matches.'}
    </p>
  {:else}
    <ul class="repos" id="gh-list" data-testid="gh-list" aria-label="GitHub repositories">
      {#each rows as r (r.name_with_owner)}
        {@const added = inFleet(r.name_with_owner)}
        {@const meta = metaOf(r)}
        <li>
          <label
            class="repo"
            class:active={r.name_with_owner === activeKey}
            class:added
            data-testid="gh-repo-row"
            data-key={r.name_with_owner}
            onmouseenter={() => (activeKey = r.name_with_owner)}
          >
            <input
              type="checkbox"
              data-testid="gh-repo-check"
              checked={added || selected.includes(r.name_with_owner)}
              disabled={disabled || added}
              onchange={() => toggle(r.name_with_owner)}
            />
            <span class="text">
              <span class="name">{r.name_with_owner}</span>
              {#if meta || r.description}
                <span class="meta">{[meta, r.description].filter(Boolean).join(' · ')}</span>
              {/if}
            </span>
            {#if added}<span class="pill" data-testid="gh-in-fleet">already in fleet</span>{/if}
          </label>
        </li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  .status { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; white-space: pre-wrap; }
  .retry {
    align-self: flex-start;
    font-size: var(--text-2xs);
    padding: 0.2rem 0.7rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .repos {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 14rem;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .repo {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    cursor: pointer;
  }
  .repo.active { background: var(--bg-hover); }
  .repo.added { cursor: default; color: var(--fg-muted); }
  .text { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .name { font-size: var(--text-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .meta {
    font-size: var(--text-xs);
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    font-size: var(--text-xs);
    color: var(--fg-muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 0 var(--space-2);
    white-space: nowrap;
  }
</style>
