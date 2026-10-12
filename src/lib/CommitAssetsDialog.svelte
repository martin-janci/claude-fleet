<!--
  Commit asset changes (gap plan G2.6, the Toolkit forms board): replaces
  the bare message prompt. It lists what the commit takes, one row per
  asset with A/M/D, and starts the message from those changes by rule
  (`commitMessageFor`), never a model: the person edits it freely. "Commit
  and push" is offered when the catalog has a remote to push to.
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { commitMessageFor, repoStatus, repoStatusStore, type RepoChange } from './assets';

  let {
    canPush = false,
    oncommit,
    oncancel,
  }: {
    /** The catalog has a remote: offer "Commit and push". */
    canPush?: boolean;
    oncommit: (message: string, push: boolean) => void;
    oncancel: () => void;
  } = $props();

  const changes = $derived<RepoChange[]>($repoStatusStore?.changes ?? []);
  const count = $derived(changes.length > 0 ? changes.length : ($repoStatusStore?.dirty ?? 0));
  let message = $state(untrack(() => commitMessageFor($repoStatusStore?.changes ?? [])));
  /** The person typed: a fresher status no longer rewrites the message. */
  let edited = $state(false);

  onMount(() => {
    void repoStatus().then(() => {
      if (!edited) message = commitMessageFor($repoStatusStore?.changes ?? []);
    });
  });

  const STATUS_WORD: Record<string, string> = { A: 'added', M: 'changed', D: 'removed' };
  const ok = $derived(message.trim() !== '');

  function submit(push: boolean) {
    if (!ok) return;
    oncommit(message.trim(), push);
  }
</script>

<Modal
  title={count === 1 ? 'Commit 1 asset change' : `Commit ${count} asset changes`}
  onclose={oncancel}
  width="480px"
  testid="commit-assets-dialog"
>
  {#if changes.length > 0}
    <ul class="changes" data-testid="commit-assets-changes">
      {#each changes as c (c.path)}
        <li data-testid="commit-assets-change">
          <code>{c.path}</code>
          <span class="st st--{c.status}" title={STATUS_WORD[c.status] ?? c.status}>{c.status}</span>
        </li>
      {/each}
    </ul>
  {/if}
  <label class="msg">
    <span>Message <span class="from" data-testid="commit-assets-from">from the changes</span></span>
    <textarea
      rows="2"
      bind:value={message}
      oninput={() => (edited = true)}
      data-testid="commit-assets-message"
    ></textarea>
  </label>
  <div class="actions">
    <button type="button" onclick={oncancel}>Cancel</button>
    <button type="button" class:primary={!canPush} disabled={!ok} onclick={() => submit(false)} data-testid="commit-assets-commit"
      >Commit</button
    >
    {#if canPush}
      <button type="button" class="primary" disabled={!ok} onclick={() => submit(true)} data-testid="commit-assets-push"
        >Commit and push</button
      >
    {/if}
  </div>
</Modal>

<style>
  .changes { list-style: none; margin: 0 0 10px; padding: 0; display: flex; flex-direction: column; gap: 2px; max-height: 200px; overflow: auto; }
  .changes li { display: flex; justify-content: space-between; gap: 8px; font-size: var(--text-xs); }
  .changes code { font-family: var(--font-mono); overflow-wrap: anywhere; }
  .st { font-family: var(--font-mono); color: var(--fg-muted); }
  .st--A { color: var(--done, var(--fg-muted)); }
  .st--D { color: var(--usage-crit); }
  .msg { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-xs); color: var(--fg-muted); }
  .from { color: var(--fg-muted); font-size: var(--text-2xs); margin-left: 6px; }
  textarea { font: inherit; padding: 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: var(--radius-sm); resize: vertical; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 10px; }
  .actions button { font-size: var(--text-xs); padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
