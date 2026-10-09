import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { checkUiBlock, fenced, formAnswerPrompt, markerOf, reportFromValue, splitRich, UI_MAX_BYTES } from './rich_blocks';

const REPORT = {
  summary: 'Wrote docs/missions.md.',
  outcome: 'done',
  tests_run: ['link check'],
  warnings: ['Ticket brief was ambiguous'],
  blockers: [],
  followups: ['Push the branch and open a PR'],
  confidence: 'medium',
};

const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', ...o }, null, 2) + '\n```';

describe('reportFromValue (report.rs report_from_value)', () => {
  it('normalises as the backend does', () => {
    const r = reportFromValue({ summary: ' Added the queue table ', outcome: 'DONE', tests_run: ['cargo test queue'], warnings: [], followups: 'index the status column', confidence: 0.8 });
    expect(r).toEqual({
      summary: 'Added the queue table',
      outcome: 'done',
      tests_run: ['cargo test queue'],
      warnings: [],
      blockers: [],
      followups: ['index the status column'],
      confidence: '0.8',
    });
  });
  it('reads an unknown outcome as partial and caps the lists', () => {
    const r = reportFromValue({ outcome: 'shipped', warnings: Array.from({ length: 30 }, (_, i) => `w${i}`), summary: 'x'.repeat(9000) })!;
    expect(r.outcome).toBe('partial');
    expect(r.warnings).toHaveLength(20);
    expect(r.summary).toHaveLength(4000);
  });
  it('is null for anything but an object', () => {
    expect(reportFromValue([1])).toBeNull();
    expect(reportFromValue('done')).toBeNull();
  });
});

describe('markerOf', () => {
  it('finds the marker alone on its line, chrome and emphasis aside', () => {
    expect(markerOf('FLEET_TASK_DONE_9d2404fd')).toBe('FLEET_TASK_DONE_9d2404fd');
    expect(markerOf('⏺ **FLEET_TASK_DONE_ab12**')).toBe('FLEET_TASK_DONE_ab12');
    expect(markerOf('print exactly FLEET_TASK_DONE_ab12 on its own line')).toBeNull();
  });
});

describe('splitRich', () => {
  it('leaves text without blocks as one Markdown run', () => {
    expect(splitRich('Hello **there**')).toEqual([{ t: 'md', source: 'Hello **there**' }]);
  });

  it('turns a marker and its fenced JSON into a report, keeping the prose around it', () => {
    const src = `All done.\n\nFLEET_TASK_DONE_9d2404fdfabee30f\n\`\`\`json\n${JSON.stringify(REPORT, null, 2)}\n\`\`\`\n\nThanks.`;
    const segs = splitRich(src);
    expect(segs.map((s) => s.t)).toEqual(['md', 'report', 'md']);
    const r = segs[1];
    if (r.t !== 'report') throw new Error('not a report');
    expect(r.marker).toBe('FLEET_TASK_DONE_9d2404fdfabee30f');
    expect(r.report.outcome).toBe('done');
    expect(r.report.followups).toEqual(['Push the branch and open a PR']);
  });

  it('reads a bare object after the marker', () => {
    const src = `FLEET_TASK_DONE_n1\n{"summary": "ok",\n "outcome": "blocked", "blockers": ["no db"]}\nthanks`;
    const segs = splitRich(src);
    expect(segs.map((s) => s.t)).toEqual(['report', 'md']);
    expect(segs[0].t === 'report' && segs[0].report.blockers).toEqual(['no db']);
  });

  it('leaves a marker with no JSON, or with JSON still being written, as text', () => {
    expect(splitRich('FLEET_TASK_DONE_n1\nA paragraph.').map((s) => s.t)).toEqual(['md']);
    expect(splitRich('FLEET_TASK_DONE_n1\n```json\n{"summary": "half').map((s) => s.t)).toEqual(['md']);
  });

  it('keeps a marker or a fleet-ui fence quoted inside another code block as code', () => {
    const src = '````markdown\nFLEET_TASK_DONE_n1\n```json\n{"outcome":"done"}\n```\n' + ui({ kind: 'callout', body: 'x' }) + '\n````';
    expect(splitRich(src)).toEqual([{ t: 'md', source: src }]);
  });

  it('draws a fleet-ui fence as a card and a json fence only when it says fleet.ui/1', () => {
    const segs = splitRich(`Before\n\n${ui({ kind: 'callout', tone: 'warning', body: 'Careful' })}\n\nAfter`);
    expect(segs.map((s) => s.t)).toEqual(['md', 'ui', 'md']);
    const tagged = splitRich('```json\n{"spec": "fleet.ui/1", "kind": "facts", "items": [["Host", "mercury"]]}\n```');
    expect(tagged.map((s) => s.t)).toEqual(['ui']);
    const plain = '```json\n{"kind": "facts"}\n```';
    expect(splitRich(plain)).toEqual([{ t: 'md', source: plain }]);
  });

  it('keeps an unclosed fleet-ui fence as Markdown while it streams', () => {
    expect(splitRich('```fleet-ui\n{"spec": "fleet.ui/1",').map((s) => s.t)).toEqual(['md']);
  });

  it('shows a broken fleet-ui block as code with what is wrong', () => {
    const [seg] = splitRich(ui({ kind: 'steps', title: 'T', steps: [] }));
    expect(seg.t).toBe('invalid');
    expect(seg.t === 'invalid' && seg.problems).toEqual(['`steps` needs at least 1 entry']);
  });
});

