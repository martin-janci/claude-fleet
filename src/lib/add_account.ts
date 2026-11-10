// Add account (M15 step G2.9): the Accounts page's + Add account, through
// the `add_account` command (this app's own fleet, or the hub's when
// paired). An API key is checked with Anthropic and written to the host
// only: it lives in the form's answers until the one call that sends it and
// is never kept here. A subscription opens a login pane on the host
// (`fleet-login--<name>`, running `claude /login`) that AddAccountLogin
// reads back and answers until the host reports the login. A setup token
// (what `claude setup-token` prints) is checked with Anthropic and written
// to the host like an API key; sessions under the profile run with it as
// CLAUDE_CODE_OAUTH_TOKEN. The same name again replaces it.
import { invokeCmd, type Result } from './result';
import type { FieldProblem, Values } from './forms/forms';

export type AccountKind = 'subscription' | 'setup_token' | 'api_key';

export interface LoginStatus {
  logged_in: boolean;
  account_uuid?: string;
  email?: string;
  /** The login pane's last lines, while not logged in; absent when no pane. */
  pane?: string;
  /** The sign-in link on screen, when the CLI printed one. */
  sign_in_url?: string;
  /** "Run it there": the command for a terminal on the host. */
  command: string;
}

export interface SetupTokenAdded {
  host_alias: string;
  profile: string;
  account_uuid: string;
  email?: string;
  /** The profile held a token already, now replaced. */
  replaced: boolean;
}

export interface ApiKeyAdded {
  host_alias: string;
  profile: string;
  account_uuid: string;
  daily_limit_usd?: number;
}

/** The keys the login pane takes: the CLI's numbered choices and these. */
export const LOGIN_KEYS = ['Enter', 'Up', 'Down', 'Escape'] as const;
export type LoginKey = (typeof LOGIN_KEYS)[number] | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9';

/** A profile name as the backend takes it (validate::claude_profile). */
export function profileProblem(name: string): string | null {
  if (!name) return 'Name the account.';
  if (name.length > 32) return '32 characters at most.';
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(name)) return 'Letters, digits, _ or -, starting with a letter or digit.';
  return null;
}

/** What the form's answers ask for. */
export interface AddAccountAnswers {
  host: string;
  kind: AccountKind;
  profile: string;
  apiKey: string;
  setupToken: string;
  /** The known account a setup token signs in to; null = none of them. */
  account: string | null;
  dailyLimit: number | null;
}

export function readAnswers(v: Values): AddAccountAnswers {
  const limit = typeof v.daily_limit === 'number' ? v.daily_limit : Number(v.daily_limit);
  return {
    host: String(v.host ?? '').trim(),
    kind: v.kind === 'api_key' || v.kind === 'setup_token' ? v.kind : 'subscription',
    profile: String(v.profile ?? '').trim(),
    apiKey: String(v.api_key ?? '').trim(),
    setupToken: String(v.setup_token ?? '').trim(),
    account: typeof v.account === 'string' && v.account !== '' && v.account !== 'none' ? v.account : null,
    dailyLimit: v.daily_limit === undefined || v.daily_limit === null || v.daily_limit === '' || !Number.isFinite(limit) || limit <= 0 ? null : limit,
  };
}

/** Field problems before anything is sent. */
export function answerProblems(a: AddAccountAnswers): FieldProblem[] {
  const out: FieldProblem[] = [];
  if (!a.host) out.push({ field: 'host', problem: 'Pick a host.' });
  const p = profileProblem(a.profile);
  if (p) out.push({ field: 'profile', problem: p });
  if (a.kind === 'api_key' && !a.apiKey.startsWith('sk-ant-')) {
    out.push({ field: 'api_key', problem: 'An Anthropic API key starts with sk-ant-.' });
  }
  if (a.kind === 'setup_token' && !a.setupToken.startsWith('sk-ant-oat')) {
    out.push({
      field: 'setup_token',
      problem: a.setupToken.startsWith('sk-ant-api')
        ? 'That is an API key; pick API key above, or paste what claude setup-token printed.'
        : 'Paste the token claude setup-token printed (sk-ant-oat…).',
    });
  }
  return out;
}

function call<T>(args: Record<string, unknown>): Promise<Result<T>> {
  return invokeCmd<T>('add_account', { args });
}

