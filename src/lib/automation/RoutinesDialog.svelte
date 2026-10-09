<!-- The Routines as a dialog (Orbit Fleet redesign 8.6), shown while
     `routinesDialogOpen` is set, until 8.4's Automation screen hosts the
     panel itself. Its own component so the copy lint sees which store opens
     it (`copy_lint.test.ts` SELF_GATED). -->
<script lang="ts">
  import Modal from '../Modal.svelte';
  import RoutinesPanel from './RoutinesPanel.svelte';
  import { routinesDialogOpen } from '../routines';
</script>

{#if $routinesDialogOpen}
  <Modal title="Routines" onclose={() => routinesDialogOpen.set(false)} width="880px" maxWidth="min(94vw, 960px)" testid="routines-dialog">
    <RoutinesPanel />
  </Modal>
{/if}
