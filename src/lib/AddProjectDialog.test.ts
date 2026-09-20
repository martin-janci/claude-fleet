import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import AddProjectDialog from './AddProjectDialog.svelte';
import { hosts } from './hosts';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const mockedInvoke = invoke as ReturnType<typeof vi.fn>;
const mockedOpen = open as ReturnType<typeof vi.fn>;

const TOKEN = 'a'.repeat(64);

const row = {
  project: { id: 7, owner: 'o', repo: 'r', base_path: '/p/o/r', last_session_at: null, adopted: false },
  worktrees: [],
};

type Handler = (args: any) => unknown;

/** Route `invoke` by command; an unrouted command answers null. A handler
 *  that throws becomes an IPC error. */
function route(handlers: Record<string, Handler>) {
  mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    const h = handlers[cmd];
    return h ? h(args) : null;
  });
}

function calls(cmd: string): any[] {
  return mockedInvoke.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]);
}

function deferred<T = unknown>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function chip(alias: string): HTMLButtonElement {
  return Array.from(document.querySelectorAll<HTMLButtonElement>('.host-pick')).find(
    (b) => (b as HTMLElement).dataset.alias === alias,
  )!;
}

function mount(onCreated = vi.fn()) {
  const onCancel = vi.fn();
  render(AddProjectDialog, { props: { onCreated, onCancel } });
  return { onCreated, onCancel };
}

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

async function fillNew(owner: string, repo: string, remote: boolean) {
  await fireEvent.click(screen.getByTestId('add-mode-new'));
  await tick();
  await fireEvent.input(screen.getByTestId('new-owner'), { target: { value: owner } });
  await fireEvent.input(screen.getByTestId('new-repo'), { target: { value: repo } });
  if (remote) await fireEvent.click(screen.getByTestId('new-create-remote'));
  await tick();
}

