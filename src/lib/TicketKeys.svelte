<script lang="ts">
  // Settings → Ticket keys: the prefix each project (and each GitHub
  // repository no project claims, and each Asana tracker) numbers its tickets
  // under — `CF-12` rather than `owner/repo#12` or a 16-digit Asana id
  // (`store::ticket_keys`). A rename is Jira-like: every key drawn follows,
  // and the old prefix keeps resolving. Fleet administration: a paired
  // desktop shows why it cannot, and the hub's CLI line.
  import { onMount } from 'svelte';
  import { hubBlock, hubStatus } from './hub';
  import {
    exampleKey,
    normalizePrefix,
    setTicketPrefix,
    ticketPrefixes,
    type TicketPrefixRow,
  } from './ticket_keys';
  import { push, pushError } from './toasts';

  const blocked = $derived(hubBlock('set_ticket_prefix', $hubStatus));

  let rows = $state<TicketPrefixRow[] | null>(null);
  let loadError = $state<string | null>(null);
  /** Row scope → what is typed in its field. */
  let edits = $state<Record<string, string>>({});
  let saving = $state<string | null>(null);

  async function load() {
    const r = await ticketPrefixes();
    if (r.ok) {
      rows = r.value;
      loadError = null;
    } else {
      loadError = r.error.message;
    }
  }

  onMount(() => {
    if (!blocked) void load();
  });

  const typed = (row: TicketPrefixRow) => edits[row.scope] ?? row.prefix;
  const wanted = (row: TicketPrefixRow) => normalizePrefix(typed(row));
  /** A suggested row saves even unchanged: saving reserves the prefix. */
  const changed = (row: TicketPrefixRow) => {
    const p = wanted(row);
    return p != null && (p !== row.prefix || !!row.suggested);
  };

  async function save(row: TicketPrefixRow) {
    const p = wanted(row);
    if (!p) return;
    saving = row.scope;
    const r = await setTicketPrefix(row, p);
    saving = null;
    if (!r.ok) {
      pushError(r.error, `Setting ${row.label}'s prefix failed`);
      return;
    }
    rows = r.value;
    const { [row.scope]: _, ...rest } = edits;
    edits = rest;
    push({
      kind: 'success',
      message: row.suggested
        ? `${row.label}: new tickets read ${p}-1, ${p}-2, …`
        : `${row.label}: tickets now read ${exampleKey(row, p)}; ${row.prefix}- keys still find them.`,
    });
  }

  function onkey(e: KeyboardEvent, row: TicketPrefixRow) {
    if (e.key === 'Enter' && changed(row)) void save(row);
    if (e.key === 'Escape' && edits[row.scope] != null) {
      e.stopPropagation();
      const { [row.scope]: _, ...rest } = edits;
      edits = rest;
    }
  }
</script>

<section class="block" data-testid="ticket-keys">
  <div class="section-header"><h4>Ticket keys</h4></div>
  <p class="blurb">
    Every project numbers its tickets under a short prefix, like Jira: <code>CF-12</code>. Tasks
    written in fleet draw the next number, and so do GitHub issues and Asana tasks, whose own ids
    are no name for a person (the tracker's id stays on hover). Jira and Linear keep their own
    keys. Renaming a prefix renames every key in it; the old keys still find their tickets.
  </p>

  {#if blocked}
    <p class="blurb" data-testid="ticket-keys-remote">{blocked}</p>
  {:else if loadError}
    <p class="error" data-testid="ticket-keys-error">Could not read the ticket keys: {loadError}</p>
  {:else if rows == null}
    <p class="blurb">Loading…</p>
  {:else if rows.length === 0}
    <p class="blurb" data-testid="ticket-keys-empty">
      No projects yet. A project's prefix appears here once it is added.
    </p>
  {:else}
    <table class="prefixes">
      <thead>
        <tr><th scope="col">Prefix</th><th scope="col">Where</th><th scope="col">Tickets</th><th></th></tr>
      </thead>
      <tbody>
        {#each rows as row (row.scope)}
          {@const bad = wanted(row) == null}
          <tr data-testid="ticket-prefix-row" data-scope={row.scope}>
            <td>
              <input
                class="prefix"
                class:bad
                data-testid="ticket-prefix-input"
                aria-label="Prefix for {row.label}"
                aria-invalid={bad}
                maxlength="10"
                spellcheck="false"
                autocapitalize="characters"
                value={typed(row)}
                oninput={(e) => (edits = { ...edits, [row.scope]: (e.currentTarget as HTMLInputElement).value })}
                onkeydown={(e) => onkey(e, row)}
              />
            </td>
            <td>
              <span class="label">{row.label}</span>
              {#if (row.former_prefixes ?? []).length > 0}
                <span class="former" data-testid="ticket-prefix-former">was {(row.former_prefixes ?? []).join(', ')}</span>
              {/if}
            </td>
            <td class="count">
              {#if row.suggested}
                <span class="suggested" data-testid="ticket-prefix-suggested" title="Nothing has used this prefix yet; the first ticket reserves it, or save it now"
                  >suggested</span
                >
              {:else}
                {row.tickets ?? 0}
              {/if}
            </td>
            <td class="act">
              {#if bad}
                <span class="hint" data-testid="ticket-prefix-hint">2–10 letters, digits or _, starting with a letter</span>
              {:else if changed(row)}
                <button
                  type="button"
                  data-testid="ticket-prefix-save"
                  disabled={saving != null}
                  onclick={() => save(row)}>{saving === row.scope ? 'Saving…' : 'Save'}</button
                >
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  .blurb {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .error {
    font-size: var(--text-2xs);
    color: var(--danger, var(--fg));
  }
  .prefixes {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 4px 6px;
  }
  td {
    padding: 4px 6px;
    border-top: 1px solid var(--border);
    vertical-align: middle;
  }
  .prefix {
    width: 8.5ch;
    font-family: var(--font-mono, monospace);
    text-transform: uppercase;
    padding: 2px 4px;
  }
  .prefix.bad {
    border-color: var(--danger, var(--accent));
  }
  .label {
    overflow-wrap: anywhere;
  }
  .former,
  .suggested,
  .hint {
    margin-left: 6px;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .count {
    white-space: nowrap;
    color: var(--fg-muted);
  }
  .act {
    text-align: right;
    white-space: nowrap;
  }
</style>
