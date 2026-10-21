// The sidebar's fleet filter (review w19-8): one Tab stop and arrow keys,
// the link state as text rather than an aria-label on a span, and axe-clean.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));
import FleetSwitcher from './FleetSwitcher.svelte';
import { PRIMARY, _resetFleetsForTest, _setFleetsForTest, fleetFilter, type FleetInfo } from './fleets';
import { expectAccessible } from './a11y_check';

function fleet(id: string): FleetInfo {
  return {
    id,
    label: id,
    url: id === PRIMARY ? null : `https://${id}.example.com`,
    color: '#5b8def',
    primary: id === PRIMARY,
    local: id === PRIMARY,
    client_name: null,
    connection: id === PRIMARY ? null : { state: 'connected' },
    unavailable: null,
  } as FleetInfo;
}

beforeEach(() => {
  _resetFleetsForTest();
  _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
  fleetFilter.set('all');
});

describe('FleetSwitcher', () => {
  it('is one Tab stop; arrows pick the next fleet', async () => {
    const { container } = render(FleetSwitcher);
    const all = screen.getByTestId('fleet-filter-all');
    const radios = screen.getAllByRole('radio');
    expect(radios.map((r) => r.getAttribute('tabindex'))).toEqual(['0', '-1', '-1']);
    all.focus();
    await fireEvent.keyDown(all, { key: 'ArrowRight' });
    expect(get(fleetFilter)).toBe(PRIMARY);
    expect(document.activeElement).toBe(screen.getByTestId(`fleet-filter-${PRIMARY}`));
    await fireEvent.keyDown(document.activeElement!, { key: 'End' });
    expect(get(fleetFilter)).toBe('company');
    expect(container.querySelector('.dot[aria-label]')).toBeNull();
    expect(screen.getByTestId('fleet-filter-company').textContent).toContain('live');
    await expectAccessible(container);
  });
});
