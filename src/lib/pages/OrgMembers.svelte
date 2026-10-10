<script lang="ts">
  // An org's members (redesign 11.2, board OrgMembers): a table of who is
  // in it, their role, their devices (for whoever administers it) and since
  // when an org share reaches them. Removing one asks what happens to what
  // was shared with them on the org's sessions (take it back, keep it read
  // only, keep it); adding one offers a pairing code for their device.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import ActionForm from './ActionForm.svelte';
  import PairingResult from './PairingResult.svelte';
  import { buildArgs, type ActionSpec, type ResourceRecord } from './resources';
  import {
    memberPairing,
    memberSessionsWord,
    orgMemberGrants,
    pairMemberDevice,
    type GrantsOnRemove,
    type MemberGrants,
    type OrgMember,
  } from '../orgs';

  let {
    record,
    members,
    addAction,
    removeAction,
    readonly = false,
    busy = false,
    options,
    run,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    record: ResourceRecord;
    members: OrgMember[];
    addAction?: ActionSpec;
    removeAction?: ActionSpec;
    readonly?: boolean;
    busy?: boolean;
    options: (param: string) => { value: string; label: string }[];
    run: (action: ActionSpec, args: Record<string, unknown>) => Promise<boolean>;
    now?: () => number;
  } = $props();

  const orgId = $derived(Number(record.id));
  const orgName = $derived(String(record.name ?? 'the org'));
  const showDevices = $derived(members.some((m) => m.devices !== undefined));
  // Redesign 11.7c: each member's live sessions, the ones you
  // may not open counted and never named.
  const showSessions = $derived(members.some((m) => (m.live_sessions ?? 0) > 0));
  const PRIVATE_TITLE = 'Private: you see that it exists, not what it is';
  const nameOf = (m: OrgMember) => m.display_name || m.name;

  /** Since when an org share reaches them, in words. */
  function sharesSince(m: OrgMember): string {
    if (m.shares_since === undefined || m.shares_since === null) return m.role === 'viewer' ? 'never (viewer)' : '—';
    const day = (s: number) => new Date(s * 1000).toISOString().slice(0, 10);
    return day(m.shares_since) === day(now()) ? 'today' : day(m.shares_since);
  }

  // --- role -----------------------------------------------------------------

  const ROLES = ['admin', 'member', 'viewer'];

  function setRole(m: OrgMember, role: string) {
    if (!addAction || role === m.role) return;
    void run(addAction, buildArgs(addAction, record, null, { person: m.name, role }));
  }

  // --- remove ---------------------------------------------------------------

  /** The member being removed, and what was shared with them (null while
   *  it is read, or when the hub does not say). */
  let removing = $state<{ member: OrgMember; grants: MemberGrants | null; loading: boolean } | null>(null);
  let choice = $state<GrantsOnRemove>('revoke');
  const shared = $derived(removing?.grants ? removing.grants.watch + removing.grants.drive : 0);
  const sharedLine = $derived(
    removing?.grants
      ? `${shared} ${shared === 1 ? 'session' : 'sessions'} of ${orgName} ${shared === 1 ? 'is' : 'are'} shared with them ` +
          `(${removing.grants.drive} to drive, ${removing.grants.watch} to watch). What happens to those shares?`
      : '',
  );

  async function askRemove(m: OrgMember) {
    choice = 'revoke';
    removing = { member: m, grants: null, loading: true };
    const r = await orgMemberGrants(orgId, m.person_id);
    if (removing?.member !== m) return;
    const g = r.ok && r.value && typeof r.value.watch === 'number' ? r.value : null;
    removing = { member: m, grants: g, loading: false };
  }

  function confirmRemove() {
    const action = removeAction;
    const r = removing;
    const asked = shared > 0;
    removing = null;
    if (!action || !r) return;
    const args = buildArgs(action, record, r.member, {});
    // Only a choice the person was shown is sent; otherwise the hub's
    // default (take the shares back) holds, as the notice says.
    void run(action, asked ? { ...args, grants: choice } : args);
  }

  // --- add, then pair --------------------------------------------------------

  const pairingHere = $derived($memberPairing && $memberPairing.org_id === orgId ? $memberPairing : null);
  let device = $state('');
  // The editor is rebuilt when the list is re-read after the add, so a
  // pairing started there arrives through the store: name its device here.
  $effect(() => {
    const p = pairingHere;
    if (p && !p.pairing && !device) device = `${p.person}-device`;
  });
  let pairError = $state<string | null>(null);
  let minting = $state(false);

  function startPairing(person: string) {
    device = `${person}-device`;
    pairError = null;
    memberPairing.set({ org_id: orgId, person });
  }

  async function add(params: Record<string, string>) {
    if (!addAction) return;
    const person = (params.person ?? '').trim();
    const ok = await run(addAction, buildArgs(addAction, record, null, params));
    if (ok && person) startPairing(person);
  }

  async function mint() {
    const p = pairingHere;
    if (!p || !device.trim()) return;
    minting = true;
    const r = await pairMemberDevice(orgId, p.person, device.trim());
    minting = false;
    if (r.ok) memberPairing.set({ ...p, pairing: r.value });
    else pairError = r.error.message;
  }
</script>

