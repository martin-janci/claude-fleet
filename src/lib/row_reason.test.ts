import { describe, expect, it } from 'vitest';
import { failureWords, rowReason, toolAsk } from './row_reason';

const base = {
  host_alias: 'mercury',
  account_uuid: 'acc',
  claude_profile: null,
  stuck_kind: null,
  current_activity: null,
  context_pct: null,
  pr_url: null,
  pending_form: null,
};
const name = (u: string) => (u === 'acc' ? 'acme' : u);
const say = (r: { lead: string; text: string } | null) => (r ? `${r.lead} ${r.text}`.trim() : null);

describe('toolAsk', () => {
  it('says what a tool call does', () => {
    expect(toolAsk('Bash(git push origin main)')).toBe('run git push origin main');
    expect(toolAsk('Edit(docs/pricing.md)')).toBe('edit docs/pricing.md');
    expect(toolAsk('WebFetch(https://example.com)')).toBe('fetch https://example.com');
    expect(toolAsk('mcp__github__create_pr(title: x)')).toBe('use github create_pr: title: x');
    expect(toolAsk('not a call')).toBeNull();
    expect(toolAsk(null)).toBeNull();
  });
});

describe('failureWords', () => {
  it('turns an API error into words, never the raw payload', () => {
    expect(failureWords('API Error: 529 {"type":"overloaded_error"}')).toBe('the API was overloaded (529)');
    expect(failureWords('API Error: 429 …')).toBe('the API rate-limited it (429)');
    expect(failureWords('API Error: 418 teapot')).toBe('the API answered 418');
    expect(failureWords(null)).toBe('the turn stopped with an error');
  });
});

describe('rowReason (HITL audit H-11)', () => {
  it('a generic permission headline gives way to the tool call', () => {
    const r = rowReason('waiting', base, undefined, name, { question: 'Do you want to proceed?', detail: 'Bash(git push origin main)' });
    expect(r).toEqual({ lead: 'Waiting for you:', text: 'run git push origin main', tone: 'waiting' });
  });

  it('a real question is kept as it is', () => {
    const r = rowReason('waiting', base, undefined, name, { question: 'What VAT ID should the footer show?', detail: null });
    expect(say(r)).toBe('Waiting for you: What VAT ID should the footer show?');
  });

  it('a form names its title', () => {
    const r = rowReason('waiting', { ...base, pending_form: { form_id: 'f', title: 'SSH details for htz-3' } }, undefined, name, null);
    expect(say(r)).toBe('Waiting for you: a form, “SSH details for htz-3”');
  });

  it('a stuck row reads Failed and takes the failed tone, whatever its state', () => {
    expect(rowReason('stuck', { ...base, stuck_kind: 'press_enter' }, undefined, name, null)?.tone).toBe('failed');
  });

  it('a stuck row says on what', () => {
    expect(say(rowReason('stuck', { ...base, stuck_kind: 'trust_prompt' }, undefined, name, null))).toBe('Failed: stuck on the trust prompt');
    expect(say(rowReason('stuck', { ...base, stuck_kind: 'oom' }, undefined, name, null))).toBe('Failed: ran out of memory');
  });

  it('a failed turn says why in words', () => {
    const r = rowReason('stop_failed', { ...base, current_activity: 'API Error: 529 {"type":"ov' }, undefined, name, null);
    expect(say(r)).toBe('Failed: the API was overloaded (529)');
  });

  it('failing CI names the pull request', () => {
    const r = rowReason('ci_failing', { ...base, pr_url: 'https://github.com/o/r/pull/131' }, undefined, name, null);
    expect(say(r)).toBe('Failed: checks failing on PR #131');
  });

  it('a full context and a silent working row say what to do', () => {
    expect(say(rowReason('context_full', { ...base, context_pct: 93.4 }, undefined, name, null))).toBe(
      'Needs you · context 93% full, compact or clear it',
    );
    expect(rowReason('stale_working', base, undefined, name, null)?.lead).toBe('Needs you ·');
  });

  it('the blocked buckets keep their words, with the lead split off', () => {
    const facts = { limited_accounts: { acc: { window: 'five_hour' as const, resets_at: 9 } }, down_hosts: ['mercury'] };
    expect(rowReason('account_limit', base, facts, name, null)).toEqual({ lead: 'Paused ·', text: '5-hour limit on acme', tone: 'paused' });
    expect(rowReason('host_down', base, facts, name, null)).toEqual({ lead: 'Needs you ·', text: 'blocked on mercury, which is down', tone: 'waiting' });
  });

  it('a row that needs nobody has no reason', () => {
    for (const b of ['working', 'idle', 'idle_long', 'done_unread', 'probably_waiting', 'lifecycle'] as const) {
      expect(rowReason(b, base, undefined, name, null)).toBeNull();
    }
  });
});
