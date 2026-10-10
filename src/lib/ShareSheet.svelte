<script lang="ts">
  // The one Share sheet for the whole app (multi-user M1).
  //
  // Structure copied from `TransferSheet.svelte`, which solves the same
  // problem: one app-wide instance, `const id = $derived($shareSheetFor)`,
  // `<Modal>` as the dialog primitive, and a self-closing effect for when the
  // session it is open on leaves the store.
  //
  // What the sheet is FOR, beyond the three buttons: a share is a decision
  // with two consequences the person taking it cannot see from the controls,
  // and both are written on screen rather than left to a doc page —
  //
  //   1. the recipient also gets the history from BEFORE the share. Granting
  //      the session grants its transcript, its journal and its timeline; there
  //      is no "from here on" share and inventing one would be a promise the
  //      reads cannot keep;
  //   2. watch and drive are enforced by FLEET, not by SSH. Anyone who
  //      independently has SSH to the host can still attach, and revoking the
  //      grant does not change that — it was never Fleet's to grant or revoke.
  //      Which is also why a share never hands over a terminal at all: the
  //      terminal is this machine's own `ssh … tmux attach`, with no hub in the
  //      path to refuse it later (spec §4.3 invariant 4).
  //
  // Org administration phase D brought team sharing back, as a GRANT to an
  // org: it reaches the org's members and admins who are in it when the share
  // is made — never someone who joins later (changing a membership never
  // widens a grant), never a viewer. The sheet says so beside the control.
  import Loader from './Loader.svelte';
  import Modal from './Modal.svelte';
  import { accessOf } from './access';
  import type { Result } from './result';
  import {
    fetchSessionAccess,
    narrowShare,
    shareSession,
    sessions,
    unshareSession,
    type SessionGrant,
    type ShareTo,
  } from './sessions';
  import { shareSheetFor, sessionBlocked } from './share';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { devices, loadDevices } from './devices';
  import { readOnlyRecipient } from './share_devices';
  import { errorSentence } from './error_copy';

  const id = $derived($shareSheetFor);
  const session = $derived(id === null ? undefined : $sessions.find((s) => s.id === id));
  /** The sheet is the owner's tool. The buttons that open it are gated too,
   *  but a row whose grant is revoked while the sheet is open must stop
   *  offering the controls — so this is read through `$accessOf`, which
   *  re-derives on a `grant:changed` with no re-list. */
  const owned = $derived($accessOf(session) === 'own');
  /** All three writes route to the hub (T13), so each asks both halves since
   *  F3: the refusal half (`$sessionBlocked` — a grantee cannot grant on) and
   *  the live link (`hubActionBlocked`), which is what stops a click from
   *  dying while the hub is unreachable. Three gates and not one, because
   *  `hubActionBlocked` answers per action name and the sheet's buttons are
   *  three different routed commands. */
  const shareBlocked = $derived(
    hubActionBlocked('session_share', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'session_share'),
  );
  const narrowBlocked = $derived(
    hubActionBlocked('session_narrow', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'session_narrow'),
  );
  const revokeBlocked = $derived(
    hubActionBlocked('session_unshare', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'session_unshare'),
  );
  /** The notice that replaces the sheet's body asks the ACCESS half only, on
   *  purpose: it explains why there are no controls, and "try again once the
   *  hub is back" would be a promise to a grantee who will still not be the
   *  owner when it is. The buttons above keep both halves. */
  const notOwnerReason = $derived($sessionBlocked(session, 'session_share'));
  const label = $derived(session?.friendly_name || session?.tmux_name || 'this session');

  /** The live grant list, as `session_access` answers it. `null` while the
   *  first read is out — told apart from `[]`, which is the real and very
   *  common "shared with nobody". */
  let grants = $state<SessionGrant[] | null>(null);
  let listError = $state<string | null>(null);
  let person = $state('');
  /** Who the draft share is for: a person, or an org (phase D). */
  let kind = $state<'person' | 'org'>('person');
  let level = $state<'watch' | 'answer' | 'drive'>('watch');
  /** Which person's grant is one click from being revoked. Revoking is not
   *  destructive the way a kill is, but it is invisible to the person it
   *  happens to, so it gets the same two-step the rest of the app uses. */
  let confirming = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  /** `id` is captured by the caller rather than re-read after the await: a
   *  list that lands after the sheet moved to another session belongs to the
   *  session that asked for it (the `WatchView.refresh` guard, same reason). */
  async function load(forId: number) {
    const r = await fetchSessionAccess(forId);
    if (forId !== $shareSheetFor) return;
    if (r.ok) {
      grants = r.value;
      listError = null;
      return;
    }
    grants = null;
    listError = `Couldn't read who this is shared with: ${errorSentence(r.error)}`;
  }

  // A fresh sheet each time it opens on a session: the draft recipient, the
  // level, the pending revoke and the error are all per-session.
  $effect(() => {
    const forId = id;
    person = '';
    kind = 'person';
    level = 'watch';
    confirming = null;
    error = null;
    grants = null;
    listError = null;
    if (forId !== null) {
      void load(forId);
      // Only feeds the read-only warning (step 5.8); a client the hub will
      // not list devices for keeps the list it had and warns about nobody.
      void loadDevices();
    }
  });

  /** Step 5.8: a drive share to someone whose every paired device is
   *  read-only can never send a prompt — say so before it is made. */
  const readOnlyWarning = $derived(
    kind === 'person' && level === 'drive' ? readOnlyRecipient(person, $devices) : null,
  );

  // Nothing to show: the row left the store (killed, reaped, or — on a paired
  // desktop — revoked out from under us).
  $effect(() => {
    if (id !== null && !session) shareSheetFor.set(null);
  });

  function close() {
    shareSheetFor.set(null);
  }

  /** Every mutation has the same shape: run it, keep the sheet open on a
   *  refusal with the reason in place, re-read the list on success. The list
   *  is re-read rather than patched locally because the hub is the authority
   *  on what the grant set now is — a narrow that the store refused as
   *  already-narrow must not leave a locally-edited row claiming otherwise. */
  async function run(
    forId: number,
    blocked: string | null,
    f: () => Promise<Result<unknown>>,
  ) {
    // Re-asked at the call, not only in the markup above (multi-user M1, F2b):
    // `owned` swaps the sheet's body for the refusal notice, but a revoke that
    // lands between a click and this line would otherwise still send. Sharing,
    // narrowing and revoking are all the `own` tier, which is exactly `owned`.
    //
    // `blocked` is the same re-ask for the OTHER half (F3): the hub link can
    // drop between the click and this line too, and the gate is a parameter
    // rather than one expression here because the three writes are three
    // different routed commands — a caller has to name its own.
    if (busy || !owned || blocked !== null) return;
    busy = true;
    error = null;
    const r = await f();
    busy = false;
    confirming = null;
    if (!r.ok) {
      error = errorSentence(r.error);
      return;
    }
    await load(forId);
  }

  function doShare() {
    const forId = id;
    const name = person.trim();
    if (forId === null || !name) return;
    const to: ShareTo = kind === 'org' ? { org: name } : name;
    void run(forId, shareBlocked, async () => {
      const r = await shareSession(forId, to, level);
      if (r.ok) person = '';
      return r;
    });
  }

  function doNarrow(to: ShareTo) {
    const forId = id;
    if (forId === null) return;
    void run(forId, narrowBlocked, () => narrowShare(forId, to));
  }

  function doRevoke(to: ShareTo) {
    const forId = id;
    if (forId === null) return;
    void run(forId, revokeBlocked, () => unshareSession(forId, to));
  }

  /** What to send back as the recipient. The hub's own name is what the
   *  sharing tools take, so a grant with no name is shown by id and its
   *  controls are disabled rather than guessing one. */
  function recipientOf(g: SessionGrant): ShareTo | null {
    if (g.org_id != null) return g.org_name ? { org: g.org_name } : null;
    return g.person_name ?? null;
  }
  /** The key the two-step revoke remembers: one per recipient. */
  function keyOf(to: ShareTo | null): string | null {
    if (to === null) return null;
    return typeof to === 'string' ? `p:${to}` : `o:${to.org}`;
  }
  function recipientLabel(g: SessionGrant): string {
    if (g.org_id != null) return `${g.org_name || `org #${g.org_id}`} (its members)`;
    return g.person_display_name || g.person_name || (g.person_id === null ? 'unknown' : `person #${g.person_id}`);
  }
