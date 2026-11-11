<script lang="ts">
  // "Sign in again" for a signed-out login: the host's own (`profile` null)
  // or a profile's. A `/login` gets the login pane over the login that is
  // there (`add_account { again: true }`), the same sign-in step + Add
  // account uses; a setup-token profile has no `/login` to renew, so it gets
  // + Add account preset to a new token for that profile.
  import { onMount } from 'svelte';
  import AddAccountDialog from './AddAccountDialog.svelte';
  import AddAccountLogin from './AddAccountLogin.svelte';
  import { startLogin } from './add_account';
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

  onMount(() => {
    if (token) return;
    void startLogin(host, profile ?? '', true).then((r) => {
      if (r.ok) {
        opened = true;
      } else {
        push({ kind: 'error', message: errorText(r.error) });
        onclose();
      }
    });
  });
</script>

{#if token}
  <AddAccountDialog {host} kind="setup_token" profile={profile ?? undefined} {pollMs} {onclose} />
{:else if opened}
  <AddAccountLogin login={{ host, profile: profile ?? '', again: true }} {pollMs} {onclose} />
{/if}
