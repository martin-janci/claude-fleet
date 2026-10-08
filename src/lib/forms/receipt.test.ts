// Step 10.1: the words a decided form's receipt and a pending form's expiry use.
import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import type { FormView } from './forms';
import { FORM_EXPIRE_SECS, answerList, answerSummary, endedMark, endedWords, expiresIn } from './receipt';

function answered(over: Partial<FormView> = {}): FormView {
  return {
    form_id: 'f_a',
    session_id: 1,
    host_alias: 'mercury',
    title: 'New project',
    state: 'answered',
    answered_by: 'Martin',
    created_at: 0,
    decided_at: 60,
    answers: { name: 'papaya-receipts', kind: 'web', targets: ['mac', 'win'], workers: 3, db: true },
    secrets: { key: '~/.cache/claude-fleet/forms/f_a/key' },
    spec: {
      spec: 'fleet.form/1',
      title: 'New project',
      steps: [
        { title: 'Basics', fields: [
          { name: 'name', type: 'text', label: 'Name', required: true },
          { name: 'kind', type: 'select', label: 'Kind', options: [['web', 'Web app'], ['cli', 'CLI']] },
          { name: 'targets', type: 'multiselect', label: 'Targets', options: [['mac', 'macOS'], ['win', 'Windows'], ['linux', 'Linux']] },
          { name: 'workers', type: 'number', label: 'Workers' },
          { name: 'db', type: 'bool', label: 'Database' },
          { name: 'key', type: 'secret', label: 'API key' } ] },
        { title: 'Hidden', when: { field: 'kind', eq: 'cli' }, fields: [
          { name: 'shell', type: 'text', label: 'Shell' } ] },
      ],
    },
    ...over,
  };
}

describe('form receipt', () => {
  it("summarises the answers in one line, as the Components board's receipt", () => {
    expect(answerSummary(answered())).toBe('papaya-receipts · Web app · macOS, Windows · 3 workers · database on · 1 secret');
  });

  it('lists every answer in form order; a secret says where it went, never what it was', () => {
    expect(answerList(answered())).toEqual([
      { name: 'name', label: 'Name', text: 'papaya-receipts' },
      { name: 'kind', label: 'Kind', text: 'Web app' },
      { name: 'targets', label: 'Targets', text: 'macOS, Windows' },
      { name: 'workers', label: 'Workers', text: '3' },
      { name: 'db', label: 'Database', text: 'yes' },
      { name: 'key', label: 'API key', text: 'written to mercury, never shown' },
    ]);
  });

  it('leaves out a hidden step, an empty answer and a long line past one line', () => {
    const f = answered({ answers: { name: 'a\nsecond line', shell: 'zsh', targets: [] }, secrets: null });
    expect(answerSummary(f)).toBe('a');
    expect(answerList(f).map((a) => a.name)).toEqual(['name']);
    expect(answerSummary(answered({ answers: { name: 'x'.repeat(60) }, secrets: null }))).toHaveLength(40);
  });

  it('says how it ended and who decided', () => {
    expect(endedWords(answered())).toBe('answered by Martin');
    expect(endedWords(answered({ state: 'declined' }))).toBe('declined by Martin');
    expect(endedWords(answered({ state: 'expired' }))).toBe('expired');
    expect(endedWords(answered({ state: 'cancelled' }))).toBe('withdrawn by the agent');
    expect(['answered', 'declined', 'expired', 'cancelled'].map(endedMark)).toEqual(['✓', '✕', '◷', '–']);
  });
});

describe('form expiry', () => {
  it('counts down in minutes in the last hour and in hours before it', () => {
    expect(expiresIn(0, 0)).toBe('expires in 24 h');
    expect(expiresIn(0, FORM_EXPIRE_SECS - 9 * 60 - 30)).toBe('expires in 9 min');
    expect(expiresIn(0, FORM_EXPIRE_SECS - 30)).toBe('expires now');
  });

  it('reads the same 24 h as the backend that expires it', () => {
    const rs = readFileSync('crates/fleet-core/src/service/forms.rs', 'utf8');
    expect(rs).toContain('pub const EXPIRE_SECS: i64 = 24 * 3600;');
    expect(FORM_EXPIRE_SECS).toBe(24 * 3600);
  });
});
