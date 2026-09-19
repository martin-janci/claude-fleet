import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { open as mockedOpen } from '@tauri-apps/plugin-dialog';
import AssetEditor from './AssetEditor.svelte';
import type { EditableAsset } from './assets';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const open = mockedOpen as ReturnType<typeof vi.fn>;

function byCmd(map: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in map) {
      const v = map[cmd];
      if (v instanceof Error || (v && typeof v === 'object' && 'code' in v)) throw v;
      return v;
    }
    throw { code: 'E_TEST', message: `unexpected ${cmd}` };
  });
}

const skillAsset = (over: Partial<EditableAsset> = {}): EditableAsset => ({
  kind: 'skill', name: 'worktree', version: '1', description: 'Create an isolated git worktree.',
  tags: ['core'], body: '# worktree\n\nSteps.', resources: [],
  allowed_tools: ['bash', 'read'], user_invocable: true, triggers: ['new worktree'],
  ...over,
});

const agentAsset = (over: Partial<EditableAsset> = {}): EditableAsset => ({
  kind: 'agent', name: 'reviewer', version: '1', description: 'Reviews code changes.',
  tags: [], body: 'You are a reviewer.', resources: [],
  tools: ['read', 'grep'], model: 'default',
  ...over,
});

const hookAsset = (over: Partial<EditableAsset> = {}): EditableAsset => ({
  kind: 'hook', name: 'on-stop', version: '1', description: 'Runs on stop.',
  tags: [], body: '', resources: [],
  event: 'stop', action: { type: 'command', command: 'echo hook' },
  ...over,
});

const mcpAsset = (over: Partial<EditableAsset> = {}): EditableAsset => ({
  kind: 'mcp_server', name: 'fleet', version: '1', description: 'Fleet control API.',
  tags: [], body: '', resources: [],
  transport: 'http', url: 'http://127.0.0.1:1234/mcp', command: undefined, args: [], env: {},
  ...over,
});

const pluginAsset = (over: Partial<EditableAsset> = {}): EditableAsset => ({
  kind: 'plugin_ref', name: 'superpowers', version: 'latest', description: 'Superpowers plugin.',
  tags: [], body: '', resources: [],
  harness: 'claude', marketplace: { name: 'sp-marketplace', source: 'github', repo: 'obra/sp' }, plugin: 'superpowers',
  ...over,
});

beforeEach(() => {
  invoke.mockReset();
  open.mockReset();
});

