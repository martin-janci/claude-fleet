// Control's escalation cards (review w19-11): each card's buttons are
// described by its line, and Later keeps focus in the list.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
import ControlEscalations from './ControlEscalations.svelte';
import { hosts } from './hosts';
import { fleetHosts } from './hosts_fixture';

beforeEach(() => {
  const since = Date.UTC(2026, 7, 9, 12) / 1000;
  hosts.set(
    fleetHosts().map((h) =>
      h.alias === 'mefistos' || h.alias === 'claude-fleet-oci'
        ? { ...h, signed_out_logins: [{ profile: null, since, checked_at: since }] }
        : h,
    ),
  );
});

describe('ControlEscalations', () => {
  it('names each button by its card and keeps focus on Later', async () => {
    render(ControlEscalations);
    await tick();
    const cards = screen.getAllByTestId('control-escalation');
    expect(cards).toHaveLength(2);
    const lefts = screen.getAllByTestId('control-escalation-later');
    for (const [i, b] of lefts.entries()) {
      const desc = document.getElementById(b.getAttribute('aria-describedby') ?? '');
      expect(desc?.textContent).toBe(cards[i].querySelector('.line')?.textContent);
    }
    const secondLine = cards[1].querySelector('.line')?.textContent;
    lefts[0].focus();
    await fireEvent.click(lefts[0]);
    await tick();
    await tick();
    const now = screen.getAllByTestId('control-escalation-later');
    expect(now).toHaveLength(1);
    expect(document.activeElement).toBe(now[0]);
    expect(document.getElementById(now[0].getAttribute('aria-describedby') ?? '')?.textContent).toBe(secondLine);
  });

  it('Later on the last card moves focus to the composer, not the page', async () => {
    const since = Date.UTC(2026, 7, 10, 12) / 1000;
    hosts.set(
      fleetHosts().map((h) =>
        h.alias === 'mefistos' ? { ...h, signed_out_logins: [{ profile: null, since, checked_at: since }] } : h,
      ),
    );
    // A form field in the transcript comes first in document order; focus
    // must still land in the Prompt box, not there.
    const decoy = document.createElement('textarea');
    document.body.appendChild(decoy);
    const box = document.createElement('textarea');
    box.dataset.testid = 'conv-composer-input';
    document.body.appendChild(box);
    try {
      render(ControlEscalations);
      await tick();
      const lefts = screen.getAllByTestId('control-escalation-later');
      expect(lefts).toHaveLength(1);
      lefts[0].focus();
      await fireEvent.click(lefts[0]);
      await tick();
      await tick();
      expect(screen.queryByTestId('control-escalations')).toBeNull();
      expect(document.activeElement).toBe(box);
    } finally {
      decoy.remove();
      box.remove();
    }
  });
});
