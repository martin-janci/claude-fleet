// "Place in group…" (work graph M14): a placement is a compare-and-set on
// the task's placement version, and only after it succeeds is "Make a rule
// for similar tasks…" offered, prefilled from where the task came from (its
// key prefix, its tracker project, or its repository).
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkPlaceDialog from './WorkPlaceDialog.svelte';
import { task } from './work_view_fixture';
import { workTreeMeta, type WorkRuleDraft, type WorkTask } from './work_view';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

// Standard upper-case keys only: the prefilled draft is the same whichever
// way the key prefix is read.
const keyTask = task({
  task_id: 'ref:ABC-12',
  item_id: null,
  key: 'ABC-12',
  kind: 'ref',
  tracker_id: null,
  tracker_name: null,
  provider: null,
  tracker_state: null,
  org_fenced: false,
  org_source: 'sessions',
  group: { id: 'key:ABC', label: 'ABC', source: 'key' },
});
const trackerTask = task({ placement_version: 2 });
const repoTask = task({
  task_id: 'item:77',
  item_id: 77,
  key: null,
  title: 'Clean up the CI cache',
  kind: 'local',
  tracker_id: null,
  tracker_name: null,
  provider: null,
  tracker_state: null,
  group: { id: 'repo:acme/api', label: 'acme/api', source: 'repo' },
});

