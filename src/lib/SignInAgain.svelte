<script lang="ts">
  // "Sign in again" for a signed-out login: the host's own (`profile` null)
  // or a profile's. A `/login` gets the login pane over the login that is
  // there (`add_account { again: true }`), the same sign-in step + Add
  // account uses; a setup-token profile has no `/login` to renew, so it gets
  // + Add account preset to a new token for that profile.
  //
  // Review w02-7: the host's own login on a Mac fleet reaches over SSH. A
  // `claude /login` there finds the Keychain locked and writes
  // ~/.claude/.credentials.json beside the Keychain copy, the split ⚿ asks
  // to undo. So that case first says so and offers + Add account (a login
  // profile, which has no Keychain copy to fight), with Sign in here anyway.
  import { onMount, untrack } from 'svelte';
  import AddAccountDialog from './AddAccountDialog.svelte';
  import AddAccountLogin from './AddAccountLogin.svelte';
  import Modal from './Modal.svelte';
  import { ownLoginSplitRisk, startLogin } from './add_account';
  import { hostByAlias } from './hosts';
  import { get } from 'svelte/store';
  import { errorText } from './error_copy';
  import { push } from './toasts';

  let {
    host,
    profile,
    token = false,
    onclose,
    pollMs = 2000,
  }: {
    host: string;
    /** `null` = the host's own login. */
    profile: string | null;
    /** The profile signs in with a setup token. */
    token?: boolean;
    onclose: () => void;
    pollMs?: number;
  } = $props();

  let opened = $state(false);
  /** The Mac own-login note is showing (w02-7); `adding` once + Add account was picked. */
  let macNote = $state(untrack(() => !token && profile === null && ownLoginSplitRisk(get(hostByAlias).get(host))));
  let adding = $state(false);

  function begin() {
    macNote = false;
    void startLogin(host, profile ?? '', true).then((r) => {
      if (r.ok) {
        opened = true;
      } else {
        push({ kind: 'error', message: errorText(r.error) });
        onclose();
      }
    });
  }

  onMount(() => {
    if (token || macNote) return;
    begin();
  });
</script>

{#if token}
  <AddAccountDialog {host} kind="setup_token" profile={profile ?? undefined} {pollMs} {onclose} />
{:else if adding}
  <AddAccountDialog {host} kind="subscription" {pollMs} {onclose} />
{:else if macNote}
  <Modal title="Sign the host login in again on {host}?" {onclose} width="420px" testid="sign-in-again-mac">
    <p class="note" data-testid="sign-in-again-mac-note">
      {host} is a Mac. Signing its own login in from fleet writes ~/.claude/.credentials.json next to the Keychain copy,
      and the two copies sign each other out. A login profile has only one copy: add one for this account instead.
    </p>
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button type="button" data-testid="sign-in-again-mac-anyway" onclick={begin}>Sign in here anyway</button>
      <button type="button" class="primary" data-autofocus="" data-testid="sign-in-again-mac-add" onclick={() => (adding = true)}
        >+ Add account…</button
      >
    </div>
  </Modal>
{:else if opened}
  <AddAccountLogin login={{ host, profile: profile ?? '', again: true }} {pollMs} {onclose} />
{/if}

<style>
  .note {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .actions {
    display: flex;
    gap: 0.4rem;
    justify-content: flex-end;
    margin-top: 0.6rem;
  }
  .actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .actions button.primary { border-color: var(--accent); }
</style>
