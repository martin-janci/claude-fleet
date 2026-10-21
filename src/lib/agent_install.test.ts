import { describe, it, expect } from 'vitest';
import {
  FAILED_SHOWN_S,
  INSTALL_POLL_MAX_MS,
  INSTALL_POLL_MS,
  installPollDelay,
  installStepText,
  shownInstall,
  type AgentInstall,
} from './agent_install';

function job(over: Partial<AgentInstall> = {}): AgentInstall {
  return {
    id: 1,
    host_alias: 'mercury',
    version: '0.6.2',
    state: 'running',
    step: 'target',
    detail: null,
    started_at: 1000,
    finished_at: null,
    ...over,
  };
}

describe('installStepText (review w14-7)', () => {
  it('names the tmux step in words, not its key', () => {
    const t = installStepText(job({ step: 'tmux' }));
    expect(t).toBe('Installing fleet-agent 0.6.2 on mercury: Checking for tmux (installing it if it is missing)…');
  });

  it('an unknown step reads the job’s detail before its raw key', () => {
    expect(installStepText(job({ step: 'later', detail: 'doing a new thing' }))).toContain(': doing a new thing…');
    expect(installStepText(job({ step: 'later' }))).toContain(': later…');
  });
});

describe('shownInstall (review w14-4)', () => {
  it('shows a running job, and a failed one that ended within a day', () => {
    expect(shownInstall([job()], 2000)?.state).toBe('running');
    const failed = job({ state: 'failed', finished_at: 5000, detail: 'no route' });
    expect(shownInstall([failed], 5000 + FAILED_SHOWN_S)).toBe(failed);
    expect(shownInstall([failed], 5001 + FAILED_SHOWN_S)).toBeNull();
  });

  it('a done job, or none, shows nothing; only the newest counts', () => {
    expect(shownInstall([job({ state: 'done', finished_at: 1500 })], 1600)).toBeNull();
    expect(shownInstall([], 1)).toBeNull();
    expect(shownInstall(null)).toBeNull();
    expect(shownInstall([job({ state: 'done', finished_at: 1500 }), job({ state: 'failed', finished_at: 1400 })], 1600)).toBeNull();
  });
});

describe('installPollDelay (review w14-4)', () => {
  it('doubles after each failed poll, up to the cap', () => {
    expect(installPollDelay(0)).toBe(INSTALL_POLL_MS);
    expect(installPollDelay(1)).toBe(INSTALL_POLL_MS * 2);
    expect(installPollDelay(2)).toBe(INSTALL_POLL_MS * 4);
    expect(installPollDelay(20)).toBe(INSTALL_POLL_MAX_MS);
  });
});
