// M15 step G2.9: + Add account. An API key goes to the host in one call and
// a refusal lands on its field; a subscription opens the login pane, shows
// its link, choices and code field, and turns Done on once the host reports
// the login.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AddAccountDialog from './AddAccountDialog.svelte';
import { hosts } from './hosts';
import { toasts } from './toasts';
import { accounts } from './accounts';
import { answerProblems, asksForCode, paneChoices, profileProblem, readAnswers, tokenAccountChoices } from './add_account';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const KEY = 'sk-ant-api03-abcdefghijklmnopqrstuvwxyz';
const TOKEN = 'sk-ant-oat01-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123';

let loggedIn = false;
let calls: Record<string, unknown>[] = [];

beforeEach(() => {
  loggedIn = false;
  calls = [];
  hosts.set([{ alias: 'mercury', hidden: false } as never]);
  accounts.set([
    { uuid: 'u-ada', email: 'ada@x.com', nickname: null } as never,
    { uuid: 'apikey-0123456789abcdef', email: null, nickname: 'Team API' } as never,
  ]);
  toasts.set([]);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd !== 'add_account') return null;
    const a = (args as { args: Record<string, unknown> }).args;
    calls.push(a);
    if (a.action === 'api_key') {
      if (a.api_key === 'sk-ant-revoked-0000000000000') {
        throw {
          code: 'E_INVALID',
          message: 'Anthropic rejected this key (401: invalid x-api-key). Check that it is not revoked.',
          details: { problems: [{ field: 'api_key', problem: 'Anthropic rejected this key (401: invalid x-api-key). Check that it is not revoked.' }] },
        };
      }
      return { host_alias: a.host_alias, profile: a.profile, account_uuid: 'apikey-0123456789abcdef', daily_limit_usd: a.daily_limit_usd };
    }
    if (a.action === 'setup_token') {
      return { host_alias: a.host_alias, profile: a.profile, account_uuid: a.account ?? 'setuptoken-0123456789abcdef', replaced: a.profile === 'old' };
    }
    if (a.action === 'start_login') return { session: `fleet-login--${a.profile}` };
    if (a.action === 'login_status') {
      return loggedIn
        ? { logged_in: true, account_uuid: 'u1', email: 'w@x.com', command: 'CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude /login' }
        : {
            logged_in: false,
            pane: 'Select login method:\n❯ 1. Claude account with subscription\n  2. Anthropic Console account\nhttps://claude.ai/oauth/authorize?x=1\nPaste code here if prompted >',
            sign_in_url: 'https://claude.ai/oauth/authorize?x=1',
            command: 'CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude /login',
          };
    }
    return { sent: true, closed: true };
  });
});

async function fill(kind: 'subscription' | 'api_key', profile: string, key?: string, limit?: string) {
  await fireEvent.click(screen.getByTestId(`form-field-kind-${kind}`));
  await fireEvent.input(screen.getByTestId('form-field-profile'), { target: { value: profile } });
  if (key !== undefined) await fireEvent.input(screen.getByTestId('form-field-api_key'), { target: { value: key } });
  if (limit !== undefined) await fireEvent.input(screen.getByTestId('form-field-daily_limit'), { target: { value: limit } });
  await fireEvent.click(screen.getByTestId('form-submit'));
}

describe('+ Add account', () => {
  it('adds an API key with its daily limit in one call', async () => {
    const onclose = vi.fn();
    render(AddAccountDialog, { props: { onclose, host: 'mercury' } });
    await fill('api_key', 'team', KEY, '25');
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls).toEqual([
      { action: 'api_key', host_alias: 'mercury', profile: 'team', api_key: KEY, daily_limit_usd: 25 },
    ]);
  });

  it('leaves the daily limit out when its field is empty, so the account keeps its limit', async () => {
    const onclose = vi.fn();
    render(AddAccountDialog, { props: { onclose, host: 'mercury' } });
    await fill('api_key', 'team', KEY);
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls).toEqual([{ action: 'api_key', host_alias: 'mercury', profile: 'team', api_key: KEY }]);
  });

  it('sends an explicit 0 to remove the limit', async () => {
    const onclose = vi.fn();
    render(AddAccountDialog, { props: { onclose, host: 'mercury' } });
    await fill('api_key', 'team', KEY, '0');
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls).toEqual([
      { action: 'api_key', host_alias: 'mercury', profile: 'team', api_key: KEY, daily_limit_usd: 0 },
    ]);
  });

  it("shows the provider's 401 on the key field", async () => {
    const onclose = vi.fn();
    render(AddAccountDialog, { props: { onclose, host: 'mercury' } });
    await fill('api_key', 'team', 'sk-ant-revoked-0000000000000');
    await waitFor(() => expect(document.body.textContent).toContain('Anthropic rejected this key (401: invalid x-api-key)'));
    expect(onclose).not.toHaveBeenCalled();
  });

  it('signs a subscription in through the login pane', async () => {
    const onclose = vi.fn();
    render(AddAccountDialog, { props: { onclose, host: 'mercury', pollMs: 20 } });
    await fill('subscription', 'work');
    await waitFor(() => expect(screen.getByTestId('add-account-login')).toBeTruthy());
    expect(calls[0]).toEqual({ action: 'start_login', host_alias: 'mercury', profile: 'work' });
    await waitFor(() => expect(screen.getByTestId('add-account-open-link')).toBeTruthy());
    expect((screen.getByTestId('add-account-done') as HTMLButtonElement).disabled).toBe(true);

    await fireEvent.click(screen.getByTestId('add-account-choice-1'));
    await waitFor(() => expect(calls).toContainEqual({ action: 'login_key', host_alias: 'mercury', profile: 'work', key: '1' }));

    await fireEvent.input(screen.getByTestId('add-account-code'), { target: { value: ' abc#def ' } });
    await fireEvent.click(screen.getByTestId('add-account-send-code'));
    await waitFor(() => expect(calls).toContainEqual({ action: 'login_code', host_alias: 'mercury', profile: 'work', code: 'abc#def' }));
    expect(screen.getByTestId('add-account-command').textContent).toContain('claude /login');

    loggedIn = true;
    await waitFor(() => expect(screen.getByTestId('add-account-signed-in').textContent).toContain('w@x.com'));
    // Review w19-9: the sign-in is announced and focus lands on Done.
    expect(screen.getByTestId('add-account-signed-in').getAttribute('role')).toBe('status');
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('add-account-done')));
    await fireEvent.click(screen.getByTestId('add-account-done'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls).toContainEqual({ action: 'end_login', host_alias: 'mercury', profile: 'work' });
  });

  it('checks the answers before sending anything', async () => {
    render(AddAccountDialog, { props: { onclose: () => {}, host: 'mercury' } });
    await fill('api_key', 'bad name', 'nope');
    await waitFor(() => expect(document.body.textContent).toContain('Letters, digits, _ or -'));
    expect(calls).toEqual([]);
  });
});

