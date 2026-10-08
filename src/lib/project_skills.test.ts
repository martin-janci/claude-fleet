import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('./files', () => ({ repoTree: vi.fn(), repoFile: vi.fn() }));

import { repoFile, repoTree } from './files';
import { clearProjectSkills, projectSkills, readSkillHead, skillPaths, PROJECT_SKILLS_TTL_MS } from './project_skills';
import { matchSlashCommands } from './conversation';

const tree = vi.mocked(repoTree);
const file = vi.mocked(repoFile);

const content = (path: string, text: string) => ({
  ok: true as const,
  value: { path, content: text, truncated: false, binary: false, is_dir: false, size: text.length },
});

beforeEach(() => {
  vi.clearAllMocks();
  clearProjectSkills();
});

describe('project skills in the slash menu (redesign 5.9)', () => {
  it('finds skills and commands under .claude/, nothing deeper or elsewhere', () => {
    expect(
      skillPaths([
        'src/main.ts',
        '.claude/skills/steward/SKILL.md',
        '.claude/skills/steward/notes.md',
        '.claude/skills/a/b/SKILL.md',
        '.claude/commands/ship.md',
        '.claude/commands/sub/x.md',
        'skills/x/SKILL.md',
      ]),
    ).toEqual([
      { name: 'ship', path: '.claude/commands/ship.md', source: 'command' },
      { name: 'steward', path: '.claude/skills/steward/SKILL.md', source: 'skill' },
    ]);
  });

  it('reads name and description from front matter, or the first line', () => {
    expect(readSkillHead('---\nname: babysit\ndescription: "Drive a PR to green"\n---\n# Body')).toEqual({
      name: 'babysit',
      description: 'Drive a PR to green',
    });
    expect(readSkillHead('# Ship the release\n\nSteps…')).toEqual({ name: null, description: 'Ship the release' });
  });

  it('reads the worktree once per TTL and labels each entry', async () => {
    tree.mockResolvedValue({ ok: true, value: { entries: ['.claude/skills/steward/SKILL.md', '.claude/commands/ship.md'], truncated: false } });
    file.mockImplementation(async (_id, path) =>
      path.endsWith('SKILL.md') ? content(path, '---\nname: steward\ndescription: Keep PRs green\n---\n') : content(path, 'Cut a release\n'),
    );
    const list = await projectSkills(7, 1_000);
    expect(list).toEqual([
      { name: 'ship', description: 'Cut a release', args: true, source: 'command' },
      { name: 'steward', description: 'Keep PRs green', args: true, source: 'skill' },
    ]);
    await projectSkills(7, 1_000 + PROJECT_SKILLS_TTL_MS - 1);
    expect(tree).toHaveBeenCalledTimes(1);
    await projectSkills(7, 1_000 + PROJECT_SKILLS_TTL_MS);
    expect(tree).toHaveBeenCalledTimes(2);
  });

  it('a session with no worktree has no project commands, not an error', async () => {
    tree.mockResolvedValue({ ok: false, error: { code: 'E_NOT_FOUND', message: 'no worktree' } });
    expect(await projectSkills(8)).toEqual([]);
  });

  it('the menu lists them after the built-ins, and never shadows one', () => {
    const extra = [
      { name: 'steward', description: 'Keep PRs green', args: true, source: 'skill' as const },
      { name: 'review', description: 'A project review', args: true, source: 'skill' as const },
    ];
    expect(matchSlashCommands('/st', extra).map((c) => c.name)).toEqual(['status', 'steward']);
    expect(matchSlashCommands('/review', extra)).toHaveLength(1);
    expect(matchSlashCommands('/review', extra)[0].source).toBeUndefined();
    expect(matchSlashCommands('/st x', extra)).toEqual([]);
  });
});

import { newDividerAnchor } from './conversation';

describe('newDividerAnchor (redesign 5.9)', () => {
  const t = (at: string, ended: string | null, uuid?: string) => ({ prompt: 'p', at, ended_at: ended, items: [], prompt_uuid: uuid });
  const seen = Date.parse('2026-10-08T10:00:00Z') / 1000;
  it('marks the first turn that started or was answered after the last look', () => {
    const turns = [
      t('2026-10-08T09:00:00Z', '2026-10-08T09:10:00Z', 'a'),
      t('2026-10-08T09:50:00Z', '2026-10-08T10:05:00Z', 'b'),
      t('2026-10-08T10:20:00Z', null, 'c'),
    ];
    expect(newDividerAnchor(turns, seen)).toBe('b');
    expect(newDividerAnchor(turns.slice(0, 1), seen)).toBeNull();
    expect(newDividerAnchor(turns, null)).toBeNull();
    expect(newDividerAnchor([t('2026-10-08T10:20:00Z', null)], seen)).toBe('2026-10-08T10:20:00Z');
  });
});
