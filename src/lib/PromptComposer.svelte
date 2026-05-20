<script lang="ts">
  import { untrack } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions, sendPrompt, type SessionRow } from './sessions';
  import { accounts, type AccountRow } from './accounts';

  let {
    source,
    onClose,
  }: {
    source: SessionRow;
    onClose: () => void;
  } = $props();

  let prompt = $state('');
  let showAllFleet = $state(false);
  let sending = $state(false);
  // Per-target error map: tmux_name@host → message
  let errors = $state<Record<string, string>>({});
  // Per-target success map: tmux_name@host → true
  let succeeded = $state<Record<string, boolean>>({});

  // Default: related sessions for the source. Toggle expands to all fleet.
  const relatedTargets = $derived(
    source.project_id === null
      ? []
      : $sessions.filter(
          (s) =>
            s.id !== source.id &&
            s.project_id === source.project_id &&
            s.worktree_id === source.worktree_id,
        ),
  );
  const allOtherTargets = $derived(
    $sessions.filter((s) => s.id !== source.id),
  );

  // List of targets to show. When showAllFleet=false → relatedTargets.
  // When true → all sessions (related ones marked).
  const displayTargets = $derived(
    showAllFleet ? allOtherTargets : relatedTargets,
  );

  // Track which targets are checked (default: all relateds checked).
  // Synchronously seed from related sessions in the store at mount time so
  // canSend is correct on the very first render.
  function initialChecked(): Record<number, boolean> {
    const map: Record<number, boolean> = {};
    if (source.project_id === null) return map;
    for (const s of get(sessions)) {
      if (
        s.id !== source.id &&
        s.project_id === source.project_id &&
        s.worktree_id === source.worktree_id
      ) {
        map[s.id] = true;
      }
    }
    return map;
  }
  let checked = $state<Record<number, boolean>>(untrack(initialChecked));

  // Initialise checked map when relatedTargets changes (newly observed sessions).
  $effect(() => {
    for (const r of relatedTargets) {
      if (checked[r.id] === undefined) checked[r.id] = true;
    }
  });

  function targetKey(s: SessionRow): string {
    return `${s.tmux_name}@${s.host_alias}`;
  }

  function accountForRow(s: SessionRow): AccountRow | null {
    if (!s.account_uuid) return null;
    return $accounts.find((a) => a.uuid === s.account_uuid) ?? null;
  }

  function accountText(a: AccountRow | null): string {
    if (!a) return '—';
    const email = a.email ?? a.uuid;
    return a.seat_tier ? `${email} (${a.seat_tier})` : email;
  }

  // Read from displayTargets — not the raw `checked` map — so stale entries
  // from a prior "Show all fleet" toggle can't keep Send enabled when none of
  // the currently-displayed rows are checked.
  const hasChecked = $derived(displayTargets.some((t) => checked[t.id]));
  const canSend = $derived(prompt.trim().length > 0 && hasChecked && !sending);

  async function send() {
    sending = true;
    errors = {};
    succeeded = {};
    const targets = displayTargets.filter((t) => checked[t.id]);
    for (const t of targets) {
      const key = targetKey(t);
      const r = await sendPrompt(t.host_alias, t.tmux_name, prompt);
      if (r.ok) {
        succeeded[key] = true;
      } else {
        errors[key] = r.error.message;
      }
    }
    sending = false;
    // Auto-close on full success
    if (Object.keys(errors).length === 0) {
      setTimeout(() => onClose(), 600);
    }
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Send prompt">
    <h3>Send prompt to session(s)</h3>

    <section class="targets">
      <h4>Targets</h4>
      {#if displayTargets.length === 0}
        <p class="muted" data-testid="composer-no-targets">
          No other sessions available{showAllFleet ? '' : ' for this worktree'}.
        </p>
      {:else}
        <ul>
          {#each displayTargets as t (t.id)}
            {@const key = targetKey(t)}
            <li class="target-row">
              <label>
                <input
                  type="checkbox"
                  bind:checked={checked[t.id]}
                  disabled={sending}
                  data-testid="target-checkbox-{t.id}"
                />
                <span class="host-badge">[{t.host_alias}]</span>
                <span class="account">{accountText(accountForRow(t))}</span>
                <span class="sess-name">{t.tmux_name}</span>
                {#if t.status !== 'running'}
                  <span class="warn" title="session may not be in claude REPL">⚠</span>
                {/if}
                {#if succeeded[key]}
                  <span class="ok">✓</span>
                {/if}
                {#if errors[key]}
                  <span class="err" data-testid="target-err-{t.id}">✗ {errors[key]}</span>
                {/if}
              </label>
            </li>
          {/each}
        </ul>
      {/if}
      <label class="show-all">
        <input
          type="checkbox"
          bind:checked={showAllFleet}
          disabled={sending}
          data-testid="show-all-fleet"
        />
        Show all fleet sessions
      </label>
    </section>

    <section class="prompt-section">
      <h4>Prompt</h4>
      <textarea
        bind:value={prompt}
        disabled={sending}
        rows="8"
        placeholder="Type a prompt to send to selected sessions…"
        data-testid="composer-textarea"
      ></textarea>
    </section>

    <div class="actions">
      <button onclick={onClose} disabled={sending}>Cancel</button>
      <button
        class="primary"
        disabled={!canSend}
        onclick={send}
        data-testid="composer-send"
      >{sending ? 'Sending…' : 'Send →'}</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 20;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 520px;
    max-height: 80vh;
    overflow: auto;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .dialog h4 {
    margin: 0 0 0.3rem 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .muted { color: var(--fg-muted); font-size: 0.85rem; margin: 0; }

  .targets ul {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .target-row label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.3rem;
    border: 1px solid transparent;
    border-radius: 4px;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .target-row label:hover { border-color: var(--border); background: var(--bg-pane); }
  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
  }
  .account { color: var(--fg-muted); font-size: 0.75rem; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sess-name { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.78rem; }
  .warn { color: #d4a017; }
  .ok { color: rgb(80, 200, 110); }
  .err { color: #e64a4a; font-size: 0.75rem; }

  .show-all {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    margin-top: 0.4rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
    cursor: pointer;
  }

  .prompt-section textarea {
    width: 100%;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    resize: vertical;
    min-height: 6rem;
  }

  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
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
  .actions button.primary {
    border-color: var(--accent);
    color: var(--fg);
  }
</style>
