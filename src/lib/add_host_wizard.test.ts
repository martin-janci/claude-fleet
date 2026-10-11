// Orbit Fleet 4.9: the add-host wizard's pure rules.
import { describe, it, expect } from 'vitest';
import {
  CHECK_KEYS,
  agentLines,
  agentDetail,
  agentsLead,
  attentionChecks,
  canAdvance,
  checkLoader,
  checkRows,
  discoveryLoader,
  checksComplete,
  nextLabel,
  resumeLine,
  stepHeading,
  type HostSetup,
  type SetupCheck,
} from './add_host_wizard';

const ok = (key: SetupCheck['key'], label: string = key): SetupCheck => ({ key, state: 'ok', label, detail: 'ok' });
const all = (over: Partial<Record<SetupCheck['key'], SetupCheck>> = {}) => CHECK_KEYS.map((k) => over[k] ?? ok(k));

describe('add-host wizard', () => {
  it('lists every check in board order, the running one and the rest pending', () => {
    const rows = checkRows([ok('tmux'), ok('ssh')], 'git');
    expect(rows.map((r) => [r.key, r.state])).toEqual([
      ['ssh', 'ok'],
      ['tmux', 'ok'],
      ['git', 'running'],
      ['agent', 'pending'],
      ['disk', 'pending'],
      ['agents', 'pending'],
    ]);
    expect(rows[2].detail).toBe('checking…');
  });

  it('opens Next on the Check step only when SSH answered and every check is in', () => {
    expect(canAdvance(1, 'mercury', 'mercury', [], false)).toBe(true);
    expect(canAdvance(1, '', 'mercury', [], false)).toBe(false);
    expect(canAdvance(2, 'm', 'm', all(), false)).toBe(true);
    expect(canAdvance(2, 'm', 'm', all(), true)).toBe(false);
    expect(canAdvance(2, 'm', 'm', all().slice(1), false)).toBe(false);
    const sshDown = all({ ssh: { key: 'ssh', state: 'fail', label: 'SSH', detail: 'timed out' } });
    expect(canAdvance(2, 'm', 'm', sshDown, false)).toBe(false);
    // A missing tmux is a warning to read, not a lock.
    const noTmux = all({ tmux: { key: 'tmux', state: 'fail', label: 'tmux not installed', detail: '' } });
    expect(canAdvance(2, 'm', 'm', noTmux, false)).toBe(true);
    expect(attentionChecks(noTmux).map((c) => c.key)).toEqual(['tmux']);
    expect(checksComplete(all())).toBe(true);
  });

  it('names the steps as the board does', () => {
    expect(stepHeading(2, 'mercury')).toBe('Check mercury');
    expect(nextLabel(2, 'mercury')).toBe('Next: Agents ›');
    expect(nextLabel(5, 'mercury')).toBe('Add mercury');
  });

  it('reads the agents the check found', () => {
    const lines = agentLines([ok('agents', 'Claude Code, Codex on PATH')]);
    expect(lines.map((l) => [l.bin, l.found])).toEqual([
      ['claude', true],
      ['codex', true],
      ['agy', false],
      ['gemini', false],
    ]);
    expect(lines[0].required).toBe(true);
  });

  it('says which agents fleet starts, Codex with Claude Code (G7.15)', () => {
    expect(agentsLead('mercury')).toBe(
      'The agents fleet can start on mercury. Fleet runs Claude Code and Codex sessions; the others are listed as they arrive.',
    );
    const [claude, codex, agy, gemini] = agentLines([ok('agents', 'Claude Code, Codex, Agy on PATH')]);
    expect([claude, codex, agy, gemini].map(agentDetail)).toEqual([
      'on PATH',
      'on PATH',
      'on PATH · sessions coming',
      'not installed',
    ]);
    expect(agentDetail(agentLines([])[0])).toBe('not found');
  });

  it('says where a saved draft was left', () => {
    const d: HostSetup = { ssh_alias: 'merc', alias: 'mercury', step: 2, checks: [], answers: {}, created_at: 1, updated_at: 2 };
    expect(resumeLine(d)).toBe('mercury (merc) · step 2 of 5, Check the host');
  });
});

describe('live loaders (4.13)', () => {
  it('Sonar waits for SSH, the Hex field checks the rest, Radar discovers', () => {
    expect(checkLoader(null, 'mercury')).toBeNull();
    expect(checkLoader('ssh', 'mercury')).toEqual({ name: 'sonar', text: 'Waiting for mercury to answer…' });
    for (const k of CHECK_KEYS.filter((k) => k !== 'ssh')) expect(checkLoader(k, 'mercury')?.name).toBe('hex-field');
    expect(new Set(CHECK_KEYS.map((k) => checkLoader(k, 'm')!.text)).size).toBe(CHECK_KEYS.length);
    expect(checkLoader('ssh', '  ')!.text).toBe('Waiting for the host to answer…');
    expect(discoveryLoader().name).toBe('radar');
  });
});
