<script lang="ts">
  // + Add account's sign-in step (M15 step G2.9): the login pane fleet
  // opened on the host (`fleet-login--<name>`, running `claude /login`).
  // Its last lines, the sign-in link (Open, Copy), its numbered choices and
  // keys as buttons, a field for the code the page shows, and Done once the
  // host reports the login; Done and Cancel close the pane. "Run it there
  // instead" is the command for a terminal on the host, whatever the CLI's
  // flow becomes.
  import Modal from './Modal.svelte';
  import { copyText } from './clipboard';
  import { openExternal } from './open_external';
  import { push } from './toasts';
  import {
    asksForCode,
    endLogin,
    loginCode,
    loginKey,
    loginStatus,
    paneChoices,
    type LoginKey,
    type LoginStatus,
  } from './add_account';

  let {
    login,
    onclose,
    pollMs = 2000,
  }: {
    login: { host: string; profile: string };
    onclose: () => void;
    pollMs?: number;
  } = $props();

  let status = $state<LoginStatus | null>(null);
  let code = $state('');
  let sending = $state(false);
  let stepError = $state<string | null>(null);

  async function poll() {
    const r = await loginStatus(login.host, login.profile);
    if (r.ok) status = r.value;
  }

  $effect(() => {
    if (status?.logged_in) return;
    void poll();
    const t = setInterval(() => void poll(), pollMs);
    return () => clearInterval(t);
  });

  async function press(key: LoginKey) {
    stepError = null;
    const r = await loginKey(login.host, login.profile, key);
    if (!r.ok) stepError = r.error.message;
    void poll();
  }

  async function sendCode() {
    if (!code.trim()) return;
    sending = true;
    stepError = null;
    const r = await loginCode(login.host, login.profile, code);
    sending = false;
    if (r.ok) code = '';
    else stepError = r.error.message;
    void poll();
  }

  async function copy(text: string, what: string) {
    if (await copyText(text)) push({ kind: 'success', message: `${what} copied.` });
  }

  async function finish() {
    await endLogin(login.host, login.profile);
    if (status?.logged_in) {
      push({
        kind: 'success',
        message: `${login.profile} on ${login.host} is signed in${status.email ? ` as ${status.email}` : ''}.`,
      });
    }
    onclose();
  }

  const choices = $derived(paneChoices(status?.pane));
</script>

<Modal label="Sign in" width="560px" testid="add-account-login">
  <div class="sheet">
    <h3>Sign in {login.profile} on {login.host}</h3>
    {#if status?.logged_in}
      <p class="done" data-testid="add-account-signed-in">
        Signed in{status.email ? ` as ${status.email}` : ''}. New sessions on {login.host} can bill it.
      </p>
    {:else}
      <p class="lead">
        Claude's own sign-in runs on {login.host}. Open the link, sign in, and paste the code the page shows.
      </p>
      {#if status?.sign_in_url}
        <div class="row">
          <button type="button" class="btn primary" data-testid="add-account-open-link" onclick={() => status?.sign_in_url && void openExternal(status.sign_in_url)}
            >Open sign-in page</button
          >
          <button type="button" class="btn" data-testid="add-account-copy-link" onclick={() => status?.sign_in_url && void copy(status.sign_in_url, 'Link')}
            >Copy link</button
          >
        </div>
      {/if}
      <pre class="pane" data-testid="add-account-pane">{status?.pane ?? 'Waiting for the login pane…'}</pre>
      <div class="row keys">
        {#each choices as c (c.key)}
          <button type="button" class="btn" data-testid={`add-account-choice-${c.key}`} onclick={() => void press(c.key)}
            >{c.key}. {c.label}</button
          >
        {/each}
        <button type="button" class="btn-quiet" data-testid="add-account-key-up" aria-label="Up" onclick={() => void press('Up')}>↑</button>
        <button type="button" class="btn-quiet" data-testid="add-account-key-down" aria-label="Down" onclick={() => void press('Down')}>↓</button>
        <button type="button" class="btn-quiet" data-testid="add-account-key-enter" onclick={() => void press('Enter')}>Enter</button>
      </div>
      <form
        class="row"
        onsubmit={(e) => {
          e.preventDefault();
          void sendCode();
        }}>
        <input
          class="code"
          class:asked={asksForCode(status?.pane)}
          type="text"
          autocomplete="off"
          spellcheck="false"
          placeholder="Code from the sign-in page"
          aria-label="Sign-in code"
          data-testid="add-account-code"
          bind:value={code} />
        <button type="submit" class="btn" data-testid="add-account-send-code" disabled={sending || !code.trim()}
          >{sending ? 'Sending…' : 'Send code'}</button
        >
      </form>
      {#if stepError}<p class="err" data-testid="add-account-step-error">{stepError}</p>{/if}
      <details class="fallback">
        <summary>Run it there instead</summary>
        <p>In a terminal on {login.host}:</p>
        <div class="row">
          <code data-testid="add-account-command">{status?.command ?? ''}</code>
          <button type="button" class="btn-quiet" data-testid="add-account-copy-command" onclick={() => status && void copy(status.command, 'Command')}
            >Copy</button
          >
        </div>
        <p>This window turns Done on when {login.host} reports the login.</p>
      </details>
    {/if}
    <div class="row">
      <span class="grow"></span>
      {#if !status?.logged_in}
        <button type="button" class="btn" data-testid="add-account-cancel" onclick={() => void finish()}>Cancel</button>
      {/if}
      <button type="button" class="btn primary" data-testid="add-account-done" disabled={!status?.logged_in} onclick={() => void finish()}
        >Done</button
      >
    </div>
  </div>
</Modal>

<style>
  .sheet {
    padding: 1rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  h3 {
    margin: 0;
    font-size: var(--text-md);
  }
  .lead,
  .fallback p {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .done {
    margin: 0;
  }
  .pane {
    margin: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-sunk);
    max-height: 16rem;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .code {
    flex: 1;
    font-family: var(--font-mono);
  }
  .code.asked {
    border-color: var(--accent);
  }
  .err {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-xs);
  }
  .fallback code {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    word-break: break-all;
  }
  .grow {
    flex: 1;
  }
</style>
