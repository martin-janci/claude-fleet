// New task (gap plan G2.1, FormsWork): the dialog ⌘N opens in Work.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import NewTaskDialog from './NewTaskDialog.svelte';
import { startAskRequest } from './new_task';
import { selectedTaskId } from './work_view';
import { clearToasts, toasts } from './toasts';
import { detectMac } from './terminal_keys';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const calls = (cmd: string) => vi.mocked(invoke).mock.calls.filter(([c]) => c === cmd);

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'list_projects') return [];
    if (cmd === 'create_work_task') return { id: 12, key: 'TASK-12', title: 'Round once', source: 'local' };
    return null;
  });
  fleetSettings.set({ ...SETTING_DEFAULTS });
  startAskRequest.set(null);
  selectedTaskId.set(null);
  clearToasts();
});

async function type(testid: string, value: string) {
  await fireEvent.input(screen.getByTestId(testid), { target: { value } });
  await tick();
}

describe('NewTaskDialog', () => {
  it('says the task stays in Fleet, checks the title on leaving it, and creates on ⌘↵', async () => {
    const onclose = vi.fn();
    const ondone = vi.fn();
    render(NewTaskDialog, { props: { onclose, ondone } });
    expect(screen.getByTestId('new-task-dialog').textContent).toMatch(/not sent to a tracker/);
    const title = screen.getByTestId('new-task-title');
    await fireEvent.blur(title);
    expect(screen.getByTestId('new-task-title-error').textContent).toMatch(/required/);
    expect((screen.getByTestId('new-task-submit') as HTMLButtonElement).disabled).toBe(true);
    await type('new-task-title', 'Round once');
    await type('new-task-notes', 'At the end');
    await fireEvent.keyDown(title, detectMac(navigator) ? { key: 'Enter', metaKey: true } : { key: 'Enter', ctrlKey: true });
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'Round once', notes: 'At the end' } });
    expect(ondone).toHaveBeenCalled();
    expect(get(toasts).at(-1)?.message).toBe('Task created: TASK-12.');
    expect(get(startAskRequest)).toBeNull();
  });

  it('a failure is a banner over the fields and the input stays', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'create_work_task') throw { code: 'E_INVALID', message: 'title has a control character' };
      return [];
    });
    const onclose = vi.fn();
    render(NewTaskDialog, { props: { onclose, initialTitle: 'Round once' } });
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await waitFor(() => expect(screen.getByTestId('new-task-error')).toBeTruthy());
    expect(onclose).not.toHaveBeenCalled();
    expect((screen.getByTestId('new-task-title') as HTMLInputElement).value).toBe('Round once');
  });

  it('"Start a session for it now" opens the new task and asks its Work button for the start menu', async () => {
    const onclose = vi.fn();
    render(NewTaskDialog, { props: { onclose, initialTitle: 'Round once' } });
    await fireEvent.click(screen.getByTestId('new-task-start-now'));
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(get(startAskRequest)).toBe('item:12');
    expect(get(selectedTaskId)).toBe('item:12');
  });
  it('holds picked files and attaches them to the task once it is made', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return [];
      if (cmd === 'create_work_task') return { id: 12, key: 'TASK-12', title: 'Round once', source: 'local' };
      if (cmd === 'attach_to_work') return { id: 3, name: 'spec.md', mime: 'text/markdown', size: 5 };
      return null;
    });
    const onclose = vi.fn();
    render(NewTaskDialog, { props: { onclose, initialTitle: 'Round once' } });
    const input = screen.getByTestId('new-task-files-input') as HTMLInputElement;
    const spec = new File(['hello'], 'spec.md', { type: 'text/markdown' });
    const empty = new File([], 'empty.txt', { type: 'text/plain' });
    await fireEvent.change(input, { target: { files: [spec, empty] } });
    // A file the task page would refuse is refused here, before the task exists.
    expect(screen.getByTestId('new-task-files-error').textContent).toMatch(/empty\.txt is empty/);
    expect(screen.getAllByTestId('new-task-file')).toHaveLength(1);
    expect(calls('attach_to_work')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls('attach_to_work')[0][1]).toEqual({
      args: { item_id: 12, name: 'spec.md', mime: 'text/markdown', data_base64: btoa('hello'), comment_id: null },
    });
    expect(get(toasts).at(-1)?.message).toBe('Task created: TASK-12.');
  });

  it('a removed file is not attached', async () => {
    const onclose = vi.fn();
    render(NewTaskDialog, { props: { onclose, initialTitle: 'Round once' } });
    const spec = new File(['hello'], 'spec.md', { type: 'text/markdown' });
    await fireEvent.change(screen.getByTestId('new-task-files-input'), { target: { files: [spec] } });
    await fireEvent.click(screen.getByTestId('new-task-file-remove'));
    expect(screen.queryByTestId('new-task-file')).toBeNull();
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls('attach_to_work')).toHaveLength(0);
  });

  it('a file that fails to attach leaves the task made, opens it and says which file', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return [];
      if (cmd === 'create_work_task') return { id: 12, key: 'TASK-12', title: 'Round once', source: 'local' };
      if (cmd === 'attach_to_work') throw { code: 'E_INVALID', message: 'too large for this hub' };
      return null;
    });
    const onclose = vi.fn();
    render(NewTaskDialog, { props: { onclose, initialTitle: 'Round once' } });
    const spec = new File(['hello'], 'spec.md', { type: 'text/markdown' });
    await fireEvent.change(screen.getByTestId('new-task-files-input'), { target: { files: [spec] } });
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(get(selectedTaskId)).toBe('item:12');
    expect(get(toasts).at(-1)?.message).toMatch(/TASK-12.*spec\.md/);
  });
  describe('the drafted title (work.draft_task_titles)', () => {
    const draftOn = () => fleetSettings.set({ ...SETTING_DEFAULTS, 'work.draft_task_titles': 'true' });
    const withDraft = (draft: () => Promise<unknown>) =>
      vi.mocked(invoke).mockImplementation(async (cmd: string) => {
        if (cmd === 'list_projects') return [];
        if (cmd === 'draft_task_title') return draft();
        if (cmd === 'create_work_task') return { id: 12, key: 'TASK-12', title: 'Round totals once', source: 'local' };
        return null;
      });
    const drafted = async () => ({ title: 'Round totals once', model: 'haiku', host_alias: 'mercury' });
    const titleValue = () => (screen.getByTestId('new-task-title') as HTMLInputElement).value;

    it('off: leaving the notes drafts nothing', async () => {
      withDraft(drafted);
      render(NewTaskDialog, { props: { onclose: vi.fn() } });
      await type('new-task-notes', 'Sums drift; round at the end.');
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      expect(calls('draft_task_title')).toHaveLength(0);
      expect(screen.queryByTestId('new-task-draft-title')).toBeNull();
    });

    it('leaving the notes with no title drafts one, marked Drafted, and Undo takes it out', async () => {
      draftOn();
      withDraft(drafted);
      render(NewTaskDialog, { props: { onclose: vi.fn() } });
      await type('new-task-notes', 'Sums drift; round at the end.');
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      await waitFor(() => expect(titleValue()).toBe('Round totals once'));
      expect(calls('draft_task_title')[0][1]).toEqual({ args: { notes: 'Sums drift; round at the end.' } });
      expect(screen.getByTestId('new-task-drafted').textContent).toMatch(/Drafted.*by haiku on mercury/);
      // Leaving the same notes again does not spend a second call.
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      expect(calls('draft_task_title')).toHaveLength(1);
      await fireEvent.click(screen.getByTestId('new-task-draft-undo'));
      expect(titleValue()).toBe('');
      expect(screen.queryByTestId('new-task-drafted')).toBeNull();
    });

    it('a typed title is never replaced: the draft is offered beside it', async () => {
      draftOn();
      let answer: (v: unknown) => void = () => {};
      withDraft(() => new Promise((res) => (answer = res)));
      render(NewTaskDialog, { props: { onclose: vi.fn() } });
      await type('new-task-notes', 'Sums drift; round at the end.');
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      await waitFor(() => expect(screen.getByTestId('new-task-drafting')).toBeTruthy());
      // The person types while the model writes.
      await type('new-task-title', 'My own title');
      answer(await drafted());
      await waitFor(() => expect(screen.getByTestId('new-task-use-draft')).toBeTruthy());
      expect(titleValue()).toBe('My own title');
      await fireEvent.click(screen.getByTestId('new-task-use-draft'));
      expect(titleValue()).toBe('Round totals once');
    });

    it('with a title already there, leaving the notes drafts nothing until Draft title is pressed', async () => {
      draftOn();
      withDraft(drafted);
      render(NewTaskDialog, { props: { onclose: vi.fn(), initialTitle: 'Mine' } });
      await type('new-task-notes', 'Sums drift; round at the end.');
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      expect(calls('draft_task_title')).toHaveLength(0);
      await fireEvent.click(screen.getByTestId('new-task-draft-title'));
      await waitFor(() => expect(screen.getByTestId('new-task-use-draft')).toBeTruthy());
      expect(titleValue()).toBe('Mine');
    });

    it('an edited draft is the person\'s: no pill, and it is what is created', async () => {
      draftOn();
      withDraft(drafted);
      const onclose = vi.fn();
      render(NewTaskDialog, { props: { onclose } });
      await type('new-task-notes', 'Sums drift; round at the end.');
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      await waitFor(() => expect(titleValue()).toBe('Round totals once'));
      await type('new-task-title', 'Round totals once, at the end');
      expect(screen.queryByTestId('new-task-drafted')).toBeNull();
      await fireEvent.click(screen.getByTestId('new-task-submit'));
      await waitFor(() => expect(onclose).toHaveBeenCalled());
      expect(calls('create_work_task')[0][1]).toEqual({
        args: { title: 'Round totals once, at the end', notes: 'Sums drift; round at the end.' },
      });
    });

    it('a failed draft says why and leaves the title to the person', async () => {
      draftOn();
      withDraft(async () => {
        throw { code: 'E_CLAUDE_CLI', message: "claude is not on mercury's login PATH" };
      });
      render(NewTaskDialog, { props: { onclose: vi.fn() } });
      await type('new-task-notes', 'Sums drift; round at the end.');
      await fireEvent.blur(screen.getByTestId('new-task-notes'));
      await waitFor(() => expect(screen.getByTestId('new-task-draft-error').textContent).toMatch(/not on mercury/));
      expect(titleValue()).toBe('');
      expect(screen.queryByTestId('new-task-error')).toBeNull();
    });
  });
});
