// Redesign step 3.11: the never-decides list and when a proposal may
// pre-select anything.
import { readFileSync, readdirSync } from 'node:fs';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { LOADER_DELAY_MS } from './Loader.svelte';
import { describe, it, expect, vi } from 'vitest';
import ProposedBy from './ProposedBy.svelte';
import DraftField from './DraftField.svelte';
import AnswerPrompt from './AnswerPrompt.svelte';
import ReviewApply from './pages/ReviewApply.svelte';
import { allDescriptors, bundle } from './pages/testing';
import { pendingInputFor, type PendingInput } from './pending_input';
import type { SessionRow } from './sessions';
import type { SettingProposal } from './pages/review';
import {
  NEVER_DECIDES,
  aiChangeLine,
  confidenceWord,
  correctionLine,
  draftedBy,
  neverDecides,
  neverDecidesField,
  preselect,
  proposedByLabel,
  type ProposalLike,
} from './ai_proposal';

const jev = (value: string, confidence_pct: number | null = 90): ProposalLike => ({
  value,
  source: 'jev',
  reason: 'same repo, 2 idle slots',
  confidence_pct,
});

describe('where AI never decides', () => {
  // The plan names these; a proposal must never reach one of them.
  const named = [
    'approve_push',
    'approve_permission',
    'share',
    'role',
    'assign_org',
    'mission_autonomy',
    'verified',
    'force_kill',
    'priority',
  ];

  it.each(named)('%s is on the list', (target) => {
    expect(neverDecides(target)).toBe(true);
  });

  it.each(NEVER_DECIDES.map((r) => r.target))('a sure proposal never pre-selects %s', (target) => {
    expect(preselect(target, jev('approve', 100))).toBeNull();
    expect(preselect(target, { value: 'approve', source: 'rule' })).toBeNull();
  });

  it.each(NEVER_DECIDES.map((r) => r.target))('ProposedBy shows nothing for %s', (target) => {
    render(ProposedBy, { proposal: jev('x', 100), field: target });
    expect(screen.queryByTestId('proposed-by')).toBeNull();
  });

  it('each row says why, and no target is listed twice', () => {
    const targets = NEVER_DECIDES.map((r) => r.target);
    expect(new Set(targets).size).toBe(targets.length);
    for (const r of NEVER_DECIDES) expect(r.why.length).toBeGreaterThan(0);
  });
});

