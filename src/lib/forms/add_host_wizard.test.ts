import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import type { Wizard } from './wizards';
import { addHostTarget, addHostWizard, runAddHost, TYPED } from './add_host_wizard';
import type { SetupCheck } from '../add_host_wizard';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const fieldOf = (w: Wizard, name: string) => w.spec.steps.flatMap((s) => s.fields).find((f) => f.name === name);

beforeEach(() => inv.mockReset());

describe('add_host wizard', () => {
  it('offers the discovered hosts, then "Another alias"', () => {
    const w = addHostWizard([
      { alias: 'mercury', hostname: '10.0.0.2', user: 'ada', port: null },
      { alias: 'venus', hostname: null, user: null, port: null },
    ] as never);
    expect(fieldOf(w, 'pick')?.options).toEqual([
      ['mercury', 'mercury · ada@10.0.0.2'],
      ['venus', 'venus'],
      [TYPED, 'Another alias'],
    ]);
    expect(w.loader).toBe('sonar');
  });

  it('names the picked or typed alias, and the fleet name falls back to it', () => {
    expect(addHostTarget({ pick: 'mercury', alias: '' })).toEqual({ sshAlias: 'mercury', alias: 'mercury' });
    expect(addHostTarget({ pick: TYPED, ssh_alias: ' venus ', alias: 'v' })).toEqual({ sshAlias: 'venus', alias: 'v' });
  });

  it('runs every check in order, then adds the host and drops the draft', async () => {
    const order: string[] = [];
    inv.mockImplementation(async (cmd: string, a: { args: { key?: string } }) => {
      order.push(a?.args?.key ? `${cmd}:${a.args.key}` : cmd);
      if (cmd === 'run_host_setup_check') return { key: a.args.key, state: 'ok', label: a.args.key, detail: '' } as SetupCheck;
      if (cmd === 'add_host') return { alias: 'm' };
      return true;
    });
    const lines: string[] = [];
    const r = await runAddHost({ pick: 'mercury', alias: 'm' }, (t) => lines.push(t));
    expect(r).toEqual({ ok: true, summary: 'm added, 6 of 6 checks ok' });
    expect(order).toEqual([
      'run_host_setup_check:ssh',
      'run_host_setup_check:tmux',
      'run_host_setup_check:git',
      'run_host_setup_check:agent',
      'run_host_setup_check:disk',
      'run_host_setup_check:agents',
      'add_host',
      'discard_host_setup',
    ]);
    expect(lines[0]).toBe('Waiting for mercury to answer…');
    expect(lines.at(-1)).toBe('Adding m…');
  });

  it('does not add a host SSH cannot reach', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'run_host_setup_check') return { key: 'ssh', state: 'fail', label: 'SSH', detail: 'connection refused' };
      return null;
    });
    const r = await runAddHost({ pick: 'mercury' });
    expect(r).toEqual({ ok: false, error: "Fleet can't reach mercury over SSH: connection refused" });
    expect(inv.mock.calls.map((c) => c[0])).toEqual(['run_host_setup_check']);
  });
});

describe('add_host as a dialog', () => {
  it('asks for an alias only when none of the discovered hosts is picked', async () => {
    const { render, screen, fireEvent } = await import('@testing-library/svelte');
    const WizardDialog = (await import('./WizardDialog.svelte')).default;
    const run = vi.fn();
    render(WizardDialog, {
      props: {
        wizard: addHostWizard([{ alias: 'mercury', hostname: null, user: null, port: null }] as never),
        run,
        onclose: () => {},
      },
    });
    expect(screen.getByTestId('form-field-ssh_alias')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('form-field-pick-mercury'));
    expect(screen.queryByTestId('form-field-ssh_alias')).toBeNull();
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(run).toHaveBeenCalledWith(expect.objectContaining({ pick: 'mercury' }));
  });
});
