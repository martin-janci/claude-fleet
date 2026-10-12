import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FilesPanel from './FilesPanel.svelte';
import type { SessionRow } from './sessions';
import type { ChangedFile } from './files';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const staged: ChangedFile = { path: 'src/a.ts', status: 'modified', staged: true, orig_path: null };

let draftFails = false;

beforeEach(() => {
  fleetSettings.set({ ...SETTING_DEFAULTS, 'work.draft_commit_messages': 'true' });
  draftFails = false;
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'repo_changes':
        return [staged];
      case 'draft_commit_message':
        if (draftFails) throw { code: 'E_INVALID_STATE', message: 'nothing is staged to write a message for' };
        return { message: 'Fix the flaky test', model: 'haiku', host_alias: 'mercury', files: 1 };
      default:
        return null;
    }
  });
});

function mount() {
  return render(FilesPanel, { props: { session: { id: 7 } as SessionRow } });
}

// Step 5.12: a commit message drafted from the staged changes, on the
// session's own host, editable, with Clear.
describe('FilesPanel Changed: commit message draft', () => {
  it('drafts on the session, shows where it came from, and Clear empties the field', async () => {
    mount();
    await fireEvent.click(await screen.findByTestId('commit-draft-run'));
    const input = (await screen.findByTestId('commit-draft-input')) as HTMLTextAreaElement;
    await waitFor(() => expect(input.value).toBe('Fix the flaky test'));
    expect(invoke).toHaveBeenCalledWith('draft_commit_message', { args: { session_id: 7 } });
    expect(screen.getByTestId('commit-draft-meta').textContent).toContain(
      'by haiku on mercury · from 1 staged file',
    );
    await fireEvent.click(screen.getByTestId('commit-draft-clear'));
    expect(input.value).toBe('');
    expect(screen.getByTestId('commit-draft-run')).toBeTruthy();
  });

  it('says why a draft failed and leaves the field empty', async () => {
    draftFails = true;
    mount();
    await fireEvent.click(await screen.findByTestId('commit-draft-run'));
    expect((await screen.findByTestId('commit-draft-error')).textContent).toContain('nothing is staged');
    expect((screen.getByTestId('commit-draft-input') as HTMLTextAreaElement).value).toBe('');
  });

});
