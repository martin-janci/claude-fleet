import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('./lost_found', async () => {
  const actual = await vi.importActual<typeof import('./lost_found')>('./lost_found');
  return { ...actual, lostTarget: vi.fn() };
});

import LostTargetForm from './LostTargetForm.svelte';
import { lostTarget, type LostTarget } from './lost_found';
import { projects, type ProjectRow } from './projects';

const mockedTarget = lostTarget as unknown as ReturnType<typeof vi.fn>;

function project(id: number, repo: string): ProjectRow {
  return {
    id,
    owner: 'acme',
    repo,
    base_path: `/p/acme/${repo}`,
    last_session_at: id,
    adopted: false,
    system: false,
  };
}

async function settle() {
  for (let i = 0; i < 4; i++) await tick();
}

function mount(target: LostTarget, over: Record<string, unknown> = {}) {
  mockedTarget.mockResolvedValueOnce({ ok: true, value: target });
  const onsubmit = vi.fn().mockResolvedValue(null);
  const oncancel = vi.fn();
  render(LostTargetForm, {
    props: {
      action: 'Adopt',
      entry: 'fleet-trn-scratch',
      args: { session_id: 7 },
      onsubmit,
      oncancel,
      ...over,
    },
  });
  return { onsubmit, oncancel };
}

const select = () => screen.getByTestId('lost-target-project') as HTMLSelectElement;

describe('LostTargetForm (4.12)', () => {
  beforeEach(() => {
    mockedTarget.mockReset();
    projects.set([
      { project: project(1, 'papaya-pos'), worktrees: [] },
      { project: project(2, 'payments-api'), worktrees: [] },
    ]);
  });

  it('prefills Jev’s proposal and says who proposed it, and why', async () => {
    mount({ project_id: 1, source: 'jev', reason: 'directory and the name scratch', confidence_pct: 82 });
    await settle();
    expect(mockedTarget).toHaveBeenCalledWith({ session_id: 7 });
    expect(select().value).toBe('1');
    expect(select()).toHaveClass('ai-pre');
    const chip = screen.getByTestId('lost-target-proposed');
    expect(chip.textContent).toContain('Proposed by Jev');
    expect(chip.textContent).toContain('directory and the name scratch');
    expect(screen.queryByTestId('lost-target-unsure')).toBeNull();
  });

  it('unsure leaves the form blank and says so', async () => {
    mount({ unsure: true }, { action: 'Restore', requireProject: true });
    await settle();
    expect(select().value).toBe('');
    expect(screen.getByTestId('lost-target-unsure').textContent).toBe(
      'Jev was unsure, so nothing is filled in',
    );
    expect(screen.queryByTestId('lost-target-proposed')).toBeNull();
    expect((screen.getByTestId('lost-target-submit') as HTMLButtonElement).disabled).toBe(true);
  });

  it('a weak answer prefills nothing', async () => {
    mount({ project_id: 1, source: 'jev', confidence_pct: 30 });
    await settle();
    expect(select().value).toBe('');
    expect(screen.queryByTestId('lost-target-proposed')).toBeNull();
  });

  it('Change clears the field and the chip', async () => {
    mount({ project_id: 2, source: 'rule', reason: 'its directory is in this project' });
    await settle();
    expect(select().value).toBe('2');
    await fireEvent.click(screen.getByTestId('lost-target-proposed-change'));
    await tick();
    expect(select().value).toBe('');
    expect(screen.queryByTestId('lost-target-proposed')).toBeNull();
  });

  it('Adopt still asks to confirm, then submits the chosen project', async () => {
    const { onsubmit } = mount({ project_id: 1, source: 'jev', confidence_pct: 90 });
    await settle();
    await fireEvent.click(screen.getByTestId('lost-target-submit'));
    await tick();
    expect(onsubmit).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain('Adopt fleet-trn-scratch into acme/papaya-pos?');
    await fireEvent.click(screen.getByTestId('lost-target-confirm'));
    await settle();
    expect(onsubmit).toHaveBeenCalledWith(1, null);
  });

  it('J10: offers the ticket the branch names, ticked, and links it only on confirm', async () => {
    const ticket = { key: 'PD-2412', title: 'Receipt totals', source: 'rule' as const, reason: 'its branch pd-2412-x names it' };
    const { onsubmit } = mount(
      { project_id: 1, source: 'rule', reason: 'its directory is in this project', ticket },
      { action: 'Restore', requireProject: true },
    );
    await settle();
    const box = screen.getByTestId('lost-target-ticket') as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(document.body.textContent).toContain('Link to PD-2412 · Receipt totals');
    expect(screen.getByTestId('lost-target-ticket-proposed').textContent).toContain('Proposed by a rule');
    expect(onsubmit).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('lost-target-submit'));
    await fireEvent.click(screen.getByTestId('lost-target-confirm'));
    await settle();
    expect(onsubmit).toHaveBeenCalledWith(1, 'PD-2412');
  });

  it('J10: an unticked ticket is not linked', async () => {
    const ticket = { key: 'PD-2412', source: 'rule' as const, reason: 'r' };
    const { onsubmit } = mount({ project_id: 1, source: 'rule', ticket }, { action: 'Restore', requireProject: true });
    await settle();
    await fireEvent.click(screen.getByTestId('lost-target-ticket'));
    await fireEvent.click(screen.getByTestId('lost-target-submit'));
    await fireEvent.click(screen.getByTestId('lost-target-confirm'));
    await settle();
    expect(onsubmit).toHaveBeenCalledWith(1, null);
  });

  it('a late proposal never overwrites what the person picked', async () => {
    let resolve!: (v: unknown) => void;
    mockedTarget.mockReturnValueOnce(new Promise((r) => (resolve = r)));
    render(LostTargetForm, {
      props: {
        action: 'Adopt',
        entry: 'scratch',
        args: { session_id: 7 },
        onsubmit: vi.fn(),
        oncancel: vi.fn(),
      },
    });
    await tick();
    select().value = '2';
    await fireEvent.change(select());
    resolve({ ok: true, value: { project_id: 1, source: 'jev', confidence_pct: 95 } });
    await settle();
    expect(select().value).toBe('2');
  });

  it('a failed proposal only loses the prefill', async () => {
    mockedTarget.mockResolvedValueOnce({ ok: false, error: { code: 'E_SHELL', message: 'no ssh' } });
    render(LostTargetForm, {
      props: {
        action: 'Adopt',
        entry: 'scratch',
        args: { session_id: 7 },
        onsubmit: vi.fn(),
        oncancel: vi.fn(),
      },
    });
    await settle();
    expect(select().value).toBe('');
    expect(screen.queryByTestId('lost-target-error')).toBeNull();
  });
});
