// Work graph M4.4 on a row: the suggestion chip (dashed, `?`), its evidence
// popover and decisions, the y / n keys, the trust checkbox, and the auto
// link's marker. Asserts the command each action invokes and its arguments.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import type { SessionRow, SessionWork } from './sessions';
import { workKeyFor, type WorkKey } from './work_keys';
import type { WorkLink } from './work';

const noop = () => {};

function props(sess: SessionRow, workKey: WorkKey | null = null) {
  return {
    sess,
    selectMode: false,
    isChecked: false,
    isRenaming: false,
    renameValue: '',
    renameInput: undefined,
    renameError: null,
    relatedCount: 0,
    nowSec: Math.floor(Date.now() / 1000),
    workKey,
    onSelectSession: vi.fn(),
    onKeySession: vi.fn(),
    toggleSelected: noop,
    beginRename: noop,
    beginLabelEdit: noop,
    onRenameKey: noop,
    commitRename: noop,
    askRecreate: noop,
    askRestart: noop,
    askKill: noop,
  };
}

const suggestion: SessionWork = {
  link_id: 31,
  item_id: null,
  key: 'ABC-99',
  title: '',
  source: 'prompt',
  state: 'suggested',
  strength: 'strong',
  rule: 'R5',
  preselected: true,
  suggestions: 1,
};

const row = (over: Partial<SessionRow> = {}): SessionRow =>
  session('mefistos', 'dev-foo', {
    id: 7,
    status: 'running',
    project_id: 4,
    work_suggested: suggestion,
    ...over,
  });

const link: WorkLink = {
  id: 31,
  ref_key: 'ABC-99',
  state: 'suggested',
  source: 'prompt',
  created_at: 1,
  rule: 'R5',
  evidence: [
    { signal: 'prompt_key', rule: 'R5', text: 'ABC-99', snippet: 'see ABC-99 for context', at: 1_790_000_000 },
  ],
};

beforeEach(() => {
  hosts.set([]);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'session_work_links') return [link];
    if (cmd === 'set_work_project_trust') return { trusted: [4] };
    return row({ work_suggested: null });
  });
});

describe('a row with a link suggestion', () => {
  it('shows a dashed ? chip that explains itself and is not the work chip', async () => {
    render(SessionRowItem, { props: props(row()) });
    const chip = screen.getByTestId('work-suggestion');
    expect(chip.dataset.state).toBe('suggested');
    expect(chip.textContent).toContain('ABC-99');
    expect(chip.textContent).toContain('?');
    expect(chip.title).toContain('suggested from the prompt · rule R5');
    expect(screen.queryByTestId('work-chip')).toBeNull();
  });

  it('opens the evidence popover from the chip and confirms', async () => {
    const p = props(row());
    render(SessionRowItem, { props: p });
    await fireEvent.click(screen.getByTestId('work-suggestion'));
    await tick();
    await tick();
    expect(invoke).toHaveBeenCalledWith('session_work_links', { args: { session_id: 7 } });
    const ev = await screen.findByTestId('why-evidence');
    expect(ev.textContent).toMatch(/mentioned ABC-99 in a prompt at \d\d:\d\d · R5/);
    await fireEvent.click(screen.getByTestId('why-confirm'));
    expect(invoke).toHaveBeenCalledWith('confirm_session_work', {
      args: { session_id: 7, link_id: 31 },
    });
    expect(p.onSelectSession).not.toHaveBeenCalled();
  });

  it('Not this rejects by link id', async () => {
    render(SessionRowItem, { props: props(row()) });
    await fireEvent.click(screen.getByTestId('work-suggestion'));
    await fireEvent.click(await screen.findByTestId('why-reject'));
    expect(invoke).toHaveBeenCalledWith('reject_session_work', {
      args: { session_id: 7, link_id: 31 },
    });
  });

  it('the trust checkbox trusts the row project', async () => {
    render(SessionRowItem, { props: props(row()) });
    await fireEvent.click(screen.getByTestId('work-suggestion'));
    const box = (await screen.findByTestId('why-trust')) as HTMLInputElement;
    box.checked = true;
    await fireEvent.change(box);
    expect(invoke).toHaveBeenCalledWith('set_work_project_trust', {
      args: { project_id: 4, on: true },
    });
  });

  it('y and n on the focused row decide the top suggestion; other keys pass on', async () => {
    const p = props(row());
    render(SessionRowItem, { props: p });
    const r = screen.getByTestId('sess-row');
    await fireEvent.keyDown(r, { key: 'y' });
    expect(invoke).toHaveBeenCalledWith('confirm_session_work', {
      args: { session_id: 7, link_id: 31 },
    });
    await tick();
    await fireEvent.keyDown(r, { key: 'n' });
    expect(invoke).toHaveBeenCalledWith('reject_session_work', {
      args: { session_id: 7, link_id: 31 },
    });
    await fireEvent.keyDown(r, { key: 'Enter' });
    expect(p.onKeySession).toHaveBeenCalledTimes(1);
  });

  it('l opens the work menu', async () => {
    render(SessionRowItem, { props: props(row({ work_suggested: null })) });
    await fireEvent.keyDown(screen.getByTestId('sess-row'), { key: 'l' });
    await tick();
    expect(screen.getByTestId('work-menu-panel')).toBeTruthy();
  });
});

describe('the work chip of an automatic link', () => {
  it('carries the auto marker and the rule in its tooltip', () => {
    const s = row({
      work_suggested: null,
      work: {
        link_id: 5,
        item_id: null,
        key: 'ABC-123',
        title: '',
        source: 'branch',
        state: 'confirmed',
        strength: 'strong',
        rule: 'R3',
      },
    });
    const wk = workKeyFor(s, new Map());
    render(SessionRowItem, { props: props(s, wk) });
    const chip = screen.getByTestId('work-chip');
    expect(chip.dataset.state).toBe('auto');
    expect(screen.getByTestId('work-chip-auto')).toBeTruthy();
    expect(chip.title).toContain('linked from the branch · rule R3');
  });
});
