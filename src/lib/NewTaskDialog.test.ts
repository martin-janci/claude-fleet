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

const calls = (cmd: string) => vi.mocked(invoke).mock.calls.filter(([c]) => c === cmd);

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'list_projects') return [];
    if (cmd === 'create_work_task') return { id: 12, key: 'TASK-12', title: 'Round once', source: 'local' };
    return null;
  });
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
});
