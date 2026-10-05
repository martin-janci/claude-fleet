import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SyncPlanView from './SyncPlanView.svelte';
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

describe('SyncPlanView', () => {
  it('renders header counts and grouped host/action rows', () => {
    const p = plan(
      [hostPlan({ actions: [action({ kind: 'skill', name: 's', op: 'create' })] })],
      { create: 1 },
    );
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-counts').textContent).toContain('create: 1');
    expect(screen.getByTestId('plan-host-local-claude')).toBeTruthy();
    expect(screen.getByTestId('plan-action-local-claude-skill-s').textContent).toContain('create');
  });

  it('shows the op and the counts as Badges, toned by what the op does', () => {
    const p = plan(
      [hostPlan({ actions: [action({ op: 'overwrite' }), action({ name: 't', op: 'create' })] })],
      { overwrite: 1, create: 1 },
    );
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
    const row = screen.getByTestId('plan-action-local-claude-skill-s');
    expect(row.querySelector('.badge.crit')?.textContent).toBe('overwrite');
    expect(screen.getByTestId('plan-action-local-claude-skill-t').querySelector('.badge.ok')?.textContent).toBe('create');
    expect(screen.getByTestId('plan-counts').querySelectorAll('.badge')).toHaveLength(2);
  });

  it('styles an unverified update reason as a warning note, never as an error', () => {
    const reason = "the catalog changed; the host copy predates fleet's file hashes, so a host edit cannot be ruled out";
    const p = plan([hostPlan({ actions: [action({ op: 'update', reason })] })], { update: 1 });
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
    const note = screen.getByTestId('plan-note-local-claude-skill-s');
    expect(note.textContent).toBe(reason);
    expect(note.className).toContain('note');
    expect(note.className).not.toContain('reason');
    expect(note.className).not.toContain('error');
    expect(screen.getByTestId('plan-action-local-claude-skill-s').querySelector('.reason')).toBeNull();
  });

  it('styles a caution reason (update/overwrite/plugin_update) as warn and an informational one (noop, remove) as muted', () => {
    const p = plan(
      [
        hostPlan({
          actions: [
            action({ name: 'u', op: 'update', reason: 'host copy unverified' }),
            action({ name: 'o', op: 'overwrite', reason: 'edited on host' }),
            action({ name: 'pu', op: 'plugin_update', reason: 'plugin moved' }),
            action({ name: 'n', op: 'noop', reason: 'private; withheld from org host, not removed' }),
            action({ name: 'r', op: 'remove', reason: 'no longer in the catalog' }),
          ],
        }),
      ],
      { update: 1 },
    );
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
    const note = (n: string) => screen.getByTestId(`plan-note-local-claude-skill-${n}`);
    for (const n of ['u', 'o', 'pu']) expect(note(n).className).toContain('caution');
    for (const n of ['n', 'r']) {
      expect(note(n).className).toContain('note');
      expect(note(n).className).not.toContain('caution');
    }
    expect(note('n').textContent).toBe('private; withheld from org host, not removed');
  });

  it('renders the Apply button red (danger) when the plan overwrites something', () => {
    const p = plan([hostPlan({ actions: [action({ op: 'overwrite' })] })], { overwrite: 1 });
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-apply').className).toContain('danger');
  });

  it('does not mark Apply as danger for a purely additive plan', () => {
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
    expect(screen.getByTestId('plan-apply').className).not.toContain('danger');
  });

  it('shows the reason on a blocked row, offers the force-partial checkbox, and disables Apply when nothing is applicable', () => {
    const p = plan(
      [hostPlan({ actions: [action({ op: 'blocked', reason: 'missing secrets: GH_TOKEN', missing_secrets: ['GH_TOKEN'] })] })],
      { blocked: 1 },
    );
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });
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
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {}, onopensecrets });
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
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied });

    await fireEvent.click(screen.getByTestId('plan-apply'));

    await waitFor(() => expect(screen.getByTestId('plan-outcome-local-claude-skill-s')).toBeTruthy());
    expect(screen.getByTestId('plan-outcome-local-claude-skill-s').textContent).toContain('done');
    expect(screen.getByTestId('plan-outcome-local-claude-skill-s').className).toContain('ok');
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
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });

    await fireEvent.click(screen.getByTestId('plan-apply'));
    syncProgress.set({ plan_id: 'plan-1', host_alias: 'local', harness: 'claude', done: 0, total: 1 });
    await waitFor(() => expect(screen.getByTestId('plan-progress').textContent).toContain('0/1'));

    resolveInvoke({ plan_id: 'plan-1', started_at: 1, finished_at: 2, hosts: [] });
  });

  it('shows a warning and a "Plan anyway" button for a host skipped as unlayered, and re-plans with allow_unlayered on click', async () => {
    const skipped = hostPlan({
      host_alias: 'oci',
      status: 'skipped',
      detail: 'no layers assigned: syncing would install the whole catalog here. Assign a role first (set_host_layers), or plan with allow_unlayered.',
    });
    const p = plan([skipped]);
    const onreplanned = vi.fn();
    invoke.mockResolvedValueOnce(plan([hostPlan({ host_alias: 'oci', actions: [action()] })], { create: 1 }));
    render(SyncPlanView, {
      plan: p,
      filter: { hostAlias: 'oci' },
      onclose: () => {},
      onapplied: () => {},
      onreplanned,
    });

    expect(screen.getByTestId('plan-unlayered-oci-claude').textContent).toContain('no layers assigned');
    await fireEvent.click(screen.getByTestId('plan-anyway-oci-claude'));

    await waitFor(() => expect(onreplanned).toHaveBeenCalled());
    const call = invoke.mock.calls.find((c) => c[0] === 'catalog_plan_sync');
    expect(call).toBeDefined();
    const args = (call![1] as { args: { host_alias: string; allow_unlayered: boolean } }).args;
    expect(args.host_alias).toBe('oci');
    expect(args.allow_unlayered).toBe(true);
    expect(onreplanned).toHaveBeenCalledWith(expect.objectContaining({ counts: { create: 1 } }));
  });

  it('from a fleet-wide filter ({}), "Plan anyway" re-plans scoped to the clicked host, not every unlayered host', async () => {
    const oci = hostPlan({
      host_alias: 'oci',
      status: 'skipped',
      detail: 'no layers assigned: syncing would install the whole catalog here. Assign a role first (set_host_layers), or plan with allow_unlayered.',
    });
    const trn = hostPlan({
      host_alias: 'trn',
      status: 'skipped',
      detail: 'no layers assigned: syncing would install the whole catalog here. Assign a role first (set_host_layers), or plan with allow_unlayered.',
    });
    const p = plan([oci, trn]);
    const onreplanned = vi.fn();
    invoke.mockResolvedValueOnce(plan([hostPlan({ host_alias: 'oci', actions: [action()] })], { create: 1 }));
    render(SyncPlanView, {
      plan: p,
      filter: {},
      onclose: () => {},
      onapplied: () => {},
      onreplanned,
    });

    await fireEvent.click(screen.getByTestId('plan-anyway-oci-claude'));

    await waitFor(() => expect(onreplanned).toHaveBeenCalled());
    const call = invoke.mock.calls.find((c) => c[0] === 'catalog_plan_sync');
    expect(call).toBeDefined();
    const args = (call![1] as { args: { host_alias: string | null; allow_unlayered: boolean } }).args;
    expect(args.host_alias).toBe('oci');
    expect(args.allow_unlayered).toBe(true);
  });

  it('surfaces E_SECRET_MISSING from apply and keeps the force-partial checkbox available', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_SECRET_MISSING', message: 'missing secrets: GH_TOKEN; set them or force_partial' });
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanView, { plan: p, onclose: () => {}, onapplied: () => {} });

    await fireEvent.click(screen.getByTestId('plan-apply'));

    await waitFor(() => expect(screen.getByTestId('plan-error')).toBeTruthy());
    expect(screen.getByTestId('plan-error').textContent).toContain('missing secrets');
    expect(screen.getByTestId('plan-error').getAttribute('role')).toBe('alert');
    expect(screen.getByTestId('plan-force-partial')).toBeTruthy();
    // The plan stays valid — Apply (now with force_partial) is one click away.
    expect(screen.getByTestId('plan-apply')).not.toBeDisabled();
  });
});