describe('+ Add account · setup token', () => {
  async function token(profile: string, value: string, account?: string) {
    await fireEvent.click(screen.getByTestId('form-field-kind-setup_token'));
    await fireEvent.input(screen.getByTestId('form-field-profile'), { target: { value: profile } });
    await fireEvent.input(screen.getByTestId('form-field-setup_token'), { target: { value } });
    if (account) await fireEvent.click(screen.getByTestId(`form-field-account-${account}`));
    await fireEvent.click(screen.getByTestId('form-submit'));
  }

  it('sends the token in one call, under the account picked', async () => {
    const onclose = vi.fn();
    render(AddAccountDialog, { props: { onclose, host: 'mercury' } });
    await token('laptop', ` ${TOKEN} `, 'u-ada');
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls).toEqual([{ action: 'setup_token', host_alias: 'mercury', profile: 'laptop', setup_token: TOKEN, account: 'u-ada' }]);
  });

  it('offers only subscription accounts, and none by default', async () => {
    render(AddAccountDialog, { props: { onclose: () => {}, host: 'mercury' } });
    await fireEvent.click(screen.getByTestId('form-field-kind-setup_token'));
    expect(screen.getByTestId('form-field-account-u-ada')).toBeTruthy();
    expect(screen.queryByTestId('form-field-account-apikey-0123456789abcdef')).toBeNull();
    await fireEvent.input(screen.getByTestId('form-field-profile'), { target: { value: 'tok' } });
    await fireEvent.input(screen.getByTestId('form-field-setup_token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(calls.length).toBe(1));
    expect(calls[0]).not.toHaveProperty('account');
  });

  it('says when a token was replaced', async () => {
    render(AddAccountDialog, { props: { onclose: () => {}, host: 'mercury', kind: 'setup_token', profile: 'old' } });
    await fireEvent.input(screen.getByTestId('form-field-setup_token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(JSON.stringify(get(toasts))).toContain('is replaced'));
    expect(calls[0]).toMatchObject({ action: 'setup_token', profile: 'old' });
  });

  it('catches an API key pasted as a token before sending', async () => {
    render(AddAccountDialog, { props: { onclose: () => {}, host: 'mercury' } });
    await token('tok', KEY);
    await waitFor(() => expect(document.body.textContent).toContain('That is an API key'));
    expect(calls).toEqual([]);
  });
});

describe('add_account helpers', () => {
  it('reads the answers', () => {
    expect(readAnswers({ host: 'm', kind: 'api_key', profile: ' p ', api_key: ' k ', daily_limit: 0 })).toEqual({
      host: 'm',
      kind: 'api_key',
      profile: 'p',
      apiKey: 'k',
      setupToken: '',
      account: null,
      dailyLimit: 0,
    });
    expect(readAnswers({ host: 'm', kind: 'api_key', profile: 'p', daily_limit: '' }).dailyLimit).toBeNull();
    expect(readAnswers({ host: 'm', kind: 'setup_token', profile: 'p', setup_token: ' t ', account: 'none' })).toMatchObject({
      kind: 'setup_token',
      setupToken: 't',
      account: null,
    });
    expect(readAnswers({ kind: 'setup_token', account: 'u1' }).account).toBe('u1');
    expect(
      tokenAccountChoices([
        { uuid: 'u2', email: 'b@x' },
        { uuid: 'setuptoken-1', email: null, display_name: 'Setup token · t' },
        { uuid: 'u1', email: 'a@x', nickname: 'Ada' },
      ]),
    ).toEqual([
      ['u1', 'Ada'],
      ['u2', 'b@x'],
    ]);
    expect(readAnswers({ host: 'm', profile: 'p', daily_limit: 5 }).kind).toBe('subscription');
    expect(profileProblem('a'.repeat(33))).toBe('32 characters at most.');
    expect(profileProblem('-x')).not.toBeNull();
    expect(answerProblems(readAnswers({ host: 'm', kind: 'subscription', profile: 'work' }))).toEqual([]);
  });

  it("reads the pane's numbered choices and its code prompt", () => {
    expect(paneChoices('Pick:\n❯ 1. Claude account with subscription\n  2. Anthropic Console account\n 10. nope')).toEqual([
      { key: '1', label: 'Claude account with subscription' },
      { key: '2', label: 'Anthropic Console account' },
    ]);
    expect(asksForCode('Paste code here if prompted >')).toBe(true);
    expect(asksForCode(undefined)).toBe(false);
  });
});
