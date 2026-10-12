<!--
  Gap plan G4.3: the session header's visibility badge, next to its state
  (SessionDetails board: "Needs you · Private"). For the owner it reads the
  live grant list (`session_access`) and says "Private" or "Shared · N";
  clicking it opens the app's one Share sheet, gated exactly as Share… is.
  An unclaimed row says so. Anyone else's row shows nothing here.
-->
<script lang="ts">
  import { fetchSessionAccess, type SessionGrant, type SessionRow } from './sessions';
  import { accessOf } from './access';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked, shareSheetFor, visibilityBadge } from './share';
  import { accessRequests } from './access_requests';

  let { session }: { session: SessionRow } = $props();

  const access = $derived($accessOf(session));
  let grants = $state<SessionGrant[] | null>(null);
  let readFor = -1;

  async function load(id: number): Promise<void> {
    readFor = id;
    const r = await fetchSessionAccess(id);
    if (readFor !== id) return;
    grants = r.ok ? r.value : null;
  }

  // Read once per session the owner opens, and again when the Share sheet
  // that was open on it closes (a share or a revoke happened there).
  let sheetWasOpen = false;
  $effect(() => {
    const id = session.id;
    const own = access === 'own' && session.visibility === 'private';
    const sheetOpen = $shareSheetFor === id;
    if (!own) {
      readFor = -1;
      grants = null;
      sheetWasOpen = false;
      return;
    }
    if (id !== readFor) {
      grants = null;
      void load(id);
    } else if (sheetWasOpen && !sheetOpen) {
      void load(id);
    }
    sheetWasOpen = sheetOpen;
  });

  const asks = $derived($accessRequests.filter((a) => a.session_id === session.id).length);
  const badge = $derived(visibilityBadge(session, access, grants, asks));
  const shareBlocked = $derived(
    hubActionBlocked('session_share', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'session_share'),
  );
</script>

{#if badge}
  {#if badge.kind !== 'unclaimed' && shareBlocked === null}
    <button
      type="button"
      class="vis vis-{badge.kind}"
      data-testid="session-visibility"
      data-kind={badge.kind}
      title="{badge.title}. Open Share."
      onclick={() => shareSheetFor.set(session.id)}>{badge.text}</button
    >
  {:else}
    <span class="vis vis-{badge.kind}" data-testid="session-visibility" data-kind={badge.kind} title={badge.title}
      >{badge.text}</span
    >
  {/if}
{/if}

<style>
  .vis {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    height: 20px;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    border: 0;
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    background: var(--bg-sunk);
    color: var(--fg-2);
    white-space: nowrap;
  }
  button.vis {
    cursor: pointer;
  }
  button.vis:hover {
    color: var(--fg);
  }
  .vis-shared {
    background: var(--accent-soft);
    color: var(--fg);
  }
</style>
