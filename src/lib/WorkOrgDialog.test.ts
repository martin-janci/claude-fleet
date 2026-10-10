// "Assign org…" (work graph M14): the org is the access boundary, so the
// move is sent only with the token of a preview for exactly that target; a
// review that answers for an old target is dropped; a changed impact is
// reviewed again and never sent with the old token; a refused move and an
// older paired hub keep Confirm disabled.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkOrgDialog from './WorkOrgDialog.svelte';
import { task } from './work_view_fixture';
import { workTreeMeta, type OrgImpact, type WorkTask } from './work_view';
import { orgs } from './orgs';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';

const local: WorkTask = task({
  task_id: 'item:77',
  item_id: 77,
  key: null,
  title: 'Clean up the CI cache',
  kind: 'local',
  tracker_id: null,
  tracker_name: null,
  provider: null,
  tracker_state: null,
  org_id: null,
  org_source: 'none',
  org_fenced: false,
  group: { id: 'repo:acme/api', label: 'acme/api', source: 'repo' },
});

const impact = (token: string | null, over: Partial<OrgImpact> = {}): OrgImpact => ({
  task_id: 'item:77',
  from_org: null,
  to_org: 2,
  allowed: true,
  reason: null,
  links: [],
  hosts_losing: [],
  hosts_gaining: ['h-c'],
  bound_clients_losing: 0,
  bound_clients_gaining: 1,
  journal_entries: 2,
  summaries: 0,
  impact_token: token,
  ...over,
});

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

const confirmBtn = () => screen.getByTestId('work-org-confirm') as HTMLButtonElement;

async function pick(value: string) {
  await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value } });
}

