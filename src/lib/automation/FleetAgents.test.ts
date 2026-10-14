import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FleetAgents from './FleetAgents.svelte';
import { ruleAgentName, type AgentRule } from '../agents';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const rule = (over: Partial<AgentRule> = {}): AgentRule => ({
  id: 12,
  agent: 'fleet',
  scope: 'l2-prod',
  level: 'propose',
  host_alias: 'mercury',
  project_id: null,
  config: '{}',
  granted_by: 'x',
  granted_at: 1,
  expires_at: null,
  definition: 'l2-support',
  bindings: '[]',
  active: true,
  ...over,
});

const form = {
  spec: 'fleet.form/1',
  title: 'New l2-support instance',
  submit: 'Start',
  steps: [{ title: 'What', fields: [{ name: 'f_instance', type: 'text', label: 'Instance', required: true }] }],
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, a: { action: string }) => {
    if (cmd !== 'agents_fleet') return { rules: [], jobs: [], pending: [] };
    if (a.action === 'definitions')
      return [
        { name: 'l2-support', description: 'L2 investigations', trigger: 'tracker_comments', problems: [], writes: 'It may write: tracker comment.' },
        { name: 'broken', description: 'x', trigger: null, problems: ['trigger: needs query'], writes: '' },
      ];
    if (a.action === 'form') return form;
    if (a.action === 'grant') return { changed: 1, id: 13 };
    return { changed: 1 };
  });
});

describe('the fleet agents on the Agents tab', () => {
  it('lists the catalog definitions with their instances', async () => {
    render(FleetAgents, { props: { rules: [rule(), rule({ id: 3, agent: 'review', definition: undefined })] } });
    const defs = await screen.findAllByTestId('fleet-agent');
    expect(defs).toHaveLength(2);
    expect(defs[0].textContent).toContain('tracker_comments · It may write: tracker comment.');
    expect(screen.getByTestId('fleet-agent-problems').textContent).toContain('trigger: needs query');
    const buttons = screen.getAllByTestId('fleet-agent-new') as HTMLButtonElement[];
    expect(buttons[1].disabled).toBe(true);
    const inst = screen.getAllByTestId('fleet-instance');
    expect(inst).toHaveLength(1);
    expect(inst[0].textContent).toContain('l2-prod at propose on mercury');
    expect(screen.queryByTestId('fleet-instance-run')).toBeNull();
  });

  it('opens the grant form and grants from its answers', async () => {
    render(FleetAgents, { props: { rules: [] } });
    await fireEvent.click((await screen.findAllByTestId('fleet-agent-new'))[0]);
    expect(invoke).toHaveBeenCalledWith('agents_fleet', { action: 'form', definition: 'l2-support', id: null, answers: null });
    const name = await screen.findByTestId('form-field-f_instance');
    await fireEvent.input(name, { target: { value: 'l2-prod' } });
    await fireEvent.keyDown(name, { key: 'Enter' });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('agents_fleet', {
        action: 'grant',
        definition: 'l2-support',
        id: null,
        answers: { f_instance: 'l2-prod' },
      }),
    );
    await waitFor(() => expect(screen.queryByTestId('form-field-f_instance')).toBeNull());
  });

  it('edits and revokes an instance', async () => {
    render(FleetAgents, { props: { rules: [rule()] } });
    await fireEvent.click(await screen.findByTestId('fleet-instance-edit'));
    expect(invoke).toHaveBeenCalledWith('agents_fleet', { action: 'form', definition: 'l2-support', id: 12, answers: null });
    await fireEvent.click(screen.getByTestId('fleet-instance-revoke'));
    await waitFor(() => expect(screen.getByTestId('fleet-agents-note').textContent).toBe('Instance 12 revoked.'));
    expect(invoke).toHaveBeenCalledWith('agents_fleet', { action: 'revoke', definition: null, id: 12, answers: null });
  });

  it('names a rule by its agent or its definition', () => {
    expect(ruleAgentName({ agent: 'review' })).toBe('PR reviewer');
    expect(ruleAgentName({ agent: 'fleet', definition: 'l2-support' })).toBe('l2-support');
  });
});
