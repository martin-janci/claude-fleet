<script lang="ts">
  // The projects an account is forbidden for (docs/accounts.md, *Keep an
  // account off a project*): work in one never starts, resumes or switches
  // onto a login on this account, wherever it is started from. Each listed
  // project has Allow; the picker below adds one. Every change replaces the
  // whole list (`set_account_projects`), and the row comes back with it.
  import { setAccountProjects, type AccountRow } from './accounts';
  import { projects } from './projects';
  import { pushError } from './toasts';

  let {
    account,
    blocked = null,
  }: {
    account: AccountRow;
    /** `hubActionBlocked('set_account_projects', …)` from the owner. */
    blocked?: string | null;
  } = $props();

  let busy = $state(false);
  const forbidden = $derived(account.forbidden_project_ids ?? []);
  /** The person's projects (never fleet's own), by name. */
  const choices = $derived(
    $projects
      .map((p) => p.project)
      .filter((p) => !p.system)
      .map((p) => ({ id: p.id, name: `${p.owner}/${p.repo}` }))
      .sort((a, b) => a.name.localeCompare(b.name)),
  );
  const nameOf = (id: number): string => choices.find((c) => c.id === id)?.name ?? `project ${id}`;
  const listed = $derived(forbidden.map((id) => ({ id, name: nameOf(id) })).sort((a, b) => a.name.localeCompare(b.name)));
  const addable = $derived(choices.filter((c) => !forbidden.includes(c.id)));

  async function save(ids: number[]) {
    if (blocked) return;
    busy = true;
    const r = await setAccountProjects(account.uuid, ids);
    busy = false;
    if (!r.ok) pushError(r.error, 'Projects not saved');
  }
  function forbid(value: string) {
    const id = Number(value);
    if (!Number.isInteger(id) || forbidden.includes(id)) return;
    void save([...forbidden, id]);
  }
  function allow(id: number) {
    void save(forbidden.filter((x) => x !== id));
  }
</script>

<section class="account-projects" data-testid="account-projects" aria-label="Projects this account may not run">
  <h4>Not for these projects</h4>
  <p class="sub">Work in these projects never starts, resumes or switches onto this account, from any device or automation.</p>
  {#if listed.length > 0}
    <ul class="chips" data-testid="account-projects-list">
      {#each listed as p (p.id)}
        <li class="chip">
          <span class="mono">{p.name}</span>
          <button
            type="button"
            class="link"
            disabled={busy || blocked !== null}
            title={blocked ?? `Let ${p.name} run on this account again`}
            aria-label="Allow {p.name} again"
            data-testid="account-projects-allow"
            onclick={() => allow(p.id)}>Allow</button
          >
        </li>
      {/each}
    </ul>
  {/if}
  {#if addable.length > 0}
    <select
      aria-label="Forbid this account for a project"
      data-testid="account-projects-add"
      disabled={busy || blocked !== null}
      title={blocked ?? ''}
      value=""
      onchange={(e) => {
        forbid(e.currentTarget.value);
        e.currentTarget.value = '';
      }}
    >
      <option value="" disabled>Forbid for a project…</option>
      {#each addable as c (c.id)}
        <option value={String(c.id)}>{c.name}</option>
      {/each}
    </select>
  {:else if listed.length === 0}
    <p class="sub">No projects yet.</p>
  {/if}
</section>

<style>
  .account-projects {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  h4 {
    margin: var(--space-4) 0 var(--space-1);
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .sub {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .mono {
    font-family: var(--mono);
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .link:hover:not(:disabled) {
    text-decoration: underline;
  }
  .link:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .chip {
    display: inline-flex;
    align-items: baseline;
    gap: var(--space-1);
    padding: 2px var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
  }
  select {
    align-self: flex-start;
    max-width: 100%;
  }
</style>
