<script lang="ts">
  // A small "Copy" button for the Conversations tab (prompts, reply text,
  // tool results). The parent shows it on hover / focus-within; after a
  // successful copy it reads "Copied" for 1.5 s. `label` names what it
  // copies for assistive tech ("Copy prompt"); `copiedNote` qualifies the
  // copy in the tooltip ("Copied (truncated at 8 000 chars)").
  import { copyText } from './clipboard';

  let {
    text,
    label = 'Copy',
    copiedNote = null,
  }: { text: string; label?: string; copiedNote?: string | null } = $props();

  const COPIED_MS = 1_500;
  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    if (!(await copyText(text))) return;
    copied = true;
    clearTimeout(timer);
    timer = setTimeout(() => (copied = false), COPIED_MS);
  }
  $effect(() => () => clearTimeout(timer));
</script>

<button
  type="button"
  class="btn btn--icon btn--quiet conv-copy"
  class:copied
  data-testid="conv-copy"
  aria-label={copied ? 'Copied' : label}
  title={copied ? (copiedNote ? `Copied (${copiedNote})` : 'Copied') : label}
  onclick={() => void copy()}>⧉</button
>

<style>
  .conv-copy.copied {
    color: var(--accent);
  }
</style>