describe('checkUiBlock', () => {
  const check = (o: Record<string, unknown>) => checkUiBlock(JSON.stringify({ spec: 'fleet.ui/1', ...o }));

  it('accepts every kind in its documented shape', () => {
    const ok = [
      { kind: 'report', title: 'Run 3', ...REPORT },
      { kind: 'steps', title: 'Set up', steps: [{ title: 'Install', code: 'pnpm i', lang: 'sh' }, { title: 'Run', body: 'Then **run**.' }] },
      { kind: 'guide', title: 'Missions', sections: [{ title: 'What', body: 'A mission is…' }] },
      { kind: 'callout', tone: 'tip', title: 'Tip', body: 'Use `verify.sh`.' },
      { kind: 'facts', items: [['Branch', 'main'], ['Commits', 3]] },
      { kind: 'choices', question: 'Next?', options: [{ label: 'Open a PR', prompt: 'Open a draft PR', hint: 'recommended' }] },
      { kind: 'form', form: { spec: 'fleet.form/1', title: 'Deploy', steps: [{ title: 'Target', fields: [{ name: 'env', type: 'select', label: 'Env', options: [['stg', 'Staging']] }] }] } },
    ];
    for (const o of ok) expect(check(o), o.kind).toMatchObject({ ok: true, block: { kind: o.kind } });
  });

  it('names what is wrong, where', () => {
    expect(check({ kind: 'chart' })).toEqual({ ok: false, problems: ['`kind` must be one of report, steps, guide, callout, facts, choices, form, progress, results, error, setting, wizard'] });
    expect(checkUiBlock('{"spec": "fleet.ui/2", "kind": "callout", "body": "x"}')).toEqual({ ok: false, problems: ['`spec` must be "fleet.ui/1"'] });
    expect(check({ kind: 'callout', tone: 'loud', body: 'x' })).toEqual({ ok: false, problems: ['`tone` must be one of info, tip, success, warning, danger'] });
    expect(check({ kind: 'choices', options: [{ label: 'A' }] })).toEqual({ ok: false, problems: ['option 1: `prompt` is required'] });
    expect(checkUiBlock('{nope')).toEqual({ ok: false, problems: ['is not valid JSON'] });
    expect(checkUiBlock('"x"'.padEnd(UI_MAX_BYTES + 1, ' '))).toEqual({ ok: false, problems: ['is larger than 32 KiB'] });
  });

  it('refuses a secret field in a reply form: its answer would land in the transcript', () => {
    const r = check({ kind: 'form', form: { spec: 'fleet.form/1', title: 'Login', steps: [{ title: 'S', fields: [{ name: 'pw', type: 'secret', label: 'Password' }] }] } });
    expect(r.ok).toBe(false);
    expect(!r.ok && r.problems[0]).toMatch(/^form › step 1 › field 1: a secret field is only for `ask`/);
  });

  it('refuses a form field FormWizard could not draw', () => {
    const r = check({ kind: 'form', form: { spec: 'fleet.form/1', title: 'X', steps: [{ title: 'S', fields: [{ name: 'Bad Name', type: 'select', label: 'L', options: [['a']] }] }] } });
    expect(!r.ok && r.problems).toEqual([
      'form › step 1 › field 1: name "Bad Name" must be lowercase letters, digits and _',
      'form › step 1 › field 1 › option 1: must be [value, label]',
    ]);
  });
});

describe('helpers', () => {
  it('formAnswerPrompt carries the title and the answers as one JSON block', () => {
    const p = formAnswerPrompt({ spec: 'fleet.form/1', title: 'Deploy', steps: [] }, { env: 'stg' });
    expect(p).toBe('Answers to the form "Deploy":\n\n```json\n{\n  "env": "stg"\n}\n```');
  });
  it('fenced picks a fence no backtick run inside can close', () => {
    expect(fenced('json', 'a ```` b')).toBe('`````json\na ```` b\n`````');
  });
});

describe('the shared fleet.ui/1 cases (docs/chat-block-examples/blocks.json)', () => {
  const doc = JSON.parse(readFileSync('docs/chat-block-examples/blocks.json', 'utf8'));
  for (const c of doc.cases as { name: string; block: unknown; problems: string[] }[]) {
    it(c.name, () => {
      const r = checkUiBlock(JSON.stringify(c.block));
      expect(r.ok ? [] : r.problems).toEqual(c.problems);
    });
  }
});

describe('the new kinds', () => {
  it('defaults a progress to running and its steps to pending', () => {
    const r = checkUiBlock(JSON.stringify({ spec: 'fleet.ui/1', kind: 'progress', id: 'a', title: 'T', steps: [{ title: 'S' }] }));
    if (!r.ok || r.block.kind !== 'progress') throw new Error('not a progress');
    expect(r.block.state).toBe('running');
    expect(r.block.steps).toEqual([{ title: 'S', state: 'pending' }]);
  });
  it('keeps an error with no next step as an empty list', () => {
    const r = checkUiBlock(JSON.stringify({ spec: 'fleet.ui/1', kind: 'error', code: 'E_X', title: 'T' }));
    if (!r.ok || r.block.kind !== 'error') throw new Error('not an error');
    expect(r.block.next).toEqual([]);
  });
  it('draws a results block in a reply as a card', () => {
    const segs = splitRich(ui({ kind: 'results', items: [{ type: 'stat', label: 'p95', value: 412 }] }));
    expect(segs.map((s) => s.t)).toEqual(['ui']);
  });
});
