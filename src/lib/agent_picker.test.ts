import { describe, it, expect } from 'vitest';
import { agentChoices, keepAgent } from './agent_picker';

// Orbit Fleet 12.4: an agent is offered only when fleet can launch it and the
// chosen host has it on the PATH.

describe('agentChoices', () => {
  it('offers Codex on a host that has it, and Claude Code everywhere', () => {
    const c = agentChoices('box', { agents_on_path: ['claude', 'codex'] });
    expect(c.map((x) => [x.agent, x.enabled])).toEqual([
      ['claude', true],
      ['codex', true],
      ['agy', false],
    ]);
  });

  it('greys Codex out on a host without it, naming the host', () => {
    const codex = agentChoices('box', { agents_on_path: ['claude'] })[1];
    expect(codex.enabled).toBe(false);
    expect(codex.tag).toBe('not on box');
    expect(codex.reason).toContain("box's PATH");
  });

  it('a host never sampled offers no Codex yet, and says why', () => {
    for (const host of [{ agents_on_path: null }, {}, undefined]) {
      const codex = agentChoices('nas', host)[1];
      expect(codex.enabled).toBe(false);
      expect(codex.reason).toContain('Not checked on nas yet');
    }
  });

  it('Agy is coming even where it is installed: fleet cannot launch it yet', () => {
    const agy = agentChoices('box', { agents_on_path: ['agy'] })[2];
    expect(agy).toMatchObject({ enabled: false, tag: 'coming' });
  });

  it('Claude Code stays offered on a host that lacks it', () => {
    expect(agentChoices('box', { agents_on_path: [] })[0].enabled).toBe(true);
  });
});

describe('keepAgent', () => {
  it('keeps Codex while the new host has it, else falls back to Claude Code', () => {
    expect(keepAgent('codex', agentChoices('a', { agents_on_path: ['codex'] }))).toBe('codex');
    expect(keepAgent('codex', agentChoices('b', { agents_on_path: [] }))).toBe('claude');
    expect(keepAgent('claude', agentChoices('b', { agents_on_path: [] }))).toBe('claude');
  });
});
