<script lang="ts">
  // A small "Copy" button for the Conversations tab (prompts, reply text,
  // tool results). It is a 24px target its parent keeps visible — the
  // opacity: 0 hover-reveal this comment used to describe was removed with
  // ToolLine's `.copy-slot`, because a control you have to find by hovering
  // is not a control a keyboard or touch user has. After a successful copy
  // it reads "Copied" for 1.5 s. `label` names what it copies for assistive
  // tech ("Copy prompt"); `copiedNote` qualifies the copy in the tooltip
  // ("Copied (truncated at 8 000 chars)").
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