</script>

{#if id !== null && session}
  <Modal
    title="Share {label}"
    onclose={busy ? undefined : close}
    width="480px"
    testid="share-sheet"
  >
    {#if !owned}
      <!-- Belt and braces: the controls that open this sheet are gated on the
           same answer, so this is what a revoke ARRIVING while it is open
           looks like, not a path a click can normally reach. -->
      <p class="err" data-testid="share-not-owner">
        {notOwnerReason ?? 'Only the session’s owner can share it.'}
      </p>
      <div class="actions">
        <button type="button" onclick={close}>Close</button>
      </div>
    {:else}
      <p class="hint">
        Shares this session through Fleet with a person, or with an org you are
        in. Revoke it whenever you like — a share is never permanent and never
        passes on: whoever you share with cannot share it further.
      </p>

      <section class="block">
        <h4>Share</h4>
        <div class="row">
          <select data-testid="share-kind" aria-label="Share with" bind:value={kind} disabled={busy}>
            <option value="person">a person</option>
            <option value="org">an org</option>
          </select>
          <input
            data-testid="share-person"
            aria-label={kind === 'org' ? 'Org' : 'Person'}
            placeholder={kind === 'org' ? 'an org you are a member of' : 'their name on this fleet'}
            bind:value={person}
            disabled={busy}
          />
          <select data-testid="share-level" aria-label="Level" bind:value={level} disabled={busy}>
            <option value="watch">watch — read only</option>
            <option value="answer">answer — can answer its questions</option>
            <option value="drive">drive — can send prompts</option>
          </select>
          <button
            type="button"
            class="primary"
            data-testid="share-confirm"
            disabled={busy || person.trim() === '' || shareBlocked !== null}
            title={shareBlocked ?? 'Share this session'}
            onclick={doShare}>{#if busy}<Loader name="comet" size={12} class="btn-loader" />{/if}{busy ? 'Sharing…' : 'Share'}</button
          >
        </div>
        {#if readOnlyWarning}
          <p class="warn" role="status" data-testid="share-readonly-warning">{readOnlyWarning}</p>
        {/if}
        <!-- Phase D: an org share is a grant to the org's members of today
             (see the comment at the top of this file). -->
        <p class="note" data-testid="share-org-note">
          An org share reaches its members and admins who are in it now — not
          anyone who joins later, and never a viewer. Share again to include
          newcomers.
        </p>
      </section>

      <section class="block">
        <h4>Shared with</h4>
        {#if listError}
          <p class="err" data-testid="share-list-error">
            {listError}
            {#if $shareSheetFor !== null}<button type="button" class="btn btn--quiet" data-testid="share-list-retry" onclick={() => void load($shareSheetFor!)}>Retry</button>{/if}
          </p>
        {:else if grants === null}
          <p class="note" data-testid="share-list-loading">Reading the grants…</p>
        {:else if grants.length === 0}
          <p class="note" data-testid="share-list-empty">
            Not shared with anyone. Only you can see this session.
          </p>
        {:else}
          <ul class="grants" data-testid="share-list">
            {#each grants as g (`${g.person_id}:${g.org_id}:${g.level}`)}
              {@const name = recipientOf(g)}
              <li class="grant" data-testid="share-grant">
                <span class="who" data-testid="share-grant-who">{recipientLabel(g)}</span>
                <span class="level" data-testid="share-grant-level">{g.level}</span>
                {#if confirming !== null && confirming === keyOf(name) && name !== null}
                  <span class="confirm" data-testid="share-revoke-confirm">
                    Revoke?
                    <button
                      type="button"
                      class="danger"
                      data-testid="share-revoke-yes"
                      disabled={busy || revokeBlocked !== null}
                      title={revokeBlocked ?? 'Revoke this grant'}
                      onclick={() => name !== null && doRevoke(name)}>Revoke</button
                    >
                    <button type="button" data-testid="share-revoke-no" disabled={busy}
                      onclick={() => (confirming = null)}>Keep</button
                    >
                  </span>
                {:else}
                  {#if g.level === 'drive' || g.level === 'answer'}
                    <!-- Narrow, never widen: there is no control here that
                         raises a level, because there is no tool that does
                         (spec §4.3 invariant 3). Widening is a revoke and a
                         fresh share, which is a decision, not a slider. -->
                    <button
                      type="button"
                      data-testid="share-narrow"
                      disabled={busy || name === null || narrowBlocked !== null}
                      title={name === null
                        ? 'This hub did not name the recipient, so this app cannot act on the grant — use fleet-hub'
                        : (narrowBlocked ?? 'Lower this grant to watch (read-only)')}
                      onclick={() => name !== null && doNarrow(name)}>Narrow to watch</button
                    >
                  {/if}
                  <button
                    type="button"
                    class="danger"
                    data-testid="share-revoke"
                    disabled={busy || name === null || revokeBlocked !== null}
                    title={name === null
                      ? 'This hub did not name the recipient, so this app cannot act on the grant — use fleet-hub'
                      : (revokeBlocked ?? 'Revoke this grant')}
                    onclick={() => (confirming = keyOf(name))}>Revoke</button
                  >
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <!-- The two things a sharer is deciding without being told, said out
           loud. Making them visible rather than silent turns the share into a
           decision taken knowingly. -->
      <section class="block consequences">
        <p data-testid="share-history-note">
          <strong>They also get the history from before the share.</strong> The
          transcript, the work journal and the session's timeline all come with
          the session — there is no "from this moment on" share.
        </p>
        <p data-testid="share-enforcement-note">
          <strong>Watch and drive are enforced by Fleet, not by SSH.</strong> A
          share never gives a terminal: attaching is a direct SSH session into
          this host that Fleet is not in the path of and could never revoke, so
          the person you share with gets a read-only snapshot of the pane
          instead. Anyone who independently has SSH to
          <code>{session.host_alias}</code> can still attach, and revoking
          changes nothing about that.
        </p>
      </section>

      {#if error}<p class="err" data-testid="share-error">{error}</p>{/if}

      <div class="actions">
        <button type="button" onclick={close} disabled={busy} data-testid="share-close">Close</button>
      </div>
    {/if}
  </Modal>
{/if}

<style>
  .hint {
    margin: 0 0 0.7rem;
    font-size: 0.85em;
    color: var(--fg-muted);
  }
  .block {
    margin: 0 0 var(--space-3);
  }
  .block h4 {
    margin: 0 0 var(--space-1);
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }
  .row input {
    flex: 1 1 10rem;
    min-width: 0;
  }
  .warn {
    margin: var(--space-1) 0 0;
    font-size: 0.85em;
    color: var(--status-waiting);
  }
  .note {
    margin: var(--space-1) 0 0;
    font-size: 0.8em;
    color: var(--fg-muted);
  }
  .grants {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .grant {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85em;
  }
  .who {
    font-weight: 600;
  }
  .level {
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: var(--text-2xs);
    padding: 0.1rem var(--space-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin-left: auto;
  }
  .grant button {
    margin-left: auto;
  }
  .grant button + button,
  .confirm button {
    margin-left: 0;
  }
  .consequences p {
    margin: 0 0 0.45rem;
    font-size: 0.8em;
    line-height: 1.45;
    color: var(--fg-muted);
  }
  .consequences code {
    font-size: 0.95em;
  }
  .err {
    color: var(--danger);
    font-size: 0.85em;
    margin: 0 0 var(--space-2);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  button {
    font-size: var(--text-xs);
    padding: var(--space-1) 0.7rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  button.primary {
    border-color: var(--accent);
  }
  button.danger {
    border-color: var(--danger);
    color: var(--danger);
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  input,
  select {
    font-size: var(--text-xs);
    padding: var(--space-1) 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-raise);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
</style>