export function addApiKey(a: AddAccountAnswers): Promise<Result<ApiKeyAdded>> {
  return call<ApiKeyAdded>({
    action: 'api_key',
    host_alias: a.host,
    profile: a.profile,
    api_key: a.apiKey,
    daily_limit_usd: a.dailyLimit,
  });
}

export function addSetupToken(a: AddAccountAnswers): Promise<Result<SetupTokenAdded>> {
  return call<SetupTokenAdded>({
    action: 'setup_token',
    host_alias: a.host,
    profile: a.profile,
    setup_token: a.setupToken,
    ...(a.account ? { account: a.account } : {}),
  });
}

/** The "Same account as" choices for a setup token: the subscription
 *  accounts fleet knows (not API keys, not other setup tokens), by label. */
export function tokenAccountChoices(accounts: readonly { uuid: string; email: string | null; nickname?: string | null; display_name?: string | null }[]): [string, string][] {
  return accounts
    .filter((a) => !a.uuid.startsWith('apikey-') && !a.uuid.startsWith('setuptoken-'))
    .map((a): [string, string] => [a.uuid, a.nickname || a.email || a.display_name || a.uuid])
    .sort((x, y) => x[1].localeCompare(y[1]));
}

export function startLogin(host: string, profile: string): Promise<Result<{ session: string }>> {
  return call({ action: 'start_login', host_alias: host, profile });
}

export function loginStatus(host: string, profile: string): Promise<Result<LoginStatus>> {
  return call<LoginStatus>({ action: 'login_status', host_alias: host, profile });
}

export function loginKey(host: string, profile: string, key: LoginKey): Promise<Result<{ sent: boolean }>> {
  return call({ action: 'login_key', host_alias: host, profile, key });
}

export function loginCode(host: string, profile: string, code: string): Promise<Result<{ sent: boolean }>> {
  return call({ action: 'login_code', host_alias: host, profile, code: code.trim() });
}

export function endLogin(host: string, profile: string): Promise<Result<{ closed: boolean }>> {
  return call({ action: 'end_login', host_alias: host, profile });
}

export function setDailyLimit(host: string, profile: string, usd: number | null): Promise<Result<unknown>> {
  return call({ action: 'daily_limit', host_alias: host, profile, daily_limit_usd: usd ?? 0 });
}

/** The numbered choices on the pane's screen ("❯ 1. Claude account …"),
 *  for buttons beside the pane. At most nine, in screen order. */
export function paneChoices(pane: string | undefined): { key: LoginKey; label: string }[] {
  if (!pane) return [];
  const out: { key: LoginKey; label: string }[] = [];
  for (const line of pane.split('\n')) {
    const m = /^\s*(?:[❯›>]\s*)?([1-9])\.\s+(.+?)\s*$/.exec(line);
    if (m && !out.some((c) => c.key === m[1])) out.push({ key: m[1] as LoginKey, label: m[2] });
  }
  return out.slice(0, 9);
}

/** Whether the pane is waiting for the code the sign-in page shows. */
export function asksForCode(pane: string | undefined): boolean {
  return !!pane && /paste code/i.test(pane);
}

/** A one-time device code on the pane ("Enter code ABCD-1234"), for the
 *  login pane's Copy code (M15 G7.12). `null` when the CLI shows none. */
export function deviceCode(pane: string | undefined): string | null {
  if (!pane) return null;
  const m = /\bcode\b[^\n]*?\b([A-Z0-9]{4,5}-[A-Z0-9]{4,5})\b/i.exec(pane);
  return m ? m[1].toUpperCase() : null;
}

/** How long the sign-in link lasts: what the pane says ("expires in 15
 *  minutes"), else the 10 minutes Claude's sign-in page gives a link. */
export const LOGIN_LINK_MINUTES = 10;
export function linkMinutes(pane: string | undefined): number {
  const m = pane ? /expires?\s+in\s+(\d{1,3})\s*min/i.exec(pane) : null;
  return m ? Number(m[1]) : LOGIN_LINK_MINUTES;
}

/** The expiry line under the link: minutes left, or that it has run out. */
export function expiryLine(seenAt: number, minutes: number, now: number): { text: string; expired: boolean } {
  const left = Math.ceil((seenAt + minutes * 60_000 - now) / 60_000);
  if (left <= 0) return { text: 'This link has expired. Start over for a new one.', expired: true };
  return { text: `The link expires in ${left} min.`, expired: false };
}
