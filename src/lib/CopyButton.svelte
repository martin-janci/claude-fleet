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
  class="conv-copy"
  class:copied
  data-testid="conv-copy"
  aria-label={copied ? 'Copied' : label}
  title={copied ? (copiedNote ? `Copied (${copiedNote})` : 'Copied') : label}
  onclick={() => void copy()}>{copied ? 'Copied' : 'Copy'}</button
>

<style>
  .conv-copy {
    padding: 0.05rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg-muted);
    font-size: 0.68rem;
    line-height: 1.4;
    cursor: pointer;
  }
  .conv-copy:hover,
  .conv-copy:focus-visible {
    border-color: var(--accent);
    color: var(--fg);
  }
  .conv-copy.copied {
    color: var(--accent);
  }
</style>