describe('SyncPlanView as a view', () => {
  it('Back closes it', async () => {
    const onclose = vi.fn();
    render(SyncPlanView, { plan: plan([]), onclose });
    await fireEvent.click(screen.getByTestId('plan-back'));
    expect(onclose).toHaveBeenCalled();
  });

  it('is a labelled section, not a dialog, and takes focus on Back', () => {
    render(SyncPlanView, { plan: plan([]), onclose: vi.fn() });
    const root = screen.getByTestId('sync-plan-view');
    expect(root.tagName).toBe('SECTION');
    expect(root.getAttribute('aria-label')).toBe('Sync plan');
    expect(document.querySelector('dialog,[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('plan-back'));
  });

  it('Back is disabled while an apply runs', async () => {
    invoke.mockImplementation(() => new Promise(() => {}));
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanView, { plan: p, onclose: vi.fn(), onapplied: vi.fn() });
    await fireEvent.click(screen.getByTestId('plan-apply'));
    expect(screen.getByTestId('plan-back')).toBeDisabled();
  });

  it('the Apply is the view’s one primary button', () => {
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanView, { plan: p, onclose: vi.fn(), onapplied: vi.fn() });
    expect(Array.from(document.querySelectorAll('.btn--primary')).map((b) => b.getAttribute('data-testid'))).toEqual(['plan-apply']);
  });
});

