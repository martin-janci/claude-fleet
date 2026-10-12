<!--
  Redesign step 9.11: a completed mission's release note, an LLM draft on
  demand (Finish's "Release note" field). Draft runs `claude -p` on the
  mission's planner host and books the run on the mission as
  `release_note`; Copy puts the text, as edited, on the clipboard.
-->
<script lang="ts">
  import DraftField from './DraftField.svelte';
  import { copyText } from './clipboard';
  import { draftReleaseNote, type Draft } from './drafts';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';

  let { missionId }: { missionId: number } = $props();

  /** Writing help's "Draft release notes" (G4.6), off by default. */
  const on = $derived(settingBool($fleetSettings, SETTING_KEYS.workDraftReleaseNotes));

  let draft = $state<Draft | null>(null);
  let text = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let copied = $state(false);

  async function run() {
    busy = true;
    error = null;
    const r = await draftReleaseNote(missionId);
    busy = false;
    if (r.ok) {
      draft = r.value;
      text = r.value.text;
    } else if (r.error.code === 'E_HUB_PROTOCOL' || r.error.code === 'E_FORBIDDEN') {
      error = 'This hub cannot draft release notes yet.';
    } else {
      error = r.error.message;
    }
  }

  async function copy() {
    copied = await copyText(text);
  }
</script>

{#if on}
<div class="note" data-testid="release-note">
  {#if draft || busy}
    <DraftField
      bind:value={text}
      label="Release note"
      model={draft?.model}
      host={draft?.host_alias}
      from={draft ? `from ${draft.from}` : null}
      {busy}
      rows={6}
      onregenerate={() => void run()}
      onclear={() => (draft = null)}
      testid="release-note-draft"
    />
    {#if text.trim()}
      <button class="btn btn--quiet" type="button" data-testid="release-note-copy" onclick={() => void copy()}
        >{copied ? 'Copied' : 'Copy release note'}</button
      >
    {/if}
  {:else}
    <button class="btn btn--quiet" type="button" data-testid="release-note-draft-btn" onclick={() => void run()}
      >Draft release note</button
    >
  {/if}
  {#if error}
    <p class="error" role="alert" data-testid="release-note-error">{error}</p>
  {/if}
</div>
{/if}

<style>
  .note {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .error {
    color: var(--danger);
    font-size: var(--text-xs);
    margin: 0;
  }
</style>