// The list above only says what is on it; these render the real callers, so
// a regression in one of them (a reorder, a pre-tick) fails here (F13).
describe('the callers never pre-select what AI never decides', () => {
  const PUSH: PendingInput = {
    kind: 'input',
    question: 'Push the 3 commits to origin/main now?',
    options: [
      { n: 1, label: 'Not yet', selected: true },
      { n: 2, label: 'Approve and push', selected: false },
      { n: 3, label: 'Yes, go ahead', selected: false },
    ],
  };
  const row = (value: string) =>
    ({
      id: 7,
      host_alias: 'local',
      tmux_name: 'dev-foo',
      claude_status: 'blocked',
      stuck_kind: null,
      pending_input: PUSH,
      proposals: [{ feature: 'quick_answer', value, source: 'jev', confidence_pct: 99 }],
    }) as unknown as SessionRow;
  const view = pendingInputFor({ rowStatus: 'blocked', rowStuck: null, rowPending: PUSH, probe: null })!;

  it('AnswerPrompt: a sure Jev pick on a push question moves nothing and draws nothing primary', () => {
    for (const value of ['o2', 'o3']) {
      const { unmount } = render(AnswerPrompt, { session: row(value), view });
      const opts = screen.getAllByTestId('answer-option');
      expect(opts.map((b) => b.textContent?.replace(/\s+/g, ''))).toEqual(['1Notyet', '2Approveandpush', '3Yes,goahead']);
      expect(opts.some((b) => b.classList.contains('primary'))).toBe(false);
      expect(screen.queryByTestId('answer-proposed')).toBeNull();
      unmount();
    }
  });

  it('ReviewApply: an agent-proposed orchestrator.max_level 1 → 3 is not ticked', () => {
    const p: SettingProposal = {
      id: 1,
      at: 0,
      key: 'orchestrator.max_level',
      value: '3',
      before: '1',
      current: '1',
      why: 'let missions run on their own',
      source: 'agent',
      source_detail: 'control API',
      state: 'pending',
    };
    render(ReviewApply, {
      proposals: [p],
      pages: bundle.pages,
      descs: new Map(allDescriptors.map((d) => [d.key, d])),
      now: () => 60,
    });
    expect((screen.getByTestId('review-tick-orchestrator.max_level') as HTMLInputElement).checked).toBe(false);
    expect(screen.getByTestId('review-apply-selected').textContent).toContain('(0)');
    expect((screen.getByTestId('review-apply-selected') as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('preselect', () => {
  it('pre-selects a sure answer for an allowed target', () => {
    expect(preselect('project', jev('p3'))).toBe('p3');
  });
  it('leaves the field empty on unsure, under the floor or with no confidence', () => {
    expect(preselect('project', jev('unsure'))).toBeNull();
    expect(preselect('project', jev('p3', 49))).toBeNull();
    expect(preselect('project', jev('p3', 70), 80)).toBeNull();
    expect(preselect('project', jev('p3', null))).toBeNull();
    expect(preselect('project', null)).toBeNull();
  });
  it('a rule needs no confidence', () => {
    expect(preselect('project', { value: 'p3', source: 'rule' })).toBe('p3');
  });
  it('labels each source and words a draft origin', () => {
    expect(proposedByLabel('jev')).toBe('Proposed by Jev');
    expect(draftedBy('haiku', 'mercury', 'from 3 changed files')).toBe(
      'by haiku on mercury · from 3 changed files',
    );
    expect(draftedBy(null, null, 'from the last 3 turns')).toBe('from the last 3 turns');
  });
});

describe('ProposedBy', () => {
  it('shows the pill, the reason and Change, and Change calls back', async () => {
    const onchange = vi.fn();
    render(ProposedBy, { proposal: jev('p3', 82), field: 'project', onchange });
    const row = screen.getByTestId('proposed-by');
    expect(row.textContent).toContain('Proposed by Jev');
    expect(row.textContent).toContain('same repo, 2 idle slots');
    expect(row.textContent).toContain('likely');
    expect(row.textContent).not.toMatch(/\d+\s*%/);
    await fireEvent.click(screen.getByTestId('proposed-by-change'));
    expect(onchange).toHaveBeenCalledOnce();
  });
  it('shows nothing under the floor', () => {
    render(ProposedBy, { proposal: jev('p3', 40), field: 'project' });
    expect(screen.queryByTestId('proposed-by')).toBeNull();
  });
  it('a rule has no confidence word', () => {
    render(ProposedBy, { proposal: { value: 'p3', source: 'rule', reason: 'last used' }, field: 'project' });
    expect(screen.queryByTestId('proposed-by-confidence')).toBeNull();
  });
});

describe('confidence is a word, never a percentage (G4.9)', () => {
  it('words each band', () => {
    expect(confidenceWord(95)).toBe('almost sure');
    expect(confidenceWord(82)).toBe('likely');
    expect(confidenceWord(55)).toBe('maybe');
    expect(confidenceWord(20)).toBe('unsure');
    expect(confidenceWord(null)).toBeNull();
  });
  it('no component renders a confidence as a percentage', () => {
    // Copy lint: a confidence value followed by `%` in markup is the bug
    // the AI patterns board names.
    const files = readdirSync('src', { recursive: true })
      .map(String)
      .filter((n) => n.endsWith('.svelte'));
    const offenders = files.filter((n) => /confidence[\w.?]*\}%/.test(readFileSync(`src/${n}`, 'utf8')));
    expect(offenders).toEqual([]);
  });
  it('words the correction and the AI-change line', () => {
    expect(correctionLine('papaya-pos', 'papaya-api')).toBe(
      'You changed papaya-pos → papaya-api · recorded as a correction',
    );
    expect(aiChangeLine('Linked to PD-2592', 'jev', true)).toBe('Linked to PD-2592 · Proposed by Jev · you confirmed');
  });
});

describe('DraftField', () => {
  it('names the model and host, keeps the ring until edited, and Clear empties it', async () => {
    const onclear = vi.fn();
    render(DraftField, {
      value: 'Fix the pairing flake',
      label: 'Summary',
      model: 'haiku',
      host: 'mercury',
      from: 'from 3 changed files',
      onregenerate: () => {},
      onclear,
    });
    const input = screen.getByTestId('draft-field-input') as HTMLTextAreaElement;
    expect(input.classList.contains('ai-pre')).toBe(true);
    expect(screen.getByTestId('draft-field-meta').textContent).toContain(
      'by haiku on mercury · from 3 changed files',
    );
    await fireEvent.input(input, { target: { value: 'Fix the pairing flake for good' } });
    expect(input.classList.contains('ai-pre')).toBe(false);
    expect(screen.getByTestId('draft-field-edited').textContent).toBe('Edited');
    expect(screen.getByTestId('draft-field-meta').textContent).toContain('your text now');
    expect(screen.queryByTestId('draft-field-drafted')).toBeNull();
    await fireEvent.click(screen.getByTestId('draft-field-clear'));
    expect(input.value).toBe('');
    expect(onclear).toHaveBeenCalledOnce();
    // Clear is undoable: only the Undo is left on the line.
    expect(screen.queryByTestId('draft-field-clear')).toBeNull();
    await fireEvent.click(screen.getByTestId('draft-field-undo'));
    expect(input.value).toBe('Fix the pairing flake for good');
    expect(screen.getByTestId('draft-field-edited')).toBeTruthy();
    expect(screen.queryByTestId('draft-field-undo')).toBeNull();
  });
  it('Regenerate replaces an untouched draft at once, with Undo back to it', async () => {
    const onregenerate = vi.fn();
    const { rerender } = render(DraftField, { value: 'First draft', label: 'Summary', onregenerate });
    await fireEvent.click(screen.getByTestId('draft-field-regenerate'));
    expect(onregenerate).toHaveBeenCalledOnce();
    expect(screen.queryByTestId('draft-field-ask')).toBeNull();
    await rerender({ value: 'Second draft', label: 'Summary', onregenerate });
    const input = screen.getByTestId('draft-field-input') as HTMLTextAreaElement;
    expect(input.value).toBe('Second draft');
    await fireEvent.click(screen.getByTestId('draft-field-undo'));
    expect(input.value).toBe('First draft');
  });
  it('Regenerate asks before it replaces edited text', async () => {
    const onregenerate = vi.fn();
    render(DraftField, { value: 'Draft', label: 'Summary', onregenerate });
    const input = screen.getByTestId('draft-field-input') as HTMLTextAreaElement;
    await fireEvent.input(input, { target: { value: 'My own words' } });
    await fireEvent.click(screen.getByTestId('draft-field-regenerate'));
    expect(onregenerate).not.toHaveBeenCalled();
    expect(screen.getByTestId('draft-field-ask').textContent).toContain('Replace your text with a new draft?');
    await fireEvent.click(screen.getByTestId('draft-field-keep'));
    expect(screen.queryByTestId('draft-field-ask')).toBeNull();
    expect(input.value).toBe('My own words');
    await fireEvent.click(screen.getByTestId('draft-field-regenerate'));
    await fireEvent.click(screen.getByTestId('draft-field-replace'));
    expect(onregenerate).toHaveBeenCalledOnce();
    expect(screen.getByTestId('draft-field-undo')).toBeTruthy();
  });
  it('says it is drafting and disables Regenerate while busy', () => {
    render(DraftField, { value: '', label: 'Summary', busy: true, onregenerate: () => {} });
    expect(screen.getByTestId('draft-field-busy').textContent).toBe('Drafting…');
    expect((screen.getByTestId('draft-field-regenerate') as HTMLButtonElement).disabled).toBe(true);
  });
  it('shows a small Atom while the LLM writes, only after 400 ms', async () => {
    const started = Date.now();
    const { rerender } = render(DraftField, { value: '', label: 'Summary', busy: true });
    expect(screen.queryByTestId('draft-field-atom')).toBeNull();
    const atom = await waitFor(() => screen.getByTestId('draft-field-atom'), { timeout: 3000 });
    expect(Date.now() - started).toBeGreaterThanOrEqual(LOADER_DELAY_MS - 5);
    expect(atom.dataset.loader).toBe('atom');
    await rerender({ value: 'Fix the flake', label: 'Summary', busy: false });
    expect(screen.queryByTestId('draft-field-atom')).toBeNull();
  });
});

describe('the ai-pre token', () => {
  const css = readFileSync('src/app.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  it('is an alias for --accent in every theme block, light and dark', () => {
    expect(css.match(/--ai-pre:\s*var\(--accent\);/g)?.length).toBe(4);
  });
  it('controls.css draws the ring from it', () => {
    const controls = readFileSync('src/lib/controls.css', 'utf8');
    expect(controls).toMatch(/\.ai-pre\s*\{[^}]*var\(--ai-pre\)/);
  });
});

describe('neverDecidesField (review r15 F14)', () => {
  it.each([
    [{ name: 'prio', label: 'Priority' }],
    [{ name: 'max_priority', label: 'Pick one' }],
    [{ name: 'x', label: 'Task size' }],
    [{ name: 'x', label: 'Who gets access?' }],
    [{ name: 'x', label: 'Pick', help: 'Approve the push?' }],
  ])('%o is a person’s call', (f) => expect(neverDecidesField(f)).toBe(true));

  it.each([[{ name: 'env', label: 'Environment' }], [{ name: 'host', label: 'Which host?' }]])(
    '%o may take a proposal',
    (f) => expect(neverDecidesField(f)).toBe(false),
  );
});
