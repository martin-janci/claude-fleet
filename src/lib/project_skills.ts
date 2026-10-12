// The project's own slash commands for the composer's menu (redesign 5.9):
// the skills in the session's worktree under `.claude/skills/<name>/SKILL.md`
// and the commands under `.claude/commands/<name>.md`, which Claude Code
// runs as `/<name>`. Read through the Files tab's `repo_tree` / `repo_file`,
// so a hub client reads them on the hub like any other worktree file, and
// cached per session: the menu asks on every keystroke.
import { get } from 'svelte/store';
import type { SlashCommand } from './conversation';
import { repoFile, repoTree } from './files';
import { CONTROL_COMMANDS } from './control_commands';
import { operatorSession } from './operator';

/** How long one read of a session's skills is reused. */
export const PROJECT_SKILLS_TTL_MS = 60_000;
/** At most this many files are read for descriptions. */
export const PROJECT_SKILLS_MAX = 40;

const SKILL_PATH = /^\.claude\/skills\/([^/]+)\/SKILL\.md$/;
const COMMAND_PATH = /^\.claude\/commands\/([^/]+)\.md$/;

/** The `.claude/` entries of a worktree listing that become commands. */
export function skillPaths(entries: readonly string[]): { name: string; path: string; source: 'skill' | 'command' }[] {
  const out: { name: string; path: string; source: 'skill' | 'command' }[] = [];
  for (const path of entries) {
    const s = SKILL_PATH.exec(path);
    if (s) out.push({ name: s[1], path, source: 'skill' });
    const c = COMMAND_PATH.exec(path);
    if (c) out.push({ name: c[1], path, source: 'command' });
  }
  return out.sort((a, b) => a.name.localeCompare(b.name)).slice(0, PROJECT_SKILLS_MAX);
}

/** A skill file's `name:` and `description:` from its front matter; a
 *  command without front matter is described by its first line. */
export function readSkillHead(text: string): { name: string | null; description: string | null } {
  const fm = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text);
  const field = (key: string) => {
    if (!fm) return null;
    const m = new RegExp(`^${key}:\\s*(.*)$`, 'm').exec(fm[1]);
    const v = m?.[1].trim().replace(/^(['"])(.*)\1$/, '$2') ?? '';
    return v === '' ? null : v;
  };
  let description = field('description');
  if (description === null) {
    const body = fm ? text.slice(fm[0].length) : text;
    const line = body.split('\n').map((l) => l.replace(/^#+\s*/, '').trim()).find((l) => l !== '');
    description = line ?? null;
  }
  return { name: field('name'), description: description && description.length > 120 ? `${description.slice(0, 119)}…` : description };
}

const cache = new Map<number, { at: number; list: Promise<SlashCommand[]> }>();

async function read(sessionId: number): Promise<SlashCommand[]> {
  const tree = await repoTree(sessionId);
  if (!tree.ok) return [];
  const found = skillPaths(tree.value.entries);
  const heads = await Promise.all(
    found.map(async (f) => {
      const r = await repoFile(sessionId, f.path);
      return r.ok && !r.value.binary ? readSkillHead(r.value.content) : { name: null, description: null };
    }),
  );
  return found.map((f, i) => ({
    name: f.source === 'skill' ? (heads[i].name ?? f.name) : f.name,
    description: heads[i].description ?? (f.source === 'skill' ? 'Project skill' : 'Project command'),
    args: true,
    source: f.source,
  }));
}

/** The session's project commands, read at most once per TTL. A failed
 *  read (no worktree, an older hub) is an empty list, not an error: the
 *  built-ins are still there. */
export function projectSkills(sessionId: number, now = Date.now()): Promise<SlashCommand[]> {
  // Control's agent: its commands are fleet's own (redesign 9.6), and its
  // directory has no worktree to list.
  if (get(operatorSession)?.id === sessionId) return Promise.resolve([...CONTROL_COMMANDS]);
  const hit = cache.get(sessionId);
  if (hit && now - hit.at < PROJECT_SKILLS_TTL_MS) return hit.list;
  const list = read(sessionId).catch(() => []);
  cache.set(sessionId, { at: now, list });
  return list;
}

/** Tests only. */
export function clearProjectSkills(): void {
  cache.clear();
}
