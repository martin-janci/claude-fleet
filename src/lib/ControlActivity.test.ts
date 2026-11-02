// Compact Control (2026-10-30): the cards above Control's composer are one
// row of counts; a count opens its cards, Escape folds them back, and a
// plan to tick opens the handed-off list by itself.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
import ControlActivity from './ControlActivity.svelte';
import { hosts } from './hosts';
import { fleetHosts } from './hosts_fixture';
import { handoffs, type ControlHandoff } from './handoffs';

const since = Date.UTC(2026, 9, 30, 9) / 1000;

beforeEach(() => {
  handoffs.set([]);
  // Two hosts with a signed-out login: two "needs a login" escalations.
  hosts.set(
    fleetHosts().map((h) =>
      h.alias === 'mefistos' || h.alias === 'claude-fleet-oci'
        ? { ...h, signed_out_logins: [{ profile: null, since, checked_at: since }] }
        : h,
    ),
  );
});

function plan(id: number): ControlHandoff {
  return {
    id,
    at: since,
    kind: 'tree',
    tool: 'propose_tree',
    item: { id: 100 + id, title: 'Remove the minimap', status: 'todo' },
    items: [{ id: 200 + id, title: 'Drop the minimap layer', status: 'todo', proposal_state: 'proposed' }],
  };
}

describe('ControlActivity', () => {
  it('shows a count instead of the cards, and the count opens them', async () => {
    render(ControlActivity);
    await tick();
    const btn = screen.getByTestId('control-activity-needs-btn');
    expect(btn.textContent).toContain('2 need you');
    expect(btn.getAttribute('aria-expanded')).toBe('false');
    // Folded, the cards are mounted (their counts keep running) but hidden.
    expect(screen.getByTestId('control-activity-needs').hidden).toBe(true);
    expect(screen.getAllByTestId('control-escalation')).toHaveLength(2);

    await fireEvent.click(btn);
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('control-activity-needs').hidden).toBe(false);

    await fireEvent.click(btn);
    expect(screen.getByTestId('control-activity-needs').hidden).toBe(true);
  });

  it('Escape folds the open list and puts focus back on its count', async () => {
    render(ControlActivity);
    await tick();
    const btn = screen.getByTestId('control-activity-needs-btn');
    await fireEvent.click(btn);
    const later = screen.getAllByTestId('control-escalation-later')[0];
    later.focus();
    await fireEvent.keyDown(later, { key: 'Escape' });
    expect(screen.getByTestId('control-activity-needs').hidden).toBe(true);
    expect(document.activeElement).toBe(btn);
  });

  it('has no row when nothing waits', async () => {
    hosts.set(fleetHosts());
    render(ControlActivity);
    await tick();
    expect(screen.queryByTestId('control-activity-strip')).toBeNull();
  });

  it('opens the handed-off list by itself when a plan waits for a tick', async () => {
    render(ControlActivity);
    await tick();
    expect(screen.getByTestId('control-activity-handoffs').hidden).toBe(true);
    handoffs.set([plan(1)]);
    await tick();
    await tick();
    const btn = screen.getByTestId('control-activity-handoffs-btn');
    expect(btn.textContent).toContain('1 plan to review');
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('control-activity-handoffs').hidden).toBe(false);
  });
});