describe('WorkOrgDialog', () => {
  let onclose: Mock<() => void>;
  let ondone: Mock<(t: WorkTask) => void>;

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    workTreeMeta.set({
      orgs: [
        { id: 1, name: 'Acme' },
        { id: 2, name: 'Globex' },
      ],
      trackers: [],
      groups: [],
    });
    orgs.set([]);
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    onclose = vi.fn();
    ondone = vi.fn();
    handlers = {
      work_org_impact: (a) => impact(`tok-${a.org_id}`, { to_org: a.org_id as number }),
      assign_work_org: () => ({ ...local, org_id: 2, org_fenced: true, org_source: 'item' }),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    workTreeMeta.set({ orgs: [], trackers: [], groups: [] });
  });

  const mount = () => render(WorkOrgDialog, { task: local, onclose, ondone });

  it('lists the orgs and sends exactly the preview token', async () => {
    mount();
    const options = Array.from((screen.getByTestId('work-org-target') as HTMLSelectElement).options, (o) => o.value);
    expect(options).toEqual(['', '1', '2']);
    expect(confirmBtn().disabled).toBe(true);
    await pick('2');
    await flush();
    expect(calls('work_org_impact')).toEqual([{ task_id: 'item:77', org_id: 2 }]);
    expect(screen.getByTestId('work-org-impact').textContent).toContain('Globex');
    expect(confirmBtn().disabled).toBe(false);
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('assign_work_org')).toEqual([{ task_id: 'item:77', org_id: 2, impact_token: 'tok-2' }]);
    expect(ondone).toHaveBeenCalledTimes(1);
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('drops a review that answers for a target no longer picked', async () => {
    const pending: Record<number, (v: OrgImpact) => void> = {};
    handlers.work_org_impact = (a) => new Promise<OrgImpact>((r) => (pending[a.org_id as number] = r));
    mount();
    await pick('1');
    await flush();
    await pick('2');
    await flush();
    // Switching dropped org 1's review and read org 2's at once.
    expect(calls('work_org_impact')).toEqual([
      { task_id: 'item:77', org_id: 1 },
      { task_id: 'item:77', org_id: 2 },
    ]);
    // Org 1's late answer: nothing to confirm, and org 2's review is still
    // the one in flight.
    pending[1](impact('tok-A', { to_org: 1 }));
    await flush();
    expect(screen.queryByTestId('work-org-impact')).toBeNull();
    expect(confirmBtn().disabled).toBe(true);
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('assign_work_org')).toEqual([]);

    pending[2](impact('tok-B', { to_org: 2 }));
    await flush();
    expect(screen.getByTestId('work-org-impact')).toBeTruthy();
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('assign_work_org')).toEqual([{ task_id: 'item:77', org_id: 2, impact_token: 'tok-B' }]);
  });

  it('on E_CONFLICT reviews again, shows the new impact, and sends only the new token', async () => {
    let n = 0;
    handlers.work_org_impact = () => impact(`tok-${++n}`, n > 1 ? { hosts_losing: ['h-d'] } : {});
    let first = true;
    handlers.assign_work_org = () => {
      if (first) {
        first = false;
        throw { code: 'E_CONFLICT', message: 'impact changed', details: {} };
      }
      return local;
    };
    mount();
    await pick('2');
    await flush();
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('work_org_impact')).toHaveLength(2);
    expect(calls('assign_work_org').map((a) => a.impact_token)).toEqual(['tok-1']);
    expect(screen.getByTestId('work-org-changed').textContent).toContain('The impact changed since you reviewed it');
    expect(screen.getByTestId('work-org-hosts-losing').textContent).toContain('h-d');
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('assign_work_org').map((a) => a.impact_token)).toEqual(['tok-1', 'tok-2']);
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('a tracker-controlled refusal says why and keeps Confirm disabled', async () => {
    handlers.work_org_impact = () => impact(null, { allowed: false, reason: 'tracker_controlled' });
    mount();
    await pick('2');
    await flush();
    expect(screen.getByTestId('work-org-refused').textContent).toContain("A tracker's task belongs to its tracker's organisation");
    expect(confirmBtn().disabled).toBe(true);
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('assign_work_org')).toEqual([]);
  });

  it('an older paired hub keeps Confirm disabled, even with an allowed impact', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    hubConnection.set({ state: 'hub_too_old', hub_contract: 4, min_contract: 5 });
    mount();
    expect(screen.getByTestId('work-org-dialog').textContent).toContain('Update the hub');
    await pick('2');
    await flush();
    expect(screen.getByTestId('work-org-impact')).toBeTruthy();
    expect(confirmBtn().disabled).toBe(true);
    await fireEvent.click(confirmBtn());
    await flush();
    expect(calls('assign_work_org')).toEqual([]);
  });

  it('picking a target reads its impact at once: the list is the confirm (G2.2)', async () => {
    mount();
    expect(calls('work_org_impact')).toEqual([]);
    await pick('2');
    await flush();
    expect(calls('work_org_impact')).toEqual([{ task_id: 'item:77', org_id: 2 }]);
    expect(screen.queryByTestId('work-org-review')).toBeNull();
    expect(confirmBtn().textContent).toBe('Move to Globex');
    expect(confirmBtn().disabled).toBe(false);
  });

  it('names who loses and gains access, and says where the spend stays (G2.2)', async () => {
    handlers.work_org_impact = (a) =>
      impact(`tok-${a.org_id}`, { from_org: 1, to_org: a.org_id as number, people_losing: ['Ondrej'], people_gaining: ['Eva'] });
    render(WorkOrgDialog, { task: { ...local, cost_micros: 3_120_000 }, onclose, ondone });
    await pick('2');
    await flush();
    expect(screen.getByTestId('work-org-person-losing').textContent).toContain('Ondrej loses access');
    expect(screen.getByTestId('work-org-person-losing').textContent).toContain('bound to Acme, not Globex');
    expect(screen.getByTestId('work-org-person-gaining').textContent).toContain('Eva gains access');
    expect(screen.getByTestId('work-org-spend').textContent).toContain('$3.12');
    expect(screen.getByTestId('work-org-spend').textContent).toContain("stays on its sessions' organisation budget");
  });

  it('an older hub (no people) and an unspent task add no lines', async () => {
    mount();
    await pick('2');
    await flush();
    expect(screen.queryByTestId('work-org-person-losing')).toBeNull();
    expect(screen.queryByTestId('work-org-spend')).toBeNull();
  });

  it('a failed read offers Try again', async () => {
    let fail = true;
    handlers.work_org_impact = (a) => {
      if (fail) throw { code: 'E_IO', message: 'hub away' };
      return impact(`tok-${a.org_id}`);
    };
    mount();
    await pick('2');
    await flush();
    expect(screen.getByTestId('work-org-error').textContent).toContain('hub away');
    fail = false;
    await fireEvent.click(screen.getByTestId('work-org-review'));
    await flush();
    expect(screen.getByTestId('work-org-impact')).toBeTruthy();
    expect(screen.queryByTestId('work-org-review')).toBeNull();
  });
});
