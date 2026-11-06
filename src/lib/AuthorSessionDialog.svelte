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
  import { errorText } from './error_copy';
  import DialogSheet from './DialogSheet.svelte';
  import { untrack } from 'svelte';
  import { spawnAuthorSession } from './assets';
  import { hosts } from './hosts';
  import { selectSessionExplicitly } from './selection';

  let {
    kind,
    name,
    instructions: preset,
    onclose,
  }: {
    /** Omit both to delegate "create a new asset". */
    kind?: string;
    name?: string;
    /** What to seed the instructions with (New asset's "Write it with
     *  Claude…" names the kind and name it was given). */
    instructions?: string;
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

  let instructions = $state(untrack(() => preset) ?? defaultInstructions());
  const startedWith = untrack(() => instructions);
  // G2.6: where the session runs. `local` is the catalog's own checkout;
  // another host works in a clone and pushes.
  let host = $state('local');
  const hostChoices = $derived($hosts.filter((h) => !h.hidden));
  let busy = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;

  async function openSession() {
    if (busy || instructions.trim() === '') return;
    busy = true;
    error = null;
    controller = new AbortController();
    const r = await spawnAuthorSession({ kind, name, instructions, host }, controller.signal);
    busy = false;
    controller = null;
    if (!r.ok) {
      error = errorText(r.error);
      return;
    }
    authorSessionOpened = true;
    selectSessionExplicitly(r.value);
    onclose();
  }

  function cancelOpen() {
    controller?.abort();
  }

  // Escape, the backdrop or Cancel with words typed asks once (FormsAnatomy's
  // "Discard changes?"), rather than throw the instructions away; the sheet
  // asks, and holds ⌘↵ while it does.
  const dirty = $derived(instructions.trim() !== '' && instructions !== startedWith);
</script>

<DialogSheet
  title="Write it with Claude"
  lead={kind && name ? `Opens a session that edits ${name} in the catalog folder.` : 'Opens a session that writes a new asset in the catalog folder.'}
  verb="Open session"
  busyVerb="Opening…"
  {busy}
  {error}
  canConfirm={instructions.trim() !== ''}
  onconfirm={() => void openSession()}
  {onclose}
  {dirty}
  testid="author-session-dialog"
  confirmTestid="author-open"
  errorTestid="author-error"
>
  {#snippet secondary()}
    {#if busy}<button type="button" class="btn btn--quiet" onclick={cancelOpen} data-testid="author-cancel-inflight">Stop waiting</button>{/if}
  {/snippet}
  <label class="field">What should change
    <textarea bind:value={instructions} rows="4" disabled={busy} data-testid="author-instructions"></textarea>
  </label>
  {#if hostChoices.length > 1}
    <label class="field">Host
      <select bind:value={host} disabled={busy} data-testid="author-host">
        {#each hostChoices as h (h.alias)}<option value={h.alias}>{h.alias}</option>{/each}
      </select>
    </label>
    {#if host !== 'local'}<p class="muted" data-testid="author-host-note">It works in a clone of the catalog on {host} and pushes when done.</p>{/if}
  {/if}
</DialogSheet>

<style>
  .muted { color: var(--fg-muted); font-size: var(--text-xs); margin: 0; }
  select { font: inherit; padding: 4px 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: var(--radius-sm); }
  textarea { font: inherit; padding: 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: var(--radius-sm); resize: vertical; }
</style>
