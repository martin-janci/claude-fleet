import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SyncPlanDialog from './SyncPlanDialog.svelte';
import { syncProgress, type SyncPlan, type SyncAction, type HostPlan } from './assets';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const action = (over: Partial<SyncAction> = {}): SyncAction => ({
  kind: 'skill', name: 's', op: 'create', reason: null, files: [], merges: [],
  backup: false, secrets: [], missing_secrets: [], ...over,
});

const hostPlan = (over: Partial<HostPlan> = {}): HostPlan => ({
  host_alias: 'local', harness: 'claude', status: 'planned', detail: null, actions: [], ...over,
});

const plan = (hosts: HostPlan[], counts: Record<string, number> = {}): SyncPlan => ({
  id: 'plan-1', computed_at: 1, hosts, counts,
});

beforeEach(() => {
  invoke.mockReset();
  syncProgress.set(null);
});

describe('SyncPlanDialog', () => {
  it('renders header counts and grouped host/action rows', () => {
    const p = plan(
      [hostPlan({ actions: [action({ kind: 'skill', name: 's', op: 'create' })] })],
      { create: 1 },
    );
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-counts').textContent).toContain('create: 1');
    expect(screen.getByTestId('plan-host-local-claude')).toBeTruthy();
    expect(screen.getByTestId('plan-action-local-claude-skill-s').textContent).toContain('create');
  });

  it('renders the Apply button red (danger) when the plan overwrites something', () => {
    const p = plan([hostPlan({ actions: [action({ op: 'overwrite' })] })], { overwrite: 1 });
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-apply').className).toContain('danger');
  });

  it('does not mark Apply as danger for a purely additive plan', () => {
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-apply').className).not.toContain('danger');
  });

  it('shows the reason on a blocked row, offers the force-partial checkbox, and disables Apply when nothing is applicable', () => {
    const p = plan(
      [hostPlan({ actions: [action({ op: 'blocked', reason: 'missing secrets: GH_TOKEN', missing_secrets: ['GH_TOKEN'] })] })],
      { blocked: 1 },
    );
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-action-local-claude-skill-s').textContent).toContain('missing secrets: GH_TOKEN');
    expect(screen.getByTestId('plan-force-partial')).toBeTruthy();
    expect(screen.getByTestId('plan-apply')).toBeDisabled();
  });

  it('a blocked row with missing secrets links to the secrets panel', async () => {
    const onopensecrets = vi.fn();
    const p = plan(
      [hostPlan({ actions: [action({ op: 'blocked', reason: 'missing secrets: GH_TOKEN', missing_secrets: ['GH_TOKEN'] })] })],
      { blocked: 1 },
    );
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {}, onopensecrets });
    await fireEvent.click(screen.getByTestId('plan-action-secrets-local-claude-skill-s'));
    expect(onopensecrets).toHaveBeenCalled();
  });

  it('applying calls catalog_apply_sync with the plan id, then renders outcomes and the restart note', async () => {
    invoke.mockResolvedValueOnce({
      plan_id: 'plan-1',
      started_at: 1,
      finished_at: 2,
      hosts: [
        {
          host_alias: 'local', harness: 'claude', status: 'applied', detail: null, restart_required: true,
          actions: [{ kind: 'skill', name: 's', op: 'create', outcome: 'done', detail: null }],
        },
      ],
    });
    const onapplied = vi.fn();
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied });

    await fireEvent.click(screen.getByTestId('plan-apply'));

    await waitFor(() => expect(screen.getByTestId('plan-outcome-local-claude-skill-s')).toBeTruthy());
    expect(screen.getByTestId('plan-outcome-local-claude-skill-s').textContent).toContain('done');
    expect(screen.getByTestId('plan-restart-local')).toBeTruthy();

    const call = invoke.mock.calls.find((c) => c[0] === 'catalog_apply_sync');
    expect(call).toBeDefined();
    const args = (call![1] as { args: { plan_id: string; force_partial: boolean } }).args;
    expect(args.plan_id).toBe('plan-1');
    expect(args.force_partial).toBe(false);
    expect(onapplied).toHaveBeenCalledWith(expect.objectContaining({ plan_id: 'plan-1' }));
  });

  it('shows a progress line from the syncProgress store while applying, scoped to this plan', async () => {
    let resolveInvoke: (v: unknown) => void = () => {};
    invoke.mockImplementation((cmd: string) => {
      if (cmd === 'catalog_apply_sync') return new Promise((res) => { resolveInvoke = res; });
      return Promise.resolve(null);
    });
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {} });

    await fireEvent.click(screen.getByTestId('plan-apply'));
    syncProgress.set({ plan_id: 'plan-1', host_alias: 'local', harness: 'claude', done: 0, total: 1 });
    await waitFor(() => expect(screen.getByTestId('plan-progress').textContent).toContain('0/1'));

    resolveInvoke({ plan_id: 'plan-1', started_at: 1, finished_at: 2, hosts: [] });
  });

  it('surfaces E_SECRET_MISSING from apply and keeps the force-partial checkbox available', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_SECRET_MISSING', message: 'missing secrets: GH_TOKEN; set them or force_partial' });
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanDialog, { plan: p, onclose: () => {}, onapplied: () => {} });

    await fireEvent.click(screen.getByTestId('plan-apply'));

    await waitFor(() => expect(screen.getByTestId('plan-error')).toBeTruthy());
    expect(screen.getByTestId('plan-error').textContent).toContain('missing secrets');
    expect(screen.getByTestId('plan-force-partial')).toBeTruthy();
    // The plan stays valid — Apply (now with force_partial) is one click away.
    expect(screen.getByTestId('plan-apply')).not.toBeDisabled();
  });
});
