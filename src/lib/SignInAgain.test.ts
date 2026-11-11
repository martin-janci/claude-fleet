// Sign in again (login health): a signed-out `/login` gets the login pane
// over the login that is there, every call marked `again`; the host's own
// login goes as profile ''; a setup-token profile gets + Add account preset
// to a new token instead, and no login pane.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SignInAgain from './SignInAgain.svelte';
import { hosts } from './hosts';
import { toasts } from './toasts';
import { get } from 'svelte/store';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
let calls: Record<string, unknown>[] = [];
let signedIn = false;

beforeEach(() => {
  calls = [];
  signedIn = false;
  hosts.set([{ alias: 'htz', hidden: false } as never]);
  toasts.set([]);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd !== 'add_account') return null;
    const a = (args as { args: Record<string, unknown> }).args;
    calls.push(a);
    if (a.action === 'start_login') return { session: `fleet-login--${a.profile || '_own'}` };
    if (a.action === 'login_status') {
      return signedIn
        ? { logged_in: true, account_uuid: 'u1', email: 'w@x.com', command: 'claude /login' }
        : { logged_in: false, pane: 'Paste code here if prompted >', command: 'claude /login' };
    }
    return { closed: true };
  });
});

describe('SignInAgain', () => {
  it("signs the host's own login in again through the login pane", async () => {
    const onclose = vi.fn();
    render(SignInAgain, { host: 'htz', profile: null, onclose, pollMs: 20 });
    await screen.findByTestId('add-account-login');
    expect(screen.getByText('Sign the host login in again on htz')).toBeTruthy();
    expect(calls[0]).toMatchObject({ action: 'start_login', host_alias: 'htz', profile: '', again: true });
    signedIn = true;
    await waitFor(() => expect(screen.getByTestId('add-account-signed-in')).toBeTruthy());
    expect(calls.filter((c) => c.action === 'login_status').every((c) => c.again === true)).toBe(true);
    await fireEvent.click(screen.getByTestId('add-account-done'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls.at(-1)).toMatchObject({ action: 'end_login', profile: '', again: true });
    expect(get(toasts).some((t) => t.message.includes('the host login on htz is signed in again'))).toBe(true);
  });

  it('a refused start says why and closes', async () => {
    invoke.mockImplementation(async () => {
      throw { code: 'E_INVALID', message: 'api on htz uses an API key: add a new key to it' };
    });
    const onclose = vi.fn();
    render(SignInAgain, { host: 'htz', profile: 'api', onclose });
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(get(toasts).some((t) => t.message.includes('uses an API key'))).toBe(true);
  });

  it('a setup-token profile gets + Add account for a new token, no login pane', async () => {
    render(SignInAgain, { host: 'htz', profile: 'tok', token: true, onclose: vi.fn() });
    await screen.findByTestId('form-field-setup_token');
    expect(calls.some((c) => c.action === 'start_login')).toBe(false);
  });
});
