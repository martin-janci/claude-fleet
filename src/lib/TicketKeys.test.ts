import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TicketKeys from './TicketKeys.svelte';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { normalizePrefix, type TicketPrefixRow } from './ticket_keys';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const ROWS: TicketPrefixRow[] = [
  { space_id: 1, project_id: 3, scope: 'project:3', label: 'acme/claude-fleet', prefix: 'CF', tickets: 12 },
  { project_id: 4, scope: 'project:4', label: 'acme/payments', prefix: 'PAY', suggested: true, tickets: 0 },
];

beforeEach(() => {
  inv.mockReset();
  hubStatus.set(STANDALONE);
});

describe('normalizePrefix', () => {
  it('upper-cases what a person types and refuses what is no prefix', () => {
    expect(normalizePrefix(' flt ')).toBe('FLT');
    expect(normalizePrefix('A_1')).toBe('A_1');
    for (const bad of ['F', '1AB', 'ABCDEFGHIJK', 'A-B', '']) expect(normalizePrefix(bad)).toBeNull();
  });
});

describe('TicketKeys', () => {
  it('lists the prefixes and renames one by its space', async () => {
    inv.mockImplementation(async (cmd: string, a?: { args?: Record<string, unknown> }) => {
      if (cmd === 'ticket_prefixes') return ROWS;
      if (cmd === 'set_ticket_prefix') {
        expect(a?.args).toEqual({ space_id: 1, prefix: 'FLT' });
        return [{ ...ROWS[0], prefix: 'FLT', former_prefixes: ['CF'] }, ROWS[1]];
      }
      throw new Error(`unexpected ${cmd}`);
    });
    render(TicketKeys);
    const inputs = await screen.findAllByTestId('ticket-prefix-input');
    expect((inputs[0] as HTMLInputElement).value).toBe('CF');
    expect(screen.getByTestId('ticket-prefix-suggested')).toBeInTheDocument();
    // Unchanged and not suggested: nothing to save on the first row.
    expect(screen.getAllByTestId('ticket-prefix-save')).toHaveLength(1);

    await fireEvent.input(inputs[0], { target: { value: 'flt' } });
    const saves = screen.getAllByTestId('ticket-prefix-save');
    expect(saves).toHaveLength(2);
    await fireEvent.click(saves[0]);
    await waitFor(() => expect(screen.getByTestId('ticket-prefix-former').textContent).toContain('was CF'));
  });

  it('saves a suggested prefix by its project, and says what a bad one lacks', async () => {
    inv.mockImplementation(async (cmd: string, a?: { args?: Record<string, unknown> }) => {
      if (cmd === 'ticket_prefixes') return ROWS;
      if (cmd === 'set_ticket_prefix') {
        expect(a?.args).toEqual({ project_id: 4, prefix: 'PAY' });
        return [ROWS[0], { ...ROWS[1], space_id: 2, suggested: false }];
      }
      throw new Error(`unexpected ${cmd}`);
    });
    render(TicketKeys);
    const inputs = await screen.findAllByTestId('ticket-prefix-input');
    await fireEvent.input(inputs[1], { target: { value: '9x' } });
    expect(screen.getByTestId('ticket-prefix-hint')).toBeInTheDocument();
    await fireEvent.input(inputs[1], { target: { value: 'pay' } });
    await fireEvent.click(screen.getByTestId('ticket-prefix-save'));
    await waitFor(() => expect(screen.queryByTestId('ticket-prefix-suggested')).toBeNull());
  });

  it('a paired desktop says why, and asks nothing', async () => {
    const remote: HubStatus = {
      ...STANDALONE,
      remote: true,
      url: 'https://fleet.example.com',
    } as HubStatus;
    hubStatus.set(remote);
    render(TicketKeys);
    expect(screen.getByTestId('ticket-keys-remote').textContent).toContain('fleet-hub tracker prefix');
    expect(inv).not.toHaveBeenCalled();
  });
});