describe('focus stays in the view', () => {
  const inView = () => screen.getByTestId('sync-plan-view').contains(document.activeElement);

  it('after "Plan anyway" re-renders the host header (the clicked button is gone), focus is back on Back', async () => {
    const skipped = hostPlan({ host_alias: 'oci', status: 'skipped', detail: 'no layers assigned: x' });
    invoke.mockResolvedValue(plan([hostPlan({ host_alias: 'oci', actions: [action()] })], { create: 1 }));
    const { rerender } = render(SyncPlanView, { plan: plan([skipped]), onclose: vi.fn(), onreplanned: () => {} });
    const onreplanned = (p: SyncPlan) => void rerender({ plan: p, onclose: vi.fn(), onreplanned: () => {} });
    await rerender({ plan: plan([skipped]), onclose: vi.fn(), onreplanned });
    const btn = screen.getByTestId('plan-anyway-oci-claude');
    btn.focus();
    await fireEvent.click(btn);
    await waitFor(() => expect(screen.queryByTestId('plan-anyway-oci-claude')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('plan-back')));
  });

  it('while an apply runs focus is on the view itself (Apply and Back are disabled), and Back again once it settles', async () => {
    let finish: (v: unknown) => void = () => {};
    invoke.mockImplementation(() => new Promise((res) => { finish = res; }));
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanView, { plan: p, onclose: vi.fn(), onapplied: vi.fn() });
    const apply = screen.getByTestId('plan-apply');
    apply.focus();
    await fireEvent.click(apply);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('sync-plan-view')));
    finish({ plan_id: 'plan-1', started_at: 1, finished_at: 2, hosts: [] });
    await waitFor(() => expect(screen.getByTestId('plan-apply')).toBeDisabled());
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('plan-back')));
    expect(inView()).toBe(true);
  });

  it('a failed apply returns focus to Back too', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_X', message: 'boom' });
    const p = plan([hostPlan({ actions: [action({ op: 'create' })] })], { create: 1 });
    render(SyncPlanView, { plan: p, onclose: vi.fn(), onapplied: vi.fn() });
    screen.getByTestId('plan-apply').focus();
    await fireEvent.click(screen.getByTestId('plan-apply'));
    await waitFor(() => expect(screen.getByTestId('plan-error')).toBeTruthy());
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('plan-back')));
  });

  it('does not take focus a person moved to another field', async () => {
    invoke.mockResolvedValue(plan([hostPlan({ host_alias: 'oci', actions: [] })]));
    const skipped = hostPlan({ host_alias: 'oci', status: 'skipped', detail: 'no layers assigned: x' });
    render(SyncPlanView, { plan: plan([skipped]), onclose: vi.fn(), onreplanned: () => {} });
    const other = document.createElement('input');
    document.body.appendChild(other);
    await fireEvent.click(screen.getByTestId('plan-anyway-oci-claude'));
    other.focus();
    await new Promise((r) => setTimeout(r, 20));
    expect(document.activeElement).toBe(other);
    other.remove();
  });
});

