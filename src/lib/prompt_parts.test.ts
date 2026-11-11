import { describe, it, expect } from 'vitest';
import { agentContext, steerPrefix } from './agent_context';
import { BUILTIN_DASHBOARDS, changePrompt, type Dashboard } from './dashboards';
import { chipLabel, promptParts } from './prompt_parts';
import type { SessionRow } from './sessions';

const session = { id: 1, name: 'ST staging deploy z mac (bez Actions)', host_alias: 'mac', tmux_name: 'ST staging deploy z mac (bez Actions)' } as unknown as SessionRow;

describe('promptParts', () => {
  it('reads Martin’s message: the context line and a built-in dashboard to copy, down to what he typed', () => {
    const ctx = agentContext({ view: 'terminal', session, hostAlias: 'mac', branch: null, friendly: false })!;
    const prompt = `${ctx.prefix}\n\n${changePrompt(BUILTIN_DASHBOARDS.developer)} Add more data to dashboard`;
    const p = promptParts(prompt);
    expect(p.text).toBe('Add more data to dashboard');
    expect(p.chips).toEqual([
      { kind: 'session', name: 'ST staging deploy z mac (bez Actions)', host: 'mac', branch: null },
      { kind: 'dashboard', title: 'My work', widget: null, copy: true },
    ]);
    expect(p.chips.map(chipLabel)).toEqual(['ST staging deploy z mac (bez Actions) · mac', 'Dashboard “My work” (your copy)']);
  });

  it('a pinned dashboard’s widget, a branch, a host and a mission', () => {
    const d: Dashboard = { id: 'dash.r42', title: 'Release 4.2', intro: '', widgets: [{ kind: 'stat', title: 'Open', query: {}, value: 'open' }] };
    const w = promptParts(`${changePrompt(d, 'Open')}split it by person`);
    expect(w.chips).toEqual([{ kind: 'dashboard', title: 'Release 4.2', widget: 'Open', copy: false }]);
    expect(w.text).toBe('split it by person');

    const ctx = agentContext({ view: 'terminal', session, hostAlias: 'mac', branch: 'main', friendly: false })!;
    expect(promptParts(`${ctx.prefix}\n\nfix it`).chips[0]).toEqual({ kind: 'session', name: session.tmux_name, host: 'mac', branch: 'main' });

    const host = agentContext({ view: 'hosts', session: null, hostAlias: 'htz', branch: null, friendly: false })!;
    expect(promptParts(`${host.prefix}\n\nwhy is it slow`)).toEqual({ chips: [{ kind: 'host', host: 'htz' }], text: 'why is it slow' });

    expect(promptParts(`${steerPrefix({ id: 4, name: 'Payments v2' })}\n\nSplit the refunds step`)).toEqual({
      chips: [{ kind: 'mission', id: 4, name: 'Payments v2' }],
      text: 'Split the refunds step',
    });
  });

  it('leaves a person’s own words alone', () => {
    for (const text of ['[context] is a word I use', 'About mission #4 "x": no blank line after', 'hello\n\n[context] the person is looking at host a with no session selected. "here" means that host.']) {
      expect(promptParts(text)).toEqual({ chips: [], text });
    }
  });

  it('keeps a person’s own words after the app’s head (review of the dashboards work, 7)', () => {
    const head = 'My pinned dashboard "R", as it is now:\n\n```fleet-ui\n{}\n```\n\nSave the change with `dashboards { pin }` under the same id, then answer with the `dashboard` block. ';
    // Their own `": ` is not part of the widget's name.
    expect(promptParts(head + 'Change the widget "Open": rename it to "Open items": and add cost')).toEqual({
      chips: [{ kind: 'dashboard', title: 'R', widget: 'Open', copy: false }],
      text: 'rename it to "Open items": and add cost',
    });
    // Nothing typed (the transcript trims), or typed on the next line.
    expect(promptParts((head + 'Change it: ').trimEnd())).toEqual({ chips: [{ kind: 'dashboard', title: 'R', widget: null, copy: false }], text: '' });
    expect(promptParts(head + 'Change it:\nadd a burn-up').text).toBe('add a burn-up');
    // CR line ends.
    const crlf = `${steerPrefix({ id: 4, name: 'P' })}\r\n\r\n${head.replace(/\n/g, '\r\n')}Change it: more`;
    expect(promptParts(crlf)).toEqual({
      chips: [{ kind: 'mission', id: 4, name: 'P' }, { kind: 'dashboard', title: 'R', widget: null, copy: false }],
      text: 'more',
    });
    // One context or mission head only: a second is the person's own words.
    const twice = 'About mission #4 "Payments v2":\n\nAbout mission #5 "Refunds":\n\ncompare them';
    expect(promptParts(twice)).toEqual({
      chips: [{ kind: 'mission', id: 4, name: 'Payments v2' }],
      text: 'About mission #5 "Refunds":\n\ncompare them',
    });
  });

  it('the older dashboard instruction reads the same', () => {
    const old = 'My pinned dashboard "R", as it is now:\n\n```fleet-ui\n{}\n```\n\nAnswer with a `dashboard` block with the same id. Change it: more';
    expect(promptParts(old)).toEqual({ chips: [{ kind: 'dashboard', title: 'R', widget: null, copy: false }], text: 'more' });
  });
});