<div class="members" data-testid="org-members">
  <table>
    <thead>
      <tr>
        <th scope="col">Person</th>
        <th scope="col">Role</th>
        {#if showDevices}<th scope="col">Devices</th>{/if}
        {#if showSessions}<th scope="col">Sessions</th>{/if}
        <th scope="col" title="An org share made before this day does not reach them">Sees shares since</th>
        {#if !readonly}<th scope="col"><span class="sr">Actions</span></th>{/if}
      </tr>
    </thead>
    <tbody>
      {#each members as m (m.person_id)}
        <tr data-testid="item-members">
          <td class="person">
            <span>{nameOf(m)}</span>{#if m.display_name && m.display_name !== m.name}<span class="dim">{m.name}</span>{/if}
          </td>
          <td>
            {#if !readonly && addAction}
              <!-- M15 G4.7: the role is changed in place, through the same
                   set_member the add form runs. -->
              <select
                aria-label={`Role of ${nameOf(m)}`}
                disabled={busy}
                data-testid={`member-role-${m.person_id}`}
                value={m.role}
                onchange={(e) => void setRole(m, (e.currentTarget as HTMLSelectElement).value)}>
                {#each ROLES as r (r)}<option value={r}>{r}</option>{/each}
              </select>
            {:else}{m.role}{/if}
          </td>
          {#if showDevices}<td class="dim" data-testid="member-devices">{m.devices?.length ? m.devices.join(', ') : 'none'}</td>{/if}
          {#if showSessions}<td
              data-testid="member-sessions"
              title={(m.private_sessions ?? 0) > 0 ? PRIVATE_TITLE : undefined}>{memberSessionsWord(m)}</td
            >{/if}
          <td data-testid="member-shares-since">{sharesSince(m)}</td>
          {#if !readonly}
            <td class="acts">
              <button
                type="button"
                class="btn btn--quiet"
                disabled={busy}
                data-testid={`member-pair-${m.person_id}`}
                onclick={() => startPairing(m.name)}>Pair a device</button>
              {#if removeAction}
                <button
                  type="button"
                  class="btn btn--quiet"
                  disabled={busy}
                  aria-label={`${removeAction.label}: ${nameOf(m)}`}
                  data-testid="item-remove-members"
                  onclick={() => void askRemove(m)}>Remove…</button>
              {/if}
            </td>
          {/if}
        </tr>
      {:else}
        <tr><td class="none" colspan="5">None</td></tr>
      {/each}
    </tbody>
  </table>

  {#if !readonly && addAction}
    <ActionForm action={addAction} {busy} {options} onrun={(params) => void add(params)} />
  {/if}

  {#if pairingHere}
    <div class="pair" data-testid="member-pairing">
      {#if pairingHere.pairing}
        <PairingResult pairing={pairingHere.pairing} onclose={() => memberPairing.set(null)} />
        <p class="dim">Once it pairs, it shows under Needs an admin until you trust it in Settings → Devices.</p>
      {:else}
        <p>Send {pairingHere.person} a pairing code for their device. It is fenced to {orgName} and theirs.</p>
        <form
          onsubmit={(e) => {
            e.preventDefault();
            void mint();
          }}>
          <input
            type="text"
            maxlength="64"
            aria-label="Device name"
            data-testid="member-pair-device"
            disabled={minting}
            bind:value={device} />
          <button type="submit" class="btn btn--primary" disabled={minting || !device.trim()} data-testid="member-pair-mint"
            >Mint a pairing code</button>
          <button type="button" class="btn btn--quiet" data-testid="member-pair-skip" onclick={() => memberPairing.set(null)}
            >Not now</button>
        </form>
        {#if pairError}<p class="error" role="alert" data-testid="member-pair-error">{pairError}</p>{/if}
      {/if}
    </div>
  {/if}
</div>

{#if removing}
  {@const m = removing.member}
  <ConfirmDialog
    title={`Remove ${nameOf(m)} from ${orgName}?`}
    confirmLabel={`Remove ${nameOf(m)}`}
    danger
    busy={removing.loading}
    confirmTestId="record-confirm"
    onconfirm={confirmRemove}
    oncancel={() => (removing = null)}>
    <p>
      They lose access to {orgName}'s hosts, projects and shared sessions right away. Their own private sessions stay
      theirs.
    </p>
    {#if removing.loading}
      <p class="dim" data-testid="member-grants-loading">Checking what is shared with them…</p>
    {:else if shared > 0 && removing.grants}
      <p data-testid="member-grants">{sharedLine}</p>
      <div role="radiogroup" aria-label="What happens to the shares" class="choices">
        <label
          ><input type="radio" name="grants" value="revoke" bind:group={choice} data-testid="member-grants-revoke" />
          Stop sharing {shared === 1 ? 'it' : `all ${shared}`} with them</label>
        <label
          ><input type="radio" name="grants" value="narrow" bind:group={choice} data-testid="member-grants-narrow" />
          Keep them, but read only</label>
        <label
          ><input type="radio" name="grants" value="keep" bind:group={choice} data-testid="member-grants-keep" />
          Keep them as they are</label>
      </div>
    {:else if removeAction?.confirm}
      <p>{removeAction.confirm}</p>
    {/if}
  </ConfirmDialog>
{/if}

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  td {
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  .person {
    display: flex;
    flex-direction: column;
  }
  .dim,
  .none {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .acts {
    text-align: right;
    white-space: nowrap;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .pair {
    margin-top: 0.6rem;
    padding: 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .pair form {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .error {
    color: var(--danger);
  }
  .choices {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    margin-top: 0.4rem;
  }
</style>
