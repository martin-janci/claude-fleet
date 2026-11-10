<script lang="ts">
  // + Add account (M15 step G2.9). The form asks for the host, the kind
  // (Claude subscription, setup token or API key) and the account's name; a
  // key or setup token is checked with Anthropic and written to the host in
  // one call, and its provider's refusal comes back on the field. A subscription opens a login
  // pane on the host and goes on to the sign-in step (AddAccountLogin).
  import WizardDialog from './forms/WizardDialog.svelte';
  import AddAccountLogin from './AddAccountLogin.svelte';
  import { WIZARDS, withChoices } from './forms/wizards';
  import { hosts } from './hosts';
  import { push } from './toasts';
  import type { IpcError } from './result';
  import type { FieldProblem, Values } from './forms/forms';
  import { addApiKey, addSetupToken, answerProblems, readAnswers, startLogin, tokenAccountChoices } from './add_account';
  import { accounts, accountLabel } from './accounts';

  let {
    onclose,
    host = undefined,
    pollMs = 2000,
  }: {
    onclose: () => void;
    /** Preselect a host (the Hosts page's row). */
    host?: string;
    pollMs?: number;
  } = $props();

  const wizard = $derived({
    ...WIZARDS.add_account,
    spec: withChoices(WIZARDS.add_account.spec, {
      host: $hosts.filter((h) => !h.hidden).map((h): [string, string] => [h.alias, h.alias]),
      account: tokenAccountChoices($accounts, accountLabel),
    }),
  });

  let busy = $state(false);
  let error = $state<IpcError | string | null>(null);
  let problems = $state<FieldProblem[]>([]);
  /** Set once the login pane is open: the sign-in step shows. */
  let login = $state<{ host: string; profile: string } | null>(null);

  function problemsOf(e: IpcError): FieldProblem[] {
    const d = (e as { details?: { problems?: FieldProblem[] } }).details;
    return Array.isArray(d?.problems) ? d.problems : [];
  }

  async function run(v: Values) {
    const a = readAnswers(v);
    problems = answerProblems(a);
    error = null;
    if (problems.length > 0) return;
    busy = true;
    if (a.kind === 'api_key') {
      const r = await addApiKey(a);
      busy = false;
      if (r.ok) {
        push({ kind: 'success', message: `${a.profile} added on ${a.host}. New sessions there can bill it.` });
        onclose();
      } else {
        problems = problemsOf(r.error);
        error = problems.length > 0 ? null : r.error;
      }
      return;
    }
    if (a.kind === 'setup_token') {
      const r = await addSetupToken(a);
      busy = false;
      if (r.ok) {
        const note = r.value.verified ? '' : ' Anthropic did not confirm it yet; the first session there will.';
        push({ kind: 'success', message: `${a.profile} added on ${a.host}. New sessions there can bill it.${note}` });
        onclose();
      } else {
        problems = problemsOf(r.error);
        error = problems.length > 0 ? null : r.error;
      }
      return;
    }
    const r = await startLogin(a.host, a.profile);
    busy = false;
    if (!r.ok) {
      problems = problemsOf(r.error);
      error = problems.length > 0 ? null : r.error;
      return;
    }
    login = { host: a.host, profile: a.profile };
  }
</script>

{#if login}
  <AddAccountLogin {login} {pollMs} {onclose} />
{:else}
  <WizardDialog
    {wizard}
    initial={host ? { host } : {}}
    {busy}
    {error}
    {problems}
    errorTestid="add-account-error"
    {run}
    {onclose} />
{/if}