describe('AssetEditor', () => {
  it('renders header fields prefilled, with a read-only name', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    expect((screen.getByTestId('editor-name') as HTMLInputElement).value).toBe('worktree');
    expect((screen.getByTestId('editor-name') as HTMLInputElement).readOnly).toBe(true);
    expect((screen.getByTestId('editor-description') as HTMLTextAreaElement).value).toContain('isolated git worktree');
    expect((screen.getByTestId('editor-version') as HTMLInputElement).value).toBe('1');
    expect((screen.getByTestId('editor-tags') as HTMLInputElement).value).toBe('core');
    expect((screen.getByTestId('editor-body') as HTMLTextAreaElement).value).toContain('Steps.');
  });

  it('Save is disabled when nothing changed, and enables once a field is edited', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    expect(screen.getByTestId('editor-save')).toBeDisabled();
    await fireEvent.input(screen.getByTestId('editor-version'), { target: { value: '2' } });
    expect(screen.getByTestId('editor-save')).not.toBeDisabled();
  });

  it('Save is disabled when the description is cleared (client error)', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.input(screen.getByTestId('editor-description'), { target: { value: '' } });
    expect(screen.getByTestId('editor-save')).toBeDisabled();
    expect(screen.getByTestId('editor-client-errors').textContent).toContain('description must not be empty');
  });

  it('Cancel calls oncancel without saving', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    const oncancel = vi.fn();
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));
    await fireEvent.click(screen.getByTestId('editor-cancel'));
    expect(oncancel).toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith('catalog_update_asset', expect.anything());
  });

  it('Save calls updateAsset with the edited draft and calls onsaved on success', async () => {
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { commit: 'sha123', lint: { errors: [], warnings: [] } },
    });
    const onsaved = vi.fn();
    render(AssetEditor, { asset: skillAsset(), onsaved, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.input(screen.getByTestId('editor-description'), { target: { value: 'A better description of the skill.' } });
    await fireEvent.click(screen.getByTestId('editor-save'));

    await waitFor(() => expect(onsaved).toHaveBeenCalledWith({ commit: 'sha123', lint: { errors: [], warnings: [] } }));
    const call = invoke.mock.calls.find((c) => c[0] === 'catalog_update_asset');
    expect(call).toBeDefined();
    const asset = (call![1] as { args: { asset: EditableAsset } }).args.asset;
    expect(asset.description).toBe('A better description of the skill.');
    expect(asset.resources).toEqual([]);
  });

  it('an E_LINT save failure shows the report inline and does not call onsaved', async () => {
    const report = { errors: [{ field: 'body', message: 'body.md must not be empty' }], warnings: [] };
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { code: 'E_LINT', message: 'the asset has lint errors and was not saved', details: report },
    });
    const onsaved = vi.fn();
    render(AssetEditor, { asset: skillAsset(), onsaved, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.input(screen.getByTestId('editor-body'), { target: { value: '' } });
    await fireEvent.click(screen.getByTestId('editor-save'));

    await waitFor(() => expect(screen.getByTestId('editor-lint-error-0')).toBeTruthy());
    expect(screen.getByTestId('editor-lint-error-0').textContent).toContain('body.md must not be empty');
    expect(screen.getByTestId('editor-save-error')).toBeTruthy();
    expect(onsaved).not.toHaveBeenCalled();
  });

  it('toggling a skill tool checkbox updates allowed_tools and it is included on save', async () => {
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { commit: 'sha1', lint: { errors: [], warnings: [] } },
    });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    expect(screen.getByTestId('editor-field-allowed_tools')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('editor-tool-allowed_tools-grep'));
    await fireEvent.click(screen.getByTestId('editor-save'));

    const call = await vi.waitUntil(() => invoke.mock.calls.find((c) => c[0] === 'catalog_update_asset'));
    const asset = (call![1] as { args: { asset: EditableAsset } }).args.asset;
    expect(asset.allowed_tools).toEqual(expect.arrayContaining(['bash', 'read', 'grep']));
  });

  it('removing a resource asks for confirmation, then calls removeResource and re-fetches the asset', async () => {
    const asset = skillAsset({ resources: [{ rel_path: 'resources/run.sh', bytes: btoa('hello') }] });
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_remove_resource: { commit: 'sha2', lint: { errors: [], warnings: [] } },
      catalog_get_asset: { asset: skillAsset({ resources: [] }), previews: [], hosts: [] },
    });
    render(AssetEditor, { asset, onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    expect(screen.getByTestId('editor-resource-resources/run.sh').textContent).toContain('5 bytes');
    await fireEvent.click(screen.getByTestId('editor-resource-remove-resources/run.sh'));

    // Clicking Remove opens a confirmation dialog and does NOT call the
    // backend until it is confirmed.
    expect(await screen.findByTestId('confirm-dialog')).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith('catalog_remove_resource', expect.anything());

    await fireEvent.click(screen.getByTestId('editor-resource-remove-confirm'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_remove_resource', { args: { kind: 'skill', name: 'worktree', rel_path: 'resources/run.sh' } }));
    await waitFor(() => expect(screen.queryByTestId('editor-resource-resources/run.sh')).toBeNull());
  });

  it('adding a resource opens the file picker and calls addResource with the picked path', async () => {
    open.mockResolvedValueOnce('/tmp/script.sh');
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_add_resource: { commit: 'sha3', lint: { errors: [], warnings: [] } },
      catalog_get_asset: {
        asset: skillAsset({ resources: [{ rel_path: 'resources/script.sh', bytes: btoa('hi') }] }),
        previews: [], hosts: [],
      },
    });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.click(screen.getByTestId('editor-resource-add'));

    expect(open).toHaveBeenCalledWith({ multiple: false });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_add_resource', { args: { kind: 'skill', name: 'worktree', local_path: '/tmp/script.sh', rel_path: null } }));
    await waitFor(() => expect(screen.getByTestId('editor-resource-resources/script.sh')).toBeTruthy());
  });

  it('renders agent kind fields: tool checklist and model select', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: agentAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'agent', name: 'reviewer' } }));

    expect(screen.getByTestId('editor-field-tools')).toBeTruthy();
    expect(screen.getByTestId('editor-field-model')).toBeTruthy();
    expect((screen.getByTestId('editor-tool-tools-read') as HTMLInputElement).checked).toBe(true);
  });

  it('renders hook kind fields: event select and command action', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: hookAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'hook', name: 'on-stop' } }));

    expect(screen.getByTestId('editor-field-event')).toBeTruthy();
    expect((screen.getByTestId('editor-field-action-command') as HTMLInputElement).value).toBe('echo hook');
  });

  it('renders mcp_server kind fields: transport, url, command, args', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: mcpAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'mcp_server', name: 'fleet' } }));

    expect(screen.getByTestId('editor-field-transport')).toBeTruthy();
    expect((screen.getByTestId('editor-field-url') as HTMLElement).textContent).toContain('URL');
  });

  it('renders plugin_ref kind fields: harness, marketplace, plugin', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: pluginAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'plugin_ref', name: 'superpowers' } }));

    expect((screen.getByTestId('editor-field-marketplace-repo') as HTMLInputElement).value).toBe('obra/sp');
    expect(screen.getByTestId('editor-field-harness')).toBeTruthy();
    expect(screen.getByTestId('editor-field-plugin')).toBeTruthy();
  });

  it('switching a hook action type drops the sibling field before save', async () => {
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { commit: 'sha-hook', lint: { errors: [], warnings: [] } },
    });
    render(AssetEditor, { asset: hookAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'hook', name: 'on-stop' } }));

    // Starts as { type: 'command', command: 'echo hook' }. Switch to http
    // and fill in a url — the stale `command` must not ride along.
    await fireEvent.change(screen.getByTestId('editor-field-action-type'), { target: { value: 'http' } });
    await fireEvent.input(screen.getByTestId('editor-field-action-url'), { target: { value: 'https://example.com/hook' } });
    await fireEvent.click(screen.getByTestId('editor-save'));

    const call = await vi.waitUntil(() => invoke.mock.calls.find((c) => c[0] === 'catalog_update_asset'));
    const asset = (call![1] as { args: { asset: EditableAsset } }).args.asset;
    expect(asset.action).toEqual({ type: 'http', url: 'https://example.com/hook' });
    expect((asset.action as Record<string, unknown>).command).toBeUndefined();
  });

  it('saving a skill with a resource sends resources verbatim (rel_path + bytes)', async () => {
    const resource = { rel_path: 'resources/run.sh', bytes: btoa('hello') };
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { commit: 'sha-res', lint: { errors: [], warnings: [] } },
    });
    render(AssetEditor, { asset: skillAsset({ resources: [resource] }), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.input(screen.getByTestId('editor-version'), { target: { value: '2' } });
    await fireEvent.click(screen.getByTestId('editor-save'));

    const call = await vi.waitUntil(() => invoke.mock.calls.find((c) => c[0] === 'catalog_update_asset'));
    const asset = (call![1] as { args: { asset: EditableAsset } }).args.asset;
    expect(asset.resources).toEqual([resource]);
  });

  it('editing a field then reverting it to the original value re-disables Save', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    expect(screen.getByTestId('editor-save')).toBeDisabled();
    await fireEvent.input(screen.getByTestId('editor-version'), { target: { value: '2' } });
    expect(screen.getByTestId('editor-save')).not.toBeDisabled();
    await fireEvent.input(screen.getByTestId('editor-version'), { target: { value: '1' } });
    expect(screen.getByTestId('editor-save')).toBeDisabled();
  });

  it('renders the install_as input for a skill, prefilled with the current value, and Save sends the edit', async () => {
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { commit: 'sha-ia', lint: { errors: [], warnings: [] } },
    });
    render(AssetEditor, { asset: skillAsset({ install_as: 'foo_bar' }), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    expect((screen.getByTestId('editor-install-as') as HTMLInputElement).value).toBe('foo_bar');
    await fireEvent.input(screen.getByTestId('editor-install-as'), { target: { value: 'baz_qux' } });
    expect(screen.getByTestId('editor-save')).not.toBeDisabled();
    await fireEvent.click(screen.getByTestId('editor-save'));

    const call = await vi.waitUntil(() => invoke.mock.calls.find((c) => c[0] === 'catalog_update_asset'));
    const asset = (call![1] as { args: { asset: EditableAsset } }).args.asset;
    expect(asset.install_as).toBe('baz_qux');
  });

  it('clearing install_as sends the key absent/null, not an empty string', async () => {
    byCmd({
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_update_asset: { commit: 'sha-ia2', lint: { errors: [], warnings: [] } },
    });
    render(AssetEditor, { asset: skillAsset({ install_as: 'foo_bar' }), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.input(screen.getByTestId('editor-install-as'), { target: { value: '' } });
    await fireEvent.click(screen.getByTestId('editor-save'));

    const call = await vi.waitUntil(() => invoke.mock.calls.find((c) => c[0] === 'catalog_update_asset'));
    const asset = (call![1] as { args: { asset: EditableAsset } }).args.asset;
    // Either an absent key or an explicit null satisfies "unset" on the
    // Rust side (`Option<String>` with `#[serde(default)]`).
    expect([undefined, null]).toContainEqual(asset.install_as);
  });

  it('rejects an install_as value with an invalid character and blocks Save', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: skillAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } }));

    await fireEvent.input(screen.getByTestId('editor-install-as'), { target: { value: 'bad name' } });
    expect(screen.getByTestId('editor-save')).toBeDisabled();
    expect(screen.getByTestId('editor-client-errors').textContent).toContain('install_as');

    await fireEvent.input(screen.getByTestId('editor-install-as'), { target: { value: '..' } });
    expect(screen.getByTestId('editor-save')).toBeDisabled();
    expect(screen.getByTestId('editor-client-errors').textContent).toContain('install_as');
  });

  it('does not render the install_as input for a hook', async () => {
    byCmd({ catalog_lint_asset: { errors: [], warnings: [] } });
    render(AssetEditor, { asset: hookAsset(), onsaved: () => {}, oncancel: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'hook', name: 'on-stop' } }));

    expect(screen.queryByTestId('editor-install-as')).toBeNull();
  });
});