describe('WorkPlaceDialog', () => {
  let onclose: Mock<() => void>;
  let ondone: Mock<(t: WorkTask, note: string | null) => void>;
  let onmakerule: Mock<(d: WorkRuleDraft) => void>;

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    workTreeMeta.set({ orgs: [], trackers: [], groups: [] });
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    onclose = vi.fn();
    ondone = vi.fn();
    onmakerule = vi.fn();
    handlers = {
      place_work: (a) => ({
        ...trackerTask,
        task_id: a.task_id as string,
        group: { id: `label:${a.group}`, label: a.group as string, source: 'manual' },
        placement_version: (a.expected_version as number) + 1,
      }),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  afterEach(() => {
    workTreeMeta.set({ orgs: [], trackers: [], groups: [] });
  });

  const mount = (t: WorkTask) => render(WorkPlaceDialog, { task: t, onclose, ondone, onmakerule });

  async function placeIn(group: string) {
    await fireEvent.input(screen.getByTestId('work-place-group'), { target: { value: group } });
    await fireEvent.click(screen.getByTestId('work-place-submit'));
    await flush();
  }

  async function makeRule(): Promise<WorkRuleDraft> {
    await fireEvent.click(screen.getByTestId('work-place-make-rule'));
    await flush();
    expect(onclose).toHaveBeenCalledTimes(1);
    expect(onmakerule).toHaveBeenCalledTimes(1);
    return onmakerule.mock.calls[0][0];
  }

  it('sends the placement with the task’s placement version', async () => {
    mount(trackerTask);
    expect(screen.queryByTestId('work-place-make-rule')).toBeNull();
    await placeIn(' Payments ');
    expect(calls('place_work')).toEqual([{ task_id: 'item:12', group: 'Payments', expected_version: 2 }]);
    expect(ondone).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('work-place-done').textContent).toContain('Payments');
    // Placed, not closed: the rule is its own step.
    expect(onclose).not.toHaveBeenCalled();
    expect(onmakerule).not.toHaveBeenCalled();
  });

  it('a task never placed is sent expecting version 0', async () => {
    mount(keyTask);
    await placeIn('Payments');
    expect(calls('place_work')[0]).toMatchObject({ task_id: 'ref:ABC-12', expected_version: 0 });
  });

  it('no rule is offered after a failed placement', async () => {
    handlers.place_work = () => {
      throw { code: 'E_INVALID', message: 'nope' };
    };
    mount(keyTask);
    await placeIn('Payments');
    expect(screen.getByTestId('work-place-error').textContent).toContain('nope');
    expect(screen.queryByTestId('work-place-make-rule')).toBeNull();
  });

  it('a key-group task: the rule matches its key prefix, into the placed group', async () => {
    mount(keyTask);
    await placeIn('Payments');
    const d = await makeRule();
    expect(d).toMatchObject({ name: 'Payments', group: 'Payments', enabled: true, expected_version: 0 });
    expect(d.id).toBeUndefined();
    expect(d.conditions).toEqual({ tracker_id: null, container: null, key_prefix: 'ABC', repo: null, title_contains: null });
  });

  it('a tracker-group task: the rule matches its tracker and project, not its key', async () => {
    mount(trackerTask);
    await placeIn('Payments');
    const d = await makeRule();
    expect(d).toMatchObject({ name: 'Payments', group: 'Payments' });
    expect(d.conditions).toEqual({ tracker_id: 1, container: 'ABC', key_prefix: null, repo: null, title_contains: null });
  });

  it('a repo-group task: the rule matches its repository', async () => {
    mount(repoTask);
    await placeIn('Infra');
    const d = await makeRule();
    expect(d).toMatchObject({ name: 'Infra', group: 'Infra' });
    expect(d.conditions).toEqual({ tracker_id: null, container: null, key_prefix: null, repo: 'acme/api', title_contains: null });
  });

  describe('the Group combobox (G2.2)', () => {
    const grp = (label: string, count: number, org_id: number | null = null) => ({
      org_id,
      group: { id: `label:${label}`, label, source: 'manual' },
      count,
    });
    beforeEach(() => {
      workTreeMeta.set({
        orgs: [],
        trackers: [],
        groups: [grp('Orbit tokens', 3, 1), grp('Orbit tokens', 1, 2), grp('Orbit redesign', 11), grp('Infra', 2), { org_id: null, group: { id: 'none', label: '', source: 'none' }, count: 40 }],
      });
    });
    const options = () => screen.queryAllByTestId('work-place-label').map((o) => `${o.dataset.label}=${o.textContent?.replace(/\s+/g, '')}`);

    it('lists each existing group once with its task count, summed across orgs', () => {
      mount(keyTask);
      expect(options()).toEqual(['Infra=Infra2', 'Orbit redesign=Orbitredesign11', 'Orbit tokens=Orbittokens4']);
      expect(screen.queryByTestId('work-place-new-group')).toBeNull();
    });

    it('filters as you type and offers a new group for what is not one', async () => {
      mount(keyTask);
      await fireEvent.input(screen.getByTestId('work-place-group'), { target: { value: 'Orbit' } });
      expect(options().map((o) => o.split('=')[0])).toEqual(['Orbit redesign', 'Orbit tokens']);
      expect(screen.getByTestId('work-place-new-group').textContent).toBe('+ New group “Orbit”');
      expect(screen.getByTestId('work-place-existing').textContent).toBe('new group');
      await fireEvent.click(screen.getAllByTestId('work-place-label').find((o) => o.dataset.label === 'Orbit tokens')!);
      expect((screen.getByTestId('work-place-group') as HTMLInputElement).value).toBe('Orbit tokens');
      expect(screen.getByTestId('work-place-existing').textContent).toBe('existing · 4 tasks');
      expect(screen.queryByTestId('work-place-new-group')).toBeNull();
      await fireEvent.click(screen.getByTestId('work-place-submit'));
      await flush();
      expect(calls('place_work')[0]).toMatchObject({ group: 'Orbit tokens' });
    });

    it('arrow keys and Enter pick an option without submitting', async () => {
      mount(keyTask);
      const input = screen.getByTestId('work-place-group');
      await fireEvent.input(input, { target: { value: 'orbit' } });
      await fireEvent.keyDown(input, { key: 'ArrowDown' });
      await fireEvent.keyDown(input, { key: 'ArrowDown' });
      expect(input.getAttribute('aria-activedescendant')).toBe('work-place-opt-1');
      await fireEvent.keyDown(input, { key: 'Enter' });
      await flush();
      expect((input as HTMLInputElement).value).toBe('Orbit tokens');
      expect(calls('place_work')).toEqual([]);
      // Up from the top wraps to the last row.
      await fireEvent.input(input, { target: { value: 'Pay' } });
      await fireEvent.keyDown(input, { key: 'ArrowUp' });
      await fireEvent.keyDown(input, { key: 'Enter' });
      expect((input as HTMLInputElement).value).toBe('Pay');
      expect(screen.getByTestId('work-place-existing').textContent).toBe('new group');
    });
  });
});
