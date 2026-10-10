// The Review dialog's reviewer and scope (redesign step 5.10). Both become
// visible lines at the head of the review prompt: the dialog shows them
// above the editable text, so nothing the reviewer reads is hidden.
import type { AssetInventoryRow } from './assets';
import { AGENT_LABELS } from './row_groups';
import { sessionAgent, type SessionRow } from './sessions';

export type ReviewScope = 'branch' | 'uncommitted' | 'last_commit';

export const REVIEW_SCOPES: { value: ReviewScope; label: string; line: string }[] = [
  {
    value: 'branch',
    label: 'Branch vs its base',
    line: 'Scope: this branch against its base branch (`git diff <base>...HEAD`), committed or not.',
  },
  {
    value: 'uncommitted',
    label: 'Uncommitted changes',
    line: 'Scope: only the uncommitted changes in this worktree (`git diff HEAD`).',
  },
  {
    value: 'last_commit',
    label: 'Last commit',
    line: 'Scope: only the last commit (`git show HEAD`).',
  },
];

/** The lines the dialog puts ahead of the prompt text. */
export function reviewPreamble(skill: string | null, scope: ReviewScope): string {
  const lines: string[] = [];
  if (skill) lines.push(`Use the ${skill} skill for this review.`);
  const s = REVIEW_SCOPES.find((x) => x.value === scope);
  if (s) lines.push(s.line);
  lines.push('Read and comment only: never commit, push or edit files.');
  return lines.join('\n');
}

/** The whole prompt the review session is seeded with. */
export function reviewPrompt(skill: string | null, scope: ReviewScope, text: string): string {
  return `${reviewPreamble(skill, scope)}\n\n${text.trim()}`;
}

/** Claude Code skills present on `host`, review skills first, then by name. */
export function reviewerSkills(rows: AssetInventoryRow[], host: string): string[] {
  const names = new Set(
    rows
      .filter(
        (r) =>
          r.host_alias === host &&
          r.kind === 'skill' &&
          r.harness === 'claude' &&
          r.state !== 'missing' &&
          r.state !== 'unsupported',
      )
      .map((r) => r.name),
  );
  const review = (n: string) => (/review/i.test(n) ? 0 : 1);
  return [...names].sort((a, b) => review(a) - review(b) || a.localeCompare(b));
}

const SKILL_LINE = /^Use the (\S+) skill for this review\./;

/** Who ran a review session, in the Reviews block's words: "pr-review
 *  skill · Claude Code", or just the agent when the skill is not known. The
 *  skill is read from the seeded prompt's first line (`reviewPreamble`)
 *  while it is still the row's last prompt; nothing else records it. */
export function reviewerOf(row: Pick<SessionRow, 'agent' | 'kind' | 'last_prompt'>): string {
  const agent = AGENT_LABELS[sessionAgent(row)] ?? 'Claude Code';
  const skill = SKILL_LINE.exec(row.last_prompt ?? '')?.[1];
  return skill ? `${skill} skill · ${agent}` : agent;
}
