import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ChatForm from './ChatForm.svelte';
import { addProjectSources, runAddProject } from './add_project_wizard';
import { WIZARDS, withChoices } from './wizards';
import { optionsOfField } from './forms';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const HOSTS: [string, string][] = [
  ['mercury', 'mercury'],
  ['mac', 'mac'],
];
const REPOS: [string, string][] = [
  ['acme/papaya-pos', 'papaya-pos'],
  ['acme/papaya-receipts', 'papaya-receipts'],
];

function row(name: string) {
  const [owner, repo] = name.split('/');
  return { project: { id: 1, owner, repo, base_path: `/p/${repo}`, last_session_at: null, adopted: false, system: false }, worktrees: [] };
}

beforeEach(() => {
  inv.mockReset();
});

describe('the Add project wizard (redesign step 10.12)', () => {
  it('opens with the fleet’s hosts and an owner’s repositories in place of the examples', () => {
    const spec = withChoices(WIZARDS.add_project.spec, { host: HOSTS, repos: REPOS });
    const fields = spec.steps.flatMap((s) => s.fields);
    expect(fields.find((f) => f.name === 'host')?.options).toEqual(HOSTS);
    expect(fields.find((f) => f.name === 'repos')?.options).toEqual(REPOS);
    expect(fields.find((f) => f.name === 'source')?.value).toBe('github');
  });

  it('has no From GitHub when there are no repositories to offer', () => {
    const spec = withChoices(WIZARDS.add_project.spec, { host: HOSTS, repos: [] });
    expect(spec.steps.map((s) => s.title)).toEqual(['Source', 'Clone', 'Folder', 'New repo']);
    const source = spec.steps[0].fields.find((f) => f.name === 'source')!;
    expect(optionsOfField(source).map((o) => o.value)).toEqual(['clone', 'folder', 'new']);
    expect(source.value).toBeUndefined();
  });

  it('reads each source as the add_project calls it makes', () => {
    expect(addProjectSources({ source: 'github', host: 'mac', repos: ['acme/a', 'acme/b'] })).toEqual({
      host: 'mac',
      sources: [
        { kind: 'clone', url: 'acme/a' },
        { kind: 'clone', url: 'acme/b' },
      ],
    });
    expect(addProjectSources({ source: 'clone', host: 'mac', url: ' acme/a ' }).sources).toEqual([{ kind: 'clone', url: 'acme/a' }]);
    expect(addProjectSources({ source: 'folder', host: 'mac', path: '~/x' }).sources).toEqual([{ kind: 'folder', path: '~/x' }]);
    // Never on GitHub from the chat: that needs the dialog's confirmation.
    expect(addProjectSources({ source: 'new', host: 'mac', owner: 'acme', repo: 'z' }).sources).toEqual([
      { kind: 'new', owner: 'acme', repo: 'z', create_remote: false },
    ]);
  });

  it('stops at the first failure and says what was already added', async () => {
    inv.mockImplementation(async (_cmd: string, a: { args: { source: { url: string } } }) => {
      if (a.args.source.url === 'acme/b') throw { code: 'E_GH', message: 'gh: not found' };
      return row(a.args.source.url);
    });
    const r = await runAddProject({ source: 'github', host: 'mac', repos: ['acme/a', 'acme/b', 'acme/c'] });
    expect(r).toEqual({ ok: false, error: 'Added acme/a. gh: not found' });
    expect(inv.mock.calls.filter((c) => c[0] === 'add_project')).toHaveLength(2);
  });

  it('adds nothing until the last button, then every ticked repository, in the chat', async () => {
    inv.mockImplementation(async (_cmd: string, a: { args: { source: { url: string } } }) => row(a.args.source.url));
    const spec = withChoices(WIZARDS.add_project.spec, { host: HOSTS, repos: REPOS });
    render(ChatForm, { props: { spec, from: 'Control', sending: WIZARDS.add_project.sending, onsubmit: runAddProject } });
    await fireEvent.click(screen.getByTestId('form-field-host-mercury'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-field-repos-acme/papaya-pos'));
    await fireEvent.click(screen.getByTestId('form-field-repos-acme/papaya-receipts'));
    expect(inv).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect((await screen.findByTestId('chat-form-summary')).textContent).toBe(
      'acme/papaya-pos, acme/papaya-receipts on mercury',
    );
    expect(inv.mock.calls.map((c) => c[1])).toMatchObject([
      { args: { host_alias: 'mercury', source: { kind: 'clone', url: 'acme/papaya-pos' } } },
      { args: { host_alias: 'mercury', source: { kind: 'clone', url: 'acme/papaya-receipts' } } },
    ]);
  });
});
