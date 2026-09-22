<script module lang="ts">
  // Module-level flag (not per-instance state, and not a Svelte store —
  // `AssetsPanel` only ever reads it once, synchronously, at the moment its
  // `visible` prop flips false→true): set when a session is successfully
  // opened here, so the panel knows to reload the catalog on the next
  // tab-focus per spec ("When the Assets tab regains focus after such a
  // session was opened, the panel reloads the catalog") — a session opened
  // this way edits the repo directly, not through an authoring command, so
  // nothing else tells the panel to refresh. Exported as a value for
  // reading (a plain `import`ed binding is read-only, so a setter is
  // exported alongside it for `AssetsPanel` to clear it once consumed).
  export let authorSessionOpened = false;
  export function clearAuthorSessionOpened(): void {
    authorSessionOpened = false;
  }
</script>

<script lang="ts">
  import Modal from './Modal.svelte';
  import { spawnAuthorSession } from './assets';
  import { selectSessionExplicitly } from './selection';

  let {
    kind,
    name,
    onclose,
  }: {
    /** Omit both to delegate "create a new asset". */
    kind?: string;
    name?: string;
    onclose: () => void;
  } = $props();

  function defaultInstructions(): string {
    // A specific kind/name (an existing asset delegated from `AssetDetail`)
    // gets the "Improve this …" prompt; without both (the generic "create a
    // new asset" entry point) there is no kind to name — the seeded prompt
    // the backend builds for that case (`build_author_prompt`) does not
    // depend on one either, since `target_of` treats a lone `kind` with no
    // `name` the same as neither being given.
    return kind && name ? `Improve this ${kind} "${name}": ` : 'Create a new asset that …';
  }

  let instructions = $state(defaultInstructions());
  let busy = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;

  async function openSession() {
    if (busy || instructions.trim() === '') return;
    busy = true;
    error = null;
    controller = new AbortController();
    const r = await spawnAuthorSession({ kind, name, instructions }, controller.signal);
    busy = false;
    controller = null;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    authorSessionOpened = true;
    selectSessionExplicitly(r.value);
    onclose();
  }

  function cancelOpen() {
    controller?.abort();
  }
</script>

<Modal title="Open in session" onclose={busy ? undefined : onclose} width="480px" testid="author-session-dialog">
  <p class="muted">
    Opens an interactive Claude session working in the catalog repo, seeded with the asset's
    location and the IR rules plus the instructions below.
  </p>
  <label>Instructions
    <textarea bind:value={instructions} rows="4" disabled={busy} data-testid="author-instructions"></textarea>
  </label>
  {#if error}<p class="error" data-testid="author-error">{error}</p>{/if}
  <div class="actions">
    <button onclick={onclose} disabled={busy}>Cancel</button>
    {#if busy}<button onclick={cancelOpen} data-testid="author-cancel-inflight">Stop waiting</button>{/if}
    <button
      class="primary"
      onclick={openSession}
      disabled={busy || instructions.trim() === ''}
      data-testid="author-open"
    >{busy ? 'Opening…' : 'Open session'}</button>
  </div>
</Modal>

<style>
  .muted { color: var(--fg-muted); font-size: 12px; margin: 0 0 8px; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--fg-muted); }
  textarea { font: inherit; padding: 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: 4px; resize: vertical; }
  .error { color: #dc2626; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 8px; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
