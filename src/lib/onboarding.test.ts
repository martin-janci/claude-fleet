import { describe, it, expect } from 'vitest';
import { deriveSteps, allRequiredComplete, type DeriveInputs, type LocalPrereqs } from './onboarding';

const okPrereqs: LocalPrereqs = {
  claude_ok: true,
  claude_version: '1.0.39',
  tmux_ok: true,
  tmux_version: '3.4',
  projects_path: '/home/u/projects/github.com',
  projects_readable: true,
  projects_count: 5,
};

const base: DeriveInputs = {
  prereqs: null,
  visibleHostCount: 0,
  provisionedHost: false,
  firstHostAlias: null,
  tunnels: [],
  projectCount: 0,
  mcpEnabled: false,
  workSessionCount: 0,
};

const byId = (steps: ReturnType<typeof deriveSteps>, id: string) => {
  const s = steps.find((s) => s.id === id);
  if (!s) throw new Error(`step "${id}" not found`);
  return s;
};

describe('deriveSteps', () => {
  it('fresh state: prereqs is active, rest pending, none done', () => {
    const steps = deriveSteps(base);
    expect(byId(steps, 'prereqs').status).toBe('active');
    expect(byId(steps, 'add-host').status).toBe('pending');
    expect(allRequiredComplete(steps)).toBe(false);
  });

  it('marks prereqs done and lists versions', () => {
    const steps = deriveSteps({ ...base, prereqs: okPrereqs });
    const p = byId(steps, 'prereqs');
    expect(p.status).toBe('done');
    expect(p.sublabel).toContain('1.0.39');
    expect(byId(steps, 'add-host').status).toBe('active');
  });

  it('lists missing tools in the prereq sublabel', () => {
    const steps = deriveSteps({ ...base, prereqs: { ...okPrereqs, tmux_ok: false } });
    expect(byId(steps, 'prereqs').status).not.toBe('done');
    expect(byId(steps, 'prereqs').sublabel).toContain('tmux');
  });

  it('provisioned host with MCP off: provision done with warn badge', () => {
    const steps = deriveSteps({ ...base, provisionedHost: true, mcpEnabled: false });
    const prov = byId(steps, 'provision');
    expect(prov.status).toBe('done');
    expect(prov.badge).toEqual({ text: 'tunnel: starts with Control API', tone: 'warn' });
  });

  it('provisioned host with MCP on + tunnel up: provision done with up badge', () => {
    const steps = deriveSteps({
      ...base,
      provisionedHost: true,
      mcpEnabled: true,
      tunnels: [{ host_alias: 'mefistos', state: 'up' }],
    });
    const prov = byId(steps, 'provision');
    expect(prov.status).toBe('done');
    expect(prov.badge).toEqual({ text: 'tunnel: up', tone: 'up' });
  });

  it('provisioned host with MCP on + tunnel down: provision not done, retry badge', () => {
    const steps = deriveSteps({
      ...base,
      provisionedHost: true,
      mcpEnabled: true,
      tunnels: [{ host_alias: 'mefistos', state: 'down' }],
    });
    const prov = byId(steps, 'provision');
    expect(prov.status).not.toBe('done');
    expect(prov.badge?.tone).toBe('warn');
  });

  it('Control API is optional and never active', () => {
    const steps = deriveSteps({ ...base, prereqs: okPrereqs, visibleHostCount: 1 });
    const mcp = byId(steps, 'mcp');
    expect(mcp.optional).toBe(true);
    expect(mcp.status).not.toBe('active');
  });

  it('session becomes active when all required steps before it are done and mcp is not', () => {
    const steps = deriveSteps({
      ...base,
      prereqs: okPrereqs,
      visibleHostCount: 1,
      provisionedHost: true,
      firstHostAlias: 'mefistos',
      projectCount: 3,
      mcpEnabled: false, // optional, not done
      workSessionCount: 0, // session not done
    });
    expect(byId(steps, 'mcp').status).toBe('pending'); // optional, never active
    expect(byId(steps, 'session').status).toBe('active'); // active skips over the optional step
  });

  it('all required complete ignores the optional Control API step', () => {
    const steps = deriveSteps({
      ...base,
      prereqs: okPrereqs,
      visibleHostCount: 1,
      provisionedHost: true,
      firstHostAlias: 'mefistos',
      projectCount: 3,
      mcpEnabled: false,
      workSessionCount: 1,
    });
    expect(allRequiredComplete(steps)).toBe(true);
  });
});
