<script lang="ts">
  // An account's compact label that edits its nickname in place (spec:
  // "Account nicknames"). Click, or `e` handled by the owner, opens the input;
  // Enter saves, Escape cancels, an empty value clears the nickname. The
  // owner decides where focus goes afterwards via `ondone`.
  import { accountLabel, setAccountNickname, type AccountRow } from './accounts';
  import { pushError } from './toasts';

  let {
    account,
    editing,
    onedit,
    ondone,
    testid,
    blocked = null,
  }: {
    account: AccountRow;
    editing: boolean;
    onedit: () => void;
    ondone: () => void;
    testid: string;
    /** `hubBlock('set_account_nickname', …)` from the owner — the nickname
     *  lives in the hub's database, and there is no tool to set it from here. */
    blocked?: string | null;
  } = $props();

  let input: HTMLInputElement | undefined = $state();
  let finished = false;

  $effect(() => {
    if (editing && input) {
      finished = false;
      input.focus();
      input.select();
    }
  });

  function finish() {
    if (finished) return;
    finished = true;
    ondone();
  }

  async function save(value: string) {
    if (finished) return;
    finished = true;
    const trimmed = value.trim();
    const r = await setAccountNickname(account.uuid, trimmed === '' ? null : trimmed);
    if (!r.ok) pushError(r.error, 'Nickname not saved');
    ondone();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      void save((e.currentTarget as HTMLInputElement).value);
    } else if (e.key === 'Escape') {
      // Cancel only: the view must not also treat this Escape as "close".
      e.preventDefault();
      e.stopPropagation();
      finish();
    }
  }
</script>

{#if editing}
  <input
    bind:this={input}
    class="nick-input"
    data-testid="{testid}-input"
    value={account.nickname ?? ''}
    maxlength="32"
    placeholder={account.email ?? 'nickname'}
    aria-label="Nickname for {account.email ?? accountLabel(account)} (empty clears it)"
    onkeydown={onKeydown}
    onblur={finish}
  />
{:else}
  <button
    type="button"
    class="nick"
    tabindex="-1"
    data-testid={testid}
    disabled={blocked !== null}
    title={blocked ?? `${account.email ?? accountLabel(account)} — click or press e to set a nickname`}
    onclick={(e) => {
      e.stopPropagation();
      onedit();
    }}>{accountLabel(account)}</button
  >
{/if}

<style>
  .nick {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    font-weight: 600;
    color: var(--fg);
    cursor: text;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }
  .nick:hover { text-decoration: underline dotted; }
  .nick-input {
    font: inherit;
    font-weight: 600;
    width: 12rem;
    max-width: 100%;
    padding: 0 0.25rem;
    border: 1px solid var(--accent);
    border-radius: 3px;
    background: var(--bg-pane);
    color: var(--fg);
  }
</style>
