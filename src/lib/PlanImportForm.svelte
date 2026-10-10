<!-- Import a plan (gap plan G2.5, the Automation forms board): a markdown
     plan pasted in, read into its steps, lanes and links, and, when its
     table has a Repo column, a repository per row. A row whose repository
     no project matches says "Row N has no repo: pick one" and offers the
     picker; until each row has one, `ready` stays null. Used on an open
     mission and in the New mission form. Text a person wrote is passed on
     as text. -->
<script lang="ts">
  import { projects } from './projects';
  import {
    PLAN_IMPORT_MAX_ROWS,
    initialRepoPicks,
    missingRepoLines,
    parsePlan,
    wireRows,
    type PlanRow,
  } from './plan_import';

  let {
    ready = $bindable(null),
    text = $bindable(''),
    testid = 'mission-import',
  }: {
    /** The rows to send, or `null` while the plan is empty, too long or a
     *  row still needs a repository. */
    ready?: PlanRow[] | null;
    text?: string;
    testid?: string;
  } = $props();

  const choices = $derived($projects.map((t) => t.project).filter((p) => !p.system));
  const parsed = $derived(text.trim() ? parsePlan(text) : null);
  const lanes = $derived(parsed ? new Set(parsed.rows.map((r) => r.lane).filter(Boolean)).size : 0);
  const links = $derived(parsed ? parsed.rows.reduce((n, r) => n + (r.needs?.length ?? 0), 0) : 0);

  // A person's picks win over what the cells name; a new paste re-reads the
  // cells for the steps not picked.
  let chosen = $state<Record<string, number | null>>({});
  const picks = $derived(parsed ? { ...initialRepoPicks(parsed.rows, choices), ...chosen } : {});
  const missing = $derived(parsed ? missingRepoLines(parsed, picks) : []);

  $effect(() => {
    const p = parsed;
    ready =
      p && p.rows.length > 0 && p.rows.length <= PLAN_IMPORT_MAX_ROWS && missing.length === 0 ? wireRows(p.rows, picks) : null;
  });
</script>

<p class="muted small">
  Paste a markdown plan. Fleet reads its step tables (#, Step, Needs, and Lane, Status or Repo when there) and a Lanes table
  (Lane, Steps in order). Each step becomes a task; importing again updates them.
</p>
<textarea
  rows="6"
  bind:value={text}
  aria-label="Plan"
  placeholder={'| # | Step | Needs | Repo |\n|---|---|---|---|\n| 1.1 | Schema | — | acme/api |\n| 1.2 | API | 1.1 | acme/api |'}
  data-testid="{testid}-text"
></textarea>
{#if parsed}
  <p class="muted small" data-testid="{testid}-preview">
    {parsed.rows.length} steps · {lanes} lanes · {links} links
    {#if parsed.rows.length > PLAN_IMPORT_MAX_ROWS} · at most {PLAN_IMPORT_MAX_ROWS} at once{/if}
  </p>
  {#each parsed.notes as n (n)}<p class="muted small">{n}</p>{/each}
  {#if parsed.hasRepos && parsed.rows.length > 0}
    <table class="repos" data-testid="{testid}-repos">
      <thead><tr><th>#</th><th>Step</th><th>Repo</th></tr></thead>
      <tbody>
        {#each parsed.rows as r, i (r.step)}
          <tr class:missing={picks[r.step] == null} data-testid="{testid}-repo-row">
            <td>{r.step}</td>
            <td>{r.title}</td>
            <td>
              <select
                aria-label="Repository for step {r.step}"
                data-testid="{testid}-repo-pick"
                value={picks[r.step] ?? ''}
                onchange={(e) => {
                  const v = (e.currentTarget as HTMLSelectElement).value;
                  chosen = { ...chosen, [r.step]: v === '' ? null : Number(v) };
                }}
              >
                <option value="">{r.repo ? `${r.repo}?` : 'pick one'}</option>
                {#each choices as p (p.id)}<option value={p.id}>{p.owner}/{p.repo}</option>{/each}
              </select>
              {#if picks[r.step] == null}<span class="warn" data-testid="{testid}-repo-missing">Row {i + 1} has no repo: pick one</span>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
{/if}

<style>
  textarea {
    width: 100%;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .repos {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  .repos th {
    text-align: left;
    color: var(--fg-muted);
    font-weight: 500;
  }
  .repos td,
  .repos th {
    padding: 2px 4px;
  }
  .repos tr.missing td {
    background: var(--bg-hover);
  }
  .warn {
    margin-left: 6px;
    color: var(--status-waiting);
  }
</style>