describe('review mode (R15)', () => {
  const owned = { assets: new Set(['skill/w', 'skill/v']), catalogs: new Set(['personal']) };
  const reviewPlan = () => plan([hostPlan({ host_alias: 'oci', actions: [
    action({ name: 'w', op: 'update', host_copy: 'unverified', catalog: 'personal' }),
    action({ name: 'v', op: 'create', catalog: 'personal' }),
    action({ name: 'other', op: 'create', catalog: 'personal' }),
  ] })], { update: 1, create: 2 });

  it('has no Apply, marks what the card would hold, and says so', () => {
    render(SyncPlanView, { plan: reviewPlan(), mode: 'review', owned, onclose: vi.fn() });
    expect(screen.queryByTestId('plan-apply')).toBeNull();
    expect(screen.queryByTestId('plan-cancel')).toBeNull();
    expect(screen.queryByTestId('plan-force-partial')).toBeNull();
    expect(screen.getByTestId('plan-held-oci-claude-skill-w')).toHaveTextContent('held — sync it yourself');
    expect(screen.getByTestId('plan-action-oci-claude-skill-v')).toBeTruthy();
    expect(screen.queryByTestId('plan-held-oci-claude-skill-v')).toBeNull();
    expect(screen.queryByTestId('plan-action-oci-claude-skill-other')).toBeNull();
    expect(screen.getByTestId('plan-review-note')).toHaveTextContent('Roll out applies only');
    expect(screen.getByRole('heading', { name: 'Roll-out review' })).toBeTruthy();
  });

  it('counts what it shows, by op and without noop — not the whole host plans', () => {
    const p = plan([hostPlan({ host_alias: 'oci', actions: [
      action({ name: 'w', op: 'update', host_copy: 'unverified', catalog: 'personal' }),
      action({ name: 'v', op: 'create', catalog: 'personal' }),
      action({ name: 'n', op: 'noop', catalog: 'personal' }),
      action({ name: 'other', op: 'create', catalog: 'personal' }),
      action({ name: 'gone', op: 'remove', catalog: 'personal' }),
    ] })], { update: 1, create: 2, remove: 1 });
    render(SyncPlanView, { plan: p, mode: 'review', owned: { assets: new Set(['skill/w', 'skill/v', 'skill/n']), catalogs: new Set(['personal']) }, onclose: vi.fn() });
    expect(Array.from(screen.getByTestId('plan-counts').querySelectorAll('.badge')).map((b) => b.textContent)).toEqual(['update: 1', 'create: 1']);
  });

  it('says so when the review shows nothing to do', () => {
    render(SyncPlanView, { plan: reviewPlan(), mode: 'review', owned: { assets: new Set(['skill/zzz']), catalogs: new Set(['personal']) }, onclose: vi.fn() });
    expect(screen.getByTestId('plan-counts')).toHaveTextContent('Nothing to do.');
  });

  it('holds an update over a copy the planner did not verify unchanged, an overwrite, and a remove', () => {
    const p = plan([hostPlan({ host_alias: 'oci', actions: [
      action({ name: 'a', op: 'update', host_copy: 'unchanged', catalog: 'personal' }),
      action({ name: 'b', op: 'update', host_copy: 'edited', catalog: 'personal' }),
      action({ name: 'c', op: 'overwrite', catalog: 'personal' }),
      action({ name: 'd', op: 'remove', catalog: 'personal' }),
    ] })]);
    const all = { assets: new Set(['skill/a', 'skill/b', 'skill/c', 'skill/d']), catalogs: new Set(['personal']) };
    render(SyncPlanView, { plan: p, mode: 'review', owned: all, onclose: vi.fn() });
    expect(screen.getByTestId('plan-action-oci-claude-skill-a')).toBeTruthy();
    for (const n of ['b', 'c', 'd']) expect(screen.getByTestId(`plan-held-oci-claude-skill-${n}`), n).toBeTruthy();
  });

  it('shows every action when no ownership is given', () => {
    render(SyncPlanView, { plan: reviewPlan(), mode: 'review', onclose: vi.fn() });
    expect(screen.getByTestId('plan-action-oci-claude-skill-other')).toBeTruthy();
  });

  it('Plan anyway stays host-scoped in a review', async () => {
    invoke.mockResolvedValue({ id: 'p2', computed_at: 2, hosts: [], counts: {} });
    const onreplanned = vi.fn();
    const p = plan([hostPlan({ host_alias: 'oci', status: 'skipped', detail: 'no layers assigned to oci' })]);
    render(SyncPlanView, { plan: p, mode: 'review', owned, filter: {}, onclose: vi.fn(), onreplanned });
    await fireEvent.click(screen.getByTestId('plan-anyway-oci-claude'));
    await waitFor(() => expect(onreplanned).toHaveBeenCalled());
    const args = (invoke.mock.calls.find((c) => c[0] === 'catalog_plan_sync')![1] as { args: { host_alias: string; allow_unlayered: boolean } }).args;
    expect([args.host_alias, args.allow_unlayered]).toEqual(['oci', true]);
  });
});
