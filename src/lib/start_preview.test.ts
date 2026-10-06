// The start preview's pure half (task → session spec P-1 / §2.2): what the
// Work button's primary half does, when a preview may start without asking,
// and what the popover sends.
import { describe, it, expect } from 'vitest';
import {
  argsWithChoice,
  baseStartArgs,
  choiceFromPreview,
  previewIsClean,
  primaryAction,
  startBlockedBy,
  type StartPreview,
} from './start_preview';

const plan = { key: 'ABC-1', title: 'T', item_id: 1, project_id: 3, host_alias: 'h', branch: 'abc-1-t', name: 'ABC-1 T' };
const preview = (over: Partial<StartPreview> = {}): StartPreview => ({
  key: 'ABC-1',
  title: 'T',
  item_id: 1,
  plan,
  projects: [{ id: 3, owner: 'acme', repo: 'api' }],
  hosts: [
    { alias: 'h', reachable: true },
    { alias: 'off', reachable: false },
  ],
  conflicts: [],
  ...over,
});

describe('primaryAction', () => {
  it('opens a live session, else continues a past one, else starts', () => {
    expect(primaryAction({ live: true, resumable: true })).toBe('open');
    expect(primaryAction({ live: false, resumable: true })).toBe('continue');
    expect(primaryAction({ live: false, resumable: false })).toBe('start');
  });
});

describe('previewIsClean', () => {
  it('is clean only when resolved with nothing in the way', () => {
    expect(previewIsClean(preview())).toBe(true);
    expect(previewIsClean(preview({ plan: null, missing: 'project' }))).toBe(false);
    expect(previewIsClean(preview({ conflicts: [{ kind: 'done', message: 'done' }] }))).toBe(false);
  });
});

describe('the start arguments', () => {
  it('start by item when there is one, else by key, with the brief on', () => {
    expect(baseStartArgs({ item_id: 1, key: 'TASK-1', project_id: 3 })).toEqual({
      item_id: 1,
      project_id: 3,
      with_brief: true,
    });
    expect(baseStartArgs({ item_id: null, key: 'ABC-9', project_id: null })).toEqual({
      reference: 'ABC-9',
      with_brief: true,
    });
  });

  it('a live session makes the first choice a parallel start, never a cross-org one', () => {
    const c = choiceFromPreview(
      preview({
        conflicts: [
          { kind: 'live_session', message: 'open', session_id: 7 },
          { kind: 'cross_org', message: 'orgs' },
        ],
      }),
    );
    expect(c.parallel).toBe(true);
    expect(c.force_cross_org).toBe(false);
    expect(argsWithChoice({ item_id: 1 }, c)).toEqual({
      item_id: 1,
      project_id: 3,
      host_alias: 'h',
      with_brief: true,
      parallel: true,
    });
  });

  it('sends a typed branch trimmed, and only when there is one', () => {
    const c = { ...choiceFromPreview(preview()), worktree: '  my-branch ' };
    expect(argsWithChoice({ item_id: 1 }, c).worktree).toBe('my-branch');
    expect(argsWithChoice({ item_id: 1 }, { ...c, worktree: '  ' }).worktree).toBeUndefined();
  });
});

describe('startBlockedBy', () => {
  it('says what is missing or in the way, in words', () => {
    const c = choiceFromPreview(preview());
    expect(startBlockedBy(preview(), c)).toBeNull();
    expect(startBlockedBy(preview({ plan: null, missing: 'project' }), { ...c, project_id: null })).toBe('Pick a repository.');
    expect(startBlockedBy(preview({ plan: null, missing: 'host' }), { ...c, host_alias: null })).toBe('Pick a host.');
    expect(startBlockedBy(preview({ conflicts: [{ kind: 'proposal', message: 'p' }] }), c)).toBe('Accept the proposal first.');
    const cross = preview({ conflicts: [{ kind: 'cross_org', message: 'o' }] });
    expect(startBlockedBy(cross, c)).toContain('crosses organisations');
    expect(startBlockedBy(cross, { ...c, force_cross_org: true })).toBeNull();
    expect(startBlockedBy(preview(), { ...c, host_alias: 'off' })).toBe('off is offline: pick another host.');
  });
});

describe('previewUnsupported', () => {
  it('recognises an older hub that has no preview_start, and nothing else', async () => {
    const { previewUnsupported } = await import('./start_preview');
    expect(previewUnsupported({ code: 'E_INVALID', message: 'preview_start needs session_id' })).toBe(true);
    expect(previewUnsupported({ code: 'E_INVALID', message: 'unknown work_link action "preview_start"; one of link' })).toBe(true);
    expect(previewUnsupported({ code: 'E_INVALID', message: 'start needs exactly one of key, url or item_id' })).toBe(false);
    expect(previewUnsupported({ code: 'E_FORBIDDEN', message: 'preview_start refused' })).toBe(false);
  });
});
