<script lang="ts">
  // Copy attach for a session on another fleet: the `ssh -t … tmux attach`
  // line on the clipboard, or off with the reason when this desktop cannot
  // reach it (`fleetAttachRefusal`).
  import { fleetAttachCommand, fleetAttachRefusal } from './fleets';
  import type { SessionRow } from './sessions';
  import { copyAndSay } from './clipboard';
  import Button from './kit/Button.svelte';

  let { session, testid }: { session: SessionRow; testid?: string } = $props();

  const refusal = $derived(fleetAttachRefusal(session));

  async function copy() {
    if (refusal) return;
    await copyAndSay(fleetAttachCommand(session), 'Attach command');
  }
</script>

<Button size="sm" variant="quiet" {testid} disabled={!!refusal} title={refusal ?? undefined} onclick={() => void copy()}>Copy attach</Button>
