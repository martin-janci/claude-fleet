<!--
  Gap plan G4.2: the session header for someone a session is shared with
  (Watch board): "Shared with you · Read", and "Ask Martin for Answer" — a
  note to the owner that confers nothing until they grant it. Once asked it
  reads "Asked for Answer · waiting for Martin". The owner's own row shows
  nothing here (VisibilityBadge is theirs).
-->
<script lang="ts">
  import { accessOf, myAccessRequests, myGrantInfo } from './access';
  import { askForAccess } from './access_requests';
  import { errorSentence } from './error_copy';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { orgs as orgList } from './orgs';
  import { isSharedAccess } from './session_scope';
  import type { SessionRow } from './sessions';
  import { askedLabel, askLabel, nextLevel, sharedWithYouChip, sharerName } from './shared_view';

  let { session }: { session: SessionRow } = $props();

  const access = $derived($accessOf(session));
  const level = $derived(isSharedAccess(access) ? access : null);
  const info = $derived($myGrantInfo.get(session.id));
  const sharer = $derived(sharerName(session, info, $orgList));
  const want = $derived(level ? nextLevel(level) : null);
  const asked = $derived($myAccessRequests.get(session.id) ?? null);
  const blocked = $derived(hubActionBlocked('session_ask_access', $hubStatus, $hubConnection));

  let busy = $state(false);
  let error = $state<string | null>(null);
  $effect(() => {
    void session.id;
    error = null;
  });

  async function ask() {
    if (!want || busy || blocked !== null) return;
    busy = true;
    error = null;
    const id = session.id;
    const r = await askForAccess(id, want);
    busy = false;
    if (id !== session.id) return;
    if (!r.ok) {
      error = errorSentence(r.error);
      return;
    }
    myAccessRequests.update((cur) => {
      const next = new Map(cur);
      next.set(id, { id: r.value.id, level: want, requestedAt: r.value.requested_at });
      return next;
    });
  }
</script>

{#if level}
  <span class="shared" data-testid="shared-with-you" data-level={level}>{sharedWithYouChip(level)}</span>
  {#if asked}
    <span class="asked" data-testid="shared-asked">{askedLabel(sharer, asked.level)}</span>
  {:else if want}
    <button
      type="button"
      class="btn btn--quiet head-btn ask"
      data-testid="shared-ask"
      disabled={busy || blocked !== null}
      title={blocked ?? `Ask the owner to share this session with you at ${want}. Nothing changes until they grant it.`}
      onclick={() => void ask()}>{askLabel(sharer, want)}</button
    >
  {/if}
  {#if error}<span class="err" role="status" data-testid="shared-ask-error">{error}</span>{/if}
{/if}

<style>
  .shared,
  .asked {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    height: 20px;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    font-size: var(--text-2xs);
    font-weight: 500;
    white-space: nowrap;
  }
  .shared {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .asked {
    color: var(--fg-muted);
  }
  .ask {
    flex-shrink: 0;
    white-space: nowrap;
  }
  .err {
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
