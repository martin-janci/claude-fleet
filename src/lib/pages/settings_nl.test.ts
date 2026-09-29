// The plain-words settings command (declarative pages P5): only a declared
// key and a value its kind accepts come out, and nothing is guessed when
// the words fit more than one setting.
import { describe, it, expect } from 'vitest';
import { allDescriptors } from './testing';
import { interpret, parseValue, splitCommand } from './settings_nl';

const descs = allDescriptors;
const d = (key: string) => descs.find((x) => x.key === key)!;
const change = (q: string) => {
  const r = interpret(q, descs);
  if (r?.kind !== 'change') throw new Error(`${q}: ${JSON.stringify(r)}`);
  return [r.d.key, r.value];
};

describe('settings in plain words', () => {
  it('is only a command with a verb; anything else is a search', () => {
    expect(splitCommand('recent work')).toBeNull();
    expect(splitCommand('set up hooks')).toBeNull();
    expect(splitCommand('set recent work to 3 days')).toEqual({ phrase: 'recent work', value: '3 days' });
    expect(splitCommand('turn press enter on')).toEqual({ phrase: 'press enter', value: 'on' });
    expect(interpret('recent work', descs)).toBeNull();
  });

  it('turns words into one setting and a value its kind holds', () => {
    expect(change('set recent work to 3 days')).toEqual(['work.recent_days', '3']);
    expect(change('Set Recent Work to 2 weeks')).toEqual(['work.recent_days', '14']);
    expect(change('change keep lost sessions to 2 days')).toEqual(['sessions.lost_ttl_secs', '172800']);
    // A bare number is in the unit the field shows (hours here).
    expect(change('set background agent idle limit to 6')).toEqual(['gc.bg_idle_secs', '21600']);
    expect(change('turn on press enter')).toEqual(['playbooks.press_enter', 'true']);
    expect(change('disable collect token usage')).toEqual(['usage.enabled', 'false']);
    const layout = d('projects.layout');
    const opt = layout.kind.type === 'choice' ? layout.kind.options[0] : '';
    expect(change(`set projects layout to ${opt}`)).toEqual(['projects.layout', opt]);
  });

  it('refuses a value out of range, of the wrong kind, or for a key it cannot edit here', () => {
    const err = (q: string) => {
      const r = interpret(q, descs);
      return r?.kind === 'error' ? r.message : JSON.stringify(r);
    };
    expect(err('set recent work to 900 days')).toContain('out of range');
    expect(err('set recent work to soon')).toContain('say a number');
    expect(err('turn on control api port')).toContain('read-only here');
    expect(err('set projects roots to x')).toContain('edit it on its page');
    expect(err('set no such thing to 3')).toContain('No setting');
  });

  it('asks which when the words fit several settings about as well', () => {
    const r = interpret('set tidy to 3', descs);
    expect(r?.kind).toBe('ambiguous');
    if (r?.kind === 'ambiguous') expect(r.options.map((o) => o.key)).toContain('work.tidy_done_days');
  });

  it('parses each kind by its own rules', () => {
    expect(parseValue(d('playbooks.press_enter'), 'yes')).toEqual({ value: 'true' });
    expect(parseValue(d('playbooks.press_enter'), 'maybe')).toEqual({ error: 'say on or off' });
    expect(parseValue(d('reconcile.interval_secs'), '90s')).toEqual({ value: '90' });
    expect(parseValue(d('reconcile.interval_secs'), '2 min')).toEqual({ value: '120' });
    expect(parseValue(d('reconcile.interval_secs'), '3 parsecs')).toEqual({ error: 'not a unit of time: parsecs' });
  });
});
