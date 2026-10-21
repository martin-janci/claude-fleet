<script lang="ts">
  // Copy attach for a session on another fleet: the `ssh -t … tmux attach`
  // line on the clipboard, or off with the reason when this desktop cannot
  // reach it (`fleetAttachRefusal`).
  import { fleetAttachCommand, fleetAttachRefusal } from './fleets';
  import type { SessionRow } from './sessions';
  import { copyText } from './clipboard';
  import { push } from './toasts';
  import Button from './kit/Button.svelte';

  let { session, testid }: { session: SessionRow; testid?: string } = $props();

  const refusal = $derived(fleetAttachRefusal(session));

  async function copy() {
    if (refusal) return;
    if (await copyText(fleetAttachCommand(session))) push({ kind: 'success', message: 'Attach command copied' });
  }
</script>

<Button size="sm" variant="quiet" {testid} disabled={!!refusal} title={refusal ?? undefined} onclick={() => void copy()}>Copy attach</Button>