beforeEach(() => {
  mockedInvoke.mockReset();
  mockedOpen.mockReset();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    { alias: 'hetzner', ssh_alias: 'hetzner', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  fleetSettings.set({ ...SETTING_DEFAULTS });
  localStorage.clear();
});

describe('AddProjectDialog', () => {
  it('renders the dialog root and all four modes', async () => {
    route({});
    mount();
    await tick();
    expect(screen.getByTestId('add-project-dialog')).toBeInTheDocument();
    for (const m of ['clone', 'github', 'folder', 'new']) {
      expect(screen.getByTestId(`add-mode-${m}`)).toBeInTheDocument();
    }
  });

  describe('clone', () => {
    it('a valid URL enables Create and sends {kind:clone, url} with the chosen host', async () => {
      route({ add_project: () => row });
      mount();
      await tick();
      await fireEvent.click(chip('mefistos'));
      const create = screen.getByTestId('add-create') as HTMLButtonElement;
      expect(create.disabled).toBe(true);
      await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'https://github.com/o/r' } });
      await tick();
      expect(create.disabled).toBe(false);
      expect(screen.getByTestId('add-path-preview').textContent).toContain('~/projects/github.com/o/r');
      await fireEvent.click(create);
      await flush();
      const sent = calls('add_project');
      expect(sent).toHaveLength(1);
      expect(sent[0].args.host_alias).toBe('mefistos');
      expect(sent[0].args.source).toEqual({ kind: 'clone', url: 'https://github.com/o/r' });
    });

    it('an unparseable value disables Create and shows why', async () => {
      route({});
      mount();
      await tick();
      await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'not a repo' } });
      await tick();
      expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(true);
      expect(screen.getByTestId('add-url-err').textContent).toContain('Not a GitHub repository');
      const field = screen.getByTestId('clone-url');
      expect(field.getAttribute('aria-invalid')).toBe('true');
      expect(field.getAttribute('aria-describedby')).toBe('add-url-err');
      await fireEvent.keyDown(screen.getByTestId('clone-url'), { key: 'Enter' });
      await flush();
      expect(calls('add_project')).toHaveLength(0);
    });

    it('Enter in the field creates', async () => {
      route({ add_project: () => row });
      const { onCreated } = mount();
      await tick();
      await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
      await fireEvent.keyDown(screen.getByTestId('clone-url'), { key: 'Enter' });
      await flush();
      expect(calls('add_project')).toHaveLength(1);
      expect(onCreated).toHaveBeenCalledWith(row, 'local');
    });
  });

  describe('github', () => {
    it('shows a failing list_github_repos message verbatim and no list', async () => {
      route({
        list_github_repos: () => {
          throw { code: 'E_GH', message: 'gh: not logged in — run gh auth login' };
        },
      });
      mount();
      await tick();
      await fireEvent.click(screen.getByTestId('add-mode-github'));
      await vi.waitFor(() =>
        expect(screen.getByTestId('gh-error').textContent).toBe('gh: not logged in — run gh auth login'),
      );
      expect(screen.queryByTestId('gh-list')).toBeNull();
      expect(screen.getByTestId('gh-error').getAttribute('role')).toBe('alert');
    });

    it('Retry re-runs a failed listing, and the filter box gets focus', async () => {
      let n = 0;
      route({
        list_github_repos: () => {
          if (++n === 1) throw { code: 'E_GH', message: 'gh: network down' };
          return [{ name_with_owner: 'o/alpha', description: null, is_private: false, updated_at: null }];
        },
      });
      mount();
      await tick();
      await fireEvent.click(screen.getByTestId('add-mode-github'));
      await vi.waitFor(() => expect(screen.getByTestId('gh-error')).toBeInTheDocument());
      await fireEvent.click(screen.getByTestId('gh-retry'));
      await vi.waitFor(() => expect(screen.getAllByTestId('gh-repo-row')).toHaveLength(1));
      expect(calls('list_github_repos')).toHaveLength(2);
      expect(document.activeElement).toBe(screen.getByTestId('gh-filter'));
    });

    it('picking a row switches to clone mode prefilled', async () => {
      route({
        list_github_repos: () => [
          { name_with_owner: 'o/alpha', description: 'first', is_private: true, updated_at: null },
          { name_with_owner: 'o/beta', description: null, is_private: false, updated_at: null },
        ],
      });
      mount();
      await tick();
      await fireEvent.click(screen.getByTestId('add-mode-github'));
      await vi.waitFor(() => expect(screen.getAllByTestId('gh-repo-row')).toHaveLength(2));
      await fireEvent.input(screen.getByTestId('gh-filter'), { target: { value: 'bet' } });
      await tick();
      const rows = screen.getAllByTestId('gh-repo-row');
      expect(rows).toHaveLength(1);
      await fireEvent.click(rows[0]);
      await tick();
      expect((screen.getByTestId('clone-url') as HTMLInputElement).value).toBe('o/beta');
      expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(false);
    });

    it("a slow reply for a previous host doesn't overwrite the current one", async () => {
      const slow = deferred();
      route({
        list_github_repos: (a) =>
          a.args.host_alias === 'mefistos'
            ? slow.promise
            : [{ name_with_owner: `${a.args.host_alias}/fast`, description: null, is_private: false, updated_at: null }],
      });
      mount();
      await tick();
      await fireEvent.click(chip('mefistos'));
      await fireEvent.click(screen.getByTestId('add-mode-github'));
      await tick();
      expect(screen.getByTestId('gh-loading')).toBeInTheDocument();
      await fireEvent.click(chip('hetzner'));
      await vi.waitFor(() => expect(screen.getAllByTestId('gh-repo-row')[0].textContent).toContain('hetzner/fast'));
      slow.resolve([{ name_with_owner: 'mefistos/slow', description: null, is_private: false, updated_at: null }]);
      await flush();
      const labels = screen.getAllByTestId('gh-repo-row').map((r) => r.textContent);
      expect(labels).toHaveLength(1);
      expect(labels[0]).toContain('hetzner/fast');
    });
  });

  describe('folder', () => {
    it('disables non-local host chips with a reason', async () => {
      route({});
      mount();
      await tick();
      await fireEvent.click(chip('mefistos'));
      await fireEvent.click(screen.getByTestId('add-mode-folder'));
      await tick();
      expect(chip('mefistos').disabled).toBe(true);
      expect(chip('mefistos').title).toMatch(/local/);
      expect(chip('local').disabled).toBe(false);
      expect(chip('local').getAttribute('aria-pressed')).toBe('true');
    });

    it('sends the chosen path as {kind:folder, path} on local', async () => {
      route({ add_project: () => row });
      mockedOpen.mockResolvedValue('/Users/me/code/thing');
      mount();
      await tick();
      await fireEvent.click(chip('mefistos'));
      await fireEvent.click(screen.getByTestId('add-mode-folder'));
      await tick();
      await fireEvent.click(screen.getByTestId('choose-folder'));
      await flush();
      expect(mockedOpen).toHaveBeenCalledWith({ directory: true, multiple: false });
      expect(screen.getByTestId('add-path-preview').textContent).toContain('/Users/me/code/thing');
      await fireEvent.click(screen.getByTestId('add-create'));
      await flush();
      const sent = calls('add_project');
      expect(sent).toHaveLength(1);
      expect(sent[0].args.host_alias).toBe('local');
      expect(sent[0].args.source).toEqual({ kind: 'folder', path: '/Users/me/code/thing' });
    });

    it('cancelling the native picker sends nothing', async () => {
      route({ add_project: () => row });
      mockedOpen.mockResolvedValue(null);
      mount();
      await tick();
      await fireEvent.click(screen.getByTestId('add-mode-folder'));
      await tick();
      await fireEvent.click(screen.getByTestId('choose-folder'));
      await flush();
      const create = screen.getByTestId('add-create') as HTMLButtonElement;
      expect(create.disabled).toBe(true);
      await fireEvent.click(create);
      await flush();
      expect(calls('add_project')).toHaveLength(0);
    });
  });

  describe('new', () => {
    it('sends create_remote: false by default', async () => {
      route({ add_project: () => row });
      mount();
      await tick();
      await fillNew('o', 'r', false);
      await fireEvent.click(screen.getByTestId('add-create'));
      await flush();
      const sent = calls('add_project');
      expect(sent).toHaveLength(1);
      expect(sent[0].args.source).toEqual({ kind: 'new', owner: 'o', repo: 'r', create_remote: false });
    });

    it('validates owner and repo live', async () => {
      route({});
      mount();
      await tick();
      await fillNew('-bad', 'r', false);
      expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(true);
      expect(screen.getByTestId('add-owner-err').textContent).toMatch(/Owner/);
      expect(screen.getByTestId('new-owner').getAttribute('aria-invalid')).toBe('true');
      expect(screen.getByTestId('new-owner').getAttribute('aria-describedby')).toBe('add-owner-err');
      expect(screen.getByTestId('new-repo').getAttribute('aria-invalid')).toBeNull();
      await fireEvent.input(screen.getByTestId('new-owner'), { target: { value: 'o' } });
      await fireEvent.input(screen.getByTestId('new-repo'), { target: { value: '..' } });
      await tick();
      expect(screen.queryByTestId('add-owner-err')).toBeNull();
      expect(screen.getByTestId('add-repo-err').textContent).toMatch(/Repository/);
      expect(screen.getByTestId('new-repo').getAttribute('aria-describedby')).toBe('add-repo-err');
    });

    describe('with "create on GitHub"', () => {
      function confirmRoute(second: () => unknown) {
        let n = 0;
        route({
          add_project: (a) => {
            n++;
            if (n === 1) {
              throw {
                code: 'E_CONFIRM_REQUIRED',
                message: 'creating o/r on GitHub needs confirmation; retry with the returned token',
                details: { confirm: TOKEN },
              };
            }
            void a;
            return second();
          },
        });
      }

      it('the first Create asks for confirmation naming the repo, with no confirm sent', async () => {
        confirmRoute(() => row);
        const { onCreated } = mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        const sent = calls('add_project');
        expect(sent).toHaveLength(1);
        expect(sent[0].args.source).toEqual({ kind: 'new', owner: 'o', repo: 'r', create_remote: true });
        expect('confirm' in sent[0].args.source).toBe(false);
        const dlg = screen.getByTestId('confirm-dialog');
        expect(dlg.textContent).toContain('o/r');
        expect(dlg.textContent).toMatch(/private/i);
        expect(dlg.textContent).toMatch(/pushes the initial commit/);
        expect(onCreated).not.toHaveBeenCalled();
      });

      it('confirming resends the identical request with the exact token', async () => {
        confirmRoute(() => row);
        const { onCreated } = mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(chip('mefistos'));
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        await fireEvent.click(screen.getByTestId('confirm-create-remote'));
        await flush();
        const sent = calls('add_project');
        expect(sent).toHaveLength(2);
        expect(sent[1].args.host_alias).toBe('mefistos');
        expect(sent[1].args.source).toEqual({ kind: 'new', owner: 'o', repo: 'r', create_remote: true, confirm: TOKEN });
        expect(onCreated).toHaveBeenCalledWith(row, 'mefistos');
      });

      it('declining sends nothing further and keeps the form', async () => {
        confirmRoute(() => row);
        mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        await fireEvent.click(screen.getByTestId('confirm-cancel'));
        await flush();
        expect(screen.queryByTestId('confirm-dialog')).toBeNull();
        expect(calls('add_project')).toHaveLength(1);
        expect((screen.getByTestId('new-owner') as HTMLInputElement).value).toBe('o');
        expect((screen.getByTestId('new-create-remote') as HTMLInputElement).checked).toBe(true);
      });

      it('a confirmed resend rejected again does not re-prompt or retry', async () => {
        route({
          add_project: () => {
            throw { code: 'E_CONFIRM_REQUIRED', message: 'needs confirmation', details: { confirm: TOKEN } };
          },
        });
        mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        await fireEvent.click(screen.getByTestId('confirm-create-remote'));
        await flush();
        expect(calls('add_project')).toHaveLength(2);
        expect(screen.queryByTestId('confirm-dialog')).toBeNull();
        expect(screen.getByTestId('add-error').textContent).toMatch(/confirmation expired/);
      });

      it('editing the form under the confirmation does not change the resent request', async () => {
        confirmRoute(() => row);
        mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        expect(screen.getByTestId('confirm-dialog')).toBeInTheDocument();
        await fireEvent.input(screen.getByTestId('new-owner'), { target: { value: 'x' } });
        await fireEvent.click(screen.getByTestId('new-create-remote'));
        await tick();
        await fireEvent.click(screen.getByTestId('confirm-create-remote'));
        await flush();
        const sent = calls('add_project');
        expect(sent).toHaveLength(2);
        expect(sent[1].args.source).toEqual({ kind: 'new', owner: 'o', repo: 'r', create_remote: true, confirm: TOKEN });
      });

      it('a double click on Confirm sends one request', async () => {
        const inflight = deferred();
        confirmRoute(() => inflight.promise);
        mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        const confirm = screen.getByTestId('confirm-create-remote');
        confirm.click();
        confirm.click();
        await flush();
        expect(calls('add_project')).toHaveLength(2);
        inflight.resolve(row);
        await flush();
      });

      it('in flight on local, the visible note warns about GitHub and describes the button', async () => {
        const inflight = deferred();
        confirmRoute(() => inflight.promise);
        mount();
        await tick();
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        await fireEvent.click(screen.getByTestId('confirm-create-remote'));
        await flush();
        const btn = screen.getByTestId('cancel-create');
        expect(btn.textContent?.trim()).toBe('Cancel');
        const note = screen.getByTestId('add-inflight-note');
        expect(note.textContent).toMatch(/GitHub repository may already have been created/);
        expect(note.textContent).not.toMatch(/local/);
        expect(btn.getAttribute('aria-describedby')).toBe(note.id);
        inflight.resolve(row);
        await flush();
      });

      it('E_CANCELLED on the create_remote run shows the backend message instead of closing', async () => {
        const inflight = deferred();
        confirmRoute(() => inflight.promise);
        const { onCreated, onCancel } = mount();
        await tick();
        await fireEvent.click(chip('mefistos'));
        await fillNew('o', 'r', true);
        await fireEvent.click(screen.getByTestId('add-create'));
        await flush();
        await fireEvent.click(screen.getByTestId('confirm-create-remote'));
        await flush();
        const stop = screen.getByTestId('cancel-create');
        expect(stop.textContent?.trim()).toBe('Stop waiting');
        await fireEvent.click(stop);
        const msg = 'ssh cancelled — the GitHub repository o/r may already exist; check GitHub before retrying.';
        inflight.reject({ code: 'E_CANCELLED', message: msg });
        await flush();
        expect(screen.getByTestId('add-error').textContent).toBe(msg);
        expect(screen.getByTestId('add-project-dialog')).toBeInTheDocument();
        expect(onCreated).not.toHaveBeenCalled();
        expect(onCancel).not.toHaveBeenCalled();
      });
    });
  });

  it('a failed create shows the message and keeps every field filled', async () => {
    route({
      add_project: () => {
        throw { code: 'E_GH', message: 'gh repo create failed: HTTP 422' };
      },
    });
    const { onCreated } = mount();
    await tick();
    await fillNew('o', 'r', false);
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(screen.getByTestId('add-error').textContent).toBe('gh repo create failed: HTTP 422');
    expect((screen.getByTestId('new-owner') as HTMLInputElement).value).toBe('o');
    expect((screen.getByTestId('new-repo') as HTMLInputElement).value).toBe('r');
    expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(false);
    expect(onCreated).not.toHaveBeenCalled();
  });

  it('in flight on local the button is "Cancel", fires cancel_command with callId, then shows Stopping…', async () => {
    const inflight = deferred();
    route({ add_project: () => inflight.promise });
    mount();
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await tick();
    const btn = screen.getByTestId('cancel-create') as HTMLButtonElement;
    expect(btn.textContent?.trim()).toBe('Cancel');
    // A local clone has nothing extra to warn about.
    expect(screen.queryByTestId('add-inflight-note')).toBeNull();
    expect(btn.hasAttribute('aria-describedby')).toBe(false);
    await fireEvent.click(btn);
    await tick();
    expect(btn.textContent?.trim()).toBe('Stopping…');
    expect(btn.disabled).toBe(true);
    const cancel = calls('cancel_command');
    expect(cancel).toHaveLength(1);
    const callId = calls('add_project')[0].args.call_id;
    expect(cancel[0]).toEqual({ callId });
    inflight.reject({ code: 'E_CANCELLED', message: 'local script cancelled' });
    await flush();
    expect(screen.queryByTestId('cancel-create')).toBeNull();
  });

  it('in flight on a remote host the button is "Stop waiting" with a visible note naming the host', async () => {
    const inflight = deferred();
    route({ add_project: () => inflight.promise });
    mount();
    await tick();
    await fireEvent.click(chip('mefistos'));
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await tick();
    const btn = screen.getByTestId('cancel-create');
    expect(btn.textContent?.trim()).toBe('Stop waiting');
    const note = screen.getByTestId('add-inflight-note');
    expect(note.textContent).toBe('mefistos may still finish the clone after you stop waiting.');
    expect(note.textContent).not.toMatch(/GitHub/);
    expect(btn.getAttribute('aria-describedby')).toBe(note.id);
    await fireEvent.click(btn);
    await tick();
    expect(calls('cancel_command')).toHaveLength(1);
    expect(calls('cancel_command')[0]).toHaveProperty('callId');
    inflight.resolve(row);
    await flush();
  });

  it('on success onCreated receives the returned row', async () => {
    route({ add_project: () => row });
    const { onCreated } = mount();
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'git@github.com:o/r.git' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(onCreated).toHaveBeenCalledOnce();
    expect(onCreated).toHaveBeenCalledWith(row, 'local');
  });

  it('folder mode reports local as the host even when another chip was chosen', async () => {
    route({ add_project: () => row });
    mockedOpen.mockResolvedValue('/Users/me/code/thing');
    const { onCreated } = mount();
    await tick();
    await fireEvent.click(chip('mefistos'));
    await fireEvent.click(screen.getByTestId('add-mode-folder'));
    await fireEvent.click(screen.getByTestId('choose-folder'));
    await flush();
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(onCreated).toHaveBeenCalledWith(row, 'local');
  });

  it('a double click on Create sends one request', async () => {
    const inflight = deferred();
    route({ add_project: () => inflight.promise });
    mount();
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    const create = screen.getByTestId('add-create');
    create.click();
    create.click();
    await flush();
    expect(calls('add_project')).toHaveLength(1);
    inflight.resolve(row);
    await flush();
  });

  it('unmounting mid-create aborts the request', async () => {
    const inflight = deferred();
    route({ add_project: () => inflight.promise });
    const onCreated = vi.fn();
    const r = render(AddProjectDialog, { props: { onCreated, onCancel: vi.fn() } });
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await tick();
    r.unmount();
    expect(calls('cancel_command')).toHaveLength(1);
    inflight.resolve(row);
    await flush();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it('while busy the fields, modes and host chips are disabled', async () => {
    const inflight = deferred();
    route({ add_project: () => inflight.promise });
    mount();
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await tick();
    expect((screen.getByTestId('clone-url') as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByTestId('add-mode-new') as HTMLButtonElement).disabled).toBe(true);
    expect(chip('local').disabled).toBe(true);
    expect(chip('mefistos').disabled).toBe(true);
    inflight.resolve(row);
    await flush();
    expect((screen.getByTestId('clone-url') as HTMLInputElement).disabled).toBe(false);
  });

  it('a native <dialog> close while busy leaves the dialog open', async () => {
    const inflight = deferred();
    route({ add_project: () => inflight.promise });
    const { onCancel } = mount();
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await tick();
    const dlg = screen.getByTestId('add-project-dialog') as HTMLDialogElement;
    dlg.removeAttribute('open');
    dlg.dispatchEvent(new Event('close'));
    await flush();
    expect(dlg.hasAttribute('open')).toBe(true);
    expect(onCancel).not.toHaveBeenCalled();
    inflight.reject({ code: 'E_GH', message: 'boom' });
    await flush();
    expect(screen.getByTestId('add-error').textContent).toBe('boom');
  });

  it('errors are announced: add-error has role=alert', async () => {
    route({ add_project: () => { throw { code: 'E_GH', message: 'nope' }; } });
    mount();
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(screen.getByTestId('add-error').getAttribute('role')).toBe('alert');
  });

  describe('mode control', () => {
    it('marks the active mode and host with aria-pressed', async () => {
      route({});
      mount();
      await tick();
      expect(screen.getByTestId('add-mode-clone').getAttribute('aria-pressed')).toBe('true');
      expect(screen.getByTestId('add-mode-new').getAttribute('aria-pressed')).toBe('false');
      expect(chip('local').getAttribute('aria-pressed')).toBe('true');
      expect(chip('mefistos').getAttribute('aria-pressed')).toBe('false');
      expect(screen.getByRole('group', { name: 'Host' })).toBeInTheDocument();
    });

    it('Left/Right arrows move between modes and keep focus on the control', async () => {
      route({});
      mount();
      await tick();
      const clone = screen.getByTestId('add-mode-clone');
      clone.focus();
      await fireEvent.keyDown(clone, { key: 'ArrowRight' });
      await tick();
      expect(screen.getByTestId('add-mode-github').getAttribute('aria-pressed')).toBe('true');
      expect(document.activeElement).toBe(screen.getByTestId('add-mode-github'));
      await fireEvent.keyDown(document.activeElement!, { key: 'ArrowLeft' });
      await fireEvent.keyDown(document.activeElement!, { key: 'ArrowLeft' });
      await tick();
      expect(screen.getByTestId('add-mode-new').getAttribute('aria-pressed')).toBe('true');
      expect(document.activeElement).toBe(screen.getByTestId('add-mode-new'));
    });
  });
});
