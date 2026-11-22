<script lang="ts">
  // Remove on a login profile (review w11-7): confirm, then `add_account
  // { action: "remove" }`. The backend closes the profile's sign-in pane,
  // deletes fleet's token and key files and ~/.claude-profiles/<name>, and
  // refuses while a live session runs under the profile (its message names
  // them). The made-up accounts no login uses any more come back in the
  // reply and leave the accounts list here; the host row follows by event.
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { removeLogin, removeLoginMessage } from './add_account';
  import { dropAccounts } from './accounts';
  import { push, pushError } from './toasts';

  let {
    host,
    profile,
    kind = 'login',
    onclose,
  }: {
    host: string;
    profile: string;
    kind?: 'login' | 'setup_token' | 'api_key';
    onclose: () => void;
  } = $props();

  let busy = $state(false);

  async function confirm() {
    busy = true;
    const r = await removeLogin(host, profile);
    busy = false;
    if (r.ok) {
      dropAccounts(r.value.accounts_removed);
      push({ kind: 'info', message: `Profile ${profile} removed from ${host}.` });
    } else {
      pushError(r.error, `Remove profile ${profile} on ${host} failed`);
    }
    onclose();
  }
</script>

<ConfirmDialog
  title="Remove profile {profile} on {host}?"
  message={removeLoginMessage(host, profile, kind)}
  confirmLabel="Remove profile"
  danger
  {busy}
  confirmTestId="confirm-remove-login"
  onconfirm={() => void confirm()}
  oncancel={onclose}
/>
