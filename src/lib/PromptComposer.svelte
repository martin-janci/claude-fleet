<script lang="ts">
  import Icon from './kit/Icon.svelte';
  import Loader from './Loader.svelte';
  import { untrack } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions, sendPrompt, type SessionRow } from './sessions';
  import { accounts, accountEmailTier, type AccountRow } from './accounts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked } from './share';
  import Modal from './Modal.svelte';

  let {
    source,
    onClose,
  }: {
    source: SessionRow;
    onClose: () => void;
  } = $props();

  // send_prompt routes, so it only needs the live connection to be up. This
  // is the half that is true of EVERY target, so it stays whole-sheet; who
  // this client is differs per target and is narrowed below.
  const sendBlocked = $derived(hubActionBlocked('send_prompt', $hubStatus, $hubConnection));
  /**
   * Per-target narrowing (multi-user M1, F2a). This sheet fans `send_prompt`
   * out over OTHER sessions — up to every session in the fleet with "Show all
   * fleet" — so one answer for the whole sheet gates nothing: the entry button
   * in `SessionDetails` asks about the SOURCE row, and the targets are not it.
   * A target shared with this client at `watch`, or not shared at all, is
   * dropped from the send and says why on its own row, exactly as
   * `BulkPromptDialog` marks a skipped one.
   */
  const targetBlocked = $derived((t: SessionRow): string | null => $sessionBlocked(t, 'send_prompt'));

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
  /** The shown targets this client may actually prompt. */
  const promptable = $derived(bulkTargets(displayTargets, 'send_prompt', $sessionBlocked));

  // Track which targets are checked (default: all relateds checked).
  // Synchronously seed from related sessions in the store at mount time so
  // canSend is correct on the very first render.
  function initialChecked(): Record<number, boolean> {
    const map: Record<number, boolean> = {};
    if (source.project_id === null) return map;
    const mayPrompt = get(sessionBlocked);
    for (const s of get(sessions)) {
      if (
        s.id !== source.id &&
        s.project_id === source.project_id &&
        s.worktree_id === source.worktree_id &&
        // Never pre-check a session this client may not prompt: a tick the
        // send then drops would read as a prompt that silently went nowhere.
        mayPrompt(s, 'send_prompt') === null
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
      if (checked[r.id] === undefined && targetBlocked(r) === null) checked[r.id] = true;
    }
  });

  function targetKey(s: SessionRow): string {
    return `${s.tmux_name}@${s.host_alias}`;
  }

  function accountForRow(s: SessionRow): AccountRow | null {
    if (!s.account_uuid) return null;
    return $accounts.find((a) => a.uuid === s.account_uuid) ?? null;
  }

  // Read from displayTargets — not the raw `checked` map — so stale entries
  // from a prior "Show all fleet" toggle can't keep Send enabled when none of
  // the currently-displayed rows are checked.
  const hasChecked = $derived(promptable.some((t) => checked[t.id]));
  const canSend = $derived(prompt.trim().length > 0 && hasChecked && !sending && sendBlocked === null);

  async function send() {
    sending = true;
    errors = {};
    succeeded = {};
    // `promptable`, not `displayTargets`: the narrowing is the gate, and it is
    // re-read here so a revoke that arrived while the sheet was open holds.
    const targets = promptable.filter((t) => checked[t.id]);
    await Promise.allSettled(
      targets.map(async (t) => {
        const key = targetKey(t);
        const r = await sendPrompt(t.host_alias, t.tmux_name, prompt);
        if (r.ok) {
          succeeded[key] = true;
        } else {
          errors[key] = r.error.message;
        }
      }),
    );
    sending = false;
    // Auto-close on full success
    if (Object.keys(errors).length === 0) {
      setTimeout(() => onClose(), 600);
    }
  }
</script>

<Modal label="Send prompt" onclose={onClose} width="520px">
  <div class="dialog">
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
            {@const why = targetBlocked(t)}
            <li class="target-row" class:not-mine={why !== null}>
              <label>
                <input
                  type="checkbox"
                  checked={checked[t.id] === true && why === null}
                  disabled={why !== null}
                  title={why ?? ''}
                  onchange={(e) => (checked[t.id] = (e.currentTarget as HTMLInputElement).checked)}
                  data-testid="target-checkbox-{t.id}"
                />
                <span class="host-badge">[{t.host_alias}]</span>
                <span class="account">{accountEmailTier(accountForRow(t))}</span>
                <span class="sess-name">{t.tmux_name}</span>
                {#if t.status !== 'running'}
                  <span class="warn" title="session may not be in claude REPL"><Icon name="warning" size={12} label="Session may not be in the Claude REPL" /></span>
                {/if}
                {#if why}
                  <span class="muted" data-testid="target-not-mine-{t.id}" title={why}>not yours</span>
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
          data-testid="show-all-fleet"
        />
        Show all fleet sessions
      </label>
    </section>

    <section class="prompt-section">
      <h4>Prompt</h4>
      <textarea
        bind:value={prompt}
        rows="8"
        placeholder="Type a prompt to send to selected sessions…"
        data-testid="composer-textarea"
      ></textarea>
    </section>

    <div class="actions">
      <button type="button" class="btn btn--quiet is-bounded" onclick={onClose}>Cancel</button>
      <button
        type="button"
        class="btn btn--primary"
        aria-disabled={!canSend}
        aria-describedby={sendBlocked ? 'composer-send-blocked' : undefined}
        onclick={canSend ? send : undefined}
        data-testid="composer-send"
      >{#if sending}<Loader name="comet" size={12} class="btn-loader" />{/if}{sending ? 'Sending…' : 'Send →'}</button>
    </div>
    {#if sendBlocked}
      <p id="composer-send-blocked" class="blocked-reason" role="status">{sendBlocked}</p>
    {/if}
  </div>
</Modal>

<style>
  .dialog {
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  .dialog h3 { margin: 0; font-size: var(--text-md); }
  .dialog h4 {
    margin: 0 0 0.3rem 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .muted { color: var(--fg-muted); font-size: var(--text-xs); margin: 0; }

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
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .target-row label:hover { border-color: var(--border); background: var(--bg-pane); }
  .target-row.not-mine label { opacity: 0.55; cursor: not-allowed; }
  .host-badge {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: var(--radius-xs);
  }
  .account { color: var(--fg-muted); font-size: var(--text-2xs); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sess-name { font-family: var(--font-mono); font-size: var(--text-2xs); }
  /* The severity tokens, not hand-picked hexes: #50c86e is the exact green
     this branch removed from attention.ts for failing its contrast floor
     (2.05:1 on --bg-pane), and its twin lived on here. */
  .warn { color: var(--usage-warn); }
  .ok { color: var(--usage-ok); }
  .err { color: var(--usage-crit); font-size: var(--text-2xs); }

  .show-all {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    margin-top: 0.4rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    cursor: pointer;
  }

  .prompt-section textarea {
    width: 100%;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    padding: 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
    resize: vertical;
    min-height: 6rem;
  }

  .actions { display: flex; gap: var(--control-gap); justify-content: flex-end; }
  .blocked-reason {
    margin: 0.35rem 0 0;
    color: var(--usage-warn);
    font-size: var(--control-font-sm);
    text-align: right;
  }
</style>
