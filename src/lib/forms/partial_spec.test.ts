import { describe, expect, it } from 'vitest';
import { finishedSpec, partialSpec } from './partial_spec';
import { WIZARDS } from './wizards';

const SPEC = JSON.stringify({
  spec: 'fleet.form/1',
  title: 'New project',
  intro: 'Control drafted this from your message.',
  steps: [
    {
      title: 'Basics',
      fields: [
        { name: 'name', type: 'text', label: 'Name', required: true },
        { name: 'host', type: 'select', label: 'Host', options: [['mercury', 'mercury'], ['mac', 'mac']] },
      ],
    },
    { title: 'Repos', fields: [{ name: 'draft_pr', type: 'bool', label: 'Open a draft PR' }] },
  ],
});

describe('partialSpec (redesign step 10.12)', () => {
  it('reads a whole spec as complete', () => {
    const p = partialSpec(SPEC);
    expect(p.complete).toBe(true);
    expect(p.title).toBe('New project');
    expect(p.steps.map((s) => s.fields.map((f) => f.name))).toEqual([['name', 'host'], ['draft_pr']]);
  });

  it('never throws and never goes backwards as the text grows', () => {
    let fields = 0;
    let title: string | null = null;
    for (let n = 0; n <= SPEC.length; n++) {
      const p = partialSpec(SPEC.slice(0, n));
      const now = p.steps.reduce((k, s) => k + s.fields.length, 0);
      expect(now).toBeGreaterThanOrEqual(fields);
      fields = now;
      if (title) expect(p.title).toBe(title);
      title = p.title ?? title;
      expect(p.complete).toBe(n === SPEC.length);
    }
    expect(fields).toBe(3);
  });

  it('shows a field only once its name, type and label are written', () => {
    const at = SPEC.indexOf('"label":"Host"');
    expect(partialSpec(SPEC.slice(0, at)).steps[0].fields.map((f) => f.name)).toEqual(['name']);
    // A label cut mid-word is not shown either.
    expect(partialSpec(SPEC.slice(0, at + 11)).steps[0].fields.map((f) => f.name)).toEqual(['name']);
    expect(partialSpec(SPEC.slice(0, at + 14)).steps[0].fields.map((f) => f.name)).toEqual(['name', 'host']);
  });

  it('keeps a title that is still being written out, and reads an empty text', () => {
    expect(partialSpec('{"spec":"fleet.form/1","title":"New pro').title).toBeNull();
    expect(partialSpec('{"spec":"fleet.form/1","title":"New project","st').title).toBe('New project');
    expect(partialSpec('')).toEqual({ title: null, intro: null, steps: [], complete: false });
  });

  it('opens a spec only once it is whole', () => {
    expect(finishedSpec(SPEC.slice(0, -1))).toBeNull();
    expect(finishedSpec(SPEC)?.title).toBe('New project');
    expect(finishedSpec('{"title":"x"}')).toBeNull();
  });

  it('reads every built-in wizard the same way', () => {
    for (const w of Object.values(WIZARDS)) {
      const text = JSON.stringify(w.spec);
      expect(partialSpec(text).complete).toBe(true);
      expect(finishedSpec(text)).toEqual(w.spec);
    }
  });
});
